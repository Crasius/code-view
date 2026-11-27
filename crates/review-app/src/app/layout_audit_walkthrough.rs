//! Clipping audit: the real app walks every screen and major state with deliberately long data
//! (organization, repository, branch and file names, titles, job and stage names, many labels and
//! reviewers, 3,000 changed files, a 100,000-line file) and [`super::clip_check`] inspects each
//! state at 960, 1280 and 1920 points wide, at 1x and 2x pixels per point, and in a short window
//! (960 × 540 at 2x, a minimum-size window at a zoom above 100 %); the list of the user's pull
//! requests, with its empty Favorites card, also at 720 points (a 960-pixel window at 133 %
//! zoom): no text cut without an ellipsis, drawn over other text or past the window, no colliding
//! controls, no popup off the window, and every text cut with "…" tells its full wording on hover.
//! Offline: a local git repository is the remote and a scripted Azure DevOps provider describes
//! the pull requests.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use async_trait::async_trait;
use egui::{Modifiers, Role, Vec2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{
    Capabilities, Connection, GitRemote, ItemKind, ItemRef, Label, LinkedItem, LinkedState,
    MergePolicy, MergeStrategy, PipelineAttempt, PipelineJob, PipelineRun, ProviderError,
    ProviderKind, PullRequestDetail, PullRequestQuery, PullRequestSummary, RerunAction, RerunKind,
    Result, ReviewThread, ReviewVote, Reviewer, RunStatus, ThreadAnchor, ThreadComment,
    ThreadOrigin, ThreadStatus, Timestamp, UserIdentity,
};
use review_store::{AppPaths, DiffLayout};
use time::macros::datetime;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::walkthrough::{
    app_with, files_tab, has, press_role, pump, saved_paths, settle_background, shows,
    shows_file_view, tab_rect, wait_until, wait_until_still, wheel,
};
use super::{ReviewApp, clip_check};
use cv_base::platform::FOCUS_MODE_KEY;
use cv_pr_diff::find::bar::last_bar as last_find_bar;
use cv_runtime::route::{PullRequestKey, Route};
use cv_shell::focus_mode::EXIT_HINT;
use cv_testkit::provider::{Script, Scripted};
use cv_testkit::{Clicks, ScratchDir, git, samples};

const CONNECTION: &str = "clip-audit";
/// The same host without a token: its stage cards offer to add one.
const ANONYMOUS: &str = "clip-audit-anonymous";
/// Hosts whose lists never answer, fail and come back empty: the list's other card states.
const SLOW: &str = "clip-audit-slow";
const BROKEN: &str = "clip-audit-broken";
const EMPTY: &str = "clip-audit-empty";
const ORGANIZATION: &str = "Fabrikam-Industries-Enterprise-Platform";
const REPOSITORY: &str = "Fabrikam-Industries-Enterprise-Platform/Fabrikam Logistics Platform/fabrikam-logistics-service-backend-orchestrator";
/// A repository whose mirror never finishes preparing: the Files tab shows its loading log.
const SLOW_REPOSITORY: &str = "Fabrikam-Industries-Enterprise-Platform/Fabrikam Logistics Platform/fabrikam-logistics-legacy-monolith-mirror";
const NUMBER: u64 = 74_123;
const TITLE: &str = "Issue an invoice for every successful charge and keep the composition keyed by (PackageId, DepotId) end to end across the downloads orchestrator";
const SOURCE_BRANCH: &str =
    "feature/CDS-48213-issue-invoices-for-every-successful-charge-and-keep-composition-keyed";
const TARGET_BRANCH: &str = "release/2026.10-fabrikam-logistics-hotfix-integration-branch";
const LONG_PATH: &str = "src/Backend/Apps/FabrikamLogistics.Creator/v1/Features/Orders/Handlers/OrderFabrikamShipmentHandlerWithRetriesAndCompensation.cs";
const LARGE_PATH: &str =
    "data/fixtures/invoices/fabrikam-logistics-invoices-large-regression-fixture.csv";
const IMAGE_PATH: &str =
    "assets/brand/fabrikam-logistics-logo-with-an-unusually-long-file-name-for-the-preview.png";
const GENERATED: usize = 3_000;
const LARGE_LINES: usize = 100_000;
const RUN: &str =
    "Fabrikam Logistics Service - Build, Test and Publish Container Images #20261019.47";
const RETRIED_JOB: &str =
    "Run the integration test suite against the staging database cluster (eu-west-1, shard 3 of 4)";
const WAITING_JOB: &str =
    "Publish the signed container images to the regional registries (waiting for an agent)";
const BASE_SIZE: Vec2 = Vec2::new(1280.0, 900.0);
/// The sizes every state is inspected at: (width, height, pixels per point).
const SIZES: [(f32, f32, f32); 7] = [
    (960.0, 800.0, 1.0),
    (1280.0, 900.0, 1.0),
    (1920.0, 1080.0, 1.0),
    (960.0, 800.0, 2.0),
    (1280.0, 900.0, 2.0),
    (1920.0, 1080.0, 2.0),
    (960.0, 540.0, 2.0),
];
/// The list's cards are also inspected this narrow: their notes put a sentence and a button in
/// one row.
const NARROW: (f32, f32, f32) = (720.0, 800.0, 1.0);

#[derive(Clone)]
struct Fixture {
    clone_url: String,
    target: String,
    /// The first commit of the feature branch, where the outdated thread was written.
    first: String,
    head: String,
}

#[test]
fn every_screen_keeps_its_text_and_controls_inside() {
    let scratch = ScratchDir::new("clip-audit");
    let fixture = create_origin(&scratch.path().join("origin"));
    let paths = saved_paths(&scratch, &connections());
    let mut audit = Audit { harness: harness(paths, fixture), findings: Vec::new() };

    audit.walk_lists();
    audit.walk_overview();
    audit.walk_files();
    audit.walk_pipelines();
    audit.walk_shell();

    let report = audit.findings.join("\n");
    assert!(audit.findings.is_empty(), "{} layout problems:\n{report}", audit.findings.len());
}

struct Audit {
    harness: Harness<'static, ReviewApp>,
    findings: Vec<String>,
}

impl Audit {
    /// Inspects the state on screen at every size.
    fn check(&mut self, state: &str) {
        self.check_at(state, &SIZES);
    }

    /// Inspects the state on screen at `sizes`; the first also for elided text without a tooltip.
    fn check_at(&mut self, state: &str, sizes: &[(f32, f32, f32)]) {
        let started = Instant::now();
        for (index, &(width, height, pixels_per_point)) in sizes.iter().enumerate() {
            resize(&mut self.harness, vec2(width, height), pixels_per_point);
            wait_for_rest(&mut self.harness);
            let size = format!("{width:.0}x{height:.0}@{pixels_per_point:.0}x");
            assert_eq!(
                self.harness.ctx.content_rect().size(),
                vec2(width, height),
                "{state} is inspected at {size}"
            );
            let mut found = clip_check::inspect(&mut self.harness);
            if index == 0 {
                found.extend(clip_check::untold_elisions(&mut self.harness));
            }
            for finding in found {
                let line = format!("{state} {size}: {finding}");
                println!("{line}");
                self.findings.push(line);
            }
        }
        resize(&mut self.harness, BASE_SIZE, 1.0);
        wait_for_rest(&mut self.harness);
        println!("checked {state} in {:.1} s", started.elapsed().as_secs_f32());
    }

    fn walk_lists(&mut self) {
        let harness = &mut self.harness;
        harness.state_mut().navigator.go(Route::MyPullRequests);
        wait_until(harness, "the list", |harness| has(harness, &TITLE[..40]));
        assert!(has(harness, "Browse repositories"), "the empty Favorites card shows");
        self.check("01-list-mine");
        self.check_at("01-list-mine", &[NARROW]);
        let harness = &mut self.harness;
        harness.click_at(
            harness
                .get_by(|node| {
                    node.role() == Role::ComboBox
                        && node.label().is_some_and(|label| label.starts_with("Repository:"))
                })
                .rect(),
        );
        wait_until(harness, "the repository picker", |harness| {
            egui::Popup::is_any_open(&harness.ctx)
        });
        self.check("02-list-repository-picker");
        let harness = &mut self.harness;
        harness.key_press(egui::Key::Escape);
        wait_for_rest(harness);
        harness.state_mut().navigator.go(Route::RepositoryPullRequests {
            connection_id: CONNECTION.to_owned(),
            repository_key: REPOSITORY.to_owned(),
        });
        wait_until(harness, "the repository's list", |harness| {
            harness.query_by_role_and_label(Role::Tab, "Merged").is_some()
                && has(harness, &TITLE[..40])
        });
        self.check("03-list-repository");
    }

    fn walk_overview(&mut self) {
        let harness = &mut self.harness;
        open(harness, CONNECTION, REPOSITORY);
        wait_until(harness, "the overview", |harness| {
            has(harness, "Fabrikam Logistics Platform Reviewers")
        });
        self.check("04-overview");
        let harness = &mut self.harness;
        // Over the main column, below the header.
        harness.hover_at(egui::pos2(BASE_SIZE.x * 0.35, BASE_SIZE.y * 0.6));
        for _ in 0..12 {
            harness.event(wheel(-400.0, egui::TouchPhase::Move, egui::MouseWheelSource::Wheel));
            harness.step();
        }
        wait_for_rest(harness);
        self.check("05-overview-activity");
        let harness = &mut self.harness;
        // The scroll slid the header away; the keyboard focus on the merge button brings it back.
        harness.get_by_role_and_label(Role::Button, "Merge options").focus();
        let window = harness.ctx.content_rect();
        wait_until_still(harness, "the merge button back in view", |harness| {
            let button = harness.query_by_role_and_label(Role::Button, "Merge options")?;
            Some(button.rect()).filter(|rect| window.contains_rect(*rect))
        });
        press_role(harness, Role::Button, "Merge options");
        wait_until(harness, "the merge options", |harness| egui::Popup::is_any_open(&harness.ctx));
        self.check("06-merge-options");
        self.harness.key_press(egui::Key::Escape);
        wait_for_rest(&mut self.harness);
    }

    fn walk_files(&mut self) {
        let harness = &mut self.harness;
        files_tab(harness);
        wait_until(harness, "the file tree", |harness| {
            harness.query_by_label("File categories").is_some()
        });
        // A card's height waits on its file's line counts: counted later, the cards move.
        wait_until(harness, "the line counts", |harness| {
            harness.state().pull_requests.iter().all(|(_, page)| !page.is_counting_lines())
        });
        pump(harness, 2.0);
        self.check("07-files-summary");
        filter_tree(&mut self.harness, "Handlers");
        let harness = &mut self.harness;
        wait_until(harness, "the handler's folder", |harness| {
            tree_node(harness, "Handlers").is_some()
        });
        settle_background(harness);
        harness.click_at(tree_node(harness, "Handlers").expect("the folder").rect());
        pump(harness, 2.0);
        self.check("08-folder-summary-with-comments");
        let harness = &mut self.harness;
        harness.click_at(
            tree_node(harness, LONG_PATH.rsplit('/').next().unwrap_or(LONG_PATH))
                .expect("the handler")
                .rect(),
        );
        wait_until(harness, "the long diff", |harness| shows_file_view(harness, LONG_PATH));
        pump(harness, 1.0);
        self.check("09-diff-side-by-side");
        self.find_in_diff();
        let harness = &mut self.harness;
        harness.click_at(harness.get_by_role_and_label(Role::RadioButton, "Inline").rect());
        wait_until(harness, "the inline diff", |harness| {
            harness.state().services.settings.diff_layout == DiffLayout::Inline
        });
        wait_for_rest(harness);
        self.check("10-diff-inline");
        // The same diff with the file tree at the right edge.
        let harness = &mut self.harness;
        harness.click_at(harness.get_by_label("Move the file tree to the right").rect());
        wait_until(harness, "the file tree at the right", |harness| {
            shows(harness, "Move the file tree to the left")
        });
        wait_for_rest(harness);
        self.check("10b-diff-inline-tree-right");
        let harness = &mut self.harness;
        harness.click_at(harness.get_by_label("Move the file tree to the left").rect());
        wait_for_rest(harness);
        let harness = &mut self.harness;
        open_origin(harness);
        self.check("11-diff-original-version");
        let harness = &mut self.harness;
        harness.key_press(egui::Key::Escape);
        wait_for_rest(harness);
        harness.key_press_modifiers(FOCUS_MODE_KEY.modifiers, FOCUS_MODE_KEY.logical_key);
        wait_for_focus_mode(harness);
        self.check("12-focus-files");
        let harness = &mut self.harness;
        harness.key_press(egui::Key::Escape);
        wait_for_rest(harness);
        for (filter, name, state) in [
            ("README", "README.md", "13-preview-markdown"),
            ("logo", IMAGE_PATH, "14-preview-image"),
            ("invoices-large", LARGE_PATH, "15-diff-large-file"),
        ] {
            filter_tree(&mut self.harness, filter);
            let harness = &mut self.harness;
            let file = name.rsplit('/').next().unwrap_or(name);
            wait_until(harness, file, |harness| tree_node(harness, file).is_some());
            settle_background(harness);
            harness.click_at(tree_node(harness, file).expect("the file").rect());
            pump(harness, 2.0);
            self.check(state);
        }
        let harness = &mut self.harness;
        harness.click_at(tab_rect(harness, "Commits").expect("the Commits tab"));
        wait_until(harness, "the commits", |harness| has(harness, "Keep the composition keyed"));
        self.check("16-commits");
    }

    /// The find widget open over the diff's focused pane with hits, then closed again.
    fn find_in_diff(&mut self) {
        let harness = &mut self.harness;
        let diff = harness.get_by_label(&format!("Diff of {LONG_PATH}")).rect();
        harness.hover_at(diff.center());
        harness.step();
        harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::F);
        harness.event(egui::Event::Text("compositionKey".to_owned()));
        wait_until(harness, "the find widget's hits", |harness| {
            last_find_bar(&harness.ctx).is_some_and(|shown| shown.count.contains(" of "))
        });
        self.check("09b-diff-find-widget");
        let harness = &mut self.harness;
        harness.key_press(egui::Key::Escape);
        wait_for_rest(harness);
        assert!(last_find_bar(&harness.ctx).is_none(), "Esc closed the find widget");
    }

    fn walk_pipelines(&mut self) {
        let harness = &mut self.harness;
        harness.click_at(harness.get_by_role_and_label(Role::Tab, "Pipelines").rect());
        wait_until(harness, "the checks table", |harness| {
            harness.query_by_label("Filter checks").is_some()
        });
        pump(harness, 1.0);
        self.check("17-checks");
        let harness = &mut self.harness;
        harness.click_at(harness.get_by_label(&format!("{RUN}, Failed")).rect());
        wait_until(harness, "the stage graph", |harness| has(harness, "Stage Build and unit test"));
        pump(harness, 1.0);
        self.check("18-stages-100");
        for (steps, state) in [(-4_i32, "19-stages-50"), (9, "20-stages-200")] {
            let harness = &mut self.harness;
            let canvas = harness.get_by_label_contains("Stage Build and unit test").rect().center();
            harness.hover_at(canvas);
            harness.step();
            let key = if steps < 0 { egui::Key::Minus } else { egui::Key::Plus };
            for _ in 0..steps.unsigned_abs() {
                harness.key_press_modifiers(Modifiers::COMMAND, key);
                harness.step();
            }
            pump(harness, 1.0);
            self.check(state);
        }
        let harness = &mut self.harness;
        harness.click_at(harness.get_by_label_contains("Zoom 200 %").rect());
        pump(harness, 1.0);
        harness.click_at(
            harness.get_by_label(&format!("{RETRIED_JOB}, 3 attempts. Switch attempt")).rect(),
        );
        wait_until(harness, "the attempts list", |harness| egui::Popup::is_any_open(&harness.ctx));
        wait_for_rest(harness);
        self.check("21-attempts");
        let harness = &mut self.harness;
        // The list closes by itself at a size where its stage card is off-canvas; Esc then
        // would leave the run.
        if egui::Popup::is_any_open(&harness.ctx) {
            harness.key_press(egui::Key::Escape);
            wait_for_rest(harness);
        }
        wait_until(harness, "the retried job", |harness| {
            harness.query_by_role_and_label(Role::Link, RETRIED_JOB).is_some()
        });
        settle_background(harness);
        harness.click_at(harness.get_by_role_and_label(Role::Link, RETRIED_JOB).rect());
        wait_until(harness, "the log", |harness| harness.query_by_label("Copy log").is_some());
        pump(harness, 1.0);
        self.check("22-log");
        let harness = &mut self.harness;
        harness.key_press_modifiers(FOCUS_MODE_KEY.modifiers, FOCUS_MODE_KEY.logical_key);
        wait_for_focus_mode(harness);
        self.check("23-focus-log");
        let harness = &mut self.harness;
        harness.key_press(egui::Key::Escape);
        wait_for_rest(harness);
        harness.click_at(harness.get_by_role_and_label(Role::TreeItem, WAITING_JOB).rect());
        wait_until(harness, "the waiting job", |harness| has(harness, "Waiting for an agent"));
        self.check("24-job-state");

        let harness = &mut self.harness;
        open(harness, ANONYMOUS, REPOSITORY);
        wait_until(harness, "the anonymous pull request", |harness| {
            harness.query_by_role_and_label(Role::Tab, "Pipelines").is_some()
        });
        settle_background(harness);
        harness.click_at(harness.get_by_role_and_label(Role::Tab, "Pipelines").rect());
        wait_until(harness, "the checks table", |harness| {
            harness.query_by_label("Filter checks").is_some()
        });
        settle_background(harness);
        harness.click_at(harness.get_by_label(&format!("{RUN}, Failed")).rect());
        wait_until(harness, "the stage graph", |harness| has(harness, "Stage Build and unit test"));
        pump(harness, 1.0);
        self.check("25-stages-without-token");
    }

    fn walk_shell(&mut self) {
        let harness = &mut self.harness;
        open(harness, CONNECTION, SLOW_REPOSITORY);
        wait_until(harness, "the slow pull request", |harness| {
            tab_rect(harness, "Files").is_some()
        });
        files_tab(harness);
        pump(harness, 2.0);
        self.check("26-files-preparing");
        let harness = &mut self.harness;
        harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::K);
        harness.event(egui::Event::Text("fabrikam logistics".to_owned()));
        wait_until(harness, "the search's results", |harness| {
            shows(harness, &format!("!{NUMBER} {TITLE}"))
        });
        wait_for_rest(harness);
        self.check("27-search");
        let harness = &mut self.harness;
        harness.key_press(egui::Key::Escape);
        wait_for_rest(harness);
        let services = &mut harness.state_mut().services;
        let retry = cv_runtime::toasts::ToastAction::retry(egui::Id::unique("layout_audit.retry"));
        services.toasts.error(format!("Could not reach {REPOSITORY}: the connection to dev.azure.com was reset while the request was in flight."), retry);
        let rerun = format!("Rerun of {RETRIED_JOB} started.");
        services.toasts.success(rerun.clone());
        wait_until(harness, "the toasts", |harness| shows(harness, &rerun));
        wait_for_rest(harness);
        self.check("28-toasts");
        let harness = &mut self.harness;
        harness.state_mut().navigator.go(Route::Settings);
        pump(harness, 1.0);
        self.check("29-settings");
    }
}

