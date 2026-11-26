//! The bar above the diff at many widths: whatever the path and the room, no two of its widgets
//! overlap and none leaves the bar.

use egui::{Rect, Role, UiBuilder, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{DiffSide, FileCategory};
use review_git::{ChangeKind, ChangedFile, FULL_FILE_CONTEXT};
use review_store::{AppPaths, DiffLayout, ThemePreference};

use cv_base::theme;
use cv_pr_diff::testing::settle;
use cv_pr_rendered::file_header::{self, HeaderInputs};
use cv_pr_rendered::preview::Preview;
use cv_services::Services;

/// The path of the reported overlap, and longer.
const LONG_PATH: &str =
    "src/DndTracker.Infrastructure/Ai/EntityEditPlanValidationPipelineConfiguration.cs";
/// The width the navigation rail and the file tree (beside the diff, on either side) take.
const SIDEBAR: f32 = 340.0;
/// Window widths from roomy down to the narrowest the files tab allows.
const WIDTHS: [f32; 6] = [1600.0, 1200.0, 1000.0, 860.0, 760.0, 700.0];

struct Scene {
    services: Services,
    preview: Preview,
    file: ChangedFile,
    is_viewed: bool,
    changes: Option<(usize, usize)>,
    whole_file: Option<DiffSide>,
    /// A rendered Markdown diff is on screen.
    is_rendered: bool,
    /// Where the bar was drawn.
    area: Rect,
}

fn file(
    path: &str,
    kind: ChangeKind,
    old_path: Option<&str>,
    (additions, deletions): (u32, u32),
) -> ChangedFile {
    ChangedFile {
        path: path.into(),
        old_path: old_path.map(Into::into),
        kind,
        old_oid: if kind == ChangeKind::Added { "0".repeat(40) } else { "1".repeat(40) },
        new_oid: if kind == ChangeKind::Deleted { "0".repeat(40) } else { "2".repeat(40) },
        additions: Some(additions),
        deletions: Some(deletions),
        is_binary: false,
        category: FileCategory::Code,
        whitespace_only: false,
    }
}

fn scene(file: ChangedFile, changes: Option<(usize, usize)>) -> Scene {
    let root = std::env::temp_dir().join(format!("review-header-{}", std::process::id()));
    let whole_file = super::whole_file_side(&file);
    Scene {
        services: Services::new(
            egui::Context::default(),
            AppPaths::new(root.join("config"), root.join("cache")),
            cv_providers::create_provider,
        ),
        preview: Preview::default(),
        file,
        is_viewed: false,
        changes,
        whole_file,
        is_rendered: false,
        area: Rect::NOTHING,
    }
}

fn harness(scene: Scene, width: f32) -> Harness<'static, Scene> {
    Harness::builder()
        .with_size([width, 64.0])
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::motion::install(ctx);
        })
        .build_ui_state(
            |ui, scene: &mut Scene| {
                theme::activate(ui.ctx(), ThemePreference::Light);
                let full = ui.max_rect();
                scene.area = Rect::from_min_max(
                    pos2(full.left() + SIDEBAR, full.top() + 8.0),
                    pos2(full.right() - 8.0, full.bottom()),
                );
                let inputs = HeaderInputs {
                    file: &scene.file,
                    is_viewed: scene.is_viewed,
                    changes: scene.changes,
                    is_rendered: scene.is_rendered,
                    whole_file: scene.whole_file,
                    hides_whitespace: false,
                    edge_hint: None,
                };
                ui.scope_builder(UiBuilder::new().max_rect(scene.area), |ui| {
                    file_header::show(ui, &inputs, &mut scene.services, &mut scene.preview);
                });
            },
            scene,
        )
}

/// The bar's widgets someone sees or uses, by name.
fn widgets(harness: &Harness<'_, Scene>) -> Vec<(String, Rect)> {
    harness
        .query_all_by(|node| {
            matches!(
                node.role(),
                Role::Button
                    | Role::Label
                    | Role::RadioButton
                    | Role::Switch
                    | Role::CheckBox
                    | Role::ComboBox
            )
        })
        .map(|node| {
            let name = node
                .accesskit_node()
                .label()
                .or_else(|| node.accesskit_node().value())
                .unwrap_or_default();
            (name, node.rect())
        })
        .collect()
}

