//! The renderer process. Diagrams are drawn by the app's own executable started with
//! [`WORKER_ARG`], fed over its standard input and output: a renderer that panics (the release
//! build aborts on panic), overflows its stack or never finishes takes down only that process.
//! The app stops it after [`super::RENDER_TIMEOUT`] and starts a new one for the next diagram.
//!
//! Messages are frames of text fields: a `u32` field count, then per field a `u32` byte length
//! and the UTF-8 bytes, all little endian. A request is the source and the theme's fields; the
//! answer is `ok`, the SVG, its width and height, or `error`, the reason and the line.

use std::io::{self, BufReader, BufWriter, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

use egui::vec2;

use super::diagram::{self, DiagramFailure, DiagramTheme, Svg};

/// The argument that makes the executable serve diagrams instead of opening its window.
const WORKER_ARG: &str = "--mermaid-worker";
/// Fields per frame and bytes per field a reader accepts; anything larger is a broken stream.
const MAX_FIELDS: usize = 64;
const MAX_FIELD_BYTES: usize = 64 * 1024 * 1024;

/// Serves diagrams on standard input and output when the executable was started as the
/// renderer process; `false` otherwise. Called first thing in `main`.
pub fn serve_if_asked() -> bool {
    if std::env::args().nth(1).as_deref() != Some(WORKER_ARG) {
        return false;
    }
    // The app closes the pipe when it no longer needs the process (or exits): then this ends,
    // and a broken pipe has nobody left to tell.
    let _app_gone = serve(io::stdin().lock(), io::stdout().lock());
    true
}

/// Answers every request read from `input` on `output` until `input` ends.
fn serve(input: impl Read, output: impl Write) -> io::Result<()> {
    let mut input = BufReader::new(input);
    let mut output = BufWriter::new(output);
    while let Some(fields) = read_frame(&mut input)? {
        let answer = match fields.split_first() {
            Some((source, theme)) => match DiagramTheme::from_fields(theme) {
                Some(theme) => diagram::draw(source, &theme),
                None => Err(DiagramFailure::new("The app and its diagram renderer disagree", None)),
            },
            None => Err(DiagramFailure::new("The app sent no diagram", None)),
        };
        match answer {
            Ok(svg) => write_frame(
                &mut output,
                &["ok", &svg.text, &svg.size.x.to_string(), &svg.size.y.to_string()],
            )?,
            Err(failure) => {
                let line = failure.line.map(|line| line.to_string()).unwrap_or_default();
                write_frame(&mut output, &["error", &failure.reason, &line])?;
            }
        }
        output.flush()?;
    }
    Ok(())
}

fn write_frame(output: &mut impl Write, fields: &[&str]) -> io::Result<()> {
    output.write_all(&(fields.len() as u32).to_le_bytes())?;
    for field in fields {
        output.write_all(&(field.len() as u32).to_le_bytes())?;
        output.write_all(field.as_bytes())?;
    }
    Ok(())
}

/// The next frame; `None` when the stream ended between frames.
fn read_frame(input: &mut impl Read) -> io::Result<Option<Vec<String>>> {
    let mut word = [0_u8; 4];
    match input.read_exact(&mut word) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let count = u32::from_le_bytes(word) as usize;
    if count > MAX_FIELDS {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "too many fields"));
    }
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        input.read_exact(&mut word)?;
        let length = u32::from_le_bytes(word) as usize;
        if length > MAX_FIELD_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "field too large"));
        }
        let mut bytes = vec![0; length];
        input.read_exact(&mut bytes)?;
        fields.push(
            String::from_utf8(bytes)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not UTF-8"))?,
        );
    }
    Ok(Some(fields))
}

/// The answer to a request, as [`serve`] wrote it.
fn parse_answer(fields: &[String]) -> Option<Result<Svg, DiagramFailure>> {
    match fields {
        [tag, svg, width, height] if tag == "ok" => {
            let size = vec2(width.parse().ok()?, height.parse().ok()?);
            Some(Ok(Svg { text: svg.clone(), size }))
        }
        [tag, reason, line] if tag == "error" => {
            Some(Err(DiagramFailure::new(reason.clone(), line.parse().ok())))
        }
        _ => None,
    }
}

/// A running renderer: where requests go and where answers come from (read on a thread of its
/// own, so a wait can time out).
pub(crate) struct Worker {
    input: Box<dyn Write + Send>,
    answers: mpsc::Receiver<io::Result<Option<Vec<String>>>>,
    /// The renderer process; the tests run the renderer on a thread instead.
    process: Option<std::process::Child>,
}

impl Worker {
    /// Starts the renderer: this executable as [`WORKER_ARG`].
    #[cfg(not(any(test, feature = "test-support")))]
    pub(crate) fn start() -> io::Result<Self> {
        use std::process::{Command, Stdio};
        let mut process = Command::new(std::env::current_exe()?)
            .arg(WORKER_ARG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = process.stdin.take().ok_or_else(|| io::Error::other("no input pipe"))?;
        let output = process.stdout.take().ok_or_else(|| io::Error::other("no output pipe"))?;
        Ok(Self::connect(Box::new(input), output, Some(process)))
    }

    /// The test binary cannot start itself as the renderer: [`serve`] runs on a thread over pipes.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn start() -> io::Result<Self> {
        let (request_reader, request_writer) = io::pipe()?;
        let (answer_reader, answer_writer) = io::pipe()?;
        std::thread::Builder::new()
            .name("mermaid-renderer".into())
            .spawn(move || serve(request_reader, answer_writer))?;
        Ok(Self::connect(Box::new(request_writer), answer_reader, None))
    }

