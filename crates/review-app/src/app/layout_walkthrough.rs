//! Walks the pane splitters, the contextual zoom and middle-click autoscroll on a real pull
//! request page, offline: a local git repository is the remote and a scripted provider describes
//! the pull request. No network, no tokens. Runs with `cargo test`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use eframe::App as _;
use egui::{
    Event, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, Role, TouchPhase, vec2,
};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{
    Capabilities, Connection, GitRemote, Label, MergeOptions, MergeStatus, MergeStrategy,
    NewThread, PipelineJob, PipelineRun, Provider, ProviderError, ProviderKind, PullRequestDetail,
    PullRequestQuery, PullRequestState, PullRequestSummary, RepositoryInfo, Result, ReviewThread,
    ReviewVote, ThreadComment, ThreadStatus, UserIdentity,
};
use review_store::{AppPaths, AppSettings, ConnectionStore, DiffLayout, PaneSizes, TreeSide};
use tokio::io::AsyncWrite;

use super::ReviewApp;
use super::clicks::Clicks;
use cv_runtime::route::{PullRequestKey, Route};
use cv_testkit::{self as test_support, ScratchDir, git};
use cv_widgets::skeleton;

const CONNECTION_ID: &str = "local-layout";
const REPOSITORY: &str = "acme/widgets";
const NUMBER: u64 = 7;
const CODE_PATH: &str = "src/widgets.rs";
const GADGETS_PATH: &str = "src/gadgets.rs";
const LOGO_PATH: &str = "assets/logo.png";

/// The local remote per connection id; the provider factory is a plain function.
static FIXTURES: Mutex<Option<HashMap<String, Fixture>>> = Mutex::new(None);

#[derive(Clone)]
struct Fixture {
    clone_url: String,
    target: String,
    head: String,
}