/// No two widgets overlap, and every one stays inside the bar.
fn assert_apart(harness: &Harness<'_, Scene>, case: &str) {
    let area = harness.state().area;
    let widgets = widgets(harness);
    assert!(widgets.len() >= 4, "{case}: the bar shows its widgets, found {widgets:?}");
    for (index, (name, rect)) in widgets.iter().enumerate() {
        assert!(
            rect.left() >= area.left() - 0.5 && rect.right() <= area.right() + 0.5,
            "{case}: {name:?} at {rect:?} leaves the bar {area:?}"
        );
        for (other, other_rect) in &widgets[index + 1..] {
            assert!(
                !rect.shrink(0.5).intersects(other_rect.shrink(0.5)),
                "{case}: {name:?} at {rect:?} overlaps {other:?} at {other_rect:?}; all widgets: {widgets:#?}"
            );
        }
    }
}

fn check_widths(make: impl Fn() -> Scene, case: &str) {
    for width in WIDTHS {
        let mut harness = harness(make(), width);
        settle(&mut harness);
        assert_apart(&harness, &format!("{case} at {width}"));
    }
}

/// Every widget but the path and its rename source, the text that changes from file to file.
fn controls(harness: &Harness<'_, Scene>) -> Vec<(String, Rect)> {
    let path = harness.state().file.path.clone();
    widgets(harness)
        .into_iter()
        .filter(|(name, _)| *name != path && !name.starts_with("Renamed from "))
        .collect()
}

/// Draws `file` with `changes` (`None` while loading or for a preview), then returns the bar's
/// controls after checking that none overlap.
fn show_file(
    harness: &mut Harness<'static, Scene>,
    width: f32,
    file: ChangedFile,
    changes: Option<(usize, usize)>,
) -> Vec<(String, Rect)> {
    let scene = harness.state_mut();
    scene.whole_file = super::whole_file_side(&file);
    scene.file = file;
    scene.changes = changes;
    redraw(harness, width)
}

fn redraw(harness: &mut Harness<'static, Scene>, width: f32) -> Vec<(String, Rect)> {
    settle(harness);
    let found = controls(harness);
    assert_apart(harness, &format!("{} at {width}", harness.state().file.path));
    found
}

/// Positions only: names (the counter's text, the counts) change with the file.
fn rects(widgets: &[(String, Rect)]) -> Vec<Rect> {
    widgets.iter().map(|(_, rect)| *rect).collect()
}

/// The rects of everything but the context choice, the layout choice and the chip that replaces
/// both for an added or deleted file.
fn around_layout_slot(widgets: &[(String, Rect)]) -> Vec<Rect> {
    widgets.iter().filter(|(name, _)| !is_in_layout_slot(name)).map(|(_, rect)| *rect).collect()
}

fn is_in_layout_slot(name: &str) -> bool {
    // The context choice is named by its tooltip.
    matches!(
        name,
        "New file"
            | "Deleted file"
            | "Unchanged lines shown around each change"
            | "Side by side"
            | "Inline"
            | "New only"
    )
}

/// A choice made in the bar, applied to the scene.
type Pick = fn(&mut Scene);

/// Every choice keeps the size of its control, whichever state it is in.
fn assert_choices_keep_the_bar(
    harness: &mut Harness<'static, Scene>,
    width: f32,
    reference: &[(String, Rect)],
) {
    let picks: [(&str, Pick); 7] = [
        ("viewed", |scene| scene.is_viewed = true),
        ("comments hidden", |scene| scene.services.settings.show_comments_in_diff = false),
        ("10 lines", |scene| scene.services.settings.context_lines = 10),
        ("full file", |scene| scene.services.settings.context_lines = FULL_FILE_CONTEXT),
        ("inline", |scene| scene.services.settings.diff_layout = DiffLayout::Inline),
        ("new only", |scene| scene.services.settings.diff_layout = DiffLayout::NewOnly),
        ("not wrapped", |scene| {
            scene.services.settings.wrap_lines = !scene.services.settings.wrap_lines;
        }),
    ];
    for (pick, apply) in picks {
        apply(harness.state_mut());
        let found = redraw(harness, width);
        assert_eq!(
            rects(&found),
            rects(reference),
            "picking {pick} at {width} moved the bar: {found:#?}\nbefore: {reference:#?}"
        );
    }
}

