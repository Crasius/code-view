//! What keeps every test that builds `cv_services::Services` (each whole-app walkthrough
//! does) hermetic and bounded:
//!
//! - its data folders keep their tokens to themselves ([`assert_hermetic`]): no test reads, writes
//!   or waits on the system's token store (the login Keychain prompts when a rebuilt test binary
//!   reads an item an earlier build wrote, and blocks until someone answers);
//! - its providers reach loopback hosts only ([`http_client`]): a request to a host no mock
//!   answers fails at once, naming the host, instead of waiting on the real network; a mock
//!   server's origin starts without an earlier test's rate-limit pause ([`fresh_origin`]);
//! - a [`Watchdog`] stops the test binary with a message once a test runs past the watchdog limit
//!   (5 min, `REVIEW_TEST_WATCHDOG_SECS` overrides it) in real time, so a call that never returns
//!   fails the run instead of hanging it; the message says where the test was ([`step`],
//!   [`frame_started`]: the wait under way, its frames, the background work in flight, a panic it
//!   hung after);
//! - at most `PROCESS_SLOTS` such tests run at once (`take_process_slot`): the processes they
//!   start (git above all) are started one after another for the whole test binary, so every
//!   further test adds to each one's wait.
//!
//! The live checks (`--ignored`, real public hosts) call [`allow_real_network`] first; the live
//! write walkthroughs, which sign in with the owner's saved connections, call
//! [`allow_system_token_vault`].
//!
//! It also holds the few things every test that waits or builds files needs, so each has one
//! reason written down instead of a copy per test module: [`let_threads_run`] (the one way a test
//! gives real threads time), [`poll_until`] and [`poll_for`] (the one way a unit test waits for
//! another thread, bounded by [`WAIT_LIMIT`]), [`git`], [`fast_import`] and [`file_url`] (fixture
//! repositories), [`ScratchDir`] and [`remove_dir`], [`samples`] (model values a test fills in by
//! struct update), [`provider`] (the scripted provider of a test that is not about HTTP) and
//! [`use_bundled_fonts`] (the same text widths on every machine). [`Clicks`] is the one way a
//! kittest test clicks. [`frame_bench`] holds the headless frame-budget benchmarks.
//!
//! A dev-only crate: the crates above `cv-widgets` take it as a dev-dependency, `cv-services` also as an
//! optional dependency under its `test-support` feature (for its watchdog and hermetic checks).
//! Its statics and thread-locals (the process slots above all) exist once per test binary.
#![forbid(unsafe_code)]

mod clicks;
pub mod frame_bench;
pub mod provider;
pub mod samples;
mod trace;

pub use clicks::{Clicks, Drags};
pub use trace::{Step, frame_ended, frame_started, step};

use std::cell::{Cell, OnceCell};
use std::io::Write;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use review_store::{AppPaths, TokenVault};

/// How long one test may keep its services in real time; `REVIEW_TEST_WATCHDOG_SECS` overrides it.
/// The slowest walkthrough (`layout_audit_walkthrough`) takes about a minute in a debug build.
const WATCHDOG_LIMIT: Duration = Duration::from_secs(300);

/// How many tests may build fixture repositories and run the app's services at once; our own
/// budget, measured on Windows. There `std` starts one process at a time for the whole test binary
/// (a process-wide lock around `CreateProcessW`, so no child inherits another's pipes), and a
/// start takes about 12 ms on an idle machine and 100 ms on a busy one. With all 32 test threads
/// in walkthroughs each git start waited 1.6 s (median; 6.5 s at p99) and a walkthrough's chain of
/// git commands outlasted its minute; with 8 at once the median wait was 0.17 s.
const PROCESS_SLOTS: usize = 8;

/// Slots held by running tests, and the signal a test gives when it ends and frees its slot.
static SLOTS_TAKEN: Mutex<usize> = Mutex::new(0);
static SLOT_FREED: Condvar = Condvar::new();

thread_local! {
    /// Set by the live checks on their test thread.
    static REAL_NETWORK: Cell<bool> = const { Cell::new(false) };
    /// Set by the live write walkthroughs on their test thread.
    static SYSTEM_VAULT: Cell<bool> = const { Cell::new(false) };
    /// This test thread's slot, freed when the test (its thread) ends.
    static PROCESS_SLOT: OnceCell<ProcessSlot> = const { OnceCell::new() };
}

/// Waits until one of the [`PROCESS_SLOTS`] is free and keeps it for the rest of this test; a
/// test that holds one already goes on at once. The fixture [`git`] and the services' [`Watchdog`]
/// take it, so a walkthrough waits here before its repositories, its clock and its time limits
/// start.
fn take_process_slot() {
    PROCESS_SLOT.with(|slot| {
        slot.get_or_init(|| {
            let taken = SLOTS_TAKEN.lock().unwrap_or_else(PoisonError::into_inner);
            let mut taken = SLOT_FREED
                .wait_while(taken, |taken| *taken >= PROCESS_SLOTS)
                .unwrap_or_else(PoisonError::into_inner);
            *taken += 1;
            ProcessSlot
        });
    });
}