#[test]
#[expect(clippy::too_many_lines, reason = "one walkthrough scenario, told step by step")]
fn layout_splitters_zoom_and_autoscroll_on_a_pull_request() {
    let scratch = ScratchDir::new("layout");
    let fixture = create_origin(&scratch.path().join("origin"));
    FIXTURES
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(CONNECTION_ID.to_owned(), fixture);
    let paths = AppPaths::new(scratch.path().join("config"), scratch.path().join("cache"));
    save_connection(&paths, CONNECTION_ID);
    let mut harness = harness(paths.clone());

    harness.state_mut().navigator.go(Route::PullRequest(PullRequestKey {
        connection_id: CONNECTION_ID.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number: NUMBER,
    }));
    wait_for(&mut harness, "the overview", |harness| {
        harness.query_by_label("Resize side column").is_some()
    });
    wait_for(&mut harness, "the file count", |harness| {
        harness.query_by_label_contains("Files (").is_some()
    });

    // Overview: the side column grows when its boundary moves left, and the width is saved.
    let handle = center(&harness, "Resize side column");
    drag(&mut harness, handle, vec2(-120.0, 0.0));
    let side = harness.state().services.settings.panes.overview_side;
    assert!(
        (side - (PaneSizes::DEFAULT.overview_side + 120.0)).abs() < 1.0,
        "side column is {side}"
    );
    assert_eq!(saved(&paths).panes.overview_side, side, "persisted on release");
    let handle = center(&harness, "Resize side column");
    double_click(&mut harness, handle);
    assert_eq!(
        harness.state().services.settings.panes.overview_side,
        PaneSizes::DEFAULT.overview_side
    );
    assert_eq!(
        saved(&paths).panes.overview_side,
        PaneSizes::DEFAULT.overview_side,
        "the reset is saved too"
    );

    // Ctrl+wheel over the breadcrumb bar zooms the app, not the code.
    ctrl_wheel(&mut harness, Pos2::new(700.0, 22.0), 1.0);
    let settings = &harness.state().services.settings;
    assert_eq!(
        (settings.ui_scale, settings.code_font_size),
        (1.1, AppSettings::DEFAULT_CODE_FONT_SIZE)
    );
    assert!(harness.query_by_label("Zoom 110 %").is_some(), "the overlay names the level");
    assert_eq!(saved(&paths).ui_scale, 1.1);
    harness.click_at(harness.get_by_label("Reset").rect());
    settle(&mut harness);
    assert_eq!(harness.state().services.settings.ui_scale, 1.0);
    assert_eq!(
        harness.query_all_by_label("Description").count(),
        1,
        "the card title is not repeated"
    );

    // Commits: SHA and time sit in fixed columns. The tab shows its count once the commits loaded.
    wait_for(&mut harness, "the commit count", |harness| {
        harness.query_by_label_contains("Commits (").is_some()
    });
    harness.click_at(harness.get_by_label_contains("Commits (").rect());
    wait_for(&mut harness, "the commit row", |harness| {
        harness.query_by_role_and_label(Role::Link, "feature").is_some()
    });

    // Files tab with the long diff.
    harness.click_at(harness.get_by_label_contains("Files (").rect());
    wait_for(&mut harness, "the file tree", |harness| {
        harness.query_by_label("Resize file tree").is_some()
    });
    let filter =
        harness.get_by(|node| node.placeholder().is_some_and(|text| text.contains("Filter files")));
    harness.click_at(filter.rect());
    filter.type_text("widgets.rs");
    wait_for(&mut harness, "the filtered file", |harness| {
        harness.query_by_role_and_label(Role::TreeItem, CODE_PATH).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, CODE_PATH).rect());
    wait_for(&mut harness, "the diff", |harness| {
        harness.query_by_label(&format!("Diff of {CODE_PATH}")).is_some()
            && harness.query_by_label("Resize diff sides").is_some()
    });

    // The tree sits at its side of the diff, its splitter on its inner edge; `inward` points from
    // the tree towards the diff.
    let tree = center(&harness, "Resize file tree");
    let diff_rect = harness.get_by_label(&format!("Diff of {CODE_PATH}")).rect();
    let search = harness.get_by_role(Role::TextInput).rect();
    let inward = match harness.state().services.settings.file_tree_side {
        TreeSide::Left => 1.0,
        TreeSide::Right => -1.0,
    };
    if inward > 0.0 {
        assert!(diff_rect.left() >= tree.x - 1.0, "the diff {diff_rect:?} starts at the tree");
        assert!(search.right() < tree.x, "the tree's search {search:?} is left of the splitter");
    } else {
        assert!(diff_rect.right() <= tree.x + 1.0, "the diff {diff_rect:?} ends at the tree");
        assert!(search.left() > tree.x, "the tree's search {search:?} is right of the splitter");
    }

    // The tree grows when its splitter moves towards the diff.
    drag(&mut harness, tree, vec2(inward * 100.0, 0.0));
    let tree_width = harness.state().services.settings.panes.file_tree;
    assert!(
        (tree_width - (PaneSizes::DEFAULT.file_tree + 100.0)).abs() < 1.0,
        "file tree is {tree_width}"
    );
    assert_eq!(saved(&paths).panes.file_tree, tree_width);

    // The tree can be dragged almost across the whole window and back: the handle stays reachable.
    let window_width = harness.ctx.content_rect().width();
    let tree = center(&harness, "Resize file tree");
    drag(&mut harness, tree, vec2(inward * window_width * 2.0, 0.0));
    let widest = harness.state().services.settings.panes.file_tree;
    assert!(widest > window_width * 0.85, "file tree reaches {widest} of {window_width}");
    let far = center(&harness, "Resize file tree");
    assert!(
        far.x > 40.0 && far.x < window_width - 40.0,
        "the handle at {} stays inside the window",
        far.x
    );
    drag(&mut harness, far, vec2(tree.x - far.x, 0.0));
    let restored = harness.state().services.settings.panes.file_tree;
    assert!(
        (restored - tree_width).abs() < 1.0,
        "dragged back to {restored}, expected {tree_width}"
    );
    assert_eq!(saved(&paths).panes.file_tree, restored);

    let split = center(&harness, "Resize diff sides");
    drag(&mut harness, split, vec2(-160.0, 0.0));
    let diff_split = harness.state().services.settings.panes.diff_split;
    assert!(diff_split < 0.45, "old side share is {diff_split}");
    assert_eq!(saved(&paths).panes.diff_split, diff_split);

    // Ctrl+wheel over the diff changes the code size only.
    let diff = harness.get_by_label(&format!("Diff of {CODE_PATH}")).rect().center();
    ctrl_wheel(&mut harness, diff, 2.0);
    let settings = &harness.state().services.settings;
    assert_eq!(
        (settings.code_font_size, settings.ui_scale),
        (AppSettings::DEFAULT_CODE_FONT_SIZE + 2.0, 1.0)
    );
    assert!(harness.query_by_label("Code 15 px").is_some());
    assert_eq!(saved(&paths).code_font_size, 15.0);

    // Middle click in the diff, pointer below the origin: the diff scrolls down.
    let origin = diff + vec2(-200.0, 0.0);
    hooked_frame(&mut harness, vec![Event::PointerMoved(origin)]);
    hooked_frame(&mut harness, vec![middle(origin, true)]);
    hooked_frame(&mut harness, vec![middle(origin, false)]);
    hooked_frame(&mut harness, vec![Event::PointerMoved(origin + vec2(0.0, 140.0))]);
    for _ in 0..30 {
        hooked_frame(&mut harness, Vec::new());
    }
    assert!(harness.state().autoscroll.is_active());
    let escape = Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    hooked_frame(&mut harness, vec![escape]);
    assert!(!harness.state().autoscroll.is_active());

    // The Markdown preview: Before and After get their own boundary.
    // The filter still holds the first search, so its placeholder is gone; replace the text.
    harness.get_by_role(Role::TextInput).focus();
    harness.step();
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    harness.step();
    harness.get_by_role(Role::TextInput).type_text("README");
    wait_for(&mut harness, "the README", |harness| {
        harness.query_by_role_and_label(Role::TreeItem, "README.md").is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, "README.md").rect());
    wait_for(&mut harness, "the preview", |harness| {
        harness.query_by_label("Resize Before and After").is_some()
    });
    settle(&mut harness);
    let handle = center(&harness, "Resize Before and After");
    drag(&mut harness, handle, vec2(120.0, 0.0));
    assert!(harness.state().services.settings.panes.preview_split > 0.55);

    // The rendered diff takes the header's diff controls like the text diff: arrows, layout, Alt+N.
    let next = harness.get_by_label("Next change (N or F7)");
    assert!(!next.accesskit_node().is_disabled(), "the change arrows work on the preview");
    harness.click_at(harness.get_by_label("New only").rect());
    settle(&mut harness);
    assert_eq!(harness.state().services.settings.diff_layout, DiffLayout::NewOnly);
    assert!(
        harness.query_by_label("Resize Before and After").is_none(),
        "New only shows one column"
    );
    harness.key_press_modifiers(Modifiers::ALT, egui::Key::N);
    settle(&mut harness);
    assert_eq!(harness.state().services.settings.diff_layout, DiffLayout::SideBySide);
    assert!(
        harness.query_by_label("Resize Before and After").is_some(),
        "Alt+N returns to the columns"
    );
}