/// Added and deleted files show a chip where the choices were; everything around stays.
fn assert_whole_files_keep_the_bar(
    harness: &mut Harness<'static, Scene>,
    width: f32,
    reference: &[(String, Rect)],
) {
    for kind in [ChangeKind::Added, ChangeKind::Deleted] {
        let found = show_file(harness, width, file(LONG_PATH, kind, None, (40, 0)), Some((0, 1)));
        assert_eq!(
            around_layout_slot(&found),
            around_layout_slot(reference),
            "{kind:?} at {width}: {found:#?}\nbefore: {reference:#?}"
        );
        let slot = reference
            .iter()
            .filter(|(name, _)| is_in_layout_slot(name))
            .fold(Rect::NOTHING, |slot, (_, rect)| slot.union(*rect));
        let (_, chip) = found.iter().find(|(name, _)| is_in_layout_slot(name)).expect("the chip");
        assert!(
            (chip.center().x - slot.center().x).abs() < 0.5,
            "{kind:?} at {width}: the chip {chip:?} is centered where the choices were {slot:?}"
        );
    }
}

#[test]
fn switching_files_moves_nothing_but_the_path() {
    for width in [1600.0, 1200.0, 760.0] {
        let code = file(LONG_PATH, ChangeKind::Modified, None, (5, 0));
        let mut harness = harness(scene(code.clone(), None), width);
        // Loading, then shown: the diff's controls switch from disabled to enabled in place.
        let reference = show_file(&mut harness, width, code.clone(), None);
        let found = show_file(&mut harness, width, code.clone(), Some((1, 4)));
        assert_eq!(
            rects(&found),
            rects(&reference),
            "showing the diff at {width} moved the bar: {found:#?}\nbefore: {reference:#?}"
        );
        assert_choices_keep_the_bar(&mut harness, width, &reference);

        let files = [
            (code.clone(), Some((3, 4))),
            (file("assets/brand/logo.png", ChangeKind::Modified, None, (0, 0)), None),
            (file("README.md", ChangeKind::Modified, None, (12, 4)), None),
            (file("README.md", ChangeKind::Modified, None, (12, 4)), Some((0, 2))),
            (
                file("docs/changelog.md", ChangeKind::Renamed, Some("CHANGELOG.md"), (1_234, 87)),
                Some((0, 12)),
            ),
        ];
        for (file, changes) in files {
            let path = file.path.clone();
            let found = show_file(&mut harness, width, file, changes);
            assert_eq!(
                rects(&found),
                rects(&reference),
                "{path} at {width} moved the bar: {found:#?}\nbefore: {reference:#?}"
            );
        }
        harness.state_mut().preview.set_enabled(false);
        let found = redraw(&mut harness, width);
        assert_eq!(
            rects(&found),
            rects(&reference),
            "switching the preview off at {width} moved the bar: {found:#?}"
        );
        assert_whole_files_keep_the_bar(&mut harness, width, &reference);
    }
}

