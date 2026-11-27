//! Favorite repositories and the overview's Favorites card, driven headless (`egui_kittest`)
//! against a mock GitLab and a mock GitHub Enterprise Server. No network, no real token.

use std::sync::{Arc, Mutex, PoisonError};

use egui::accesskit::{Role, Toggled};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{Connection, ProviderKind};
use review_store::{AppPaths, AppSettings};
use serde_json::{Value, json};
use wiremock::{Request, Respond, ResponseTemplate};

use super::ReviewApp;
use super::github_mock::GitHubHost;
use super::mock_host::MockHost;
use super::walkthrough::{
    app_with, has, press, pump, saved_paths, saved_settings, secondary_press, set_focus,
    wait_until_unsettled,
};
use cv_lists::pull_request_list::FAVORITES_LANE;
use cv_runtime::route::Route;
use cv_shell::attention::Attention;
use cv_testkit::{Clicks, ScratchDir, samples};

const TOKEN: &str = "glpat-mock-only";
const GROUP: &str = "team";
const APP_LISTING: &str = "/api/v4/projects/team%2Fapp/merge_requests";

/// The star in the repository browser saves the favorite at once; after a restart the
/// repository's own page shows it starred, and its star there removes it again.
#[test]
fn star_in_the_repository_browser_persists() {
    let gitlab = GitLabHost::start();
    let scratch = ScratchDir::new("favorites-star");
    let paths = saved_paths(&scratch, &[gitlab.connection("gitlab", "Team GitLab")]);
    let mut harness = start_app(&paths);
    go(&mut harness, Route::Repositories);
    wait_until_unsettled(&mut harness, "the repositories", |harness| {
        has(harness, "Favorite team/api")
    });
    assert_eq!(toggled(&harness, "Favorite team/api"), Some(Toggled::False));

    press(&mut harness, "Favorite team/api");
    assert_eq!(toggled(&harness, "Favorite team/api"), Some(Toggled::True), "the star fills");
    assert_eq!(
        saved_favorites(&harness),
        [("gitlab".to_owned(), "team/api".to_owned())],
        "saved at once"
    );
    // Keyboard: Space on the focused star stars the other repository too.
    harness.get_by_label("Favorite team/app").focus();
    harness.key_press(egui::Key::Space);
    pump(&mut harness, 0.5);
    assert_eq!(saved_favorites(&harness).len(), 2);
    drop(harness);

    let mut harness = start_app(&paths);
    wait_until_unsettled(&mut harness, "the Favorites card after the restart", |harness| {
        has(harness, "Favorites (2 repositories)")
    });
    assert!(has(&harness, "2 repositories"));
    go(
        &mut harness,
        Route::RepositoryPullRequests {
            connection_id: "gitlab".to_owned(),
            repository_key: "team/api".to_owned(),
        },
    );
    wait_until_unsettled(&mut harness, "the repository's star", |harness| {
        has(harness, "Favorite team/api")
    });
    assert_eq!(
        toggled(&harness, "Favorite team/api"),
        Some(Toggled::True),
        "starred after the restart"
    );
    press(&mut harness, "Favorite team/api");
    assert_eq!(toggled(&harness, "Favorite team/api"), Some(Toggled::False));
    assert_eq!(saved_favorites(&harness), [("gitlab".to_owned(), "team/app".to_owned())]);
}

