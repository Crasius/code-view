//! The list rows' checks and approvals indicators, offline: the real providers of GitHub, GitLab
//! and Azure DevOps against scripted hosts that count every request. Each host lists 50 pull
//! requests; the newest few carry the states under test (all passed, one failed, running, none,
//! a policy the token may not read; no, one, five approvals, approved with suggestions, waiting
//! for the author, changes requested or rejected; a required review or approval rule, met or
//! missing). The indicators and the status pill ("Approved" or "Active") are asserted through
//! their accessible names, which carry the tooltip's full text; a required review never counts as
//! a check. A refresh after the running checks
//! finished shows their outcome, and asks again only for what was running or failed.
//!
//! `cargo test -p review-app pr_list_status -- --nocapture` prints the requests of each listing.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{Connection, ProviderKind};
use review_store::{AppPaths, ConnectionStore};
use serde_json::{Value, json};
use wiremock::{MockServer, Request, Respond, ResponseTemplate};

use super::ReviewApp;
use cv_settings::onboarding::CliTools;
use cv_testkit as test_support;
use cv_testkit::Clicks;
use cv_widgets::numbers::number_prefix;

const ROWS: u64 = 50;
const TIMEOUT: Duration = Duration::from_secs(120);
const STEP_SECONDS: f32 = 0.25;

#[test]
fn pr_list_status_github_search_rows_bring_their_checks_and_votes() {
    let host = Host::start("github", ProviderKind::GitHub, github);
    host.enter("listed");
    let mut harness = host.harness(1440.0);
    let row = |number: u64| Row::new(ProviderKind::GitHub, number);
    wait_until(&mut harness, "the checks of the newest rows", |harness| {
        row(1).checks(harness).is_some_and(|text| text.contains("passed, 2 of 2"))
    });
    wait_idle(&mut harness);

    let checks = |number| row(number).checks(&harness).unwrap_or_default();
    assert!(
        checks(1).starts_with("Checks of #1: passed, 2 of 2 passed\nbuild: passed\nlint: passed\n"),
        "{}",
        checks(1)
    );
    assert!(checks(2).starts_with("Checks of #2: failed, 1 of 2 passed"), "{}", checks(2));
    assert!(checks(2).contains("lint: failed"), "{}", checks(2));
    assert!(checks(3).starts_with("Checks of #3: running, 0 of 1 passed"), "{}", checks(3));
    assert!(checks(4).starts_with("Checks of #4: none configured"), "{}", checks(4));
    assert!(checks(1).ends_with("Click to open the checks on GitHub"), "{}", checks(1));

    let approvals = |number| row(number).approvals(&harness);
    assert_eq!(approvals(1), None, "no reviewers: the column stays quiet");
    assert_eq!(
        approvals(5).as_deref(),
        Some(
            "Approvals of #5: 5 approved\nAnn: Approved\nBo: Approved\nCy: Approved\nDee: Approved\nEd: Approved"
        ),
        "every approver in the text; the stack shows three and \"+2\" (unit test of the stack)"
    );
    assert_eq!(
        approvals(6).as_deref(),
        Some("Approvals of #6: 1 changes requested; no approvals\nAnn: Changes requested")
    );
    assert_eq!(approvals(7).as_deref(), Some("Approvals of #7: 1 approved\nBo: Approved"));
    assert_eq!(
        approvals(8).as_deref(),
        Some(
            "Approvals of #8: 1 approved\nRequired review: missing · Waiting for an approving review\nBo: Approved"
        )
    );

    assert_github_status_pills(&harness);

    // The checks button is keyboard reachable and opens the pull request's Checks tab.
    let label = checks(1);
    harness.get_by_role_and_label(Role::Button, &label).focus();
    harness.step();
    assert!(harness.get_by_role_and_label(Role::Button, &label).is_focused());
    harness.click_at(harness.get_by_role_and_label(Role::Button, &label).rect());
    harness.step();
    assert_eq!(
        opened_urls(&harness),
        ["https://example.invalid/acme/widgets/pull/1/checks"],
        "a click opens the checks on GitHub"
    );

    let listed = host.endpoints("listed");
    assert_eq!(listed.get("POST /api/graphql search"), Some(&1), "{listed:?}");
    assert!(
        !listed.keys().any(|endpoint| endpoint.contains("statuses")),
        "search rows cost no request for their checks: {listed:?}"
    );

    host.finish_running_checks();
    host.enter("refresh");
    refresh(&mut harness);
    wait_until(&mut harness, "the finished check", |harness| {
        row(3).checks(harness).is_some_and(|text| text.starts_with("Checks of #3: passed"))
    });
    wait_idle(&mut harness);
    let refreshed = host.endpoints("refresh");
    assert_eq!(
        refreshed,
        BTreeMap::from([("POST /api/graphql search".to_owned(), 1)]),
        "a refresh is the listing alone"
    );
    host.report(&["listed", "refresh"]);
}

