//! The native messaging host: `CodeView.exe` started by the browser for the extension. It reads
//! framed requests on stdin until the browser closes it and answers each on stdout; it never opens
//! a window. Requests and answers are listed in `docs/browser-extension.md`.
//!
//! `check` locates the link, answers `pass` for a pull request the app itself just opened in the
//! browser ([`super::passes`]) and otherwise reads it once with its connection's token
//! (`link_check`); `open` only locates it and hands it over: the extension asks `open` after a
//! `check` said open, and the app checks the link again when it arrives, with a read whose detail
//! its page then reuses.
//!
//! What the host needs from the app sits behind [`AppSide`], so tests answer with connections and
//! reads of their own and record the hand-overs instead of starting Code View. Nothing the host
//! writes carries a token: answers hold states and fixed messages, and errors name files, not
//! contents.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::SystemTime;

use cv_runtime::route::PullRequestKey;
use cv_services::connection::providers::ProviderFactory;
use review_core::Connection;
use review_store::{AppPaths, ConnectionStore};
use serde_json::{Value, json};

use super::framing::{read_message, write_message};
use super::link_check::{self, LinkState};
use super::passes;
use crate::links::link_access::{self, Target};
use crate::single_instance::{self, Endpoint};

/// How a link reached Code View.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandOver {
    /// The running window took it.
    Forwarded,
    /// Code View was not running and starts with it.
    Started,
}

/// What the host reads from and does in the app.
pub trait AppSide {
    /// The saved connections, with their tokens, as the app loads them.
    fn connections(&self) -> io::Result<Vec<Connection>>;
    /// Reads the pull request `key` once through `connection`, within [`link_access::BUDGET`];
    /// `None` when no answer came in time.
    fn read(
        &self,
        connection: &Connection,
        key: &PullRequestKey,
    ) -> Option<review_core::Result<()>>;
    /// The app opened the pull request `key` in the browser within `super::passes::TTL`.
    fn is_passed(&self, key: &PullRequestKey) -> bool;
    /// Opens `url` in Code View.
    fn hand_over(&self, url: &str) -> io::Result<HandOver>;
}

/// The user's real data folders, hosts and Code View.
pub struct InstalledApp {
    pub paths: AppPaths,
    /// The client the reads go through (`review_core::http::build_client`; tests pass the
    /// loopback-only one).
    pub http: reqwest::Client,
    /// Builds the provider a read goes through. The bin passes `cv_providers::create_provider`:
    /// this crate does not link the provider crates.
    pub factory: ProviderFactory,
}

impl AppSide for InstalledApp {
    fn connections(&self) -> io::Result<Vec<Connection>> {
        ConnectionStore::open(&self.paths).map(|store| store.all().to_vec())
    }

    fn read(
        &self,
        connection: &Connection,
        key: &PullRequestKey,
    ) -> Option<review_core::Result<()>> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
        let provider = (self.factory)(connection.clone(), self.http.clone());
        // Inside the runtime: the timeout's timer needs its reactor from the start.
        let read = async {
            let read = provider.pull_request(&key.repository_key, key.number);
            tokio::time::timeout(link_access::BUDGET, read).await
        };
        // repo-lint: expect(RL013) the host has no window and no UI thread; this one read is its work
        let answer = runtime.block_on(read).ok()?;
        Some(answer.map(drop))
    }

    fn is_passed(&self, key: &PullRequestKey) -> bool {
        passes::contains(&passes::file(&self.paths), key, SystemTime::now())
    }

    fn hand_over(&self, url: &str) -> io::Result<HandOver> {
        match single_instance::send(&Endpoint::of(&self.paths), url) {
            Ok(()) => Ok(HandOver::Forwarded),
            Err(error) if single_instance::is_absent(&error) => {
                start_app(url)?;
                Ok(HandOver::Started)
            }
            Err(error) => Err(error),
        }
    }
}

/// Starts this executable as the app with `url`; the host exits right after, the app runs on.
fn start_app(url: &str) -> io::Result<()> {
    let exe: PathBuf = std::env::current_exe()?;
    // The browser's pipes stay with the host: the app gets none of them.
    let child = std::process::Command::new(exe)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    // Not waited for: the app outlives the host.
    drop(child);
    Ok(())
}