/// One card lists the starred repositories of GitLab and GitHub together, grouped by repository,
/// the most recently updated repository first; without the grouping the rows mix, most recently
/// updated first, each with its platform. GitLab asks once per starred project; GitHub (with a
/// token) covers both starred repositories with one search.
#[test]
fn card_mixes_two_platforms_newest_updated_first() {
    let gitlab = GitLabHost::start();
    let github = GitHubHost::start("acme", &github_rows());
    let (_scratch, mut harness) = with_favorites(
        "favorites-mixed",
        &[gitlab.connection("gitlab", "Team GitLab"), github.connection("github", "Acme GitHub")],
        &[("gitlab", "team/app"), ("github", "acme/widgets"), ("github", "acme/tools")],
    );
    // The counts below are of the card's first load. The window goes to the background once that
    // load reached both hosts: the list's own refresh (`LIST_INTERVAL`) waits for the focus then,
    // however many frames a slow machine steps before the count. Coming back is no return that
    // refreshes the card (`Attention::without_pauses`).
    harness.state_mut().attention = Attention::without_pauses();
    let searches_repositories =
        |github: &GitHubHost| github_searches(github).iter().any(|query| query.contains("repo:"));
    wait_until_unsettled(&mut harness, "the card's first load at both hosts", |_| {
        gitlab.count(APP_LISTING) > 0 && searches_repositories(&github)
    });
    set_focus(&mut harness, false);
    wait_until_unsettled(&mut harness, "the Favorites card's rows", |harness| {
        has(harness, "Favorites (3 repositories), 4 pull requests, expanded")
    });
    pump(&mut harness, 1.0);
    let in_order = |harness: &Harness<'_, ReviewApp>, order: [&str; 4]| {
        let tops: Vec<f32> = order.iter().map(|label| first_top(harness, label)).collect();
        tops.windows(2).all(|pair| pair[0] < pair[1]).then_some(()).ok_or(tops)
    };
    let grouped = [
        "12 Widgets: faster startup",
        "5 App: retry uploads",
        "4 App: new settings page",
        "11 Tools: lint the docs",
    ];
    let by_repository = in_order(&harness, grouped);
    assert!(
        by_repository.is_ok(),
        "the newest repository first, its rows together: {by_repository:?}"
    );
    assert!(has(&harness, "team/app on Team GitLab, 2 pull requests, expanded"));
    assert!(has(&harness, "acme/widgets on Acme GitHub, 1 pull request, expanded"));
    harness.state_mut().services.settings.repository_sections.is_grouped = false;
    pump(&mut harness, 0.5);
    let mixed = [
        "12 Widgets: faster startup",
        "5 App: retry uploads",
        "11 Tools: lint the docs",
        "4 App: new settings page",
    ];
    let by_update = in_order(&harness, mixed);
    assert!(by_update.is_ok(), "ungrouped, newest updated first across platforms: {by_update:?}");
    assert!(has(&harness, "3 repositories"));
    assert_eq!(gitlab.count(APP_LISTING), 1, "one request for the starred GitLab project");
    let searches: Vec<String> =
        github_searches(&github).into_iter().filter(|query| query.contains("repo:")).collect();
    assert_eq!(
        searches,
        ["is:pr is:open repo:acme/tools repo:acme/widgets sort:updated-desc"],
        "one search covers both"
    );

    // The overview's state filter applies: merged pull requests are asked for, not filtered here.
    set_focus(&mut harness, true);
    press(&mut harness, "State: Active");
    press(&mut harness, "Merged");
    wait_until_unsettled(&mut harness, "the merged listing", |_| gitlab.count(APP_LISTING) == 2);
    let merged = gitlab.queries(APP_LISTING);
    assert!(merged.last().is_some_and(|query| query.contains("state=merged")), "{merged:?}");
}

/// The overview's role filter narrows the Favorites card like every other card: "Created by me"
/// asks each starred repository for the user's own pull requests (GitLab: the project's listing
/// with `scope=created_by_me`; GitHub: the search with `author:@me`), so rows of others leave it.
#[test]
fn card_follows_the_role_filter() {
    let gitlab = GitLabHost::start();
    let github = GitHubHost::start("acme", &github_rows());
    let (_scratch, mut harness) = with_favorites(
        "favorites-role",
        &[gitlab.connection("gitlab", "Team GitLab"), github.connection("github", "Acme GitHub")],
        &[("gitlab", "team/app"), ("github", "acme/widgets"), ("github", "acme/tools")],
    );
    wait_until_unsettled(&mut harness, "the Favorites card's rows", |harness| {
        has(harness, "Favorites (3 repositories), 4 pull requests, expanded")
    });
    press(&mut harness, "Created by me");
    // Grace wrote both merge requests of team/app; the mock lists none of them as the user's.
    wait_until_unsettled(&mut harness, "the user's own favorites", |harness| {
        has(harness, "Favorites (3 repositories), 2 pull requests, expanded")
    });
    assert!(!has(&harness, "App: retry uploads"), "rows of others leave the card");
    let listed = gitlab.queries(APP_LISTING);
    assert!(listed.last().is_some_and(|query| query.contains("scope=created_by_me")), "{listed:?}");
    let searches: Vec<String> =
        github_searches(&github).into_iter().filter(|query| query.contains("repo:")).collect();
    assert_eq!(
        searches.last().map(String::as_str),
        Some("is:pr author:@me is:open repo:acme/tools repo:acme/widgets sort:updated-desc")
    );
}

