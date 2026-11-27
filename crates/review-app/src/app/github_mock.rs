//! A GitHub Enterprise Server stand-in for the end-to-end tests. wiremock answers the REST v3 and
//! GraphQL calls of the real GitHub provider (`{base}/api/v3`, `{base}/api/graphql`) and serves a
//! local repository over git's smart HTTP protocol at `{base}/{owner}/{name}.git`, so the mirror
//! fetches it like any remote. No network and no real token: everything authenticates with
//! [`TOKEN`].

use std::cell::Cell;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, PoisonError};

use cv_testkit::{Clicks, ScratchDir, git, samples};
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{Connection, ProviderKind};
use serde_json::{Value, json};
use tokio::runtime::Runtime;
use wiremock::matchers::{body_string_contains, header, method, path, path_regex};
use wiremock::{Mock, MockBuilder, MockServer, Request, Respond, ResponseTemplate};

use super::ReviewApp;
use super::walkthrough::{files_tab, open_in_tree, shows_file_view, wait_until};

pub const TOKEN: &str = "test-token";
/// `Basic base64("x-access-token:test-token")`, how the provider hands the token to git.
const GIT_AUTHORIZATION: &str = "Basic eC1hY2Nlc3MtdG9rZW46dGVzdC10b2tlbg==";
pub const OWNER: &str = "acme";
pub const REPOSITORY: &str = "acme/widgets";
pub const NUMBER: u64 = 7;
pub const TITLE: &str = "Rename the greeting";
pub const FILE: &str = "src/lib.rs";
/// GraphQL node id of the review thread that already exists on line 3.
pub const THREAD_ID: &str = "PRRT_existing";
pub const THREAD_TEXT: &str = "Should this stay public?";
/// The current thread's line and text after [`GitHubHost::force_push_outdating_the_thread`].
pub const CURRENT_LINE: u32 = 20;
pub const CURRENT_TEXT: &str = "Is this line still needed?";

/// Opens the pull request from its row in "My pull requests" and waits for its tabs.
pub fn open_pull_request(harness: &mut Harness<'_, ReviewApp>) {
    click_pull_request_row(harness);
    wait_until(harness, "the pull request", |harness| {
        harness.query_by_role_and_label(Role::Tab, "Overview").is_some()
    });
}

/// Clicks the pull request's row in "My pull requests" once it shows.
pub fn click_pull_request_row(harness: &mut Harness<'_, ReviewApp>) {
    let row = format!("{NUMBER} {TITLE}");
    wait_until(harness, "the pull request row", |harness| harness.query_by_label(&row).is_some());
    harness.click_at(harness.get_by_label(&row).rect());
}

/// Opens [`FILE`] in the Files tab once the mirror fetched it; returns the diff's area.
pub fn open_diff(harness: &mut Harness<'_, ReviewApp>) -> egui::Rect {
    files_tab(harness);
    open_in_tree(harness, FILE);
    // The existing thread's card sits under line 3 once the diff rows are built.
    wait_until(harness, "the diff", |harness| {
        shows_file_view(harness, FILE) && harness.query_by_label(THREAD_TEXT).is_some()
    });
    harness.get_by_label(&format!("Diff of {FILE}")).rect()
}

const TIMESTAMP: &str = "2026-10-01T10:00:00Z";
const BASE_TEXT: &str = "pub fn greet() -> &'static str {\n    \"Hello\"\n}\n\npub fn farewell() -> &'static str {\n    \"Bye\"\n}\n";
const HEAD_TEXT: &str = "pub fn greeting() -> &'static str {\n    \"Hello, world\"\n}\n\npub fn farewell() -> &'static str {\n    \"Bye\"\n}\n";

/// wiremock's servers run on their own threads; this runtime only mounts mocks and reads requests.
static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| Runtime::new().expect("tokio runtime"));

pub struct GitHubHost {
    server: MockServer,
    owner: String,
    /// Head commit of pull request [`NUMBER`], when the host has it.
    head_sha: Option<String>,
    /// The repository behind the pull request, for pushes during a test.
    origin: Option<Origin>,
    /// Priority of the next mock that replaces an answer (wiremock prefers lower numbers; 5 is the default).
    next_priority: Cell<u8>,
    /// In front of the server once [`Self::hold_batch_downloads`] was called.
    relay: Option<Relay>,
}

impl GitHubHost {
    /// A host whose "mine" search lists `rows` (see [`search_row`]) for the organization `owner`.
    pub fn start(owner: &str, rows: &[Value]) -> Self {
        let server = RUNTIME.block_on(MockServer::start());
        cv_testkit::fresh_origin(&server.uri());
        let host = Self {
            server,
            owner: owner.to_owned(),
            head_sha: None,
            origin: None,
            next_priority: Cell::new(4),
            relay: None,
        };
        host.mount(
            api("GET", &format!("/users/{owner}"))
                .respond_with(ok(json!({ "login": owner, "type": "Organization" }))),
        );
        let search = json!({ "data": { "search": { "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": rows } } });
        host.mount(graphql("search(type: ISSUE").respond_with(ok(search)));
        host
    }

    /// A host with pull request [`NUMBER`] in [`REPOSITORY`]: its detail, one review thread, an empty
    /// pipeline, a label catalog, the repository over git, and successful answers to every write.
    pub fn with_pull_request(scratch: &Path) -> Self {
        Self::with_pull_request_files(scratch, &[])
    }

    /// [`Self::with_pull_request`] whose pull request also changes `files`, next to [`FILE`].
    pub fn with_pull_request_files(scratch: &Path, files: &[ExtraFile]) -> Self {
        let mut host = Self::start(OWNER, &[search_row(NUMBER, TITLE, REPOSITORY)]);
        let origin = Origin::create(scratch, files);
        host.mount_pull_request(&origin);
        host.mount_writes();
        // git authenticates as `x-access-token` with the connection's token.
        host.mount(
            Mock::given(path_regex(format!(r"^/{REPOSITORY}\.git/")))
                .and(header("authorization", GIT_AUTHORIZATION))
                .respond_with(SmartGit { project_root: origin.project_root.clone() }),
        );
        host.head_sha = Some(origin.head.clone());
        host.origin = Some(origin);
        host
    }