/// A taken slot; dropped with its test thread, also when the test panicked.
struct ProcessSlot;

impl Drop for ProcessSlot {
    fn drop(&mut self) {
        *SLOTS_TAKEN.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
        SLOT_FREED.notify_one();
    }
}

/// Lets the services built on this test thread reach real hosts and run without a watchdog.
pub fn allow_real_network() {
    REAL_NETWORK.with(|allowed| allowed.set(true));
}

fn is_real_network_allowed() -> bool {
    REAL_NETWORK.with(Cell::get)
}

/// Lets the services built on this test thread read the owner's tokens from the system's store,
/// and reach the real hosts they belong to. Only the live write walkthroughs (`--ignored`) call
/// it; every other test keeps failing [`assert_hermetic`] when its folders name that store.
pub fn allow_system_token_vault() {
    allow_real_network();
    SYSTEM_VAULT.with(|allowed| allowed.set(true));
}

/// Tests point the app at folders of their own; those never use the system's token store (they
/// keep tokens in the file, or refuse them to walk the setup's fix message), unless the test
/// thread opted in with [`allow_system_token_vault`].
pub fn assert_hermetic(paths: &AppPaths) {
    if SYSTEM_VAULT.with(Cell::get) {
        return;
    }
    assert_ne!(
        paths.token_vault(),
        TokenVault::System,
        "a test reached for the system's token store: build its folders with AppPaths::new, never AppPaths::from_environment"
    );
}

/// The app's HTTP client; in tests it resolves loopback names only (mock servers listen on
/// 127.0.0.1, which needs no resolving), unless [`allow_real_network`] was called.
pub fn http_client() -> reqwest::Client {
    if is_real_network_allowed() {
        return review_core::http::build_client();
    }
    review_core::http::client_builder()
        .dns_resolver(LoopbackOnly)
        .build()
        .expect("TLS backend must initialize")
}

/// Hands a test's mock server over without the rate-limit pauses an earlier test left on its
/// origin; call it right after the server starts. The request budget pauses a host's requests
/// after a 429 for the whole process, keyed by origin (`review_core::http::budget`), and
/// wiremock's `MockServer::start` hands out pooled servers again, port and all, once their test
/// ended. A test that ended with a pause (a `Retry-After` of 40 minutes it never retried) left
/// it to the next test on that server: its rows' checks waited out the pause, their updating edge
/// asked for every frame, and the harness's startup ran frames until the watchdog stopped the
/// binary (GitLab job 25128).
pub fn fresh_origin(server_uri: &str) {
    review_core::http::resume(server_uri.trim_end_matches('/'));
}

struct LoopbackOnly;

impl Resolve for LoopbackOnly {
    #[expect(clippy::print_stderr, reason = "the test's captured output names the unmocked host")]
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            if host.eq_ignore_ascii_case("localhost") {
                let addrs: Vec<SocketAddr> = ("127.0.0.1", 0).to_socket_addrs()?.collect();
                return Ok(Box::new(addrs.into_iter()) as Addrs);
            }
            let message =
                format!("tests reach mock servers on 127.0.0.1 only; nothing mocks {host}");
            eprintln!("{message}");
            Err(message.into())
        })
    }
}

/// [`WATCHDOG_LIMIT`], or `REVIEW_TEST_WATCHDOG_SECS` when set.
fn watchdog_limit() -> Duration {
    std::env::var("REVIEW_TEST_WATCHDOG_SECS")
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .map_or(WATCHDOG_LIMIT, Duration::from_secs)
}

/// Stops the test binary once the test that started it runs past the watchdog limit (5 min,
/// `REVIEW_TEST_WATCHDOG_SECS` overrides it); dropping it (the test's services go away) calls it
/// off.
pub struct Watchdog {
    _done: Option<mpsc::Sender<()>>,
}

impl Watchdog {
    pub fn start() -> Self {
        take_process_slot();
        if is_real_network_allowed() {
            return Self { _done: None };
        }
        let limit = watchdog_limit();
        let test = std::thread::current().name().unwrap_or("a test").to_owned();
        let trace = trace::current();
        trace::note_panics();
        let (done, waiting) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("test watchdog".to_owned())
            .spawn(move || {
                if waiting.recv_timeout(limit) == Err(RecvTimeoutError::Timeout) {
                    // Straight to the process's stderr: the test's captured output dies with it.
                    // A stderr that cannot be written to leaves only the exit code to tell.
                    let _unreported = writeln!(
                        std::io::stderr(),
                        "\n{test} still runs after {} s: something it waits on never returned (a system dialog such as a \
                         Keychain prompt, a lock, an unmocked host). Stopping the test binary instead of hanging; \
                         REVIEW_TEST_WATCHDOG_SECS raises the limit.{}",
                        limit.as_secs(),
                        trace.describe(Instant::now())
                    );
                    std::process::exit(101);
                }
            })
            .expect("start the test watchdog");
        Self { _done: Some(done) }
    }
}