/// Answers requests from `origin` on `input` until it closes. Only the pinned extension's origin
/// is served; any other gets one `error` answer.
pub(crate) fn serve(
    origin: &str,
    input: &mut impl Read,
    output: &mut impl Write,
    app: &dyn AppSide,
) -> io::Result<()> {
    if origin != super::extension_origin() {
        let answer = error("This host answers only the Code View extension.");
        return write_message(output, answer.to_string().as_bytes());
    }
    loop {
        let message = match read_message(input) {
            Ok(Some(message)) => message,
            Ok(None) => return Ok(()),
            Err(failure) => {
                // The stream lost its framing: answer once and stop.
                let answer = error(&format!("Unreadable request: {failure}"));
                return write_message(output, answer.to_string().as_bytes());
            }
        };
        let answer = match serde_json::from_slice::<Value>(&message) {
            Ok(request) => answer(&request, app),
            Err(_) => error("The request is not JSON."),
        };
        write_message(output, answer.to_string().as_bytes())?;
    }
}

/// The answer to one request.
pub fn answer(request: &Value, app: &dyn AppSide) -> Value {
    let url = request.get("url").and_then(Value::as_str);
    match (request.get("type").and_then(Value::as_str), url) {
        (Some("ping"), _) => json!({ "type": "pong", "version": env!("CARGO_PKG_VERSION") }),
        (Some("check"), Some(url)) => check(url, app),
        (Some("open"), Some(url)) => open(url, app),
        (Some("check" | "open"), None) => error("The request names no url."),
        _ => error("Unknown request type."),
    }
}

fn check(url: &str, app: &dyn AppSide) -> Value {
    let connections = match connections(app) {
        Ok(connections) => connections,
        Err(message) => {
            return json!({ "type": "check", "state": "unavailable", "message": message });
        }
    };
    let is_passed = |key: &PullRequestKey| app.is_passed(key);
    match link_check::check(url, &connections, is_passed, |connection, key| {
        app.read(connection, key)
    }) {
        LinkState::NoAccess(denial) => json!({
            "type": "check",
            "state": "no_access",
            "denial": link_check::denial_name(denial),
        }),
        state => json!({ "type": "check", "state": state.as_str() }),
    }
}

/// Hands over a pull request of a saved connection; anything else is answered as `check` would,
/// without a read.
fn open(url: &str, app: &dyn AppSide) -> Value {
    let connections = match connections(app) {
        Ok(connections) => connections,
        Err(message) => {
            return json!({ "type": "open", "state": "unavailable", "message": message });
        }
    };
    let state = match link_access::locate(url, &connections) {
        Target::PullRequest { .. } => None,
        Target::NotConnected(_) => Some(LinkState::NotConnected),
        Target::NotAPullRequest => Some(LinkState::NotAPullRequest),
    };
    if let Some(state) = state {
        return json!({ "type": "open", "state": state.as_str() });
    }
    match app.hand_over(url) {
        Ok(HandOver::Forwarded) => json!({ "type": "open", "state": "forwarded" }),
        Ok(HandOver::Started) => json!({ "type": "open", "state": "started" }),
        Err(failure) => json!({
            "type": "error",
            "message": format!("Code View could not take the link: {failure}"),
        }),
    }
}

fn connections(app: &dyn AppSide) -> Result<Vec<Connection>, String> {
    app.connections()
        .map_err(|failure| format!("Code View could not read its connections: {failure}"))
}