    /// From now on the answer to every large fetch (one chunk of the line counts' blob downloads)
    /// waits until the returned hold lets it through, opens or is dropped, as on a slow host; every
    /// other request, the few blobs of one file among them, is answered at once. The connections made
    /// after this reach the server through a [`Relay`], so call it before [`Self::connection`].
    pub fn hold_batch_downloads(&mut self) -> HeldDownloads {
        let gate = Arc::new(Gate::default());
        self.relay = Some(Relay::start(*self.server.address(), Arc::clone(&gate)));
        HeldDownloads(gate)
    }

    /// Someone replies on the existing thread: the pull request's `updated_at` moves and the
    /// thread gains the comment. Live updates pick it up with their next probe.
    pub fn publish_reply(&self, author: &str, body: &str) {
        const REPLIED_AT: &str = "2026-10-01T11:00:00Z";
        let origin = self.origin.as_ref().expect("a host with a pull request");
        self.answer_pull(ok(pull_json(
            &self.server.uri(),
            &origin.base,
            self.head_sha(),
            REPLIED_AT,
        )));
        let mut reply = comment(777, body, author, 77);
        reply["createdAt"] = json!(REPLIED_AT);
        self.mount(
            graphql("reviewThreads(first: 100")
                .respond_with(ok(review_threads(vec![reply])))
                .with_priority(self.take_priority()),
        );
    }

    /// Besides the thread on line 3, the pull request has one current thread by cy on each of
    /// `threads`, on its head lines (before the page opens).
    pub fn add_threads(&self, threads: &[MockThread]) {
        let mut nodes = existing_thread_nodes();
        for (index, thread) in threads.iter().enumerate() {
            let id = 801 + index as u64;
            nodes.push(json!({
                "id": thread.id, "isResolved": false, "isOutdated": false, "path": thread.path,
                "line": thread.lines.end(), "originalLine": thread.lines.end(), "diffSide": "RIGHT",
                "startLine": thread.lines.start(), "originalStartLine": thread.lines.start(),
                "startDiffSide": "RIGHT",
                "comments": { "pageInfo": { "hasNextPage": false, "endCursor": null },
                    "nodes": [comment(id, thread.text, "cy", 3)] }
            }));
        }
        self.mount(
            graphql("reviewThreads(first: 100")
                .respond_with(ok(thread_page(&Value::Array(nodes))))
                .with_priority(self.take_priority()),
        );
    }

    /// Besides the thread on line 3, the pull request has one review thread by "me" (see
    /// [`Self::sign_in_as_me`]) per entry of `comments` (before the page opens). Editing one
    /// (`PATCH /pulls/comments/{id}`) or deleting it (`DELETE`) changes what the threads answer
    /// from then on, as on GitHub: a thread whose last comment is deleted is gone.
    pub fn add_own_comments(&self, comments: &[OwnComment]) {
        let nodes = comments
            .iter()
            .map(|own| {
                json!({
                    "id": own.thread_id, "isResolved": false, "isOutdated": false, "path": FILE,
                    "line": own.line, "originalLine": own.line, "diffSide": "RIGHT",
                    "comments": { "pageInfo": { "hasNextPage": false, "endCursor": null },
                        "nodes": [comment(own.comment_id, own.text, "me", 9)] }
                })
            })
            .collect();
        let own = OwnThreads(Arc::new(Mutex::new(nodes)));
        self.mount(
            graphql("reviewThreads(first: 100")
                .respond_with(own.clone())
                .with_priority(self.take_priority()),
        );
        self.mount(comment_api("PATCH").respond_with(OwnEdit(own.clone())));
        self.mount(comment_api("DELETE").respond_with(OwnDelete(own)));
    }

    /// The next edit of a review comment is refused with 500; the ones after it go through.
    pub fn refuse_next_comment_edit(&self) {
        let refused =
            ResponseTemplate::new(500).set_body_json(json!({ "message": "Server Error" }));
        self.mount(comment_api("PATCH").respond_with(refused).up_to_n_times(1).with_priority(1));
    }

    /// The pull request's description becomes `body` (before the page opens).
    pub fn describe(&self, body: &str) {
        let origin = self.origin.as_ref().expect("a host with a pull request");
        let mut pull = pull_json(&self.server.uri(), &origin.base, self.head_sha(), TIMESTAMP);
        pull["body"] = json!(body);
        self.answer_pull(ok(pull));
    }

    /// `GET /user` names the token's user: "me" (id 9), the author of the search rows.
    pub fn sign_in_as_me(&self) {
        self.mount(
            api("GET", "/user").respond_with(ok(json!({ "id": 9, "login": "me", "name": "Me" }))),
        );
    }

    /// `GET /user` names the token's user: "mona" (id 1), the author of pull request [`NUMBER`].
    pub fn sign_in_as_author(&self) {
        self.mount(
            api("GET", "/user")
                .respond_with(ok(json!({ "id": 1, "login": "mona", "name": "Mona" }))),
        );
    }

    /// "me" reviewed the pull request with `state` (`APPROVED`, `CHANGES_REQUESTED`).
    pub fn my_review(&self, state: &str) {
        let review = json!({ "id": 31, "user": { "id": 9, "login": "me" }, "state": state, "body": "", "submitted_at": TIMESTAMP });
        let priority = self.take_priority();
        self.mount(
            repository_api("GET", &format!("/pulls/{NUMBER}/reviews"))
                .respond_with(ok(json!([review])))
                .with_priority(priority),
        );
    }