/// A Favorites card the role filter empties says so and offers to clear the filters, which brings
/// its rows back.
#[test]
fn card_emptied_by_a_filter_offers_to_clear_it() {
    let gitlab = GitLabHost::start();
    let (_scratch, mut harness) = with_favorites(
        "favorites-role-empty",
        &[gitlab.connection("gitlab", "Team GitLab")],
        &[("gitlab", "team/app")],
    );
    wait_until_unsettled(&mut harness, "the Favorites card's rows", |harness| {
        has(harness, "Favorites (1 repository), 2 pull requests, expanded")
    });
    press(&mut harness, "Assigned to me");
    wait_until_unsettled(&mut harness, "the emptied card", |harness| {
        has(harness, "Favorites (1 repository), 0 pull requests, expanded")
    });
    let listed = gitlab.queries(APP_LISTING);
    assert!(
        listed.last().is_some_and(|query| query.contains("reviewer_id=1")),
        "the user's reviews of the project: {listed:?}"
    );
    assert!(has(&harness, "No pull requests in this view."));
    harness
        .click_at(harness.query_all_by_label("Clear filters").next().expect("a way back").rect());
    harness.run_steps(4);
    wait_until_unsettled(&mut harness, "the rows again", |harness| {
        has(harness, "Favorites (1 repository), 2 pull requests, expanded")
    });
    assert!(has(&harness, "App: retry uploads"));
}

/// Under a role, the favorites of a connection without a token (which cannot tell who "me" is)
/// leave the card as that connection's own card does, but not silently: the header counts them as
/// hidden, a note at the top of the card says why from the first frame on, and "Add token" opens
/// the connection's setup at its token step.
#[test]
fn hidden_by_a_role_are_named_with_the_way_to_a_token() {
    let gitlab = GitLabHost::start();
    let public = Connection { token: None, ..gitlab.connection("public", "Public GitLab") };
    let (_scratch, mut harness) = with_favorites(
        "favorites-role-hidden",
        &[gitlab.connection("gitlab", "Team GitLab"), public],
        &[("gitlab", "team/app"), ("public", "team/api")],
    );
    wait_until_unsettled(&mut harness, "the Favorites card's rows", |harness| {
        has(harness, "Favorites (2 repositories), 2 pull requests, expanded")
    });
    assert!(!has(&harness, "hidden"), "everything is listed while no role is chosen");
    press(&mut harness, "Created by me");
    let note = "1 repository hidden: Public GitLab has no token to know who you are.";
    let placed = harness.get_by_label_contains(note).rect();
    wait_until_unsettled(&mut harness, "the user's own favorites", |harness| {
        has(harness, "Favorites (1 repository, 1 hidden), 0 pull requests, expanded")
    });
    pump(&mut harness, 1.0);
    assert_eq!(
        harness.get_by_label_contains(note).rect(),
        placed,
        "the note keeps its place while the rest loads"
    );
    assert!(has(&harness, "Clear filters"), "the emptied card still offers its way back");
    let api = gitlab.queries("/api/v4/projects/team%2Fapi/merge_requests");
    assert!(
        api.iter().all(|query| !query.contains("created_by_me")),
        "the anonymous connection is not asked for the user's part: {api:?}"
    );

    press(&mut harness, "Add token");
    pump(&mut harness, 0.5);
    assert_eq!(harness.state().route, Route::Settings);
    wait_until_unsettled(&mut harness, "the token step", |harness| {
        harness.query_by_role(egui::accesskit::Role::PasswordInput).is_some()
    });
}