#[test]
fn pr_list_status_gitlab_asks_the_rows_on_screen_in_one_request() {
    let host = Host::start("gitlab", ProviderKind::GitLab, gitlab);
    host.enter("listed");
    let mut harness = host.harness(1440.0);
    let row = |number: u64| Row::new(ProviderKind::GitLab, number);
    wait_until(&mut harness, "the checks of the newest rows", |harness| {
        row(1).checks(harness).is_some_and(|text| text.contains("passed, 2 of 2"))
    });
    wait_idle(&mut harness);
    let checks = |number| row(number).checks(&harness).unwrap_or_default();
    assert!(
        checks(2).starts_with("Checks of !2: failed, 1 of 2 passed\nbuild: passed\n"),
        "the approval rule is no check: {}",
        checks(2)
    );
    assert!(checks(3).starts_with("Checks of !3: running"), "{}", checks(3));
    assert!(checks(4).starts_with("Checks of !4: none configured"), "{}", checks(4));
    assert_eq!(
        row(2).approvals(&harness).as_deref(),
        Some(
            "Approvals of !2: 1 changes requested, 1 approved\nApprovals: missing · 1 of 2 approvals\nAnn: Changes requested\nBo: Approved"
        )
    );
    let status = |number| row(number).status(&harness).unwrap_or_default();
    assert_eq!(status(2), "Status of !2: Active\nApprovals: missing · 1 of 2 approvals");
    assert_eq!(
        status(5),
        "Status of !5: Approved\nThe approvals it needs are in and no reviewer blocks it. The reviewers beside it show who approved.\nApprovals: met · 1 of 1 approval"
    );
    assert!(
        status(6).starts_with("Status of !6: Approved\n"),
        "an approver without a rule: {}",
        status(6)
    );
    assert_eq!(status(1), "Status of !1: Active", "nobody approved");

    let listed = host.endpoints("listed");
    let on_screen = harness
        .query_all_by(|node| node.label().is_some_and(|label| label.starts_with("Checks of !")))
        .count();
    assert!(on_screen > 4 && on_screen < ROWS as usize, "{on_screen} rows on screen");
    assert_eq!(
        listed.get("POST /api/graphql statuses"),
        Some(&1),
        "one request for the {on_screen} rows on screen: {listed:?}"
    );
    let asked = host.asked_merge_requests();
    assert_eq!(asked.len(), on_screen, "the rows on screen, not all {ROWS}: {asked:?}");

    host.finish_running_checks();
    host.enter("refresh");
    refresh(&mut harness);
    wait_until(&mut harness, "the finished pipeline", |harness| {
        row(3).checks(harness).is_some_and(|text| text.starts_with("Checks of !3: passed"))
    });
    wait_idle(&mut harness);
    let refreshed = host.endpoints("refresh");
    assert_eq!(
        refreshed.get("POST /api/graphql statuses"),
        Some(&1),
        "the rows still running, in one request: {refreshed:?}"
    );
    host.report(&["listed", "refresh"]);
}