fn error(message: &str) -> Value {
    json!({ "type": "error", "message": message })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::{self, Cursor};
    use std::time::SystemTime;

    use cv_runtime::route::PullRequestKey;
    use cv_testkit::{ScratchDir, samples};
    use review_core::{Connection, ProviderError, ProviderKind};
    use review_store::{AppPaths, ConnectionStore, TokenVault};
    use serde_json::{Value, json};

    use super::{AppSide, HandOver, InstalledApp, serve};
    use crate::browser::framing::{read_message, write_message};
    use crate::browser::{extension_origin, passes};

    const PULL_REQUEST: &str = "https://github.com/acme/widgets/pull/7";
    /// A pull request the scripted host answers with 404, as GitHub does for a hidden repository.
    const HIDDEN: &str = "https://github.com/acme/secret/pull/9";
    const SECRET: &str = "ghp_secret_token_value";

    /// Connections from a scratch folder (tokens in the file vault, never the system's), and the
    /// hand-overs recorded.
    struct ScratchApp {
        paths: AppPaths,
        handed: RefCell<Vec<String>>,
        reads: RefCell<Vec<u64>>,
        is_running: bool,
    }

    impl ScratchApp {
        fn new(scratch: &ScratchDir) -> Self {
            let paths = scratch.app_paths();
            assert_eq!(paths.token_vault(), TokenVault::File);
            let mut store = ConnectionStore::open(&paths).expect("empty store");
            store
                .upsert(Connection {
                    name: "Acme".to_owned(),
                    token: Some(SECRET.to_owned()),
                    ..samples::connection("gh", ProviderKind::GitHub, "https://github.com")
                })
                .expect("saved");
            Self { paths, handed: RefCell::default(), reads: RefCell::default(), is_running: true }
        }
    }

    impl AppSide for ScratchApp {
        fn connections(&self) -> io::Result<Vec<Connection>> {
            ConnectionStore::open(&self.paths).map(|store| store.all().to_vec())
        }

        fn read(
            &self,
            _connection: &Connection,
            key: &PullRequestKey,
        ) -> Option<review_core::Result<()>> {
            self.reads.borrow_mut().push(key.number);
            let hidden = ProviderError::Status {
                status: 404,
                path: "/pulls".to_owned(),
                body: String::new(),
            };
            Some(if key.repository_key == "acme/secret" { Err(hidden) } else { Ok(()) })
        }

        fn is_passed(&self, key: &PullRequestKey) -> bool {
            passes::contains(&passes::file(&self.paths), key, SystemTime::now())
        }

        fn hand_over(&self, url: &str) -> io::Result<HandOver> {
            self.handed.borrow_mut().push(url.to_owned());
            Ok(if self.is_running { HandOver::Forwarded } else { HandOver::Started })
        }
    }

    /// Frames `requests`, serves them from `origin` and returns the answers.
    fn exchange(origin: &str, requests: &[Value], app: &dyn AppSide) -> Vec<Value> {
        let mut input = Vec::new();
        for request in requests {
            write_message(&mut input, request.to_string().as_bytes()).expect("framed");
        }
        let mut output = Vec::new();
        serve(origin, &mut Cursor::new(input), &mut output, app).expect("served");
        let mut answers = Vec::new();
        let mut output = Cursor::new(output);
        while let Some(message) = read_message(&mut output).expect("framed answer") {
            answers.push(serde_json::from_slice(&message).expect("JSON answer"));
        }
        answers
    }

    #[test]
    fn answers_ping_check_and_open_in_order() {
        let scratch = ScratchDir::new("host-requests");
        let app = ScratchApp::new(&scratch);
        let requests = [
            json!({ "type": "ping" }),
            json!({ "type": "check", "url": PULL_REQUEST }),
            json!({ "type": "check", "url": "https://gitlab.com/a/b/-/merge_requests/1" }),
            json!({ "type": "check", "url": "https://example.org/" }),
            json!({ "type": "open", "url": PULL_REQUEST }),
            json!({ "type": "open", "url": "https://example.org/a/b/pull/1" }),
        ];
        let answers = exchange(&extension_origin(), &requests, &app);
        let states: Vec<Option<&str>> =
            answers.iter().map(|answer| answer.get("state").and_then(Value::as_str)).collect();
        assert_eq!(
            states,
            [
                None,
                Some("open"),
                Some("not_connected"),
                Some("not_a_pull_request"),
                Some("forwarded"),
                Some("not_a_pull_request")
            ]
        );
        assert_eq!(answers.first().and_then(|pong| pong.get("type")), Some(&json!("pong")));
        assert_eq!(*app.handed.borrow(), [PULL_REQUEST], "only the connected link is handed over");
        assert_eq!(*app.reads.borrow(), [7], "only check reads, and only the connected link");
        for answer in &answers {
            assert!(!answer.to_string().contains(SECRET), "no token in {answer}");
        }
    }

    #[test]
    fn a_pull_request_its_token_cannot_see_answers_no_access() {
        let scratch = ScratchDir::new("host-hidden");
        let app = ScratchApp::new(&scratch);
        let answers =
            exchange(&extension_origin(), &[json!({ "type": "check", "url": HIDDEN })], &app);
        assert_eq!(
            answers,
            [json!({ "type": "check", "state": "no_access", "denial": "not_found" })]
        );
        assert!(app.handed.borrow().is_empty(), "a refused link is not handed over");
    }

    #[test]
    fn a_pull_request_the_app_just_opened_answers_pass_without_a_read() {
        let scratch = ScratchDir::new("host-pass");
        let app = ScratchApp::new(&scratch);
        let key = PullRequestKey {
            connection_id: "gh".to_owned(),
            repository_key: "acme/secret".to_owned(),
            number: 9,
        };
        passes::record(&passes::file(&app.paths), &key, SystemTime::now()).expect("recorded");
        let requests = [
            json!({ "type": "check", "url": format!("{HIDDEN}/files#r1") }),
            json!({ "type": "check", "url": PULL_REQUEST }),
        ];
        let answers = exchange(&extension_origin(), &requests, &app);
        assert_eq!(
            answers,
            [
                json!({ "type": "check", "state": "pass" }),
                json!({ "type": "check", "state": "open" })
            ]
        );
        assert_eq!(*app.reads.borrow(), [7], "only the pull request without a pass is read");
    }

    #[test]
    fn starts_the_app_when_none_runs() {
        let scratch = ScratchDir::new("host-start");
        let app = ScratchApp { is_running: false, ..ScratchApp::new(&scratch) };
        let answers =
            exchange(&extension_origin(), &[json!({ "type": "open", "url": PULL_REQUEST })], &app);
        assert_eq!(answers, [json!({ "type": "open", "state": "started" })]);
    }

    #[test]
    fn refuses_other_origins_and_bad_requests() {
        let scratch = ScratchDir::new("host-refuse");
        let app = ScratchApp::new(&scratch);
        let stranger = exchange(
            "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/",
            &[json!({ "type": "open", "url": PULL_REQUEST })],
            &app,
        );
        assert_eq!(stranger.len(), 1);
        assert_eq!(stranger.first().and_then(|answer| answer.get("type")), Some(&json!("error")));
        assert!(app.handed.borrow().is_empty(), "nothing handed over for a stranger");

        let answers = exchange(
            &extension_origin(),
            &[
                json!({ "type": "open" }),
                json!({ "type": "delete", "url": PULL_REQUEST }),
                json!("ping"),
            ],
            &app,
        );
        assert_eq!(answers.len(), 3);
        assert!(
            answers.iter().all(|answer| answer.get("type") == Some(&json!("error"))),
            "{answers:?}"
        );
    }

    #[test]
    fn an_unreadable_connections_file_answers_unavailable() {
        let scratch = ScratchDir::new("host-unreadable");
        let app = ScratchApp::new(&scratch);
        std::fs::write(app.paths.connections_file(), b"{ not json").expect("corrupt");
        let answers =
            exchange(&extension_origin(), &[json!({ "type": "check", "url": PULL_REQUEST })], &app);
        assert_eq!(
            answers.first().and_then(|answer| answer.get("state")),
            Some(&json!("unavailable"))
        );
    }

    /// The real read runs its provider and timeout inside its own runtime: a 404 from a loopback
    /// host comes back as the host's answer (it once panicked with "no reactor running").
    ///
    /// The read runs on a plain thread, which has no runtime of its own: a timeout built outside
    /// the read's runtime then fails this test.
    #[tokio::test]
    async fn the_installed_app_reads_through_its_own_runtime() {
        let server = wiremock::MockServer::start().await;
        cv_testkit::fresh_origin(&server.uri());
        let scratch = ScratchDir::new("host-installed-read");
        let app = InstalledApp {
            paths: scratch.app_paths(),
            http: cv_testkit::http_client(),
            factory: cv_providers::create_provider,
        };
        let connection = Connection {
            token: Some(SECRET.to_owned()),
            ..samples::connection("ghe", ProviderKind::GitHub, &server.uri())
        };
        let key = PullRequestKey {
            connection_id: "ghe".to_owned(),
            repository_key: "acme/secret".to_owned(),
            number: 9,
        };
        let answer =
            std::thread::spawn(move || app.read(&connection, &key)).join().expect("the read ran");
        let status = answer.and_then(Result::err).as_ref().and_then(ProviderError::status);
        assert_eq!(status, Some(404));
    }
}