/// The folder view zooms like the file view: Ctrl+wheel over a card's code changes the code size
/// and leaves the app's scale (`pixels_per_point`) alone; over the folder view's header, which is
/// no code, it zooms the app.
#[test]
fn folder_view_zooms_the_code_over_cards_and_the_app_elsewhere() {
    const FOLDER_ID: &str = "local-folder-zoom";
    let scratch = ScratchDir::new("folder-zoom");
    let fixture = create_origin(&scratch.path().join("origin"));
    FIXTURES.lock().unwrap().get_or_insert_with(HashMap::new).insert(FOLDER_ID.to_owned(), fixture);
    let paths = AppPaths::new(scratch.path().join("config"), scratch.path().join("cache"));
    save_connection(&paths, FOLDER_ID);
    let mut harness = harness(paths.clone());
    harness.state_mut().navigator.go(Route::PullRequest(PullRequestKey {
        connection_id: FOLDER_ID.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number: NUMBER,
    }));
    wait_for(&mut harness, "the file count", |harness| {
        harness.query_by_label_contains("Files (").is_some()
    });
    harness.click_at(harness.get_by_label_contains("Files (").rect());
    let card = format!("Diff of {GADGETS_PATH}");
    wait_for(&mut harness, "the folder view's cards", |harness| {
        harness.query_by_label(&card).is_some()
    });
    let scale = harness.ctx.pixels_per_point();

    let code = harness.get_by_label(&card).rect().center();
    ctrl_wheel(&mut harness, code, 1.0);
    let settings = &harness.state().services.settings;
    assert_eq!(
        (settings.code_font_size, settings.ui_scale),
        (AppSettings::DEFAULT_CODE_FONT_SIZE + 1.0, 1.0),
        "the code zooms, the app does not"
    );
    assert_eq!(harness.ctx.pixels_per_point(), scale, "the app keeps its scale");
    assert!(harness.query_by_label("Code 14 px").is_some(), "the overlay names the code size");
    assert_eq!(saved(&paths).code_font_size, 14.0);

    let header = harness.get_by_label("4 changed files").rect().center();
    ctrl_wheel(&mut harness, header, 1.0);
    let settings = &harness.state().services.settings;
    assert_eq!(
        (settings.code_font_size, settings.ui_scale),
        (AppSettings::DEFAULT_CODE_FONT_SIZE + 1.0, 1.1),
        "outside the code the app zooms"
    );
    assert!(
        (harness.ctx.pixels_per_point() - scale * 1.1).abs() < 0.001,
        "the app's scale follows: {}",
        harness.ctx.pixels_per_point()
    );
}