/// Gives the window `size` points at `pixels_per_point`. The scale goes first, in a frame of its
/// own: in the frame a scale changes, egui takes the window from the frame before, scaled to the
/// new points, instead of the size the harness sends (so 1920 × 1080 at 1x became 960 × 540 at 2x).
#[expect(clippy::float_cmp, reason = "a scale the harness was given back unchanged, not computed")]
fn resize(harness: &mut Harness<'_, ReviewApp>, size: Vec2, pixels_per_point: f32) {
    if harness.ctx.pixels_per_point() != pixels_per_point {
        harness.set_pixels_per_point(pixels_per_point);
        harness.step();
    }
    harness.set_size(size);
}

/// The file tree's row whose path ends with `name`.
fn tree_node<'tree>(
    harness: &'tree Harness<'_, ReviewApp>,
    name: &str,
) -> Option<egui_kittest::Node<'tree>> {
    let (name, suffix) = (name.to_owned(), format!("/{name}"));
    harness
        .query_all_by(move |node| {
            node.role() == Role::TreeItem
                && node.label().is_some_and(|label| label == name || label.ends_with(&suffix))
        })
        .next()
}

/// Replaces the file tree's filter with `text`.
fn filter_tree(harness: &mut Harness<'_, ReviewApp>, text: &str) {
    let field = harness
        .query_all_by_role(Role::TextInput)
        .find(|node| {
            node.accesskit_node()
                .placeholder()
                .is_some_and(|placeholder| placeholder.contains("Filter files"))
                || node.value().is_some_and(|value| !value.is_empty())
        })
        .expect("the file filter");
    let id = field.accesskit_node().id();
    field.focus();
    harness.step();
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    harness.event(egui::Event::Text(text.to_owned()));
    wait_until(harness, &format!("the file filter holding {text:?}"), |harness| {
        harness
            .query_by(|node| node.id() == id)
            .is_some_and(|field| field.value().as_deref() == Some(text))
    });
}

