//! One Code View window per data folder: a second start (`CodeView <url>`, or the browser
//! extension's native host) hands its link to the running window and exits, and that window
//! comes to the front and opens the link like a command-line link.
//!
//! The endpoint is local to the user: a named pipe only its owner may open on Windows, a Unix
//! socket in a folder only its owner may enter elsewhere ([`platform`]). The first instance listens
//! on a thread of its own that blocks in accept, so it costs nothing while idle and the UI thread
//! never waits on it; each connection is read on a short-lived thread, so a client that stalls
//! holds up nobody.
//!
//! The exchange is three lines: the server sends its process id (so the client can let it take the
//! foreground, which the user just asked for), the client sends the link (an empty line only
//! brings the window to the front), the server answers `ok`. Links wait in the egui context until
//! the app's frame takes them and checks each pull request's access first ([`ForwardedLinks`]).

mod forwarded;
mod platform;

use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

use egui::{Id, ViewportCommand};
use review_store::AppPaths;

pub use self::forwarded::{Forwarded, ForwardedLinks};
pub use self::platform::{Endpoint, is_absent};
use self::platform::{Listener as SystemListener, Stream};

/// The longest link line either side reads; a pull request URL is far shorter (our own limit).
const MAX_LINE: usize = 8 * 1024;
/// How long a client waits for the running instance to take its link (our own budget; the
/// exchange takes milliseconds).
const SEND_WAIT: Duration = Duration::from_secs(3);
const INBOX_ID: &str = "single_instance_inbox";

/// What starting up found.
pub enum Claim {
    /// No other instance runs: this one serves the endpoint from now on.
    First(Listener),
    /// The running instance took the link.
    HandedOver,
    /// The endpoint could not be used either way; this instance runs on its own.
    Alone(io::Error),
}

/// Becomes the instance for `paths`, or hands `link` to the one that runs.
pub fn claim(paths: &AppPaths, link: Option<&str>) -> Claim {
    let endpoint = Endpoint::of(paths);
    let mut last_error = None;
    // Twice: an instance that started at the same moment may win the bind in between.
    for _ in 0..2 {
        match send(&endpoint, link.unwrap_or_default()) {
            Ok(()) => return Claim::HandedOver,
            Err(error) if platform::is_absent(&error) => {}
            Err(error) => return Claim::Alone(error),
        }
        match SystemListener::bind(&endpoint) {
            Ok(listener) => return Claim::First(Listener(listener)),
            Err(error) => last_error = Some(error),
        }
    }
    Claim::Alone(last_error.unwrap_or_else(|| io::Error::other("the endpoint stayed busy")))
}

/// Hands `link` to the instance listening on `endpoint`. An error that
/// [`platform::is_absent`] accepts means none listens.
pub fn send(endpoint: &Endpoint, link: &str) -> io::Result<()> {
    if link.len() > MAX_LINE || link.contains(['\n', '\r']) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a single-line link"));
    }
    let mut stream = platform::connect(endpoint)?;
    let link = link.to_owned();
    let (done, outcome) = mpsc::channel();
    // A thread of its own, so a running instance that never answers costs a bounded wait.
    std::thread::spawn(move || {
        // Fails only when the waiting side gave up already; nobody is left to tell.
        let _gave_up = done.send(exchange(&mut stream, &link));
    });
    outcome.recv_timeout(SEND_WAIT).map_err(|_| {
        io::Error::new(io::ErrorKind::TimedOut, "the running Code View did not answer")
    })?
}

fn exchange(stream: &mut Stream, link: &str) -> io::Result<()> {
    let pid = read_line(stream)?;
    if let Ok(pid) = pid.parse() {
        platform::allow_foreground(pid);
    }
    stream.write_all(format!("{link}\n").as_bytes())?;
    stream.flush()?;
    match read_line(stream)?.as_str() {
        "ok" => Ok(()),
        other => {
            Err(io::Error::new(io::ErrorKind::InvalidData, format!("unexpected answer {other:?}")))
        }
    }
}

/// The first instance's endpoint, until [`Listener::serve`] starts answering on it.
pub struct Listener(SystemListener);