    /// Merging is refused with 403, as for a token without write access.
    pub fn refuse_merge(&self) {
        self.mount(
            repository_api("PUT", &format!("/pulls/{NUMBER}/merge")).respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(json!({ "message": "Resource not accessible by integration" })),
            ),
        );
    }

    /// `GET /pulls/7` fails with 500 from now on, until [`Self::restore_pull_request`].
    pub fn fail_pull_request(&self) {
        self.answer_pull(
            ResponseTemplate::new(500).set_body_json(json!({ "message": "Server Error" })),
        );
    }

    /// `GET /pulls/7` answers again, after [`Self::fail_pull_request`].
    pub fn restore_pull_request(&self) {
        let origin = self.origin.as_ref().expect("a host with a pull request");
        self.answer_pull(ok(pull_json(
            &self.server.uri(),
            &origin.base,
            self.head_sha(),
            TIMESTAMP,
        )));
    }

    /// `GET /pulls/7` reads after `delay` (before the page opens), with no reviews: a slow host.
    /// Both answers outrank those a host with a pull request mounted.
    pub fn delay_detail(&self, delay: std::time::Duration) {
        let priority = self.take_priority();
        self.mount(
            repository_api("GET", &format!("/pulls/{NUMBER}"))
                .respond_with(ok(json!({})).set_delay(delay))
                .with_priority(priority),
        );
        self.mount(
            repository_api("GET", &format!("/pulls/{NUMBER}/reviews"))
                .respond_with(ok(json!([])))
                .with_priority(priority),
        );
    }

    /// `POST /markdown` with `token` renders an image as GitHub does, its `<img>` pointing at
    /// `source`: the signed address of an attachment the token reaches, else the address as written.
    pub fn render_images_as(&self, token: &str, source: &str) {
        let html = format!(
            r#"<p dir="auto"><a target="_blank" rel="noopener noreferrer" href="{source}"><img src="{source}" style="max-width: 100%;"></a></p>"#
        );
        self.mount(
            Mock::given(method("POST"))
                .and(path("/api/v3/markdown"))
                .and(header("authorization", format!("Bearer {token}").as_str()))
                .respond_with(
                    ResponseTemplate::new(200).set_body_raw(html, "text/html;charset=utf-8"),
                ),
        );
    }

    /// The author pushes a commit that puts `lines` on top of [`FILE`]: the origin gets it on
    /// `refs/pull/7/head` and the pull request names the new head. Returns the new head.
    pub fn push_lines_on_top(&mut self, lines: &str) -> String {
        let origin = self.origin.as_ref().expect("a host with a pull request");
        let file = origin.work.join(FILE);
        let text = std::fs::read_to_string(&file).expect("head file");
        std::fs::write(&file, format!("{lines}{text}")).expect("pushed file");
        git(&origin.work, &["commit", "-q", "-am", "Document the greeting"]);
        let bare = origin.project_root.join(format!("{REPOSITORY}.git"));
        let bare_path = bare.to_string_lossy().into_owned();
        git(&origin.work, &["push", "-q", &bare_path, "feature"]);
        let head = git(&bare, &["rev-parse", "feature"]);
        git(&bare, &["update-ref", &format!("refs/pull/{NUMBER}/head"), &head]);
        self.publish_head(&head, "2026-10-01T12:00:00Z");
        head
    }

    /// The author rewrites the branch: one commit on `main` puts `text` into [`FILE`] and is force
    /// pushed, and the review thread turns outdated, written on the replaced head (its original
    /// line 1); a current thread by mona sits on [`CURRENT_LINE`] of the new head. With
    /// [`ReplacedHead::Collected`] the host collects the replaced commit as garbage, as GitHub may
    /// after a force push. Returns the replaced head.
    pub fn force_push_outdating_the_thread(
        &mut self,
        text: &str,
        replaced_head: ReplacedHead,
    ) -> String {
        let origin = self.origin.as_ref().expect("a host with a pull request");
        let replaced = self.head_sha().to_owned();
        git(&origin.work, &["reset", "-q", "--hard", "main"]);
        std::fs::write(origin.work.join(FILE), text).expect("rewritten file");
        git(&origin.work, &["commit", "-q", "-am", "Rename and document the greeting"]);
        let bare = origin.project_root.join(format!("{REPOSITORY}.git"));
        let bare_path = bare.to_string_lossy().into_owned();
        git(&origin.work, &["push", "-q", "--force", &bare_path, "feature"]);
        let head = git(&bare, &["rev-parse", "feature"]);
        git(&bare, &["update-ref", &format!("refs/pull/{NUMBER}/head"), &head]);
        if replaced_head == ReplacedHead::Collected {
            git(&bare, &["reflog", "expire", "--expire=now", "--all"]);
            git(&bare, &["gc", "-q", "--prune=now"]);
        }
        let comment_url =
            format!("{}/{REPOSITORY}/pull/{NUMBER}#discussion_r501", self.server.uri());
        let comments = |id: u64, body: &str, login: &str| {
            json!({ "pageInfo": { "hasNextPage": false, "endCursor": null },
                "nodes": [comment(id, body, login, id)] })
        };
        let threads = thread_page(&json!([{
            "id": THREAD_ID, "isResolved": false, "isOutdated": true, "path": FILE, "line": null, "originalLine": 1,
            "diffSide": "RIGHT",
            "origin": { "nodes": [{ "originalCommit": { "oid": replaced }, "url": comment_url,
                "diffHunk": "@@ -1,3 +1,3 @@\n-pub fn greet() -> &'static str {\n+pub fn greeting() -> &'static str {" }] },
            "comments": comments(501, THREAD_TEXT, "cy")
        }, {
            "id": "PRRT_current", "isResolved": false, "isOutdated": false, "path": FILE, "line": CURRENT_LINE,
            "originalLine": CURRENT_LINE, "diffSide": "RIGHT",
            "origin": { "nodes": [{ "originalCommit": { "oid": head }, "url": null, "diffHunk": "" }] },
            "comments": comments(502, CURRENT_TEXT, "mona")
        }]));
        let priority = self.take_priority();
        self.mount(
            graphql("reviewThreads(first: 100").respond_with(ok(threads)).with_priority(priority),
        );
        self.publish_head(&head, "2026-10-01T12:00:00Z");
        replaced
    }

    /// Merging succeeds.
    pub fn accept_merge(&self) {
        self.mount(
            repository_api("PUT", &format!("/pulls/{NUMBER}/merge"))
                .respond_with(ok(json!({ "merged": true, "sha": self.head_sha() }))),
        );
    }

    /// The pull request is merged on the host: `GET /pulls/7` answers it merged from now on.
    pub fn answer_merged(&self) {
        let origin = self.origin.as_ref().expect("a host with a pull request");
        let merged_at = "2026-10-02T12:00:00Z";
        let mut pull = pull_json(&self.server.uri(), &origin.base, self.head_sha(), merged_at);
        pull["state"] = json!("closed");
        pull["merged"] = json!(true);
        pull["merged_at"] = json!(merged_at);
        pull["closed_at"] = json!(merged_at);
        self.answer_pull(ok(pull));
    }

    /// The pull request names `head` from now on, with an empty pipeline for it.
    fn publish_head(&mut self, head: &str, updated_at: &str) {
        let origin = self.origin.as_ref().expect("a host with a pull request");
        self.answer_pull(ok(pull_json(&self.server.uri(), &origin.base, head, updated_at)));
        self.mount_empty_checks(head);
        self.head_sha = Some(head.to_owned());
    }

    /// `GET /pulls/7` answers `response` from now on, outranking the answer before.
    fn answer_pull(&self, response: ResponseTemplate) {
        self.mount(
            repository_api("GET", &format!("/pulls/{NUMBER}"))
                .respond_with(response)
                .with_priority(self.take_priority()),
        );
    }

    /// The commit `head` has no check runs and a successful status without entries.
    fn mount_empty_checks(&self, head: &str) {
        let commit = format!("/commits/{head}");
        self.mount(
            repository_api("GET", &format!("{commit}/check-runs"))
                .respond_with(ok(json!({ "total_count": 0, "check_runs": [] }))),
        );
        self.mount(
            repository_api("GET", &format!("{commit}/status"))
                .respond_with(ok(json!({ "state": "success", "statuses": [] }))),
        );
    }

    /// The head commit gets a finished Actions run "CI" with one failed job, `job`, whose log is
    /// `log`; the Pipelines tab lists the run and the job. The job failed in an earlier attempt
    /// too, and starting a rerun is refused (403), as for a token without the permission. Used by
    /// the focus mode walkthrough, which runs with the Windows title bar.
    #[cfg(windows)]
    pub fn add_workflow_job(&self, job: &str, log: &str) {
        let (run_id, job_id, earlier_id) = (900, 901, 899);
        let uri = self.server.uri();
        let run = json!({
            "id": run_id, "name": "CI", "display_title": TITLE, "status": "completed", "conclusion": "failure",
            "event": "pull_request", "html_url": format!("{uri}/{REPOSITORY}/actions/runs/{run_id}"),
            "path": ".github/workflows/ci.yml", "head_sha": self.head_sha(),
            "run_started_at": TIMESTAMP, "updated_at": "2026-10-01T10:04:00Z"
        });
        let priority = self.take_priority();
        self.mount(
            repository_api("GET", "/actions/runs")
                .respond_with(ok(json!({ "total_count": 1, "workflow_runs": [run] })))
                .with_priority(priority),
        );
        let job_json = |id: u64, attempt: u32| {
            json!({
                "id": id, "name": job, "status": "completed", "conclusion": "failure", "run_attempt": attempt,
                "started_at": TIMESTAMP, "completed_at": format!("2026-10-01T10:0{}:00Z", attempt * 2),
                "html_url": format!("{uri}/{REPOSITORY}/actions/runs/{run_id}/job/{id}"), "steps": []
            })
        };
        let jobs =
            json!({ "total_count": 2, "jobs": [job_json(earlier_id, 1), job_json(job_id, 2)] });
        self.mount(
            repository_api("GET", &format!("/actions/runs/{run_id}/jobs")).respond_with(ok(jobs)),
        );
        for id in [job_id, earlier_id] {
            self.mount(repository_api("GET", &format!("/actions/jobs/{id}/logs")).respond_with(
                ResponseTemplate::new(200).set_body_raw(log.as_bytes().to_vec(), "text/plain"),
            ));
        }
        let refused = ResponseTemplate::new(403)
            .set_body_json(json!({ "message": "Resource not accessible by integration" }));
        for path in [
            format!("/actions/jobs/{job_id}/rerun"),
            format!("/actions/runs/{run_id}/rerun-failed-jobs"),
            format!("/actions/runs/{run_id}/rerun"),
        ] {
            self.mount(repository_api("POST", &path).respond_with(refused.clone()));
        }
    }

    /// Each replaced answer outranks the one before it.
    fn take_priority(&self) -> u8 {
        let priority = self.next_priority.get();
        assert!(priority >= 1, "at most four replaced answers per test");
        self.next_priority.set(priority - 1);
        priority
    }

    pub fn connection(&self, id: &str, name: &str) -> Connection {
        // Any host but github.com is GitHub Enterprise Server: REST below /api/v3, GraphQL at /api/graphql.
        let base_url =
            self.relay.as_ref().map_or_else(|| self.server.uri(), |relay| relay.uri.clone());
        Connection {
            name: name.to_owned(),
            organizations: vec![self.owner.clone()],
            token: Some(TOKEN.to_owned()),
            ..samples::connection(id, ProviderKind::GitHub, &base_url)
        }
    }

    pub fn head_sha(&self) -> &str {
        self.head_sha.as_deref().expect("a host with a pull request")
    }

    pub fn requests(&self) -> Vec<Request> {
        RUNTIME.block_on(self.server.received_requests()).expect("request recording is on")
    }

    /// The first request matching `verb` and `request_path` (below the server root).
    pub fn find(&self, verb: &str, request_path: &str) -> Option<Request> {
        self.matching(verb, request_path).next()
    }

    /// The requests matching `verb` and `request_path` (below the server root) so far.
    pub fn count(&self, verb: &str, request_path: &str) -> usize {
        self.matching(verb, request_path).count()
    }

    /// The JSON bodies of the requests matching `verb` and `request_path`, in order.
    pub fn bodies(&self, verb: &str, request_path: &str) -> Vec<Value> {
        self.matching(verb, request_path)
            .map(|request| request.body_json().expect("a JSON body"))
            .collect()
    }

    fn matching(&self, verb: &str, request_path: &str) -> impl Iterator<Item = Request> {
        let (verb, request_path) = (verb.to_owned(), request_path.to_owned());
        self.requests().into_iter().filter(move |request| {
            request.method.as_str() == verb && request.url.path() == request_path
        })
    }

    /// The first GraphQL request whose document contains `needle`, decoded.
    pub fn find_graphql(&self, needle: &str) -> Option<Value> {
        self.requests()
            .into_iter()
            .filter(|request| {
                request.method.as_str() == "POST" && request.url.path() == "/api/graphql"
            })
            .filter_map(|request| request.body_json::<Value>().ok())
            .find(|body| body["query"].as_str().is_some_and(|query| query.contains(needle)))
    }

    fn mount(&self, mock: Mock) {
        RUNTIME.block_on(mock.mount(&self.server));
    }

    fn mount_pull_request(&self, origin: &Origin) {
        let pull = pull_json(&self.server.uri(), &origin.base, &origin.head, TIMESTAMP);
        self.mount(repository_api("GET", &format!("/pulls/{NUMBER}")).respond_with(ok(pull)));
        self.mount(
            repository_api("GET", &format!("/pulls/{NUMBER}/reviews")).respond_with(ok(json!([]))),
        );
        self.mount(
            repository_api("GET", &format!("/issues/{NUMBER}/comments"))
                .respond_with(ok(json!([]))),
        );
        self.mount(
            graphql("reviewThreads(first: 100").respond_with(ok(review_threads(Vec::new()))),
        );
        // The lookup after creating a thread finds nothing yet, as when GitHub's index lags; the
        // provider then falls back to the REST comment it just created.
        self.mount(graphql("reviewThreads(last: 20").respond_with(ok(thread_page(&json!([])))));
        self.mount(
            repository_api("GET", "/actions/runs")
                .respond_with(ok(json!({ "total_count": 0, "workflow_runs": [] }))),
        );
        self.mount_empty_checks(&origin.head);
        self.mount(repository_api("GET", "/labels").respond_with(ok(json!([
            { "name": "bug", "color": "d73a4a", "description": "Something is broken" },
            { "name": "enhancement", "color": "a2eeef", "description": "New feature or request" },
        ]))));
    }

    fn mount_writes(&self) {
        let created = json!({
            "id": 601, "path": FILE, "line": 1, "side": "RIGHT", "subject_type": "line",
            "user": { "id": 9, "login": "me" }, "body": "created", "created_at": TIMESTAMP
        });
        self.mount(
            repository_api("POST", &format!("/pulls/{NUMBER}/comments"))
                .respond_with(ResponseTemplate::new(201).set_body_json(created)),
        );
        self.mount(
            repository_api("POST", &format!("/pulls/{NUMBER}/reviews"))
                .respond_with(ok(json!({ "id": 1, "state": "APPROVED" }))),
        );
        self.mount(
            repository_api("PUT", &format!("/issues/{NUMBER}/labels")).respond_with(ok(json!([]))),
        );
        let reply = json!({ "data": { "addPullRequestReviewThreadReply": { "comment": {
            "id": "PRRC_502", "databaseId": 502, "body": "reply", "createdAt": TIMESTAMP,
            "lastEditedAt": null, "author": { "login": "me", "databaseId": 9 }
        } } } });
        self.mount(graphql("addPullRequestReviewThreadReply").respond_with(ok(reply)));
    }

    /// Answers for links to other pull requests of [`REPOSITORY`], detail and reviews alike:
    /// [`READABLE_NUMBER`] reads like [`NUMBER`]; [`HIDDEN_NUMBER`] gets 404 and
    /// [`FORBIDDEN_NUMBER`] 403, as GitHub answers a token that cannot see or read a private
    /// repository; [`FAILING_NUMBER`] gets 500, and [`SLOW_NUMBER`] reads after `slow_by`.
    pub fn add_linked_pull_requests(&self, slow_by: std::time::Duration) {
        let origin = self.origin.as_ref().expect("a host with a pull request");
        let readable = |number: u64| {
            let mut pull = pull_json(&self.server.uri(), &origin.base, self.head_sha(), TIMESTAMP);
            pull["number"] = json!(number);
            pull["html_url"] = json!(format!("{}/{REPOSITORY}/pull/{number}", self.server.uri()));
            pull
        };
        let answers = [
            (READABLE_NUMBER, ok(readable(READABLE_NUMBER)), ok(json!([]))),
            (SLOW_NUMBER, ok(readable(SLOW_NUMBER)).set_delay(slow_by), ok(json!([]))),
        ];
        let refusals = [
            (HIDDEN_NUMBER, 404, "Not Found"),
            (FORBIDDEN_NUMBER, 403, "Resource not accessible by personal access token"),
            (FAILING_NUMBER, 500, "Server Error"),
        ];
        let refused = refusals.map(|(number, status, message)| {
            let answer = ResponseTemplate::new(status).set_body_json(json!({ "message": message }));
            (number, answer.clone(), answer)
        });
        for (number, detail, reviews) in answers.into_iter().chain(refused) {
            self.mount(repository_api("GET", &format!("/pulls/{number}")).respond_with(detail));
            self.mount(
                repository_api("GET", &format!("/pulls/{number}/reviews")).respond_with(reviews),
            );
        }
    }
}