/// The bar's comments switch turns "Show comments in the diff" off and on again, saved for the
/// next start.
#[test]
fn the_comments_switch_is_saved() {
    let root = std::env::temp_dir().join(format!("review-header-comments-{}", std::process::id()));
    // Left over from an earlier run, or absent.
    let _was_absent = std::fs::remove_dir_all(&root);
    let paths = AppPaths::new(root.join("config"), root.join("cache"));
    let mut scene = scene(file(LONG_PATH, ChangeKind::Modified, None, (5, 0)), Some((0, 4)));
    scene.services =
        Services::new(egui::Context::default(), paths.clone(), cv_providers::create_provider);
    let mut harness = harness(scene, 1600.0);
    settle(&mut harness);
    assert!(harness.state().services.settings.show_comments_in_diff, "comments show by default");
    for expected in [false, true] {
        harness.get_by_role_and_label(Role::Switch, "Show comments in the diff").click();
        settle(&mut harness);
        assert_eq!(harness.state().services.settings.show_comments_in_diff, expected);
        let restarted =
            Services::new(egui::Context::default(), paths.clone(), cv_providers::create_provider);
        assert_eq!(restarted.settings.show_comments_in_diff, expected, "saved");
    }
    // Best effort: the next run removes what is left.
    let _best_effort = std::fs::remove_dir_all(&root);
}

/// Over a rendered Markdown diff the wrap switch's slot holds the Full width switch, in the same
/// place; it switches the reading column to full width and back, saved for the next start.
#[test]
fn the_width_switch_takes_the_wrap_slot_and_is_saved() {
    let root = std::env::temp_dir().join(format!("review-header-width-{}", std::process::id()));
    // Left over from an earlier run, or absent.
    let _was_absent = std::fs::remove_dir_all(&root);
    let paths = AppPaths::new(root.join("config"), root.join("cache"));
    let mut scene = scene(
        file("docs/engineering/billing-invoicing.md", ChangeKind::Modified, None, (5, 0)),
        Some((0, 4)),
    );
    scene.services =
        Services::new(egui::Context::default(), paths.clone(), cv_providers::create_provider);
    let mut harness = harness(scene, 1600.0);
    settle(&mut harness);
    let wrap = harness.get_by_role_and_label(Role::Switch, "Wrap long lines").rect();
    harness.state_mut().is_rendered = true;
    settle(&mut harness);
    // Side by side each half fills its width: the switch rests in its place.
    let resting = harness.get_by_role_and_label(Role::Switch, "Full width");
    assert_eq!(resting.rect(), wrap, "the switch takes the wrap switch's place");
    assert!(resting.accesskit_node().is_disabled(), "side by side the switch rests");
    harness.state_mut().services.settings.diff_layout = DiffLayout::Inline;
    settle(&mut harness);
    assert_eq!(
        harness.get_by_role_and_label(Role::Switch, "Full width").rect(),
        wrap,
        "in one column it stays in place"
    );
    assert!(
        !harness.state().services.settings.rendered_width.full,
        "the reading column by default"
    );
    harness.get_by_role_and_label(Role::Switch, "Full width").click();
    settle(&mut harness);
    assert!(harness.state().services.settings.rendered_width.full);
    let restarted = Services::new(egui::Context::default(), paths, cv_providers::create_provider);
    assert!(restarted.settings.rendered_width.full, "saved");
    // Best effort: the next run removes what is left.
    let _best_effort = std::fs::remove_dir_all(&root);
}

#[test]
fn long_paths_never_overlap_the_controls() {
    // The reported case: a long C# path with four changes, next to the layout controls.
    check_widths(
        || scene(file(LONG_PATH, ChangeKind::Modified, None, (5, 0)), Some((0, 4))),
        "modified",
    );
    check_widths(
        || {
            let renamed = file(
                "docs/architecture/decisions/0007-validation-pipelines.md",
                ChangeKind::Renamed,
                Some(LONG_PATH),
                (120, 3),
            );
            let mut scene = scene(renamed, Some((12, 345)));
            scene.is_viewed = true;
            scene
        },
        "renamed-markdown",
    );
    check_widths(
        || scene(file(LONG_PATH, ChangeKind::Added, None, (1_234, 0)), Some((0, 1))),
        "added",
    );
    check_widths(
        || scene(file(LONG_PATH, ChangeKind::Deleted, None, (0, 87)), Some((0, 1))),
        "deleted",
    );
    check_widths(|| scene(file(LONG_PATH, ChangeKind::Modified, None, (5, 0)), None), "loading");
}
