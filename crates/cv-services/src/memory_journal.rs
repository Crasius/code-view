//! The memory journal: the app writes down what it holds, so a window that grew over hours tells
//! afterwards which part grew, without anyone watching it. One line at start, one every
//! `TICK`, and one whenever the private bytes pass a new `RISE_STEP` above the first reading,
//! so a fast rise shows between two ticks. The file (`AppPaths::memory_journal_file`) moves aside
//! once it reaches `ROTATE_AT`; the one before it goes. A line holds numbers only:
//!
//! ```text
//! 2026-10-28T12:05:00Z tick uptime_s=300 private=183500800 live=96468992 images=0 diagrams=0
//!   page_answers=1048576 markdown=262144 grammars=31457280 diffs=0 file_texts=0 diff_colors=0
//!   code_colors=0 http_answers=524288 textures=14 texture_bytes=8388608 handles=612 threads=-
//! ```
//!
//! (one line in the file). Bytes are bytes; `-` is a number this system does not give
//! ([`cv_base::platform::heap`]). The reason is `start`, `tick` or `rise`. Texture bytes are what
//! egui has handed to the GPU (width × height × bytes per pixel), the font atlas included.
//!
//! Its own thread wakes every `CHECK`, reads the private bytes and writes when a line is due; the
//! UI thread never waits on it.

use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use cv_base::platform::heap;
use cv_runtime::cache::memory;

/// A line this often (our number): 288 lines a day, about 100 KB.
const TICK: Duration = Duration::from_mins(5);
/// How often the thread looks at the private bytes for a rise (our number).
const CHECK: Duration = Duration::from_secs(10);
/// A rise of this much above the last step writes a line at once (our number).
const RISE_STEP: usize = 256 * 1024 * 1024;
/// The file moves aside at this size (our number): about ten days of ticks.
const ROTATE_AT: u64 = 1024 * 1024;

/// Starts the journal's thread, which writes to `file` for the rest of the process.
pub fn start(file: PathBuf, ctx: egui::Context) {
    let journal = Journal { file, rotate_at: ROTATE_AT };
    // Without the thread there is no journal; the app works the same.
    let _no_journal = std::thread::Builder::new().name("memory-journal".into()).spawn(move || {
        let started = Instant::now();
        let mut schedule = Schedule::new(started);
        loop {
            let now = Instant::now();
            if let Some(reason) = schedule.due(now, heap::private_bytes()) {
                let line =
                    Reading::take(&ctx).line(OffsetDateTime::now_utc(), now - started, reason);
                // A full disk or a locked file loses this line; the next one tries again.
                let _lost_line = journal.append(&line);
            }
            #[expect(
                clippy::disallowed_methods,
                reason = "the journal owns this thread and only waits between readings; no UI or task waits on it"
            )]
            std::thread::sleep(CHECK);
        }
    });
}

/// Why a line was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    Start,
    Tick,
    Rise,
}

impl Reason {
    fn label(self) -> &'static str {
        match self {
            Reason::Start => "start",
            Reason::Tick => "tick",
            Reason::Rise => "rise",
        }
    }
}

/// When the next lines are due.
struct Schedule {
    next_tick: Instant,
    /// Set by the first reading of the private bytes.
    rise: Option<Rise>,
    has_started: bool,
}

/// The private bytes rises count from, and those that write the next rise line.
struct Rise {
    base: usize,
    next: usize,
}

impl Schedule {
    fn new(now: Instant) -> Self {
        Self { next_tick: now + TICK, rise: None, has_started: false }
    }

    /// The reason for a line at `now` with `private` bytes, if one is due.
    fn due(&mut self, now: Instant, private: Option<usize>) -> Option<Reason> {
        let is_rise = self.is_rise(private);
        if !self.has_started {
            self.has_started = true;
            return Some(Reason::Start);
        }
        if now >= self.next_tick {
            while self.next_tick <= now {
                self.next_tick += TICK;
            }
            return Some(Reason::Tick);
        }
        is_rise.then_some(Reason::Rise)
    }

    /// Whether `private` passed the next step; moves the step past it.
    fn is_rise(&mut self, private: Option<usize>) -> bool {
        let Some(private) = private else { return false };
        let rise = self.rise.get_or_insert(Rise { base: private, next: private + RISE_STEP });
        if private < rise.next {
            return false;
        }
        rise.next = rise.base + ((private - rise.base) / RISE_STEP + 1) * RISE_STEP;
        true
    }
}

/// What the process holds at one moment.
struct Reading {
    private: Option<usize>,
    live: usize,
    holders: Vec<(&'static str, usize)>,
    textures: usize,
    texture_bytes: usize,
    handles: Option<u32>,
    threads: Option<u32>,
}

impl Reading {
    fn take(ctx: &egui::Context) -> Self {
        let (textures, texture_bytes) = {
            let manager = ctx.tex_manager();
            let manager = manager.read();
            (manager.num_allocated(), manager.allocated().map(|(_, meta)| meta.bytes_used()).sum())
        };
        Self {
            private: heap::private_bytes(),
            live: heap::live_bytes(),
            holders: memory::readings(),
            textures,
            texture_bytes,
            handles: heap::handle_count(),
            threads: heap::thread_count(),
        }
    }