/// Without favorites the card says what a star does and leads to the repositories; a row's
/// context menu stars its repository right there.
#[test]
fn empty_card_leads_to_the_repositories_and_rows_star_from_their_menu() {
    let gitlab = GitLabHost::start();
    let scratch = ScratchDir::new("favorites-empty");
    let paths = saved_paths(&scratch, &[gitlab.connection("gitlab", "Team GitLab")]);
    let mut harness = start_app(&paths);
    wait_until_unsettled(&mut harness, "the overview", |harness| has(harness, "Team change 1"));
    assert!(has(&harness, "Star a repository to see its pull requests here"));
    assert_eq!(gitlab.count(APP_LISTING), 0, "nothing starred, nothing asked");

    secondary_press(&mut harness, "1 Team change 1");
    press(&mut harness, "Add team/app to favorites");
    wait_until_unsettled(&mut harness, "the starred project's rows", |harness| {
        has(harness, "Favorites (1 repository), 2 pull requests, expanded")
    });
    assert_eq!(saved_favorites(&harness), [("gitlab".to_owned(), "team/app".to_owned())]);

    secondary_press(&mut harness, "5 App: retry uploads");
    press(&mut harness, "Remove team/app from favorites");
    wait_until_unsettled(&mut harness, "the empty card again", |harness| {
        has(harness, "Star a repository")
    });
    press(&mut harness, "Browse repositories");
    assert_eq!(harness.state().route, Route::Repositories);
}

/// The overview's star tool hides the Favorites lane: the card leaves and asks nothing, and a star
/// given then lifts the repository's merge request to the top of its card, marked, the rest in
/// their order. The choice survives a restart; the tool brings the lane back with today's order,
/// and the card's own menu hides it again.
#[test]
fn lane_hidden_lifts_starred_rows_and_asks_nothing() {
    let gitlab = GitLabHost::start();
    let scratch = ScratchDir::new("favorites-lane");
    let paths = saved_paths(&scratch, &[gitlab.connection("gitlab", "Team GitLab")]);
    let mut harness = start_app(&paths);
    wait_until_unsettled(&mut harness, "the overview", |harness| has(harness, "Team change 1"));
    assert!(has(&harness, "Favorites ("), "the lane is shown by default");
    assert_eq!(lane_tool(&harness), Some(Toggled::True));
    press_lane_tool(&mut harness);
    assert!(!has(&harness, "Favorites ("), "the card leaves");
    assert_eq!(lane_tool(&harness), Some(Toggled::False));
    assert!(!saved_lane(&harness), "saved at once");
    assert!(first_top(&harness, "2 API change 2") < first_top(&harness, "1 Team change 1"));

    secondary_press(&mut harness, "1 Team change 1");
    press(&mut harness, "Add team/app to favorites");
    pump(&mut harness, 1.0);
    assert_eq!(saved_favorites(&harness), [("gitlab".to_owned(), "team/app".to_owned())]);
    assert!(
        first_top(&harness, "1 Team change 1") < first_top(&harness, "2 API change 2"),
        "the starred repository's row leads its card"
    );
    assert!(has(&harness, "team/app is a favorite") && !has(&harness, "team/api is a favorite"));
    assert!(!has(&harness, "Favorites ("), "a star does not bring the lane back");
    assert_eq!(gitlab.count(APP_LISTING), 0, "no favorites request while the lane is hidden");
    drop(harness);

    let mut harness = start_app(&paths);
    wait_until_unsettled(&mut harness, "the overview after the restart", |harness| {
        has(harness, "Team change 1")
    });
    pump(&mut harness, 1.0);
    assert_eq!(lane_tool(&harness), Some(Toggled::False), "still hidden after the restart");
    assert!(!has(&harness, "Favorites ("));
    assert!(first_top(&harness, "1 Team change 1") < first_top(&harness, "2 API change 2"));
    assert_eq!(gitlab.count(APP_LISTING), 0, "the restart asks for no favorites either");

    press_lane_tool(&mut harness);
    wait_until_unsettled(&mut harness, "the lane again", |harness| {
        has(harness, "Favorites (1 repository), 2 pull requests, expanded")
    });
    assert!(saved_lane(&harness));
    assert_eq!(gitlab.count(APP_LISTING), 1, "the lane asks once it is back");
    assert!(!has(&harness, "is a favorite"), "with the lane shown rows carry no mark");
    assert!(
        first_top(&harness, "2 API change 2") < first_top(&harness, "1 Team change 1"),
        "with the lane shown the cards keep their own order"
    );

    secondary_press(&mut harness, "Favorites (1 repository), 2 pull requests, expanded");
    press(&mut harness, "Hide the Favorites lane");
    pump(&mut harness, 0.5);
    assert!(!has(&harness, "Favorites (") && !saved_lane(&harness), "the card's menu hides it");
}