#[test]
fn pr_list_status_azure_devops_policies_votes_and_a_denied_policy() {
    let host = Host::start("azure", ProviderKind::AzureDevOps, azure);
    host.enter("listed");
    let mut harness = host.harness(1440.0);
    let row = |number: u64| Row::new(ProviderKind::AzureDevOps, number);
    wait_until(&mut harness, "the checks of the newest rows", |harness| {
        row(1).checks(harness).is_some_and(|text| text.contains("passed, 1 of 1"))
            && row(4).checks(harness).is_some_and(|text| text.contains("unknown"))
    });
    wait_idle(&mut harness);
    let checks = |number| row(number).checks(&harness).unwrap_or_default();
    assert_eq!(
        checks(1),
        "Checks of !1: passed, 1 of 1 passed\nweb-ci: passed (required) · Build\n\
         Click to open the checks on Azure DevOps",
        "the reviewer policy is no check"
    );
    assert!(checks(2).starts_with("Checks of !2: failed"), "{}", checks(2));
    assert!(checks(3).starts_with("Checks of !3: running"), "{}", checks(3));
    let denied = checks(4);
    assert!(
        denied.starts_with("Checks of !4: unknown\n") && denied.contains("403"),
        "a denied policy says why: {denied}"
    );
    assert!(
        denied.ends_with("Click to open the checks on Azure DevOps"),
        "and the way on: {denied}"
    );

    assert_azure_votes(&harness);

    let listed = host.endpoints("listed");
    let on_screen = harness
        .query_all_by(|node| node.label().is_some_and(|label| label.starts_with("Checks of !")))
        .count();
    let evaluations = "GET /north/Web/_apis/policy/evaluations";
    assert_eq!(
        listed.get(evaluations),
        Some(&on_screen),
        "one per row on screen ({on_screen}), not all {ROWS}: {listed:?}"
    );
    assert_eq!(listed.get("GET /north/Web/_apis/git/repositories/site"), Some(&1), "{listed:?}");

    host.finish_running_checks();
    host.enter("refresh");
    refresh(&mut harness);
    wait_until(&mut harness, "the finished build", |harness| {
        row(3).checks(harness).is_some_and(|text| text.starts_with("Checks of !3: passed"))
    });
    wait_idle(&mut harness);
    let refreshed = host.endpoints("refresh");
    assert_eq!(
        refreshed.get(evaluations),
        Some(&2),
        "only the running and the denied row ask again: {refreshed:?}"
    );
    assert!(!refreshed.contains_key("GET /north/Web/_apis/git/repositories/site"));
    host.report(&["listed", "refresh"]);
}

/// The Azure DevOps rows' votes with their reviewer policy, and their status pills: approved,
/// approved with suggestions, waiting for the author, rejected with its policy denied.
fn assert_azure_votes(harness: &Harness<'_, ReviewApp>) {
    let row = |number| Row::new(ProviderKind::AzureDevOps, number);
    let approvals = |number| row(number).approvals(harness).unwrap_or_default();
    let status = |number| row(number).status(harness).unwrap_or_default();
    let policy = "Reviewer policies: met · Minimum number of reviewers: 1 approval required";
    assert_eq!(
        approvals(1),
        format!("Approvals of !1: 1 approved\n{policy}\nAnn: Approved (required)")
    );
    assert_eq!(
        approvals(2),
        format!(
            "Approvals of !2: 1 approved with suggestions\n{policy}\nBo: Approved with suggestions"
        )
    );
    assert_eq!(
        approvals(3),
        format!(
            "Approvals of !3: 1 waiting for author; no approvals\n{policy}\nCy: Waiting for author"
        )
    );
    assert_eq!(approvals(4), "Approvals of !4: 1 rejected; no approvals\nDee: Rejected");
    assert!(status(1).starts_with("Status of !1: Approved\n"), "{}", status(1));
    assert!(status(2).starts_with("Status of !2: Approved\n"), "with suggestions: {}", status(2));
    let waiting = format!("Status of !3: Active\n{policy}");
    assert_eq!(status(3), waiting, "waiting for the author blocks");
    assert_eq!(status(4), "Status of !4: Active", "rejected, its policy unknown");
}

/// The GitHub rows' status pills: "Approved" by the votes or the review decision, "Active" with
/// the review still required. A required review is no check: row 1 counts its two checks.
fn assert_github_status_pills(harness: &Harness<'_, ReviewApp>) {
    let status =
        |number| Row::new(ProviderKind::GitHub, number).status(harness).unwrap_or_default();
    let waiting = "Required review: missing · Waiting for an approving review";
    assert_eq!(status(1), format!("Status of #1: Active\n{waiting}"));
    assert!(status(5).starts_with("Status of #5: Approved\n"), "{}", status(5));
    assert!(
        status(7).starts_with("Status of #7: Approved\n"),
        "approved without a rule: {}",
        status(7)
    );
    assert_eq!(status(6), "Status of #6: Active", "changes requested is not approved");
    assert_eq!(
        status(8),
        format!("Status of #8: Active\n{waiting}"),
        "a second approval is missing"
    );
    assert_eq!(status(2), "Status of #2: Active", "nobody voted");
}