/// Lets threads outside the test thread (the background runtime, a mock server, a renderer)
/// make progress in real time. Polling loops call it between checks. A walkthrough that calls it
/// for a fixed time with nothing to poll (an idle window, a frame interval) deviates from
/// RS-TEST-06; an injected clock would remove that wait.
#[expect(
    clippy::disallowed_methods,
    reason = "the walkthroughs drive real background threads and mock servers, which progress only in real time"
)]
pub fn let_threads_run(duration: Duration) {
    std::thread::sleep(duration);
}

/// How long any wait of a test may go on; our own budget. Below the watchdog's limit (300 s), so a
/// wait that never ends fails its own test and not the test binary. `review-app`'s walkthrough
/// waits use the same number.
pub const WAIT_LIMIT: Duration = Duration::from_secs(180);

/// The real time [`poll_for`] gives other threads between two checks.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Calls `is_done` until it holds, giving real threads 5 ms between checks ([`let_threads_run`]).
/// Panics with `what` after [`WAIT_LIMIT`]. Labelled for the watchdog's trace ([`step`]).
pub fn poll_until(what: &str, mut is_done: impl FnMut() -> bool) {
    poll_for(what, || is_done().then_some(()));
}

/// [`poll_until`] for a wait that hands something back: the first `Some` of `find`.
pub fn poll_for<T>(what: &str, mut find: impl FnMut() -> Option<T>) -> T {
    let _step = step(format!("waiting for {what}"));
    let deadline = Instant::now() + WAIT_LIMIT;
    loop {
        if let Some(found) = find() {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "{what} did not happen within {} s",
            WAIT_LIMIT.as_secs()
        );
        let_threads_run(POLL_INTERVAL);
    }
}

/// Swaps the machine's fonts for the app's bundled ones, for a test that asserts what fits beside
/// what: the system fonts differ in width between machines (DejaVu Sans, a common Linux UI font,
/// runs wider than Segoe UI), the bundled ones are the same everywhere.
pub fn use_bundled_fonts(ctx: &egui::Context) {
    ctx.set_fonts(cv_base::theme::fonts::definitions(&cv_base::platform::FontSources::default()));
}

/// Runs `git` in `dir` with a fixed identity and line endings, for fixture repositories; the
/// trimmed standard output. Panics with git's message when it fails.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir);
    git_output(command.args(GIT_FIXTURE_CONFIG).args(args), args)
}

/// `git` on the repository `git_dir` (`--git-dir`), or without one from the system's temp folder,
/// so git never reads the checkout the tests run from (a worktree whose `.git` file names a path
/// this machine does not have stops every git command started inside it). With the user's own
/// configuration (streams for `fast-import` name their own committer).
pub fn git_in(git_dir: Option<&Path>, args: &[&str]) -> String {
    let mut command = Command::new("git");
    match git_dir {
        Some(git_dir) => command.arg("--git-dir").arg(git_dir),
        None => command.current_dir(std::env::temp_dir()),
    };
    git_output(command.args(args), args)
}

/// Lets the app's mirror clone `repository` (a work tree, a bare repository or a git dir)
/// blob-less and fetch blobs by id, as review-git's mirror sets for itself.
pub fn serve_partial_clones(repository: &Path) {
    git(repository, &["config", "uploadpack.allowFilter", "true"]);
    git(repository, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
}

/// The same author, committer and line endings on every machine.
const GIT_FIXTURE_CONFIG: &[&str] =
    &["-c", "user.name=Test", "-c", "user.email=test@example.org", "-c", "core.autocrlf=false"];

#[expect(
    clippy::disallowed_methods,
    reason = "test fixtures build real repositories; the test thread owns the wait for git"
)]
fn git_output(command: &mut Command, args: &[&str]) -> String {
    take_process_slot();
    let output = command.output().expect("git runs");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Feeds `stream` to `git fast-import` in the repository `git_dir`. Panics with git's message.
pub fn fast_import(git_dir: &Path, stream: &[u8]) {
    take_process_slot();
    let mut child = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("git fast-import runs");
    // A git that stops early closes its input: its own message says why, not the failed write.
    let fed = child.stdin.take().expect("git fast-import's input").write_all(stream);
    let output = child.wait_with_output().expect("git fast-import ends");
    assert!(
        output.status.success(),
        "git fast-import: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fed.expect("feed git fast-import");
}

/// `path` as a `file:///` URL git clones from, with forward slashes on every system.
pub fn file_url(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    format!("file:///{}", path.trim_start_matches('/'))
}

/// A test's own folder under the system temp folder (`review-<name>-<process id>`): emptied when
/// made, removed when dropped.
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    pub fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("review-{name}-{}", std::process::id()));
        remove_dir(&path);
        std::fs::create_dir_all(&path).expect("scratch folder");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The app's data folders below this one: `config` and `cache`.
    pub fn app_paths(&self) -> AppPaths {
        AppPaths::new(self.0.join("config"), self.0.join("cache"))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        remove_dir(&self.0);
    }
}