/// A diff, a Markdown preview, an image and diffs again, opened one after another: the header
/// keeps every control in place from the first frame on, and a diff that is ready in time (cached,
/// or worked out ahead from the mirror) never flashes a placeholder.
#[test]
fn switching_files_keeps_the_header_still_without_placeholders() {
    const SWITCH_ID: &str = "local-switch";
    let scratch = ScratchDir::new("switch");
    let fixture = create_origin(&scratch.path().join("origin"));
    FIXTURES.lock().unwrap().get_or_insert_with(HashMap::new).insert(SWITCH_ID.to_owned(), fixture);
    let paths = AppPaths::new(scratch.path().join("config"), scratch.path().join("cache"));
    save_connection(&paths, SWITCH_ID);
    let mut harness = harness(paths);
    harness.state_mut().navigator.go(Route::PullRequest(PullRequestKey {
        connection_id: SWITCH_ID.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number: NUMBER,
    }));
    wait_for(&mut harness, "the file count", |harness| {
        harness.query_by_label_contains("Files (").is_some()
    });
    harness.click_at(harness.get_by_label_contains("Files (").rect());
    wait_for(&mut harness, "the tree", |harness| {
        harness.query_by_role_and_label(Role::TreeItem, CODE_PATH).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, CODE_PATH).rect());
    wait_for(&mut harness, "the diff", |harness| {
        harness.query_by_label(&format!("Diff of {CODE_PATH}")).is_some()
    });
    // Line counts, colors and the diffs of the neighbouring files.
    wait_for(&mut harness, "the work ahead", |harness| {
        harness.state().services.background.running() == 0
    });
    let reference = header_controls(&harness);
    assert!(reference.len() >= 8, "the header shows its controls: {reference:#?}");

    for path in ["README.md", LOGO_PATH, GADGETS_PATH, CODE_PATH] {
        // A pointer rests on the row before it clicks: past the dwell, its diff loads ahead
        // even in another folder, where the open file's neighbours do not reach.
        harness.get_by_role_and_label(Role::TreeItem, path).hover();
        harness.run_steps(12); // 200 ms at 60 frames per second: past the 150 ms dwell.
        wait_for(&mut harness, "the hovered file's diff", |harness| {
            harness.state().services.background.running() == 0
        });
        harness.click_at(harness.get_by_role_and_label(Role::TreeItem, path).rect());
        let clicked = harness.ctx.input(|input| input.time);
        for step in 0..40 {
            harness.step();
            // Wall time at least as long as the frames say, so a slow load really is late.
            test_support::let_threads_run(Duration::from_millis(16));
            let waited = harness.ctx.input(|input| input.time) - clicked;
            // The click's frame still draws the file it left, with that file's placeholder if its
            // own load is late; a placeholder counts for `path` once the header names it.
            let is_shown = header_names(&harness, path);
            assert!(is_shown || step == 0, "{path} is not open in frame {step}");
            let placeholder = ["Loading the diff", "Loading the preview"]
                .into_iter()
                .find(|label| is_shown && harness.query_by_label(label).is_some());
            if let Some(placeholder) = placeholder {
                assert!(
                    waited >= skeleton::GRACE_SECONDS,
                    "{path}: {placeholder:?} after {waited:.3} s, within the grace period"
                );
                assert!(
                    !is_rust(path),
                    "{path} was ready ahead, yet {placeholder:?} showed after {waited:.3} s"
                );
            }
            let found = header_controls(&harness);
            assert_eq!(
                rects(&found),
                rects(&reference),
                "{path}, frame {step}: the header moved: {found:#?}\nbefore: {reference:#?}"
            );
            // The diffs were opened before or worked out ahead: on screen as soon as the file is.
            if is_rust(path) && is_shown {
                assert!(
                    harness.query_by_label(&format!("Diff of {path}")).is_some(),
                    "{path} is not on screen in frame {step}"
                );
            }
        }
        // Moving on releases what the last file's plan loaded ahead, so the next file is ready
        // ahead only once this file's plan has finished (slow under a loaded test run).
        wait_for(&mut harness, "the work ahead", |harness| {
            harness.state().services.background.running() == 0
        });
    }
}