/// On a narrow card the checks indicator is an icon without its count, both indicators stay
/// reachable, and nothing in the rows is cut or overlaps.
#[cfg(debug_assertions)]
#[test]
fn pr_list_status_indicators_fit_a_narrow_window() {
    let host = Host::start("narrow", ProviderKind::GitHub, github);
    let mut harness = host.harness(1440.0);
    super::clip_check::install(&harness.ctx);
    let row = Row::new(ProviderKind::GitHub, 5);
    wait_until(&mut harness, "the rows", |harness| row.checks(harness).is_some());
    wait_idle(&mut harness);
    let wide = row.checks_width(&harness);
    harness.set_size(egui::vec2(720.0, 900.0));
    for _ in 0..20 {
        harness.step();
    }
    let narrow = row.checks_width(&harness);
    assert!(wide > 50.0 && narrow < 30.0, "with its count {wide}, icon only {narrow}");
    assert!(row.approvals(&harness).is_some(), "the approvals stay on a narrow card");
    // The rows' own problems; the cards and header above them are not this walkthrough's.
    let rows = harness
        .query_all_by(|node| {
            node.role() == Role::Link
                && node.label().is_some_and(|label| label.contains(" Change "))
        })
        .filter_map(|node| node.accesskit_node().bounding_box())
        .map(|bounds| bounds.y0 as f32..=bounds.y1 as f32)
        .reduce(|first, next| first.start().min(*next.start())..=first.end().max(*next.end()))
        .expect("pull request rows");
    let findings: Vec<String> = super::clip_check::inspect(&mut harness)
        .into_iter()
        .filter(|finding| finding_top(finding).is_some_and(|top| rows.contains(&top)))
        .collect();
    assert!(findings.is_empty(), "layout problems in the rows:\n{}", findings.join("\n"));
}

/// The top edge of a finding's first rect, written `[x,y w×h]`.
fn finding_top(finding: &str) -> Option<f32> {
    let (_, rect) = finding.split_once('[')?;
    let (corner, _) = rect.split_once(' ')?;
    corner.split_once(',')?.1.parse().ok()
}

/// One row's indicators, found by their accessible names.
struct Row {
    checks: String,
    approvals: String,
    status: String,
}

impl Row {
    fn new(kind: ProviderKind, number: u64) -> Self {
        let number = format!("{}{number}", number_prefix(kind));
        Self {
            checks: format!("Checks of {number}: "),
            approvals: format!("Approvals of {number}: "),
            status: format!("Status of {number}: "),
        }
    }

    fn label(harness: &Harness<'_, ReviewApp>, prefix: &str) -> Option<String> {
        harness
            .query_all_by(|node| {
                node.role() == Role::Button
                    && node.label().is_some_and(|label| label.starts_with(prefix))
            })
            .next()
            .and_then(|node| node.accesskit_node().label())
    }

    fn checks(&self, harness: &Harness<'_, ReviewApp>) -> Option<String> {
        Self::label(harness, &self.checks)
    }

    fn approvals(&self, harness: &Harness<'_, ReviewApp>) -> Option<String> {
        Self::label(harness, &self.approvals)
    }

    /// The status pill's text; a label node carries it as its value.
    fn status(&self, harness: &Harness<'_, ReviewApp>) -> Option<String> {
        harness
            .query_all_by_role(Role::Label)
            .find_map(|node| node.value().filter(|text| text.starts_with(&self.status)))
    }

    fn checks_width(&self, harness: &Harness<'_, ReviewApp>) -> f32 {
        let label = self.checks(harness).expect("the checks indicator");
        let bounds = harness
            .get_by_role_and_label(Role::Button, &label)
            .accesskit_node()
            .bounding_box()
            .expect("laid out");
        (bounds.x1 - bounds.x0) as f32 / harness.ctx.pixels_per_point()
    }
}

/// What a scripted host answers; `finished` turns the running checks into passed ones.
type Script = fn(&Request, &Scene) -> ResponseTemplate;

#[derive(Default)]
struct Scene {
    base_url: String,
    finished: AtomicBool,
    /// (phase, endpoint, body) of every request.
    hits: Mutex<Vec<(String, String, String)>>,
    phase: Mutex<String>,
}

struct Responder {
    scene: Arc<Scene>,
    script: Script,
}