impl Listener {
    /// Answers every later start on a thread of its own; their links reach `ctx`
    /// ([`ForwardedLinks`]).
    pub fn serve(self, ctx: egui::Context) {
        let mut listener = self.0;
        // Without the thread later starts open windows of their own, as without the endpoint.
        let _runs_alone =
            std::thread::Builder::new().name("single-instance".to_owned()).spawn(move || {
                // Ends only when the system refuses a new pipe instance or socket; the app keeps
                // running, later starts then open windows of their own.
                while let Ok(stream) = listener.accept() {
                    let ctx = ctx.clone();
                    std::thread::spawn(move || {
                        // A client that hung up or sent garbage: its start has its own error to show.
                        let _client_gone = answer(stream, &ctx);
                    });
                }
            });
    }
}

fn answer(mut stream: Stream, ctx: &egui::Context) -> io::Result<()> {
    stream.write_all(format!("{}\n", std::process::id()).as_bytes())?;
    stream.flush()?;
    let link = read_line(&mut stream)?;
    deliver(ctx, link.trim());
    stream.write_all(b"ok\n")?;
    stream.flush()
}

/// Queues `link` for the app's frame and brings the window to the front.
fn deliver(ctx: &egui::Context, link: &str) {
    if !link.is_empty() {
        queue_link(ctx, link);
    }
    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(ViewportCommand::Focus);
    ctx.request_repaint();
}

#[derive(Clone, Default)]
struct Inbox(Vec<String>);

/// Queues `link` for the app's frame, as a later start's link: the command-line link of this
/// start takes the same way ([`ForwardedLinks`]).
pub fn queue_link(ctx: &egui::Context, link: &str) {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Inbox>(Id::unique(INBOX_ID)).0.push(link.to_owned());
    });
}

/// The links queued since the last call, oldest first.
fn take_links(ctx: &egui::Context) -> Vec<String> {
    cv_base::mailbox::take::<Inbox>(ctx, Id::unique(INBOX_ID))
        .map(|inbox| inbox.0)
        .unwrap_or_default()
}

/// One line up to [`MAX_LINE`] bytes, without its line break. The lines are tiny, so reading
/// byte by byte costs nothing and never reads past the line into the other side's next one.
fn read_line(stream: &mut impl Read) -> io::Result<String> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        if stream.read(&mut byte)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the other side hung up"));
        }
        match byte {
            [b'\n'] => break,
            [other] => line.push(other),
        }
        if line.len() > MAX_LINE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
        }
    }
    String::from_utf8(line).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::{Claim, claim, take_links};
    use cv_testkit::{ScratchDir, poll_for};

    #[test]
    fn a_second_start_hands_its_link_to_the_first() {
        let scratch = ScratchDir::new("single-instance");
        let paths = scratch.app_paths();
        let Claim::First(listener) = claim(&paths, None) else {
            panic!("the first start serves the endpoint");
        };
        let ctx = egui::Context::default();
        listener.serve(ctx.clone());

        let url = "https://github.com/acme/widgets/pull/7";
        assert!(
            matches!(claim(&paths, Some(url)), Claim::HandedOver),
            "the second start hands over"
        );
        let links = poll_for("the forwarded link", || {
            Some(take_links(&ctx)).filter(|links| !links.is_empty())
        });
        assert_eq!(links, vec![url.to_owned()]);

        // A start without a link only brings the window up; nothing is queued.
        assert!(matches!(claim(&paths, None), Claim::HandedOver));
        assert_eq!(take_links(&ctx), Vec::<String>::new());
    }

    #[test]
    fn data_folders_of_their_own_get_instances_of_their_own() {
        let first = ScratchDir::new("single-instance-a");
        let second = ScratchDir::new("single-instance-b");
        let Claim::First(listener) = claim(&first.app_paths(), None) else {
            panic!("the first folder's start serves");
        };
        listener.serve(egui::Context::default());
        assert!(
            matches!(claim(&second.app_paths(), None), Claim::First(_)),
            "another data folder is another instance"
        );
    }

    #[test]
    fn a_multi_line_link_is_refused() {
        let scratch = ScratchDir::new("single-instance-lines");
        let endpoint = super::Endpoint::of(&scratch.app_paths());
        let error = super::send(&endpoint, "https://a\nhttps://b").expect_err("refused");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
}