/// Removes a test folder and everything in it; one that is not there is fine. A folder that
/// stays (a file still open on Windows) is reported, not failed: the next run empties it.
#[expect(
    clippy::print_stderr,
    reason = "the test's captured output reports the folder left behind"
)]
pub fn remove_dir(path: &Path) {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            eprintln!("could not remove {}: {error}", path.display());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use review_store::{AppPaths, TokenVault};

    use super::{
        ScratchDir, assert_hermetic, fast_import, file_url, git_in, http_client, poll_for,
    };

    #[test]
    fn poll_for_returns_what_another_thread_hands_over() {
        let answer = Arc::new(Mutex::new(None));
        let writer = Arc::clone(&answer);
        let worker = std::thread::spawn(move || {
            super::let_threads_run(Duration::from_millis(20));
            *writer.lock().unwrap() = Some(42);
        });
        assert_eq!(poll_for("the worker's answer", || *answer.lock().unwrap()), 42);
        worker.join().unwrap();
    }

    #[test]
    fn fast_import_writes_the_branch_of_a_two_commit_stream() {
        let scratch = ScratchDir::new("fast-import");
        let git_dir = scratch.path().join("origin.git");
        git_in(None, &["init", "--bare", "--quiet", &git_dir.display().to_string()]);
        let stream = b"commit refs/heads/main\nmark :1\ncommitter Test <test@example.org> 1700000001 +0000\n\
            data 5\nfirst\nM 100644 inline a.txt\ndata 2\na\n\n\
            commit refs/heads/main\nmark :2\ncommitter Test <test@example.org> 1700000002 +0000\n\
            data 6\nsecond\nfrom :1\nM 100644 inline a.txt\ndata 2\nb\n\n";
        fast_import(&git_dir, stream);

        let head = git_in(Some(&git_dir), &["rev-parse", "refs/heads/main"]);
        let parent = git_in(Some(&git_dir), &["rev-parse", "refs/heads/main~1"]);
        assert_eq!(head.len(), 40, "rev-parse names the branch's commit: {head}");
        assert_ne!(head, parent, "the branch holds both commits");
        let content = git_in(Some(&git_dir), &["show", "refs/heads/main:a.txt"]);
        assert_eq!(content, "b", "the branch's tree is the second commit's");
        let listed = git_in(None, &["ls-remote", &file_url(&git_dir), "refs/heads/main"]);
        assert!(listed.starts_with(&head), "git reaches the repository by its URL: {listed}");
    }

    #[test]
    #[should_panic(expected = "a test reached for the system's token store")]
    fn folders_on_the_system_token_store_fail_without_the_live_opt_in() {
        let paths =
            AppPaths::new("config".into(), "cache".into()).with_token_vault(TokenVault::System);
        assert_hermetic(&paths);
    }

    /// A 429 pauses its origin for the whole process; the next test on that origin (a pooled mock
    /// server) starts without the pause.
    #[test]
    fn a_fresh_origin_carries_no_pause_of_an_earlier_test() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let uri = format!("http://{}", listener.local_addr().expect("its address"));
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("the request");
            let mut request = [0_u8; 4096];
            let _read = stream.read(&mut request);
            stream
                .write_all(
                    b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 2391\r\nRateLimit-Remaining: 0\r\n\
                      Content-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("the answer");
        });
        let runtime =
            tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        let request = http_client().post(format!("{uri}/api/graphql"));
        let answer =
            runtime.block_on(review_core::http::without_waiting(review_core::http::send(request)));
        assert!(answer.is_err(), "the 429 is no answer");
        let is_paused =
            || review_core::http::rate_limits().iter().any(|notice| notice.origin == uri);
        assert!(is_paused(), "the 429 paused its origin");

        super::fresh_origin(&format!("{uri}/"));
        assert!(!is_paused(), "the next test finds the origin unpaused");
    }

    #[test]
    fn requests_to_unmocked_hosts_fail_at_once_and_name_the_host() {
        let runtime =
            tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        let error = runtime
            .block_on(async { http_client().get("https://api.github.com/user").send().await })
            .expect_err("no real network in tests");
        let chain = format!("{error:?}");
        assert!(chain.contains("nothing mocks api.github.com"), "{chain}");
    }
}