impl Respond for Responder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body = String::from_utf8_lossy(&request.body).into_owned();
        let kind = match request.url.path() {
            "/api/graphql" if body.contains("search(") => " search",
            "/api/graphql" if body.contains("headPipeline") || body.contains("pr1:") => " statuses",
            "/api/graphql" if body.contains("diffStatsSummary") => " sizes",
            _ => "",
        };
        let endpoint = format!("{} {}{kind}", request.method, template(request.url.path()));
        let phase = self.scene.phase.lock().unwrap_or_else(PoisonError::into_inner).clone();
        self.scene
            .hits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((phase, endpoint, body));
        (self.script)(request, &self.scene)
    }
}

struct Host {
    _runtime: tokio::runtime::Runtime,
    _server: MockServer,
    scene: Arc<Scene>,
    paths: AppPaths,
}

impl Host {
    fn start(name: &str, kind: ProviderKind, script: Script) -> Self {
        let root = std::env::temp_dir()
            .join(format!("review-pr-list-status-{name}-{}", std::process::id()));
        test_support::remove_dir(&root);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("mock runtime");
        let server = runtime.block_on(MockServer::start());
        cv_testkit::fresh_origin(&server.uri());
        let scene = Arc::new(Scene { base_url: server.uri(), ..Scene::default() });
        let responder = Responder { scene: Arc::clone(&scene), script };
        runtime.block_on(
            wiremock::Mock::given(wiremock::matchers::any()).respond_with(responder).mount(&server),
        );
        let paths = AppPaths::new(root.join("config"), root.join("cache"));
        let organization = match kind {
            ProviderKind::GitHub => "acme",
            ProviderKind::GitLab => "group",
            ProviderKind::AzureDevOps => "north",
        };
        ConnectionStore::open(&paths)
            .expect("empty store")
            .upsert(Connection {
                id: format!("status-{name}"),
                name: format!("Mock {}", kind.label()),
                kind,
                base_url: server.uri(),
                organizations: vec![organization.to_owned()],
                token: Some("mock-only".to_owned()),
            })
            .expect("store the connection");
        Self { _runtime: runtime, _server: server, scene, paths }
    }

    fn harness(&self, width: f32) -> Harness<'static, ReviewApp> {
        let paths = self.paths.clone();
        let builder = Harness::builder()
            .with_size([width, 900.0])
            .with_step_dt(STEP_SECONDS)
            .allow_missing_glyphs();
        let mut harness = super::startup::start(builder, move |creation| {
            ReviewApp::new(creation, paths, cv_providers::create_provider)
        });
        harness.state_mut().settings.cli_tools = Some(CliTools::default());
        harness
    }

    fn enter(&self, phase: &str) {
        *self.scene.phase.lock().unwrap_or_else(PoisonError::into_inner) = phase.to_owned();
    }

    fn finish_running_checks(&self) {
        self.scene.finished.store(true, Ordering::SeqCst);
    }

    fn endpoints(&self, phase: &str) -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::new();
        for (hit, endpoint, _) in
            self.scene.hits.lock().unwrap_or_else(PoisonError::into_inner).iter()
        {
            if hit == phase {
                *counts.entry(endpoint.clone()).or_default() += 1;
            }
        }
        counts
    }

    /// The merge requests the statuses requests named.
    fn asked_merge_requests(&self) -> Vec<u64> {
        let hits = self.scene.hits.lock().unwrap_or_else(PoisonError::into_inner);
        let mut asked: Vec<u64> = hits
            .iter()
            .filter(|(_, endpoint, _)| endpoint.ends_with(" statuses"))
            .flat_map(|(_, _, body)| iids(body))
            .collect();
        asked.sort_unstable();
        asked.dedup();
        asked
    }

    fn report(&self, phases: &[&str]) {
        for phase in phases {
            let endpoints = self.endpoints(phase);
            let total: usize = endpoints.values().sum();
            println!("{phase}: {total} requests");
            for (endpoint, hits) in endpoints {
                println!("    {hits:>4}  {endpoint}");
            }
        }
    }
}

/// The merge request iids a GitLab GraphQL request names.
fn iids(body: &str) -> Vec<u64> {
    let body: Value = serde_json::from_str(body).unwrap_or_default();
    body["variables"]["iids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|iid| iid.parse().ok())
        .collect()
}

fn ok(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" }))
}

/// Newest first: lower numbers were created later.
fn created_at(number: u64) -> String {
    let seconds = 200 - number;
    format!("2026-10-10T10:{:02}:{:02}Z", seconds / 60, seconds % 60)
}

fn names(count: usize) -> &'static [&'static str] {
    &["Ann", "Bo", "Cy", "Dee", "Ed"][..count]
}