/// The file header names `path`: the file is the one on screen.
fn header_names(harness: &Harness<'_, ReviewApp>, path: &str) -> bool {
    harness
        .query_by(|node| {
            node.role() == Role::Label
                && node.label().or_else(|| node.value()).as_deref() == Some(path)
        })
        .is_some()
}

/// The header's controls (not its labels: the path, the line counts and the counter change from
/// file to file), found in the band of the Viewed toggle on the diff's side of the file tree.
fn header_controls(harness: &Harness<'_, ReviewApp>) -> Vec<(String, egui::Rect)> {
    let band =
        harness.get_by_role_and_label(Role::CheckBox, "Viewed").rect().expand2(vec2(0.0, 2.0));
    let splitter = harness.get_by_label("Resize file tree").rect();
    let beside_diff = |rect: egui::Rect| match harness.state().services.settings.file_tree_side {
        TreeSide::Left => rect.left() >= splitter.right(),
        TreeSide::Right => rect.right() <= splitter.left(),
    };
    harness
        .query_all_by(|node| {
            matches!(
                node.role(),
                Role::Button | Role::RadioButton | Role::Switch | Role::CheckBox | Role::ComboBox
            )
        })
        .filter(|node| {
            let rect = node.rect();
            beside_diff(rect) && rect.top() >= band.top() && rect.bottom() <= band.bottom()
        })
        .map(|node| (node.accesskit_node().label().unwrap_or_default(), node.rect()))
        .collect()
}

fn rects(controls: &[(String, egui::Rect)]) -> Vec<egui::Rect> {
    controls.iter().map(|(_, rect)| *rect).collect()
}

fn harness(paths: AppPaths) -> Harness<'static, ReviewApp> {
    let builder = Harness::builder().with_size([1440.0, 900.0]).with_step_dt(1.0 / 60.0);
    super::startup::start(builder, |creation| ReviewApp::new(creation, paths, local_provider))
}