/// Opens the outdated thread's original version from the handler's diff.
fn open_origin(harness: &mut Harness<'_, ReviewApp>) {
    const SHOW_ORIGIN: &str = "Show original version";
    wait_until(harness, "the outdated thread", |harness| {
        harness.query_by_label(SHOW_ORIGIN).is_some() || has(harness, "not shown in this diff")
    });
    settle_background(harness);
    if harness.query_by_label(SHOW_ORIGIN).is_none() {
        harness.click_at(harness.get_by_label_contains("not shown in this diff").rect());
        wait_until(harness, "the outdated card", |harness| {
            harness.query_by_label(SHOW_ORIGIN).is_some()
        });
        settle_background(harness);
    }
    harness.click_at(harness.get_by_label(SHOW_ORIGIN).rect());
    wait_until(harness, "the original version", |harness| has(harness, "Showing the version from"));
}

fn open(harness: &mut Harness<'_, ReviewApp>, connection_id: &str, repository_key: &str) {
    harness.state_mut().navigator.go(Route::PullRequest(PullRequestKey {
        connection_id: connection_id.to_owned(),
        repository_key: repository_key.to_owned(),
        number: NUMBER,
    }));
    harness.step();
}

fn harness(paths: AppPaths, fixture: Fixture) -> Harness<'static, ReviewApp> {
    let builder = Harness::builder().with_size(BASE_SIZE).with_step_dt(1.0 / 60.0);
    let mut harness = app_with(builder, paths, move |connection: Connection, _| {
        Arc::new(Scripted(ClipAuditScript {
            connection_id: connection.id,
            fixture: fixture.clone(),
        }))
    });
    clip_check::install(&harness.ctx);
    harness.state_mut().title_bar.integrate_for_test();
    harness
}