/// `GET /pulls/7` with the given commits and `updated_at`.
fn pull_json(server_uri: &str, base: &str, head: &str, updated_at: &str) -> Value {
    json!({
        "number": NUMBER, "node_id": "PR_node7", "title": TITLE, "body": "Renames `greet` to `greeting`.",
        "state": "open", "draft": false, "user": { "id": 1, "login": "mona" },
        "html_url": format!("{server_uri}/{REPOSITORY}/pull/{NUMBER}"),
        "created_at": TIMESTAMP, "updated_at": updated_at, "closed_at": null, "merged_at": null,
        "labels": [{ "name": "bug", "color": "d73a4a", "description": "Something is broken" }],
        "requested_reviewers": [], "requested_teams": [],
        "head": { "ref": "feature", "sha": head, "repo": { "full_name": REPOSITORY, "name": "widgets" } },
        "base": { "ref": "main", "sha": base, "repo": { "full_name": REPOSITORY, "name": "widgets" } },
        "mergeable": true, "mergeable_state": "clean"
    })
}

/// The one review thread on line 3: [`THREAD_TEXT`] by cy, then `replies`.
fn review_threads(replies: Vec<Value>) -> Value {
    let mut comments = vec![comment(501, THREAD_TEXT, "cy", 3)];
    comments.extend(replies);
    thread_page(&json!([{
        "id": THREAD_ID, "isResolved": false, "isOutdated": false, "path": FILE, "line": 3, "originalLine": 3,
        "diffSide": "RIGHT",
        "comments": { "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": comments }
    }]))
}