/// GitHub Enterprise Server: the "mine" search lists [`ROWS`] pull requests of `acme/widgets`,
/// each with its head commit's check rollup and its reviews.
fn github(request: &Request, scene: &Scene) -> ResponseTemplate {
    match request.url.path() {
        "/api/v3/users/acme" => ok(json!({ "login": "acme", "type": "Organization" })),
        "/api/v3/user" => ok(json!({ "login": "me", "id": 9, "name": "Me" })),
        "/api/graphql" if String::from_utf8_lossy(&request.body).contains("search(") => {
            let finished = scene.finished.load(Ordering::SeqCst);
            let rows: Vec<Value> = (1..=ROWS).map(|number| github_row(number, finished)).collect();
            ok(json!({ "data": { "search": {
                "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": rows } } }))
        }
        _ => not_found(),
    }
}

fn github_row(number: u64, finished: bool) -> Value {
    // The search cannot ask whether a check is required: that takes the pull request's number.
    let run = |name: &str, conclusion: Option<&str>| {
        json!({ "__typename": "CheckRun", "name": name,
                "status": if conclusion.is_some() { "COMPLETED" } else { "IN_PROGRESS" },
                "conclusion": conclusion })
    };
    let (state, contexts) = match number {
        1 => ("SUCCESS", vec![run("build", Some("SUCCESS")), run("lint", Some("SUCCESS"))]),
        2 => ("FAILURE", vec![run("build", Some("SUCCESS")), run("lint", Some("FAILURE"))]),
        3 if finished => ("SUCCESS", vec![run("build", Some("SUCCESS"))]),
        3 => ("PENDING", vec![run("build", None)]),
        4 => ("", Vec::new()),
        _ => ("SUCCESS", vec![run("build", Some("SUCCESS"))]),
    };
    let rollup = (!state.is_empty()).then(|| {
        json!({ "state": state, "contexts": { "totalCount": contexts.len(), "nodes": contexts } })
    });
    let review = |name: &str, state: &str| {
        let id = names(5).iter().position(|known| *known == name).unwrap_or_default() + 1;
        json!({ "state": state, "author": { "login": name.to_lowercase(), "databaseId": id, "name": name } })
    };
    let reviews: Vec<Value> = match number {
        5 => names(5).iter().map(|name| review(name, "APPROVED")).collect(),
        6 => vec![review("Ann", "CHANGES_REQUESTED")],
        7 | 8 => vec![review("Bo", "APPROVED")],
        _ => Vec::new(),
    };
    // Row 1's branch protection waits for a review; row 8's wants a second approval.
    let decision = match number {
        1 | 8 => json!("REVIEW_REQUIRED"),
        _ => Value::Null,
    };
    let mut row =
        super::github_mock::search_row(number, &format!("Change {number}"), "acme/widgets");
    row["createdAt"] = json!(created_at(number));
    row["updatedAt"] = json!(if finished && number == 3 {
        "2026-10-11T10:00:00Z"
    } else {
        "2026-10-10T12:00:00Z"
    });
    row["latestOpinionatedReviews"] =
        json!({ "pageInfo": { "hasNextPage": false }, "nodes": reviews });
    row["reviewDecision"] = decision;
    row["commits"] = json!({ "nodes": [{ "commit": { "statusCheckRollup": rollup } }] });
    row
}

/// GitLab: the group `group` lists [`ROWS`] merge requests of `group/app`; GraphQL answers their
/// pipelines, approvals and reviewers, and their sizes.
fn gitlab(request: &Request, scene: &Scene) -> ResponseTemplate {
    let body = String::from_utf8_lossy(&request.body);
    match request.url.path() {
        "/api/v4/user" => ok(json!({ "id": 1, "username": "ada", "name": "Ada" })),
        "/api/v4/groups/group/merge_requests" => {
            ok(Value::Array((1..=ROWS).map(|iid| gitlab_merge_request(iid, scene)).collect()))
        }
        "/api/graphql" if body.contains("headPipeline") => {
            let finished = scene.finished.load(Ordering::SeqCst);
            let nodes: Vec<Value> =
                iids(&body).into_iter().map(|iid| gitlab_status(iid, finished)).collect();
            ok(json!({ "data": { "project": { "mergeRequests": { "nodes": nodes } } } }))
        }
        "/api/graphql" if body.contains("diffStatsSummary") => {
            let nodes: Vec<Value> = iids(&body)
                .into_iter()
                .map(|iid| json!({ "iid": iid.to_string(), "diffStatsSummary": { "additions": 3, "deletions": 1, "fileCount": 1 } }))
                .collect();
            ok(json!({ "data": { "project": { "mergeRequests": { "nodes": nodes } } } }))
        }
        _ => not_found(),
    }
}