/// Waits until focus mode is on (its strip offers the way out) and the screen is at rest.
fn wait_for_focus_mode(harness: &mut Harness<'_, ReviewApp>) {
    wait_until(harness, "focus mode", |harness| shows(harness, EXIT_HINT));
    wait_for_rest(harness);
}

/// Waits until no node on screen moved from one frame to the next: the state an audit inspects.
/// Background work may still run (a mirror that never finishes preparing).
fn wait_for_rest(harness: &mut Harness<'_, ReviewApp>) {
    wait_until_still(harness, "the screen", |harness| {
        let rects: Vec<_> = harness
            .query_all_by(|_| true)
            .filter_map(|node| node.accesskit_node().bounding_box())
            .collect();
        Some(rects)
    });
}

fn write(root: &Path, path: &str, bytes: &[u8]) {
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().expect("a folder")).expect("file folder");
    std::fs::write(full, bytes).expect("file");
}

/// The long handler, every ninth line changed, some lines far wider than any pane.
fn handler(changed: bool) -> String {
    (0..1_500)
        .map(|index| {
            let edited = changed && index % 9 == 4;
            let value = if edited { index * 3 } else { index };
            if index % 50 == 7 {
                format!("        var compositionKey{index} = new CompositionKey(packageId: order.PackageId, depotId: order.DepotId, region: order.Region, attempt: {value}); // keyed by (PackageId, DepotId) end to end so retries and compensation never pick another invoice\n")
            } else {
                format!("        var step{index} = await handler.ExecuteAsync(order, {value});\n")
            }
        })
        .collect()
}