fn saved(paths: &AppPaths) -> AppSettings {
    AppSettings::load(&paths.settings_file()).expect("settings file")
}

fn center(harness: &Harness<'_, ReviewApp>, label: &str) -> Pos2 {
    harness.get_by_label(label).rect().center()
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn middle(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Middle, pressed, modifiers: Modifiers::NONE }
}

/// Press at `from`, move by `by` in two steps, release: a pointer drag.
fn drag(harness: &mut Harness<'_, ReviewApp>, from: Pos2, by: egui::Vec2) {
    harness.hover_at(from);
    harness.step();
    harness.event(button(from, true));
    harness.step();
    harness.hover_at(from + by * 0.5);
    harness.step();
    harness.hover_at(from + by);
    harness.step();
    harness.event(button(from + by, false));
    harness.step();
    settle(harness);
}

fn double_click(harness: &mut Harness<'_, ReviewApp>, at: Pos2) {
    harness.hover_at(at);
    harness.step();
    for _ in 0..2 {
        harness.event(button(at, true));
        harness.event(button(at, false));
        harness.step();
    }
    settle(harness);
}

fn ctrl_wheel(harness: &mut Harness<'_, ReviewApp>, at: Pos2, notches: f32) {
    harness.hover_at(at);
    harness.step();
    harness.event(Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: vec2(0.0, notches),
        phase: TouchPhase::Move,
        modifiers: Modifiers::COMMAND,
        source: MouseWheelSource::Wheel,
    });
    harness.step();
    settle(harness);
}

/// One frame whose raw input first runs through the app's `raw_input_hook`, as eframe does.
fn hooked_frame(harness: &mut Harness<'_, ReviewApp>, events: Vec<Event>) {
    let ctx = harness.ctx.clone();
    let mut input = std::mem::take(harness.input_mut());
    input.events.extend(events);
    harness.state_mut().raw_input_hook(&ctx, &mut input);
    *harness.input_mut() = input;
    harness.step();
}

fn wait_for(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    let started = Instant::now();
    super::action_e2e::wait_for(harness, what, |harness| is_done(harness).then_some(()), |_| {});
    println!("{what} after {:.1} s", started.elapsed().as_secs_f32());
    settle(harness);
}

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    for _ in 0..20 {
        harness.step();
    }
}