fn gitlab_merge_request(iid: u64, scene: &Scene) -> Value {
    let user =
        |id: u64, name: &str| json!({ "id": id, "username": name.to_lowercase(), "name": name });
    let finished = scene.finished.load(Ordering::SeqCst);
    json!({
        "id": 1000 + iid, "iid": iid, "project_id": 42, "title": format!("Change {iid}"), "description": "",
        "state": "opened", "author": user(2, "Grace"), "source_branch": format!("feature-{iid}"), "target_branch": "main",
        "created_at": created_at(iid),
        "updated_at": if finished && iid == 3 { "2026-10-11T10:00:00Z" } else { "2026-10-10T12:00:00Z" },
        "web_url": format!("{}/group/app/-/merge_requests/{iid}", scene.base_url),
        "references": { "full": format!("group/app!{iid}") }, "labels": [], "reviewers": [],
        "sha": format!("{iid:040x}"), "detailed_merge_status": "mergeable",
    })
}

fn gitlab_status(iid: u64, finished: bool) -> Value {
    let user = |id: u64, name: &str| json!({ "id": format!("gid://gitlab/User/{id}"), "username": name.to_lowercase(), "name": name });
    let stage = |name: &str, status: &str| json!({ "name": name, "status": status });
    let pipeline = |status: &str, stages: Vec<Value>| json!({ "status": status, "stages": { "nodes": stages } });
    let mut node = json!({ "iid": iid.to_string(), "approvalsRequired": 0, "approvalsLeft": 0,
                           "approvedBy": { "nodes": [] }, "reviewers": { "nodes": [] } });
    node["headPipeline"] = match iid {
        1 => pipeline("SUCCESS", vec![stage("build", "success"), stage("test", "success")]),
        2 => pipeline("FAILED", vec![stage("build", "success"), stage("test", "failed")]),
        3 if finished => pipeline("SUCCESS", vec![stage("build", "success")]),
        3 => pipeline("RUNNING", vec![stage("build", "running")]),
        4 => Value::Null,
        _ => pipeline("SUCCESS", vec![stage("build", "success")]),
    };
    if iid == 2 {
        let mut ann = user(11, "Ann");
        ann["mergeRequestInteraction"] = json!({ "reviewState": "REQUESTED_CHANGES" });
        let mut bo = user(12, "Bo");
        bo["mergeRequestInteraction"] = json!({ "reviewState": "UNREVIEWED" });
        node["approvalsRequired"] = json!(2);
        node["approvalsLeft"] = json!(1);
        node["approvedBy"] = json!({ "nodes": [user(12, "Bo")] });
        node["reviewers"] = json!({ "nodes": [ann, bo] });
    }
    // Row 5 meets its rule; row 6 has no rule and one approver who is no reviewer.
    if iid == 5 {
        node["approvalsRequired"] = json!(1);
        node["approvedBy"] = json!({ "nodes": [user(12, "Bo")] });
    }
    if iid == 6 {
        node["approvedBy"] = json!({ "nodes": [user(13, "Cy")] });
    }
    node
}

/// Azure DevOps: the organization `north` lists [`ROWS`] pull requests of `north/Web/site` with
/// their reviewers' votes; each one's policy evaluations come one request each, pull request 4's
/// denied.
fn azure(request: &Request, scene: &Scene) -> ResponseTemplate {
    let finished = scene.finished.load(Ordering::SeqCst);
    match request.url.path() {
        "/north/_apis/connectionData" => {
            ok(json!({ "authenticatedUser": { "id": "me-id", "providerDisplayName": "Ada" } }))
        }
        "/north/_apis/git/pullrequests" => {
            let skip = query(request, "$skip").unwrap_or(0);
            let top = query(request, "$top").unwrap_or(100);
            let rows: Vec<Value> = (1..=ROWS)
                .map(|id| azure_pull_request(id, finished))
                .skip(skip as usize)
                .take(top as usize)
                .collect();
            ok(json!({ "count": rows.len(), "value": rows }))
        }
        "/north/Web/_apis/git/repositories/site" => ok(json!({
            "id": "site-id", "name": "site", "project": { "id": "web-id", "name": "Web" }
        })),
        "/north/Web/_apis/policy/evaluations" => {
            let artifact = request
                .url
                .query_pairs()
                .find(|(key, _)| key == "artifactId")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default();
            let number: u64 =
                artifact.rsplit('/').next().and_then(|id| id.parse().ok()).unwrap_or_default();
            if number == 4 {
                return ResponseTemplate::new(403).set_body_json(
                    json!({ "message": "TF401027: You need the Git 'Read' permission." }),
                );
            }
            ok(json!({ "count": 2, "value": azure_evaluations(number, finished) }))
        }
        _ => not_found(),
    }
}