/// The thread nodes of [`review_threads`] without replies: the one thread on line 3.
fn existing_thread_nodes() -> Vec<Value> {
    review_threads(Vec::new())["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"]
        .as_array()
        .expect("the existing thread")
        .clone()
}

/// A review comment node `PRRC_{id}` by `login`, written at [`TIMESTAMP`] and never edited.
fn comment(id: u64, body: &str, login: &str, author_id: u64) -> Value {
    json!({
        "id": format!("PRRC_{id}"), "databaseId": id, "body": body, "createdAt": TIMESTAMP,
        "lastEditedAt": null, "author": { "login": login, "databaseId": author_id }
    })
}

/// The REST path of pull request `number` of [`REPOSITORY`], below the server root.
pub fn pull_path(number: u64) -> String {
    format!("/api/v3/repos/{REPOSITORY}/pulls/{number}")
}

fn thread_page(nodes: &Value) -> Value {
    json!({ "data": { "repository": { "pullRequest": { "reviewThreads": {
        "pageInfo": { "hasNextPage": false, "endCursor": null, "hasPreviousPage": false, "startCursor": null },
        "nodes": nodes
    } } } } })
}

/// One row of the GraphQL "mine" search.
pub fn search_row(number: u64, title: &str, repository: &str) -> Value {
    let name = repository.rsplit('/').next().unwrap_or(repository);
    json!({
        "number": number, "title": title, "url": format!("https://example.invalid/{repository}/pull/{number}"),
        "state": "OPEN", "isDraft": false, "createdAt": TIMESTAMP, "updatedAt": TIMESTAMP,
        "headRefName": "feature", "baseRefName": "main",
        "author": { "login": "me", "avatarUrl": null, "databaseId": 9, "name": "Me" },
        "repository": { "nameWithOwner": repository, "name": name },
        "labels": { "pageInfo": { "hasNextPage": false }, "nodes": [] },
        "reviewRequests": { "pageInfo": { "hasNextPage": false }, "nodes": [] },
        "latestOpinionatedReviews": { "pageInfo": { "hasNextPage": false }, "nodes": [] }
    })
}

