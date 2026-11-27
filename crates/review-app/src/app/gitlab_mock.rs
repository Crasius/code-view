//! What the scripted GitLab hosts of the walkthroughs share: the merge request object of the REST
//! API v4 that each scenario adds its own fields to, a user, the list's line counts from GraphQL,
//! the split of a request path below a project's merge requests, and [`GitLabProject`], a plain
//! host for a walkthrough that needs only a list and its merge requests. A host with answers of
//! its own keeps its own router.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use review_core::{Connection, ProviderKind};
use serde_json::{Value, json};
use wiremock::{Request, Respond, ResponseTemplate};

use super::mock_host::MockHost;
use super::recording::iids;
use super::walkthrough::LIMIT;
use cv_testkit::samples;

/// The id every scripted host gives its one project.
pub(super) const PROJECT_ID: u64 = 42;

/// The group of [`GitLabProject`] and its one project.
pub(super) const GROUP: &str = "team";
pub(super) const PROJECT: &str = "team/app";

/// Merge request `iid` of `project` on the host at `base_url`: open, "Change {iid}" by Grace
/// Hopper, `feature-{iid}` into `main` at `head` over `base`, mergeable; the fields the provider
/// needs. A scenario sets its own on the value (`value["reviewers"] = ..`).
pub(super) fn merge_request(
    base_url: &str,
    project: &str,
    iid: u64,
    head: &str,
    base: &str,
) -> Value {
    json!({
        "id": 1000 + iid, "iid": iid, "project_id": PROJECT_ID, "title": format!("Change {iid}"),
        "state": "opened", "author": { "id": 2, "username": "grace", "name": "Grace Hopper" },
        "source_branch": format!("feature-{iid}"), "target_branch": "main",
        "created_at": "2026-10-01T10:00:00Z", "updated_at": "2026-10-17T10:00:00Z",
        "web_url": format!("{base_url}/{project}/-/merge_requests/{iid}"),
        "references": { "full": format!("{project}!{iid}") }, "sha": head,
        "diff_refs": { "base_sha": base, "start_sha": base, "head_sha": head },
        "detailed_merge_status": "mergeable",
    })
}

/// A user of the REST API v4 whose name is its `username`.
pub(super) fn user(id: u64, username: &str) -> Value {
    json!({ "id": id, "username": username, "name": username })
}

/// GraphQL's `diffStatsSummary` of the merge requests a request names: iid × 10 lines added, iid
/// deleted, in 2 files.
pub(super) fn diff_stats_summaries(request: &Request) -> Value {
    let nodes: Vec<Value> = iids(&request.body).into_iter().map(|iid| json!({ "iid": iid.to_string(), "diffStatsSummary": { "additions": iid * 10, "deletions": iid, "fileCount": 2 } })).collect();
    json!({ "data": { "project": { "mergeRequests": { "nodes": nodes } } } })
}

/// The merge request a request `path` below `project`'s merge requests asks about, and what of it:
/// the rest of the path after the iid, empty for the merge request itself.
pub(super) fn merge_request_route<'p>(path: &'p str, project: &str) -> Option<(u64, &'p str)> {
    let prefix = format!("/api/v4/projects/{}/merge_requests/", urlencoding::encode(project));
    let rest = path.strip_prefix(&prefix)?;
    let (iid, tail) = rest.split_once('/').unwrap_or((rest, ""));
    Some((iid.parse().ok()?, tail))
}

/// A GitLab group [`GROUP`] whose project [`PROJECT`] (merged with merge commits) has the merge
/// requests 1 to `count`, each with no threads, approvals or pipelines. Signed in as Ada.
pub(super) struct GitLabProject {
    host: MockHost,
    /// How long every answer takes, in milliseconds.
    delay: Arc<AtomicU64>,
}

impl GitLabProject {
    pub(super) fn start(count: u64) -> Self {
        let delay = Arc::new(AtomicU64::new(0));
        let host = MockHost::start(|base_url| Answers {
            base_url: base_url.to_owned(),
            count,
            delay: delay.clone(),
        });
        Self { host, delay }
    }

    /// A connection `id` to the host, with a token.
    pub(super) fn connection(&self, id: &str) -> Connection {
        Connection {
            name: "Mock GitLab".to_owned(),
            organizations: vec![GROUP.to_owned()],
            token: Some("glpat-mock-only".to_owned()),
            ..samples::connection(id, ProviderKind::GitLab, &self.host.uri())
        }
    }

    /// From now on no answer arrives within a test: each comes a minute after the longest wait.
    pub(super) fn hold_answers(&self) {
        let held = LIMIT + Duration::from_secs(60);
        self.delay.store(u64::try_from(held.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
    }
}

struct Answers {
    base_url: String,
    count: u64,
    delay: Arc<AtomicU64>,
}

impl Respond for Answers {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request.url.path();
        let project = format!("/api/v4/projects/{}", urlencoding::encode(PROJECT));
        let body = match merge_request_route(path, PROJECT) {
            Some((iid, "")) if iid <= self.count => self.merge_request(iid),
            Some((_, "approvals")) => json!({ "approved_by": [] }),
            Some(_) => json!([]),
            None if path == format!("/api/v4/groups/{GROUP}/merge_requests") => {
                Value::Array((1..=self.count).rev().map(|iid| self.merge_request(iid)).collect())
            }
            None if path == "/api/v4/user" => {
                json!({ "id": 1, "username": "ada", "name": "Ada Lovelace" })
            }
            None if path == project => json!({
                "id": PROJECT_ID, "name": "app", "path_with_namespace": PROJECT,
                "web_url": format!("{}/{PROJECT}", self.base_url), "default_branch": "main",
                "http_url_to_repo": format!("{}/{PROJECT}.git", self.base_url),
                "merge_method": "merge", "squash_option": "default_off",
            }),
            None => {
                return ResponseTemplate::new(404)
                    .set_body_json(json!({ "message": "404 Not Found" }));
            }
        };
        let delay = Duration::from_millis(self.delay.load(Ordering::Relaxed));
        ResponseTemplate::new(200).set_body_json(body).set_delay(delay)
    }
}

impl Answers {
    fn merge_request(&self, iid: u64) -> Value {
        merge_request(&self.base_url, PROJECT, iid, &"a".repeat(40), &"b".repeat(40))
    }
}