    fn connect(
        input: Box<dyn Write + Send>,
        output: impl Read + Send + 'static,
        process: Option<std::process::Child>,
    ) -> Self {
        let (sender, answers) = mpsc::channel();
        // Without the reader thread its sender drops at once, and the first draw reports the
        // renderer as stopped.
        let _reader = std::thread::Builder::new().name("mermaid-answers".into()).spawn(move || {
            let mut output = BufReader::new(output);
            loop {
                let frame = read_frame(&mut output);
                let is_last = !matches!(frame, Ok(Some(_)));
                if sender.send(frame).is_err() || is_last {
                    break;
                }
            }
        });
        Self { input, answers, process }
    }

    /// Draws `source` in `theme`. `Err` means the renderer is gone (it crashed, broke the
    /// protocol or took longer than `timeout` and was stopped): the caller drops this worker.
    pub(crate) fn draw(
        &mut self,
        source: &str,
        theme: &DiagramTheme,
        timeout: Duration,
    ) -> Result<Result<Svg, DiagramFailure>, DiagramFailure> {
        let theme = theme.fields();
        let mut fields = Vec::with_capacity(theme.len() + 1);
        fields.push(source);
        fields.extend(theme.iter().map(String::as_str));
        let sent = write_frame(&mut self.input, &fields).and_then(|()| self.input.flush());
        if sent.is_err() {
            return Err(DiagramFailure::retryable("The diagram renderer stopped"));
        }
        match self.answers.recv_timeout(timeout) {
            Ok(Ok(Some(answer))) => parse_answer(&answer).ok_or_else(|| {
                DiagramFailure::retryable("The diagram renderer answered something else")
            }),
            Ok(Ok(None) | Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(DiagramFailure::new("The diagram renderer stopped on this diagram", None))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Err(DiagramFailure::retryable(format!(
                "Took longer than {} s to draw",
                timeout.as_secs_f32()
            ))),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(process) = &mut self.process {
            // Both fail only for a process that already exited, which is the goal.
            let _exited = process.kill();
            let _reaped = process.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor};
    use std::time::Duration;

    use cv_base::theme::Palette;

    use super::{Worker, read_frame, serve, write_frame};
    use crate::mermaid::diagram::DiagramTheme;

    #[test]
    fn frames_round_trip_and_reject_garbage() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &["a", "", "ünï"]).unwrap();
        let mut reader = Cursor::new(bytes);
        assert_eq!(
            read_frame(&mut reader).unwrap(),
            Some(vec!["a".to_owned(), String::new(), "ünï".to_owned()])
        );
        assert_eq!(read_frame(&mut reader).unwrap(), None, "the end between frames is no error");
        assert!(
            read_frame(&mut Cursor::new(u32::MAX.to_le_bytes().to_vec())).is_err(),
            "an absurd field count"
        );
        assert!(
            read_frame(&mut Cursor::new([1, 0, 0, 0, 9, 0, 0, 0, b'x'].to_vec())).is_err(),
            "a cut field"
        );
    }

    #[test]
    fn the_renderer_answers_drawings_and_errors() {
        let theme = DiagramTheme::from_palette(&Palette::LIGHT);
        let mut worker = Worker::start().unwrap();
        let svg = worker.draw("graph TD; A-->B", &theme, Duration::from_secs(20)).unwrap().unwrap();
        assert!(svg.text.contains("<svg") && svg.size.y > 0.0);
        let failure =
            worker.draw("bogusDiagram\n x", &theme, Duration::from_secs(20)).unwrap().unwrap_err();
        assert_eq!(failure.line, Some(1));
        assert!(
            worker.draw("graph LR; X-->Y", &theme, Duration::from_secs(20)).unwrap().is_ok(),
            "it serves on after an error"
        );
    }

    #[test]
    fn a_silent_renderer_times_out_and_a_dead_one_is_reported() {
        let theme = DiagramTheme::from_palette(&Palette::LIGHT);
        // Answers never come: the request is written to a sink, the answer pipe stays open.
        let (silent, _keep_open) = io::pipe().unwrap();
        let mut worker = Worker::connect(Box::new(io::sink()), silent, None);
        let failure =
            worker.draw("graph TD; A-->B", &theme, Duration::from_millis(50)).unwrap_err();
        assert!(failure.reason.starts_with("Took longer than") && failure.can_retry, "{failure:?}");
        // The renderer's output closes without an answer: it died on the diagram.
        let mut worker = Worker::connect(Box::new(io::sink()), io::empty(), None);
        let failure = worker.draw("graph TD; A-->B", &theme, Duration::from_secs(5)).unwrap_err();
        assert_eq!(failure.reason, "The diagram renderer stopped on this diagram");
        assert!(!failure.can_retry);
    }

    #[test]
    fn serve_ends_with_its_input() {
        let mut output = Vec::new();
        serve(io::empty(), &mut output).unwrap();
        assert_eq!(output, Vec::<u8>::new());
    }
}