/// REST v3 call with the bearer token and GitHub's media type.
fn api(verb: &str, api_path: &str) -> MockBuilder {
    Mock::given(method(verb))
        .and(path(format!("/api/v3{api_path}")))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(header("accept", "application/vnd.github+json"))
}

fn repository_api(verb: &str, repository_path: &str) -> MockBuilder {
    api(verb, &format!("/repos/{REPOSITORY}{repository_path}"))
}

/// `verb` on any review comment, `/pulls/comments/{id}`, with the bearer token.
fn comment_api(verb: &str) -> MockBuilder {
    Mock::given(method(verb))
        .and(path_regex(format!(r"^/api/v3/repos/{REPOSITORY}/pulls/comments/\d+$")))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
}

/// GraphQL request whose document contains `needle`.
fn graphql(needle: &str) -> MockBuilder {
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(body_string_contains(needle))
}

fn ok(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

/// Answers git's smart HTTP protocol through `git http-backend`, the CGI program git ships for
/// exactly that, so fetches behave as against a real host (protocol v2, partial-clone filters).
pub(super) struct SmartGit {
    pub(super) project_root: PathBuf,
}

impl Respond for SmartGit {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let header = |name: &str| {
            request
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        let mut backend = Command::new("git")
            .arg("http-backend")
            .env("GIT_PROJECT_ROOT", &self.project_root)
            .env("GIT_HTTP_EXPORT_ALL", "1")
            .env("REQUEST_METHOD", request.method.as_str())
            .env("PATH_INFO", request.url.path())
            .env("QUERY_STRING", request.url.query().unwrap_or_default())
            .env("CONTENT_TYPE", header("content-type"))
            .env("HTTP_CONTENT_ENCODING", header("content-encoding"))
            .env("GIT_PROTOCOL", header("git-protocol"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("git http-backend starts");
        backend
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(&request.body)
            .expect("request body reaches git");
        let output = backend.wait_with_output().expect("git http-backend finishes");
        cgi_response(&output.stdout)
    }
}

/// The large fetches' answers held by [`GitHubHost::hold_batch_downloads`]. They go out a given
/// number at a time ([`Self::let_through`]), or all once this opens or is dropped, so a failing
/// test never leaves a fetch hanging.
pub struct HeldDownloads(Arc<Gate>);

impl HeldDownloads {
    /// Lets the next `fetches` held answers (waiting or still to come) go out.
    pub fn let_through(&self, fetches: usize) {
        self.0.pass(fetches);
    }

    /// Lets every held answer go out, now and from now on.
    pub fn open(&self) {
        self.0.open();
    }

    /// The answers held right now: large fetches the server answered and the gate keeps.
    pub fn waiting(&self) -> usize {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner).waiting
    }
}

impl Drop for HeldDownloads {
    fn drop(&mut self) {
        self.0.open();
    }
}

/// Closed until [`Self::open`]; [`Self::wait`] blocks the calling thread until then or until it
/// takes one of the passes [`Self::pass`] handed out.
#[derive(Default)]
struct Gate {
    state: Mutex<GateState>,
    changed: Condvar,
}

#[derive(Default)]
struct GateState {
    is_open: bool,
    passes: usize,
    /// Threads blocked in [`Gate::wait`].
    waiting: usize,
}

impl Gate {
    fn open(&self) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).is_open = true;
        self.changed.notify_all();
    }

    fn pass(&self, passes: usize) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).passes += passes;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.waiting += 1;
        while !state.is_open && state.passes == 0 {
            state = self.changed.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
        state.waiting -= 1;
        if !state.is_open {
            state.passes -= 1;
        }
    }
}

/// How git marks a compressed request body. git compresses a body only above 1 KiB and writes a
/// want line of 50 bytes per object, so only a fetch of more objects than one file has carries it.
const LARGE_FETCH: &[u8] = b"content-encoding: gzip";

/// A loopback relay in front of the mock server, on threads of its own: bytes pass unchanged both
/// ways, but on a connection that sent a [`LARGE_FETCH`] the answer waits for the [`Gate`].
/// wiremock answers each request at once and can only hold one for a fixed time, so the hold
/// lives here, where blocking a thread stops nothing else.
struct Relay {
    uri: String,
    address: SocketAddr,
    is_stopped: Arc<AtomicBool>,
}

impl Relay {
    fn start(server: SocketAddr, gate: Arc<Gate>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a relay port");
        let address = listener.local_addr().expect("the relay's address");
        let is_stopped = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&is_stopped);
        std::thread::spawn(move || {
            for client in listener.incoming() {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let client = client.expect("an accepted connection");
                let upstream = TcpStream::connect(server).expect("the mock server answers");
                relay_connection(client, upstream, Arc::clone(&gate));
            }
        });
        Self { uri: format!("http://{address}"), address, is_stopped }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.is_stopped.store(true, Ordering::Release);
        // Wakes the accepting thread so it sees the stop; a refused connection means it is gone.
        let _gone = TcpStream::connect(self.address);
    }
}