/// A repository with a long Rust file changed every few lines, a small one, a changed README and a
/// recolored image.
fn create_origin(origin: &Path) -> Fixture {
    std::fs::create_dir_all(origin.join("src")).expect("origin folder");
    std::fs::create_dir_all(origin.join("assets")).expect("assets folder");
    let gadgets = "pub fn gadget() -> u32 {\n    1\n}\n";
    std::fs::write(origin.join(GADGETS_PATH), gadgets).expect("write gadgets");
    image::RgbaImage::from_pixel(24, 24, image::Rgba([40, 90, 200, 255]))
        .save(origin.join(LOGO_PATH))
        .expect("write logo");
    git(origin, &["init", "-q", "-b", "main"]);
    git(origin, &["config", "uploadpack.allowFilter", "true"]);
    git(origin, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
    let line = |index: usize, changed: bool| {
        let value = if changed { index * 3 } else { index };
        format!(
            "    let widget_{index} = Widget::new(\"widget {index}\", {value}); // keeps the layout stable\n"
        )
    };
    let original: String = (0..900).map(|index| line(index, false)).collect();
    let changed: String = (0..900).map(|index| line(index, index % 12 == 5)).collect();
    std::fs::write(origin.join(CODE_PATH), format!("fn build() {{\n{original}}}\n"))
        .expect("write code");
    let readme = "# Widgets\n\nA small library of widgets.\n\n## Usage\n\nCall `build()` once at start-up.\n";
    std::fs::write(origin.join("README.md"), readme).expect("write readme");
    git(origin, &["add", "-A"]);
    git(origin, &["commit", "-q", "-m", "base"]);
    let target = git(origin, &["rev-parse", "HEAD"]);

    git(origin, &["switch", "-q", "-c", "feature"]);
    std::fs::write(origin.join(CODE_PATH), format!("fn build() {{\n{changed}}}\n"))
        .expect("write code");
    let readme = readme
        .replace("A small library", "A small, fast library")
        .replace("once at start-up", "once at start-up, then `run()`");
    std::fs::write(origin.join("README.md"), format!("{readme}\n## License\n\nMIT\n"))
        .expect("write readme");
    std::fs::write(origin.join(GADGETS_PATH), gadgets.replace("    1\n", "    2\n"))
        .expect("write gadgets");
    image::RgbaImage::from_pixel(24, 24, image::Rgba([220, 120, 30, 255]))
        .save(origin.join(LOGO_PATH))
        .expect("write logo");
    git(origin, &["add", "-A"]);
    git(origin, &["commit", "-q", "-m", "feature"]);
    let head = git(origin, &["rev-parse", "HEAD"]);
    Fixture {
        clone_url: format!("file:///{}", origin.display().to_string().replace('\\', "/")),
        target,
        head,
    }
}

fn save_connection(paths: &AppPaths, id: &str) {
    let mut store = ConnectionStore::open(paths).expect("empty store");
    store
        .upsert(Connection {
            id: id.to_owned(),
            name: "Local".to_owned(),
            kind: ProviderKind::GitHub,
            base_url: ProviderKind::GitHub.default_base_url().to_owned(),
            organizations: Vec::new(),
            token: None,
        })
        .expect("store the connection");
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "a ProviderFactory, whose signature hands the connection and the client over"
)]
fn local_provider(connection: Connection, _http: reqwest::Client) -> Arc<dyn Provider> {
    let fixture = FIXTURES
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|fixtures| fixtures.get(&connection.id).cloned());
    Arc::new(LocalProvider { fixture })
}

struct LocalProvider {
    fixture: Option<Fixture>,
}

impl LocalProvider {
    fn fixture(&self) -> Result<&Fixture> {
        self.fixture.as_ref().ok_or_else(|| ProviderError::Local("no local fixture".to_owned()))
    }

    fn unused<T>() -> Result<T> {
        Err(ProviderError::Unsupported("Not part of the layout walkthrough.".to_owned()))
    }
}

fn author() -> UserIdentity {
    UserIdentity {
        id: "1".into(),
        display_name: "Ada Lovelace".into(),
        login: Some("ada".into()),
        avatar_url: None,
    }
}

static CAPABILITIES: std::sync::LazyLock<Capabilities> =
    std::sync::LazyLock::new(|| Capabilities {
        votes: vec![ReviewVote::Approved, ReviewVote::None],
        merge_strategies: vec![MergeStrategy::Merge],
        has_label_catalog: false,
        running_logs: review_core::RunningLogs::Growing,
    });

#[async_trait]
impl Provider for LocalProvider {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    async fn current_user(&self) -> Result<UserIdentity> {
        Ok(author())
    }

    async fn list_repositories(&self, _search: Option<&str>) -> Result<Vec<RepositoryInfo>> {
        Ok(Vec::new())
    }

    async fn git_remote(&self, _repo: &str) -> Result<GitRemote> {
        Ok(GitRemote { clone_url: self.fixture()?.clone_url.clone(), authorization_header: None })
    }

    async fn list_pull_requests(
        &self,
        _query: &PullRequestQuery,
    ) -> Result<Vec<PullRequestSummary>> {
        Ok(Vec::new())
    }