/// The overview's star tool: whether it is on.
fn lane_tool(harness: &Harness<'_, ReviewApp>) -> Option<Toggled> {
    harness.get_by_role_and_label(Role::Button, FAVORITES_LANE).accesskit_node().toggled()
}

/// Clicks the star tool; the header shows its new state from the frame after the click took effect.
fn press_lane_tool(harness: &mut Harness<'_, ReviewApp>) {
    harness.click_at(harness.get_by_role_and_label(Role::Button, FAVORITES_LANE).rect());
    pump(harness, 1.0);
}

fn saved_lane(harness: &Harness<'_, ReviewApp>) -> bool {
    saved_settings(harness).favorites.is_lane_shown
}

fn github_rows() -> Vec<Value> {
    let row = |number: u64, title: &str, repository: &str, updated: &str| {
        let name = repository.rsplit('/').next().unwrap_or(repository);
        json!({
            "number": number, "title": title, "url": format!("https://ghe.example/{repository}/pull/{number}"),
            "state": "OPEN", "isDraft": false, "createdAt": "2026-10-01T09:00:00Z", "updatedAt": updated,
            "headRefName": format!("feature/{number}"), "baseRefName": "main",
            "author": { "login": "mona", "name": "Mona Lisa", "databaseId": 1 },
            "repository": { "nameWithOwner": repository, "name": name },
            "labels": { "nodes": [] }, "reviewRequests": { "nodes": [] }, "latestOpinionatedReviews": { "nodes": [] },
            "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "SUCCESS" } } }] },
        })
    };
    vec![
        row(12, "Widgets: faster startup", "acme/widgets", "2026-10-18T10:00:00Z"),
        row(11, "Tools: lint the docs", "acme/tools", "2026-10-16T10:00:00Z"),
    ]
}

/// The `q` of every GraphQL search the host received.
fn github_searches(github: &GitHubHost) -> Vec<String> {
    github
        .requests()
        .into_iter()
        .filter(|request| request.url.path() == "/api/graphql")
        .filter_map(|request| request.body_json::<Value>().ok())
        .filter_map(|body| body["variables"]["q"].as_str().map(str::to_owned))
        .collect()
}

/// A GitLab group `team` with the projects `team/app` and `team/api`; the group lists one merge
/// request of each project, newest first, `team/app` two others. Records every request's path and query.
struct GitLabHost {
    host: MockHost,
    hits: Arc<Mutex<Vec<(String, String)>>>,
}

impl GitLabHost {
    fn start() -> Self {
        let hits = Arc::new(Mutex::new(Vec::new()));
        let host = MockHost::start(|base_url| GitLabResponder {
            hits: hits.clone(),
            base_url: base_url.to_owned(),
        });
        Self { host, hits }
    }

    fn connection(&self, id: &str, name: &str) -> Connection {
        Connection {
            name: name.to_owned(),
            organizations: vec![GROUP.to_owned()],
            token: Some(TOKEN.to_owned()),
            ..samples::connection(id, ProviderKind::GitLab, &self.host.uri())
        }
    }

    fn count(&self, path: &str) -> usize {
        self.queries(path).len()
    }

    /// The query strings of the requests to `path`, in order.
    fn queries(&self, path: &str) -> Vec<String> {
        self.hits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(hit, _)| hit == path)
            .map(|(_, query)| query.clone())
            .collect()
    }
}

struct GitLabResponder {
    hits: Arc<Mutex<Vec<(String, String)>>>,
    base_url: String,
}