/// A repository whose feature branch changes the handler twice, a README, an image, a
/// 100,000-line fixture and 3,000 generated clients.
fn create_origin(origin: &Path) -> Fixture {
    std::fs::create_dir_all(origin).expect("origin folder");
    git(origin, &["init", "-q", "-b", "main"]);
    git(origin, &["config", "uploadpack.allowFilter", "true"]);
    git(origin, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
    write(origin, LONG_PATH, handler(false).as_bytes());
    let readme = "# Fabrikam Logistics\n\nIssues invoices for every charge.\n\n## Usage\n\nCall `Orchestrator.Start()` once.\n";
    write(origin, "README.md", readme.as_bytes());
    let mut logo = Vec::new();
    image::RgbaImage::from_pixel(48, 48, image::Rgba([40, 90, 200, 255]))
        .write_to(&mut std::io::Cursor::new(&mut logo), image::ImageFormat::Png)
        .expect("encode the logo");
    write(origin, IMAGE_PATH, &logo);
    let client = |index: usize, version: usize| {
        format!(
            "namespace Generated.Clients;\n\npublic sealed partial class FabrikamLogisticsClient{index} {{ public const int Version = {version}; }}\n"
        )
    };
    for index in 0..GENERATED {
        write(origin, &generated_path(index), client(index, 1).as_bytes());
    }
    git(origin, &["add", "-A"]);
    git(origin, &["commit", "-q", "-m", "Add the downloads orchestrator"]);
    let target = git(origin, &["rev-parse", "HEAD"]);

    git(origin, &["switch", "-q", "-c", "feature"]);
    write(
        origin,
        LONG_PATH,
        handler(true).replace("ExecuteAsync(order, 40)", "ExecuteAsync(order, 41)").as_bytes(),
    );
    git(origin, &["commit", "-q", "-am", "Issue an invoice for every successful charge"]);
    let first = git(origin, &["rev-parse", "HEAD"]);
    write(origin, LONG_PATH, handler(true).as_bytes());
    let readme =
        readme.replace("every charge", "every successful charge, keyed by (PackageId, DepotId)");
    write(origin, "README.md", format!("{readme}\n## Invoices\n\n| Field | Meaning |\n| --- | --- |\n| PackageId | The package bought |\n").as_bytes());
    let mut logo = Vec::new();
    image::RgbaImage::from_pixel(48, 48, image::Rgba([220, 120, 30, 255]))
        .write_to(&mut std::io::Cursor::new(&mut logo), image::ImageFormat::Png)
        .expect("encode the logo");
    write(origin, IMAGE_PATH, &logo);
    let large = (0..LARGE_LINES).fold(String::new(), |mut text, index| {
        writeln!(
            text,
            "{index},PKG-{:06},DEPOT-{:04},{}.{:02}",
            index * 7 % 1_000_000,
            index % 9_000,
            index % 500,
            index % 100
        )
        .unwrap();
        text
    });
    write(origin, LARGE_PATH, large.as_bytes());
    for index in 0..GENERATED {
        write(origin, &generated_path(index), client(index, 2).as_bytes());
    }
    git(origin, &["add", "-A"]);
    git(
        origin,
        &[
            "commit",
            "-q",
            "-m",
            "Keep the composition keyed by (PackageId, DepotId) end to end across the downloads orchestrator and its generated clients",
        ],
    );
    let head = git(origin, &["rev-parse", "HEAD"]);
    Fixture { clone_url: cv_testkit::file_url(origin), target, first, head }
}

fn generated_path(index: usize) -> String {
    format!(
        "generated/clients/fabrikam-logistics/module_{:02}/FabrikamLogisticsClient{index:04}.g.cs",
        index / 100
    )
}

fn connections() -> Vec<Connection> {
    let hosts = [
        (CONNECTION, "Contoso Fabrikam Industries Enterprise (dev.azure.com)", true),
        (ANONYMOUS, "Contoso Fabrikam Industries Enterprise without a token", false),
        (SLOW, "Fabrikam Slow Host Behind A Very Distant Proxy", true),
        (BROKEN, "Northwind Broken Host With An Expired Certificate", true),
        (EMPTY, "Tailspin Toys Host Without Any Pull Requests", true),
    ];
    let base_url = format!("https://dev.azure.com/{ORGANIZATION}");
    hosts
        .into_iter()
        .map(|(id, name, has_token)| Connection {
            name: name.to_owned(),
            organizations: vec![ORGANIZATION.to_owned()],
            token: has_token.then(|| "scripted".to_owned()),
            ..samples::connection(id, ProviderKind::AzureDevOps, &base_url)
        })
        .collect()
}

struct ClipAuditScript {
    connection_id: String,
    fixture: Fixture,
}

fn person(id: &str, name: &str) -> UserIdentity {
    UserIdentity {
        id: id.to_owned(),
        display_name: name.to_owned(),
        login: Some(name.to_lowercase().replace(' ', ".")),
        avatar_url: None,
    }
}

fn at(day: u8, hour: u8) -> Timestamp {
    datetime!(2026-10-01 00:00 UTC)
        + time::Duration::days(i64::from(day))
        + time::Duration::hours(i64::from(hour))
}

const LABELS: [&str; 14] = [
    "needs-review",
    "performance",
    "backend",
    "breaking-change-requires-migration-guide",
    "invoicing",
    "fabrikam-logistics",
    "security-review-required-before-release",
    "eu-west-1",
    "hotfix",
    "database-schema",
    "good first issue",
    "do-not-merge-before-2026-10-31",
    "observability",
    "customer-escalation-CDS-48213",
];

fn labels(count: usize) -> Vec<Label> {
    LABELS
        .iter()
        .take(count)
        .map(|name| Label {
            name: (*name).to_owned(),
            color: Some("0e8a16".to_owned()),
            description: Some(format!("{name}: a label with a description")),
        })
        .collect()
}

fn reviewers() -> Vec<Reviewer> {
    let reviewer = |id: &str, name: &str, vote: ReviewVote, is_required: bool, is_group: bool| {
        Reviewer { identity: person(id, name), vote, is_required, is_group }
    };
    vec![
        reviewer(
            "g1",
            "[Fabrikam-Industries-Enterprise-Platform]\\Fabrikam Logistics Platform Reviewers (Backend Services)",
            ReviewVote::None,
            true,
            true,
        ),
        reviewer(
            "r1",
            "Maximiliane Theodora von Hohenzollern-Sigmaringen",
            ReviewVote::Approved,
            true,
            false,
        ),
        reviewer(
            "r2",
            "Bartholomew Fitzgerald-Okonkwo",
            ReviewVote::ApprovedWithSuggestions,
            false,
            false,
        ),
        reviewer("r3", "Anastasia Konstantinopoulou", ReviewVote::WaitingForAuthor, false, false),
        reviewer("r4", "Jean-Baptiste Emmanuel Zorg", ReviewVote::ChangesRequested, false, false),
        reviewer("r5", "Siobhán Ní Mhaolchatha", ReviewVote::None, false, false),
        reviewer(
            "g2",
            "[Fabrikam-Industries-Enterprise-Platform]\\Security Champions",
            ReviewVote::None,
            true,
            true,
        ),
        reviewer(
            "r6",
            "Wolfeschlegelsteinhausenbergerdorff Hubert",
            ReviewVote::Approved,
            false,
            false,
        ),
    ]
}

fn summary(number: u64, repository_key: &str, index: usize) -> PullRequestSummary {
    let titles = [
        TITLE,
        "Move the regional registries to the new signing keys and rotate every service principal that publishes container images",
        "Fix",
        "Speed up the composition lookup by caching (PackageId, DepotId) pairs across requests without leaking memory under load",
    ];
    PullRequestSummary {
        title: titles[index % titles.len()].to_owned(),
        author: person("a1", "Maximiliane Theodora von Hohenzollern-Sigmaringen"),
        is_draft: index % 5 == 3,
        source_branch: SOURCE_BRANCH.to_owned(),
        target_branch: TARGET_BRANCH.to_owned(),
        created_at: at(2, 9),
        updated_at: Some(at(17, 15)),
        web_url: format!(
            "https://dev.azure.com/{ORGANIZATION}/_git/repository/pullrequest/{number}"
        ),
        labels: labels([14, 3, 0, 8][index % 4]),
        reviewers: reviewers(),
        checks: Some(
            [RunStatus::Failed, RunStatus::Running, RunStatus::Succeeded, RunStatus::Queued]
                [index % 4],
        ),
        ..samples::summary(repository_key, number)
    }
}

/// Markdown with every block the description renders: headings, lists, a table, code, a quote and
/// a link far longer than any column.
const DESCRIPTION: &str = "## Description\n\
Every successful charge now issues an invoice. Composition is keyed by `(PackageId, DepotId)` end to end, so retries and compensation never pick another invoice.\n\n\
### Changes\n\
- Fewer allocations in `OrderFabrikamShipmentHandlerWithRetriesAndCompensation.ExecuteAsync`\n\
- [x] Migration guide written\n\
- [ ] Load test in eu-west-1\n\n\
| Area | Before | After |\n| --- | --- | --- |\n| Invoices per charge | 0 or 1 | exactly 1 |\n| Lookup | 120 ms | 4 ms |\n\n\
```csharp\nvar compositionKey = new CompositionKey(packageId: order.PackageId, depotId: order.DepotId, region: order.Region, attempt: attempt); // keyed end to end\n```\n\n\
> Rollback: revert this pull request and run the compensation job for every order since the deployment.\n\n\
See https://dev.azure.com/Fabrikam-Industries-Enterprise-Platform/Fabrikam%20Logistics%20Platform/_workitems/edit/48213?view=history&fullScreen=true for the full history.";

fn comment(id: &str, author: &str, body: &str, day: u8) -> ThreadComment {
    ThreadComment {
        id: id.to_owned(),
        author: person(id, author),
        body: body.to_owned(),
        created_at: at(day, 10),
        updated_at: None,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "fixture data: one thread per kind of row the audit draws"
)]
fn threads(fixture: &Fixture) -> Vec<ReviewThread> {
    let anchor = |line: u32| ThreadAnchor {
        path: LONG_PATH.to_owned(),
        old_path: None,
        old_line: None,
        new_line: Some(line),
        range: None,
    };
    let thread = |id: &str,
                  anchor: Option<ThreadAnchor>,
                  status: ThreadStatus,
                  comments: Vec<ThreadComment>| ReviewThread {
        id: id.to_owned(),
        status,
        anchor,
        is_outdated: false,
        origin: None,
        is_system: false,
        can_reply: true,
        can_resolve: true,
        comments,
    };
    let long = "Should the compensation also run when the invoice service answers with 409 Conflict? Today we retry three times, then give up silently, which leaves the order without an invoice and nobody notices until month end. A dead-letter queue with an alert would make this visible: https://learn.microsoft.com/en-us/azure/service-bus-messaging/service-bus-dead-letter-queues";
    let outdated = ReviewThread {
        is_outdated: true,
        origin: Some(ThreadOrigin {
            base_sha: Some(fixture.target.clone()),
            head_sha: fixture.first.clone(),
            anchor: anchor(41),
            diff_hunk: None,
            web_url: Some(format!(
                "https://dev.azure.com/{ORGANIZATION}/_git/repository/pullrequest/{NUMBER}?discussionId=9"
            )),
        }),
        ..thread(
            "9",
            Some(ThreadAnchor { new_line: None, ..anchor(41) }),
            ThreadStatus::Active,
            vec![comment(
                "c9",
                "Anastasia Konstantinopoulou",
                "Is 41 the right attempt count here, or should it come from the retry policy settings?",
                4,
            )],
        )
    };
    vec![
        thread(
            "1",
            Some(anchor(5)),
            ThreadStatus::Active,
            vec![
                comment("c1", "Bartholomew Fitzgerald-Okonkwo", long, 3),
                comment(
                    "c2",
                    "Maximiliane Theodora von Hohenzollern-Sigmaringen",
                    "Good point. I added a dead-letter queue and an alert on its length.",
                    4,
                ),
            ],
        ),
        thread(
            "2",
            Some(anchor(58)),
            ThreadStatus::Resolved,
            vec![comment(
                "c3",
                "Jean-Baptiste Emmanuel Zorg",
                "Nit: `compositionKey58` is never read.",
                5,
            )],
        ),
        outdated,
        thread(
            "3",
            None,
            ThreadStatus::Active,
            vec![comment(
                "c4",
                "Wolfeschlegelsteinhausenbergerdorff Hubert",
                "Deployed to the staging slot fabrikam-logistics-orchestrator-staging-eu-west-1-blue; invoices look right.",
                6,
            )],
        ),
        ReviewThread {
            is_system: true,
            can_reply: false,
            can_resolve: false,
            ..thread(
                "4",
                None,
                ThreadStatus::Active,
                vec![comment(
                    "c5",
                    "Maximiliane Theodora von Hohenzollern-Sigmaringen",
                    "Maximiliane Theodora von Hohenzollern-Sigmaringen updated the pull request status to Active and added [Fabrikam-Industries-Enterprise-Platform]\\Fabrikam Logistics Platform Reviewers (Backend Services) as a required reviewer",
                    2,
                )],
            )
        },
    ]
}

fn job(id: &str, name: &str, status: RunStatus, minutes: (u8, u8), log: bool) -> PipelineJob {
    let started = (status != RunStatus::Queued)
        .then(|| at(17, 9) + time::Duration::minutes(i64::from(minutes.0)));
    let finished = status
        .is_finished()
        .then(|| at(17, 9) + time::Duration::minutes(i64::from(minutes.1)))
        .filter(|_| started.is_some());
    PipelineJob {
        id: id.to_owned(),
        name: name.to_owned(),
        status,
        started_at: started,
        finished_at: finished,
        log_id: log.then(|| format!("log-{id}")),
        web_url: Some(format!(
            "https://dev.azure.com/{ORGANIZATION}/_build/results?buildId=20261019&view=logs&j={id}"
        )),
        reruns: if status == RunStatus::Failed {
            vec![RerunAction::new(RerunKind::Job, id)]
        } else {
            Vec::new()
        },
        attempt: 1,
        ..PipelineJob::default()
    }
}

fn stage(id: &str, name: &str, after: &[&str], children: Vec<PipelineJob>) -> PipelineJob {
    let status = RunStatus::combined(children.iter().map(|job| job.status));
    let reruns = if status == RunStatus::Failed {
        vec![
            RerunAction::new(RerunKind::StageFailed, id),
            RerunAction::new(RerunKind::StageAll, id),
        ]
    } else {
        Vec::new()
    };
    PipelineJob {
        id: id.to_owned(),
        name: name.to_owned(),
        status,
        started_at: children.iter().filter_map(|job| job.started_at).min(),
        finished_at: children.iter().filter_map(|job| job.finished_at).max(),
        depends_on: after.iter().map(|before| (*before).to_owned()).collect(),
        children,
        reruns,
        attempt: 1,
        ..PipelineJob::default()
    }
}

fn stages() -> Vec<PipelineJob> {
    use RunStatus::*;
    let retried = PipelineJob {
        attempt: 3,
        previous_attempts: vec![
            PipelineAttempt {
                number: 1,
                source: review_core::AttemptSource::Lazy("attempt-1".to_owned()),
            },
            PipelineAttempt {
                number: 2,
                source: review_core::AttemptSource::Lazy("attempt-2".to_owned()),
            },
        ],
        ..job("retried", RETRIED_JOB, Failed, (20, 104), true)
    };
    vec![
        stage(
            "build",
            "Build and unit test all backend services (Release, x64, .NET 10, analyzers as errors)",
            &[],
            vec![
                job(
                    "restore",
                    "Restore NuGet packages from the internal feed (fabrikam-logistics-packages)",
                    Succeeded,
                    (0, 3),
                    true,
                ),
                job(
                    "compile",
                    "Compile the solution with analyzers as errors",
                    Succeeded,
                    (3, 14),
                    true,
                ),
                job("unit", "Unit tests", Succeeded, (14, 19), true),
            ],
        ),
        stage(
            "integration",
            "Integration tests against the staging database cluster (eu-west-1)",
            &["build"],
            vec![
                retried,
                job(
                    "contract",
                    "Consumer-driven contract tests for the invoicing API",
                    Succeeded,
                    (20, 31),
                    true,
                ),
                job("smoke", "Smoke", Succeeded, (20, 22), true),
            ],
        ),
        stage(
            "publish",
            "Publish signed container images to the regional registries",
            &["integration"],
            vec![
                job("waiting", WAITING_JOB, Queued, (0, 0), false),
                job("sbom", "Generate the software bill of materials", Skipped, (0, 0), false),
            ],
        ),
    ]
}

fn runs() -> Vec<PipelineRun> {
    let run =
        |id: &str, name: &str, status: RunStatus, description: &str, has_jobs: bool| PipelineRun {
            id: id.to_owned(),
            name: name.to_owned(),
            status,
            description: Some(description.to_owned()),
            web_url: Some(format!(
                "https://dev.azure.com/{ORGANIZATION}/_build/results?buildId={id}"
            )),
            started_at: Some(at(17, 9)),
            finished_at: status.is_finished().then(|| at(17, 11)),
            has_jobs,
            is_head: true,
            reruns: if status == RunStatus::Failed && has_jobs {
                vec![RerunAction::new(RerunKind::RunFailed, id)]
            } else {
                Vec::new()
            },
        };
    vec![
        run(
            "20261019",
            RUN,
            RunStatus::Failed,
            &format!("Triggered by {SOURCE_BRANCH} for refs/pull/{NUMBER}/merge"),
            true,
        ),
        run(
            "status-1",
            "Security scan: container images and third-party dependencies (Defender for DevOps)",
            RunStatus::Succeeded,
            "No vulnerabilities above the configured threshold in 1,284 packages",
            false,
        ),
        run(
            "status-2",
            "Code coverage",
            RunStatus::Neutral,
            "Coverage 81.4 % (-0.2 %) across 3,000 changed files and 100,000 changed lines",
            false,
        ),
    ]
}

fn log_text() -> String {
    let mut text = String::new();
    for index in 0..2_000 {
        match index % 40 {
            0 => writeln!(text, "##[section]Starting: {RETRIED_JOB} step {index}").unwrap(),
            7 => writeln!(text, "\u{1b}[31mFAILED\u{1b}[0m FabrikamLogistics.Orchestrator.IntegrationTests.InvoiceIssuedForEveryChargeWhenCompositionIsKeyedByPackageAndDepot(attempt: {index}) Expected exactly 1 invoice for order 7f3c2a9e-1b4d-4e8f-9a6c-2d5e8f1b3c7a in region eu-west-1 but found 0 after 3 retries; compensation did not run because the dead-letter queue fabrikam-logistics-invoices-dlq was not configured on the staging namespace").unwrap(),
            _ => writeln!(text, "2026-10-18T09:{:02}:{:02}.1234567Z \u{1b}[32mPASS\u{1b}[0m test {index}", index / 60 % 60, index % 60).unwrap(),
        }
    }
    text
}

static CAPABILITIES: LazyLock<Capabilities> = LazyLock::new(|| Capabilities {
    votes: vec![
        ReviewVote::Approved,
        ReviewVote::ApprovedWithSuggestions,
        ReviewVote::WaitingForAuthor,
        ReviewVote::ChangesRequested,
        ReviewVote::None,
    ],
    merge_strategies: MergeStrategy::ALL.to_vec(),
    has_label_catalog: false,
    running_logs: review_core::RunningLogs::Growing,
});

#[async_trait]
impl Script for ClipAuditScript {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        Ok(person("a1", "Maximiliane Theodora von Hohenzollern-Sigmaringen"))
    }

    async fn git_remote(&self, repo: &str) -> Result<GitRemote> {
        if repo == SLOW_REPOSITORY {
            std::future::pending::<()>().await;
        }
        Ok(GitRemote { clone_url: self.fixture.clone_url.clone(), authorization_header: None })
    }

    async fn list_pull_requests(
        &self,
        query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        match self.connection_id.as_str() {
            SLOW => std::future::pending().await,
            BROKEN => Err(ProviderError::Network(format!(
                "error sending request for url (https://dev.azure.com/{ORGANIZATION}/_apis/git/pullrequests?searchCriteria.status=active&api-version=7.1): certificate has expired"
            ))),
            EMPTY | ANONYMOUS => Ok(Vec::new()),
            _ => {
                let repository = query.repository_key.as_deref().unwrap_or(REPOSITORY);
                Ok((0..24)
                    .map(|index| summary(NUMBER - index as u64 * 37, repository, index))
                    .collect())
            }
        }
    }

    async fn pull_request(&self, repo: &str, number: u64) -> Result<PullRequestDetail> {
        let mut summary = summary(number, repo, 0);
        summary.labels = labels(LABELS.len());
        Ok(PullRequestDetail {
            summary,
            description: DESCRIPTION.to_owned(),
            target_sha: self.fixture.target.clone(),
            head_sha: self.fixture.head.clone(),
            git_fetch_refs: vec!["refs/heads/feature".to_owned(), "refs/heads/main".to_owned()],
            ..samples::detail(repo, number)
        })
    }

    async fn merge_policy(&self, _repo: &str, _target_branch: &str) -> Result<MergePolicy> {
        let allowed = vec![MergeStrategy::Merge, MergeStrategy::Squash, MergeStrategy::Rebase];
        Ok(MergePolicy::reported(allowed, Some(MergeStrategy::Squash)).with_settings_url(Some(
            format!("https://dev.azure.com/{ORGANIZATION}/_settings/repositories"),
        )))
    }

    async fn threads(&self, _repo: &str, _number: u64) -> Result<Vec<ReviewThread>> {
        Ok(threads(&self.fixture))
    }

    async fn linked_references(&self, repo: &str, _number: u64) -> Result<Vec<ItemRef>> {
        Ok(vec![
            ItemRef {
                kind: ItemKind::WorkItem,
                repository_key: ORGANIZATION.to_owned(),
                number: 48_213,
            },
            ItemRef {
                kind: ItemKind::PullRequest,
                repository_key: repo.to_owned(),
                number: 74_001,
            },
        ])
    }

    async fn linked_items(&self, references: &[ItemRef]) -> Result<Vec<LinkedItem>> {
        Ok(references
            .iter()
            .map(|reference| LinkedItem {
                reference: reference.clone(),
                kind: reference.kind,
                repository_key: reference.repository_key.clone(),
                title: "Customers are charged but receive no invoice when the composition lookup races the compensation job in eu-west-1".to_owned(),
                state: LinkedState::Open,
                state_label: "Active".to_owned(),
                checks: Some(RunStatus::Failed),
                item_type: (reference.kind == ItemKind::WorkItem).then(|| "Customer Escalation Bug".to_owned()),
                web_url: format!("https://dev.azure.com/{ORGANIZATION}/_workitems/edit/{}", reference.number),
            })
            .collect())
    }

    async fn available_labels(&self, _repo: &str) -> Result<Vec<Label>> {
        Ok(labels(LABELS.len()))
    }

    async fn pipeline_runs(&self, _repo: &str, _number: u64) -> Result<Vec<PipelineRun>> {
        Ok(runs())
    }

    async fn pipeline_jobs(
        &self,
        _repo: &str,
        _number: u64,
        run_id: &str,
    ) -> Result<Vec<PipelineJob>> {
        Ok(if run_id == "20261019" { stages() } else { Vec::new() })
    }

    async fn pipeline_attempt(
        &self,
        _repo: &str,
        _number: u64,
        _run_id: &str,
        target: &str,
    ) -> Result<PipelineJob> {
        let number = if target == "attempt-1" { 1 } else { 2 };
        Ok(PipelineJob {
            attempt: number,
            ..job(target, RETRIED_JOB, RunStatus::Failed, (20, 50), true)
        })
    }

    async fn download_log(
        &self,
        _repo: &str,
        _number: u64,
        _log_id: &str,
        sink: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<()> {
        sink.write_all(log_text().as_bytes())
            .await
            .map_err(|error| ProviderError::Local(error.to_string()))
    }

    async fn rerun(
        &self,
        _repo: &str,
        _number: u64,
        _run_id: &str,
        _action: &RerunAction,
    ) -> Result<()> {
        Ok(())
    }
}