/// Copies `client` to `upstream` and back, each way on its own thread. The way back waits for
/// `gate` once the way there has carried a [`LARGE_FETCH`]: the mark is noted before its bytes
/// go on, so no byte of that answer passes before the check. One wait per connection: each
/// download is a git process of its own, with its own connection.
fn relay_connection(client: TcpStream, upstream: TcpStream, gate: Arc<Gate>) {
    let is_held = Arc::new(AtomicBool::new(false));
    let mut from_client = client.try_clone().expect("a second handle of the client");
    let mut to_upstream = upstream.try_clone().expect("a second handle of the server");
    let marks = Arc::clone(&is_held);
    std::thread::spawn(move || {
        let mut buffer = [0; 16 * 1024];
        // The tail of the bytes before and the bytes just read, so a mark split across two reads
        // is found too.
        let mut seen = Vec::new();
        while let Ok(read @ 1..) = from_client.read(&mut buffer) {
            let bytes = &buffer[..read];
            seen.extend(bytes.iter().map(u8::to_ascii_lowercase));
            if seen.windows(LARGE_FETCH.len()).any(|window| window == LARGE_FETCH) {
                marks.store(true, Ordering::Release);
            }
            seen.drain(..seen.len().saturating_sub(LARGE_FETCH.len()));
            if to_upstream.write_all(bytes).is_err() {
                break;
            }
        }
        // The server may have closed already; then there is nothing left to end.
        let _closed = to_upstream.shutdown(Shutdown::Write);
    });
    std::thread::spawn(move || {
        let (mut from_upstream, mut to_client) = (upstream, client);
        let mut buffer = [0; 16 * 1024];
        let mut is_let_through = false;
        while let Ok(read @ 1..) = from_upstream.read(&mut buffer) {
            if !is_let_through && is_held.load(Ordering::Acquire) {
                gate.wait();
                is_let_through = true;
            }
            if to_client.write_all(&buffer[..read]).is_err() {
                break;
            }
        }
        // git may have closed already; then there is nothing left to end.
        let _closed = to_client.shutdown(Shutdown::Write);
    });
}

/// CGI output: header lines (`Status` among them), a blank line, then the body.
fn cgi_response(output: &[u8]) -> ResponseTemplate {
    let split = output.windows(4).position(|window| window == b"\r\n\r\n").map(|at| (at, 4));
    let split =
        split.or_else(|| output.windows(2).position(|window| window == b"\n\n").map(|at| (at, 2)));
    let Some((end, separator)) = split else {
        return ResponseTemplate::new(500).set_body_bytes(output.to_vec());
    };
    let head = String::from_utf8_lossy(&output[..end]);
    let mut status = 200;
    let mut content_type = String::new();
    let mut headers = Vec::new();
    for line in head.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_owned();
        match name.trim().to_ascii_lowercase().as_str() {
            "status" => {
                status = value
                    .split_whitespace()
                    .next()
                    .and_then(|code| code.parse().ok())
                    .unwrap_or(500);
            }
            "content-type" => content_type = value,
            other => headers.push((other.to_owned(), value)),
        }
    }
    let response = ResponseTemplate::new(status).append_headers(headers);
    response.set_body_raw(output[end + separator..].to_vec(), &content_type)
}

/// A file the pull request changes besides [`FILE`]: its bytes in the base and in the head.
#[derive(Clone, PartialEq)]
pub struct ExtraFile {
    pub path: &'static str,
    pub base: Vec<u8>,
    pub head: Vec<u8>,
}

/// What the host keeps of the head [`GitHubHost::force_push_outdating_the_thread`] replaces.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReplacedHead {
    /// Still fetchable by its id.
    Kept,
    /// Collected as garbage.
    Collected,
}

/// A review thread by "me" for [`GitHubHost::add_own_comments`]: its node id, its one comment's
/// database id, its head line in [`FILE`] and its text.
pub struct OwnComment {
    pub thread_id: &'static str,
    pub comment_id: u64,
    pub line: u32,
    pub text: &'static str,
}

/// The review threads of [`GitHubHost::add_own_comments`] as they are now, after the edits and
/// deletions so far; served after the thread on line 3.
#[derive(Clone)]
struct OwnThreads(Arc<Mutex<Vec<Value>>>);

impl OwnThreads {
    fn nodes(&self) -> std::sync::MutexGuard<'_, Vec<Value>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Respond for OwnThreads {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let mut nodes = existing_thread_nodes();
        nodes.extend(self.nodes().iter().cloned());
        ok(thread_page(&Value::Array(nodes)))
    }
}

/// The comment's database id: the last segment of `/pulls/comments/{id}`.
fn comment_id(request: &Request) -> u64 {
    let id = request.url.path().rsplit('/').next().unwrap_or_default();
    id.parse().expect("a numeric comment id")
}

/// `PATCH /pulls/comments/{id}`: the comment takes the new body and an edit time, and the answer is
/// the REST review comment.
struct OwnEdit(OwnThreads);

impl Respond for OwnEdit {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let id = comment_id(request);
        let body = request.body_json::<Value>().expect("a JSON body")["body"].clone();
        let edited_at = "2026-10-01T10:30:00Z";
        let mut nodes = self.0.nodes();
        let Some((comment, line)) = nodes.iter_mut().find_map(|thread| {
            let line = thread["line"].clone();
            let comments = thread["comments"]["nodes"].as_array_mut()?;
            comments
                .iter_mut()
                .find(|comment| comment["databaseId"] == id)
                .map(|comment| (comment, line))
        }) else {
            return ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" }));
        };
        comment["body"] = body.clone();
        comment["lastEditedAt"] = json!(edited_at);
        ok(json!({
            "id": id, "path": FILE, "line": line, "side": "RIGHT", "subject_type": "line",
            "user": { "id": 9, "login": "me" }, "body": body, "created_at": TIMESTAMP,
            "updated_at": edited_at
        }))
    }
}

/// `DELETE /pulls/comments/{id}`: the comment is gone, and its thread with its last comment.
struct OwnDelete(OwnThreads);