    /// The journal line, without its line break.
    fn line(&self, wall: OffsetDateTime, uptime: Duration, reason: Reason) -> String {
        let wall = wall.replace_nanosecond(0).unwrap_or(wall);
        let wall = wall.format(&Rfc3339).unwrap_or_else(|_| "-".to_owned());
        let mut holders = String::new();
        for (label, bytes) in &self.holders {
            holders.push(' ');
            holders.push_str(&label.replace(' ', "_"));
            holders.push('=');
            holders.push_str(&bytes.to_string());
        }
        format!(
            "{wall} {} uptime_s={} private={} live={}{holders} textures={} texture_bytes={} \
             handles={} threads={}",
            reason.label(),
            uptime.as_secs(),
            or_dash(self.private),
            self.live,
            self.textures,
            self.texture_bytes,
            or_dash(self.handles),
            or_dash(self.threads),
        )
    }
}

/// The number, or `-` where the system gives none.
fn or_dash(value: Option<impl ToString>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

/// The journal file and its bound.
struct Journal {
    file: PathBuf,
    rotate_at: u64,
}

impl Journal {
    /// Appends `line`; a file at its bound first moves aside to [`Self::previous`].
    fn append(&self, line: &str) -> io::Result<()> {
        if fs::metadata(&self.file).is_ok_and(|meta| meta.len() >= self.rotate_at) {
            fs::rename(&self.file, self.previous())?;
        }
        if let Some(folder) = self.file.parent() {
            fs::create_dir_all(folder)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(&self.file)?;
        writeln!(file, "{line}")
    }

    /// `memory.log` → `memory.1.log`.
    fn previous(&self) -> PathBuf {
        let stem = self.file.file_stem().unwrap_or_default().to_string_lossy();
        self.file.with_file_name(format!("{stem}.1.log"))
    }
}

#[cfg(test)]
mod tests {
    use cv_testkit::ScratchDir;

    use super::*;

    const FIELDS: [&str; 17] = [
        "uptime_s",
        "private",
        "live",
        "images",
        "diagrams",
        "page_answers",
        "markdown",
        "grammars",
        "diffs",
        "file_texts",
        "diff_colors",
        "code_colors",
        "http_answers",
        "textures",
        "texture_bytes",
        "handles",
        "threads",
    ];

    /// The time, the reason and `name=number` (or `-`) pairs, nothing else.
    fn parse(line: &str) -> (OffsetDateTime, &str, Vec<(&str, Option<u64>)>) {
        let mut parts = line.split(' ');
        let wall =
            OffsetDateTime::parse(parts.next().expect("a time"), &Rfc3339).expect("RFC 3339");
        let reason = parts.next().expect("a reason");
        let fields = parts
            .map(|part| {
                let (name, value) = part.split_once('=').expect("name=value");
                let value = (value != "-").then(|| value.parse().expect("a number"));
                (name, value)
            })
            .collect();
        (wall, reason, fields)
    }

    #[test]
    fn writes_a_parseable_line_and_rotates_at_its_bound() {
        let scratch = ScratchDir::new("memory-journal");
        let file = scratch.path().join("logs").join("memory.log");
        let journal = Journal { file: file.clone(), rotate_at: 1_000 };
        let reading = Reading::take(&egui::Context::default());
        let wall = OffsetDateTime::now_utc();
        let line = reading.line(wall, Duration::from_secs(300), Reason::Tick);

        let (parsed_wall, reason, fields) = parse(&line);
        assert_eq!(parsed_wall.unix_timestamp(), wall.unix_timestamp());
        assert_eq!(reason, "tick");
        for name in FIELDS {
            assert!(fields.iter().any(|(field, _)| *field == name), "{name} in {line}");
        }
        assert!(fields.contains(&("uptime_s", Some(300))), "{line}");
        let live = fields.iter().find(|(name, _)| *name == "live").and_then(|(_, value)| *value);
        assert!(live.is_some_and(|live| live > 0), "the heap is counted: {line}");
        #[cfg(windows)]
        assert!(fields.iter().any(|(name, value)| *name == "private" && value.is_some()), "{line}");

        let mut lines_to_bound = 0;
        while fs::metadata(&file).map_or(0, |meta| meta.len()) < journal.rotate_at {
            journal.append(&line).expect("the line is written");
            lines_to_bound += 1;
            assert!(!journal.previous().exists(), "nothing moves aside below the bound");
        }
        journal.append(&line).expect("the line is written");
        let previous = fs::read_to_string(journal.previous()).expect("the previous file");
        assert_eq!(previous.lines().count(), lines_to_bound);
        assert!(previous.lines().all(|kept| kept == line));
        let current = fs::read_to_string(&file).expect("the journal");
        assert_eq!(current.lines().count(), 1, "the journal starts over");
        assert_eq!(
            journal.previous().file_name().and_then(|name| name.to_str()),
            Some("memory.1.log")
        );
    }

    #[test]
    fn writes_at_start_every_tick_and_on_each_new_rise_step() {
        const MB: usize = 1024 * 1024;
        let started = Instant::now();
        let mut schedule = Schedule::new(started);
        let at = |seconds: u64| started + Duration::from_secs(seconds);
        assert_eq!(schedule.due(at(0), Some(100 * MB)), Some(Reason::Start));
        assert_eq!(schedule.due(at(10), Some(300 * MB)), None, "below one step");
        assert_eq!(schedule.due(at(20), Some(400 * MB)), Some(Reason::Rise));
        assert_eq!(schedule.due(at(30), Some(500 * MB)), None, "within the same step");
        assert_eq!(schedule.due(at(40), Some(1_000 * MB)), Some(Reason::Rise), "two steps at once");
        assert_eq!(schedule.due(at(50), Some(1_100 * MB)), None);
        assert_eq!(schedule.due(at(TICK.as_secs()), Some(200 * MB)), Some(Reason::Tick));
        assert_eq!(schedule.due(at(TICK.as_secs() + 10), Some(200 * MB)), None);
        assert_eq!(
            schedule.due(at(TICK.as_secs() * 3), None),
            Some(Reason::Tick),
            "missed ticks fold"
        );
        assert_eq!(schedule.due(at(TICK.as_secs() * 3 + 10), None), None);
    }
}