fn query(request: &Request, name: &str) -> Option<u64> {
    request.url.query_pairs().find(|(key, _)| key == name).and_then(|(_, value)| value.parse().ok())
}

fn azure_pull_request(id: u64, finished: bool) -> Value {
    let reviewer = |name: &str, vote: i32| {
        json!({ "id": format!("{name}-id"), "displayName": name, "uniqueName": format!("{name}@north"),
                "vote": vote, "isRequired": name == "Ann" })
    };
    let reviewers = match id {
        1 => vec![reviewer("Ann", 10)],
        2 => vec![reviewer("Bo", 5)],
        3 => vec![reviewer("Cy", -5)],
        4 => vec![reviewer("Dee", -10)],
        _ => Vec::new(),
    };
    let head =
        if finished && id == 3 { format!("{:040x}", id + 1000) } else { format!("{id:040x}") };
    json!({
        "pullRequestId": id, "status": "active", "title": format!("Change {id}"),
        "repository": { "id": "site-id", "name": "site", "project": { "id": "web-id", "name": "Web" } },
        "createdBy": { "id": "author", "displayName": "Grace" },
        "creationDate": created_at(id),
        "sourceRefName": format!("refs/heads/feature/{id}"), "targetRefName": "refs/heads/main",
        "lastMergeSourceCommit": { "commitId": head },
        "lastMergeTargetCommit": { "commitId": format!("{:040x}", 1_000_000) },
        "reviewers": reviewers,
    })
}

fn azure_evaluations(number: u64, finished: bool) -> Value {
    let build = match number {
        2 => "rejected",
        3 if !finished => "running",
        _ => "approved",
    };
    json!([
        { "status": build, "configuration": { "isBlocking": true, "isEnabled": true,
            "type": { "displayName": "Build" }, "settings": { "buildDefinitionId": 3 } },
          "context": { "buildDefinitionName": "web-ci" } },
        { "status": "approved", "configuration": { "isBlocking": true, "isEnabled": true,
            "type": { "id": "fa4e907d-c16b-4a4c-9dfa-4906e5d171dd", "displayName": "Minimum number of reviewers" },
            "settings": { "minimumApproverCount": 1 } } }
    ])
}

/// Numbers and long hex segments of a path become placeholders.
fn template(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit()) {
                ":id"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn refresh(harness: &mut Harness<'_, ReviewApp>) {
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Refresh (F5)").rect());
    harness.step();
}

fn opened_urls(harness: &Harness<'_, ReviewApp>) -> Vec<String> {
    harness
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
            _ => None,
        })
        .collect()
}

/// Steps until no background work runs for a while.
fn wait_idle(harness: &mut Harness<'_, ReviewApp>) {
    let started = Instant::now();
    let mut idle_frames = 0;
    while idle_frames < 20 {
        harness.step();
        test_support::let_threads_run(Duration::from_millis(5));
        idle_frames =
            if harness.state().services.background.running() == 0 { idle_frames + 1 } else { 0 };
        assert!(started.elapsed() < TIMEOUT, "background work did not finish");
    }
}

fn wait_until(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    let started = Instant::now();
    loop {
        harness.step();
        if is_done(harness) {
            return;
        }
        if started.elapsed() > TIMEOUT {
            let labels: Vec<String> = harness
                .query_all_by(|node| node.label().is_some())
                .filter_map(|node| node.accesskit_node().label())
                .collect();
            panic!("timed out waiting for {what}; labels on screen:\n{}", labels.join("\n"));
        }
        test_support::let_threads_run(Duration::from_millis(10));
    }
}