impl Respond for OwnDelete {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let id = comment_id(request);
        let mut nodes = self.0.nodes();
        for thread in nodes.iter_mut() {
            if let Some(comments) = thread["comments"]["nodes"].as_array_mut() {
                comments.retain(|comment| comment["databaseId"] != id);
            }
        }
        nodes.retain(|thread| {
            thread["comments"]["nodes"].as_array().is_some_and(|comments| !comments.is_empty())
        });
        ResponseTemplate::new(204)
    }
}

/// A review thread for [`GitHubHost::add_threads`]: its node id, file, head lines and text.
pub struct MockThread {
    pub id: &'static str,
    pub path: &'static str,
    pub lines: std::ops::RangeInclusive<u32>,
    pub text: &'static str,
}

/// The repository behind the pull request, bare below `project_root` as `{owner}/{name}.git`:
/// `main` with [`BASE_TEXT`] and `refs/pull/7/head` with [`HEAD_TEXT`].
struct Origin {
    project_root: PathBuf,
    /// The working copy pushes are made in.
    work: PathBuf,
    base: String,
    head: String,
}

impl Origin {
    /// The repositories for `files` in `scratch`, written from the [`Template`] built for the same
    /// files earlier in this test run.
    fn create(scratch: &Path, files: &[ExtraFile]) -> Self {
        let template = Template::of(files);
        template.folder.write(scratch);
        Self {
            project_root: scratch.join("hosted"),
            work: scratch.join("work"),
            base: template.base.clone(),
            head: template.head.clone(),
        }
    }

    fn build(scratch: &Path, files: &[ExtraFile]) -> Self {
        let work = scratch.join("work");
        std::fs::create_dir_all(work.join("src")).expect("work folder");
        git(&work, &["init", "-q", "-b", "main"]);
        let write = |path: &str, bytes: &[u8]| {
            let full = work.join(path);
            std::fs::create_dir_all(full.parent().expect("a folder")).expect("file folder");
            std::fs::write(full, bytes).expect("file");
        };
        write(FILE, BASE_TEXT.as_bytes());
        for file in files {
            write(file.path, &file.base);
        }
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-q", "-m", "Add greetings"]);
        git(&work, &["switch", "-q", "-c", "feature"]);
        write(FILE, HEAD_TEXT.as_bytes());
        for file in files {
            write(file.path, &file.head);
        }
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-q", "-m", "Rename the greeting"]);

        let project_root = scratch.join("hosted");
        let bare = project_root.join(format!("{REPOSITORY}.git"));
        let bare_path = bare.to_string_lossy().into_owned();
        git(&work, &["clone", "-q", "--bare", ".", &bare_path]);
        // The clone names its source, a folder that goes with the template.
        git(&bare, &["remote", "remove", "origin"]);
        git(&bare, &["update-ref", &format!("refs/pull/{NUMBER}/head"), "feature"]);
        git(&bare, &["config", "uploadpack.allowFilter", "true"]);
        git(&bare, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
        Self {
            project_root,
            work,
            base: git(&bare, &["rev-parse", "main"]),
            head: git(&bare, &["rev-parse", "feature"]),
        }
    }
}

/// An [`Origin`]'s folders as bytes, built once per test run and set of files: building one takes
/// a dozen git processes, and the test binary starts its processes one at a time (see
/// [`cv_testkit`]), so every walkthrough building its own made each one wait longer.
struct Template {
    /// The extra files it was built with.
    files: Vec<ExtraFile>,
    /// The scratch folder it was built in.
    folder: FolderBytes,
    base: String,
    head: String,
}

static TEMPLATES: Mutex<Vec<Arc<Template>>> = Mutex::new(Vec::new());

impl Template {
    /// The template for `files`; the first test asking for it builds it, the others wait for that.
    fn of(files: &[ExtraFile]) -> Arc<Self> {
        let mut templates = TEMPLATES.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(template) = templates.iter().find(|template| template.files == files) {
            return template.clone();
        }
        let scratch = ScratchDir::new("origin-template");
        let origin = Origin::build(scratch.path(), files);
        let template = Arc::new(Self {
            files: files.to_vec(),
            folder: FolderBytes::read(scratch.path()),
            base: origin.base,
            head: origin.head,
        });
        templates.push(template.clone());
        template
    }
}

/// A folder's subfolders and files as bytes. A repository built once per test run is kept this
/// way and written into each test's own [`ScratchDir`], which removes it when the test ends.
pub(super) struct FolderBytes {
    /// The folders, parents first, relative to the root.
    folders: Vec<PathBuf>,
    /// The files with their bytes, relative to the root.
    contents: Vec<(PathBuf, Vec<u8>)>,
}

impl FolderBytes {
    pub(super) fn read(root: &Path) -> Self {
        let (mut folders, mut contents) = (Vec::new(), Vec::new());
        read_tree(root, Path::new(""), &mut folders, &mut contents);
        Self { folders, contents }
    }

    /// Writes the folders and files below `root`, creating it when it is not there.
    pub(super) fn write(&self, root: &Path) {
        std::fs::create_dir_all(root).expect("a copy's root");
        for folder in &self.folders {
            std::fs::create_dir_all(root.join(folder)).expect("a copied folder");
        }
        for (file, bytes) in &self.contents {
            std::fs::write(root.join(file), bytes).expect("a copied file");
        }
    }
}

/// Collects the folders and files below `root.join(relative)`, parents first.
fn read_tree(
    root: &Path,
    relative: &Path,
    folders: &mut Vec<PathBuf>,
    contents: &mut Vec<(PathBuf, Vec<u8>)>,
) {
    for entry in std::fs::read_dir(root.join(relative)).expect("a template folder") {
        let entry = entry.expect("a template entry");
        let path = relative.join(entry.file_name());
        if entry.file_type().expect("an entry's type").is_dir() {
            folders.push(path.clone());
            read_tree(root, &path, folders, contents);
        } else {
            contents
                .push((path.clone(), std::fs::read(root.join(&path)).expect("a template file")));
        }
    }
}

/// Pull requests of [`REPOSITORY`] besides [`NUMBER`], for [`GitHubHost::add_linked_pull_requests`].
pub const READABLE_NUMBER: u64 = 8;
pub const HIDDEN_NUMBER: u64 = 9;
pub const FORBIDDEN_NUMBER: u64 = 10;
pub const FAILING_NUMBER: u64 = 11;
pub const SLOW_NUMBER: u64 = 12;