    async fn pull_request(&self, repo: &str, number: u64) -> Result<PullRequestDetail> {
        let fixture = self.fixture()?;
        let created_at = time::macros::datetime!(2026-10-01 09:30 UTC);
        Ok(PullRequestDetail {
            summary: PullRequestSummary {
                repository_key: repo.to_owned(),
                repository_name: "widgets".to_owned(),
                number,
                title: "Speed up widget construction".to_owned(),
                author: author(),
                state: PullRequestState::Open,
                is_draft: false,
                source_branch: "feature".to_owned(),
                target_branch: "main".to_owned(),
                created_at,
                updated_at: None,
                web_url: format!("https://github.com/{repo}/pull/{number}"),
                labels: ["performance", "widgets", "needs-review", "good first issue", "backend"]
                    .into_iter()
                    .map(|name| Label { name: name.to_owned(), color: Some("0e8a16".to_owned()), description: None })
                    .collect(),
                reviewers: Vec::new(),
                checks: None,
                diff_stat: review_core::DiffStat::default(),
                head_sha: None,
                target_sha: None,
            },
            // Starts with the heading most PR templates use; the card must not repeat it.
            description: "## Description\nBuilds every widget with its final value right away.\n\n- Fewer allocations\n- Same layout"
                .to_owned(),
            target_sha: fixture.target.clone(),
            head_sha: fixture.head.clone(),
            merge_status: MergeStatus::Mergeable,
            git_fetch_refs: vec!["refs/heads/feature".to_owned(), "refs/heads/main".to_owned()],
            closed_at: None,
            auto_complete: None,
        })
    }

    async fn merge(&self, _repo: &str, _number: u64, _options: &MergeOptions) -> Result<()> {
        Self::unused()
    }

    async fn close(&self, _repo: &str, _number: u64) -> Result<()> {
        Self::unused()
    }

    async fn reopen(&self, _repo: &str, _number: u64) -> Result<()> {
        Self::unused()
    }

    async fn set_draft(&self, _repo: &str, _number: u64, _is_draft: bool) -> Result<()> {
        Self::unused()
    }

    async fn update_title_description(
        &self,
        _repo: &str,
        _number: u64,
        _title: &str,
        _description: &str,
    ) -> Result<()> {
        Self::unused()
    }

    async fn set_vote(
        &self,
        _repo: &str,
        _number: u64,
        _vote: ReviewVote,
        _comment: Option<&str>,
    ) -> Result<()> {
        Self::unused()
    }

    async fn add_reviewer(&self, _repo: &str, _number: u64, _user: &str) -> Result<()> {
        Self::unused()
    }

    async fn remove_reviewer(&self, _repo: &str, _number: u64, _reviewer_id: &str) -> Result<()> {
        Self::unused()
    }

    async fn threads(&self, _repo: &str, _number: u64) -> Result<Vec<ReviewThread>> {
        Ok(Vec::new())
    }

    async fn create_thread(
        &self,
        _repo: &str,
        _number: u64,
        _thread: &NewThread,
    ) -> Result<ReviewThread> {
        Self::unused()
    }

    async fn reply(
        &self,
        _repo: &str,
        _number: u64,
        _thread_id: &str,
        _body: &str,
    ) -> Result<ThreadComment> {
        Self::unused()
    }

    async fn set_thread_status(
        &self,
        _repo: &str,
        _number: u64,
        _thread_id: &str,
        _status: ThreadStatus,
    ) -> Result<()> {
        Self::unused()
    }

    async fn available_labels(&self, _repo: &str) -> Result<Vec<Label>> {
        Ok(Vec::new())
    }

    async fn set_labels(&self, _repo: &str, _number: u64, _labels: &[String]) -> Result<()> {
        Self::unused()
    }

    async fn pipeline_runs(&self, _repo: &str, _number: u64) -> Result<Vec<PipelineRun>> {
        Ok(Vec::new())
    }

    async fn pipeline_jobs(
        &self,
        _repo: &str,
        _number: u64,
        _run_id: &str,
    ) -> Result<Vec<PipelineJob>> {
        Ok(Vec::new())
    }

    async fn download_log(
        &self,
        _repo: &str,
        _number: u64,
        _log_id: &str,
        _sink: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<()> {
        Self::unused()
    }
}

fn is_rust(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
}