impl Respond for GitLabResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request.url.path().to_owned();
        self.hits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((path.clone(), request.url.query().unwrap_or_default().to_owned()));
        let body = match path.as_str() {
            "/api/v4/user" => json!({ "id": 1, "username": "ada", "name": "Ada" }),
            "/api/v4/groups/team/merge_requests" => json!([
                self.merge_request("team/api", 2, "API change 2", "2026-10-14T10:00:00Z"),
                self.merge_request("team/app", 1, "Team change 1", "2026-10-12T10:00:00Z"),
            ]),
            "/api/v4/groups/team/projects" => {
                json!([self.project(42, "app"), self.project(43, "api")])
            }
            // Grace wrote these and reviews nobody's: the user's own listing of the project is empty.
            APP_LISTING if is_mine(request) => json!([]),
            APP_LISTING => json!([
                self.merge_request("team/app", 5, "App: retry uploads", "2026-10-17T10:00:00Z"),
                self.merge_request("team/app", 4, "App: new settings page", "2026-10-15T10:00:00Z"),
            ]),
            "/api/v4/projects/team%2Fapi/merge_requests" => json!([]),
            _ => {
                return ResponseTemplate::new(404)
                    .set_body_json(json!({ "message": "404 Not Found" }));
            }
        };
        ResponseTemplate::new(200).set_body_json(body)
    }
}

/// The request narrows to the user's merge requests (created by or assigned to them).
fn is_mine(request: &Request) -> bool {
    request
        .url
        .query_pairs()
        .any(|(key, value)| (key == "scope" && value == "created_by_me") || key == "reviewer_id")
}

impl GitLabResponder {
    fn project(&self, id: u64, name: &str) -> Value {
        json!({
            "id": id, "name": name, "path_with_namespace": format!("{GROUP}/{name}"), "description": format!("The {name} project"),
            "web_url": format!("{}/{GROUP}/{name}", self.base_url), "default_branch": "main",
            "http_url_to_repo": format!("{}/{GROUP}/{name}.git", self.base_url),
        })
    }

    fn merge_request(&self, project: &str, iid: u64, title: &str, updated: &str) -> Value {
        json!({
            "id": 1000 + iid, "iid": iid, "project_id": 42, "title": title, "description": "",
            "state": "opened", "draft": false, "author": { "id": 2, "username": "grace", "name": "Grace Hopper" },
            "source_branch": format!("feature-{iid}"), "target_branch": "main",
            "created_at": "2026-10-01T10:00:00Z", "updated_at": updated,
            "web_url": format!("{}/{project}/-/merge_requests/{iid}", self.base_url),
            "references": { "full": format!("{project}!{iid}") }, "labels": [], "reviewers": [],
            "sha": "a".repeat(40), "detailed_merge_status": "mergeable",
        })
    }
}

fn saved_favorites(harness: &Harness<'_, ReviewApp>) -> Vec<(String, String)> {
    saved_settings(harness)
        .favorites
        .repositories()
        .iter()
        .map(|favorite| (favorite.connection_id.clone(), favorite.repository_key.clone()))
        .collect()
}

fn toggled(harness: &Harness<'_, ReviewApp>, label: &str) -> Option<Toggled> {
    harness.get_by_label(label).accesskit_node().toggled()
}

/// Top of the first node labelled `label`: the Favorites card leads, so its copy comes first.
fn first_top(harness: &Harness<'_, ReviewApp>, label: &str) -> f32 {
    harness
        .query_all_by_label(label)
        .map(|node| node.rect().min.y)
        .reduce(f32::min)
        .unwrap_or_else(|| panic!("no row labelled {label}"))
}

/// The app on `connections`, started with `favorites` (connection id, repository) saved; the
/// scratch folder named `name` lives as long as the returned guard.
fn with_favorites(
    name: &str,
    connections: &[Connection],
    favorites: &[(&str, &str)],
) -> (ScratchDir, Harness<'static, ReviewApp>) {
    let scratch = ScratchDir::new(name);
    let paths = saved_paths(&scratch, connections);
    let mut settings = AppSettings::default();
    for (connection_id, repository_key) in favorites {
        settings.favorites.toggle(connection_id, repository_key);
    }
    settings.save(&paths.settings_file()).expect("save the favorites");
    let harness = start_app(&paths);
    (scratch, harness)
}

fn start_app(paths: &AppPaths) -> Harness<'static, ReviewApp> {
    let builder = Harness::builder().with_size([1440.0, 900.0]).with_step_dt(0.25);
    app_with(builder, paths.clone(), cv_providers::create_provider)
}

fn go(harness: &mut Harness<'_, ReviewApp>, route: Route) {
    harness.state_mut().navigator.go(route);
    harness.step();
}
