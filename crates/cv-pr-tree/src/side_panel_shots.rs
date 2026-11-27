//! The Files side panel with a change list shaped like a large pull request (356 files, about
//! +10,900 −5,100 lines): its header never overlaps itself at any width, and the category
//! dropdown works with the mouse and the keyboard. No network, no tokens.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cv_testkit::Clicks;
use egui::{Rect, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::FileCategory;
use review_git::{ChangeKind, ChangedFile};
use review_store::ThemePreference;

use super::{FileTree, TreeInputs, side_panel};
use cv_base::theme;
use cv_widgets::focus;

struct Scene {
    tree: FileTree,
    files: Arc<Vec<ChangedFile>>,
    viewed: HashSet<String>,
    width: f32,
    theme: ThemePreference,
    is_counting: bool,
}

/// 356 files: 300 code, 44 tests, 7 docs, 3 config, 1 generated, 1 asset.
fn files() -> Vec<ChangedFile> {
    let mut files = Vec::new();
    let mut push =
        |count: usize, folder: &str, extension: &str, category: FileCategory, lines: (u32, u32)| {
            for index in 0..count {
                files.push(ChangedFile {
                    path: format!("{folder}/file_{index:03}.{extension}"),
                    old_path: None,
                    kind: ChangeKind::Modified,
                    old_oid: "1".repeat(40),
                    new_oid: "2".repeat(40),
                    additions: Some(lines.0 + index as u32 % 7),
                    deletions: Some(lines.1 + index as u32 % 5),
                    is_binary: false,
                    category,
                    whitespace_only: false,
                });
            }
        };
    push(300, "src/app", "rs", FileCategory::Code, (30, 15));
    push(44, "tests", "rs", FileCategory::Tests, (20, 9));
    push(7, "docs", "md", FileCategory::Docs, (12, 4));
    push(3, "build", "toml", FileCategory::Config, (2, 1));
    push(1, "generated", "lock", FileCategory::Generated, (900, 610));
    push(1, "assets", "png", FileCategory::Assets, (0, 0));
    files
}

fn harness(theme: ThemePreference, width: f32) -> Harness<'static, Scene> {
    let builder = Harness::builder().with_size([640.0, 720.0]).with_context_setup(|ctx| {
        theme::fonts::install(ctx);
        theme::motion::set_reduced(ctx, false);
    });
    let scene = Scene {
        tree: FileTree::default(),
        files: Arc::new(files()),
        viewed: HashSet::new(),
        width,
        theme,
        is_counting: false,
    };
    builder.build_ui_state(
        |ui, scene: &mut Scene| {
            focus::track_modality(ui.ctx());
            let palette = theme::activate(ui.ctx(), scene.theme);
            ui.painter().rect_filled(ui.max_rect(), 0, palette.ground);
            let viewed = scene.viewed.clone();
            let is_viewed = move |file: &ChangedFile| viewed.contains(&file.path);
            scene.tree.refresh(&scene.files, &is_viewed, scene.viewed.len() as u64);
            let threads = HashMap::new();
            let inputs = TreeInputs {
                files: &scene.files,
                selected: None,
                selected_folder: None,
                is_viewed: &is_viewed,
                thread_counts: &threads,
                vote: super::VoteOffer::None,
            };
            side_panel::show(ui, scene.width, &mut scene.tree, &inputs, scene.is_counting);
            focus::paint_for_focused_widget(ui.ctx());
        },
        scene,
    )
}

/// Leaf widgets of the header (everything above the first tree row).
fn header_rects(harness: &Harness<'_, Scene>) -> Vec<(String, Rect)> {
    let tree_top = tree_top(harness);
    let roles = [
        Role::TextInput,
        Role::ComboBox,
        Role::Button,
        Role::Label,
        Role::Link,
        Role::ProgressIndicator,
    ];
    harness
        .query_all_by(|node| roles.contains(&node.role()))
        .filter(|node| node.rect().bottom() <= tree_top)
        .map(|node| {
            (
                format!("{:?} {:?}", node.accesskit_node().role(), node.accesskit_node().label()),
                node.rect(),
            )
        })
        .collect()
}

fn assert_no_overlap(harness: &Harness<'_, Scene>, width: f32) {
    let rects = header_rects(harness);
    assert!(rects.len() >= 5, "header widgets at {width}: {rects:?}");
    // The panel's content area: from the search field's left edge, the panel width without its
    // 8 px margins.
    let content_left = rects.iter().map(|(_, rect)| rect.left()).fold(f32::INFINITY, f32::min);
    let content_right = content_left + width - 16.0 + 0.5;
    for (index, (name, rect)) in rects.iter().enumerate() {
        assert!(
            rect.right() <= content_right,
            "{name} ends at {} beyond {content_right} at width {width}: {rects:?}",
            rect.right()
        );
        for (other_name, other) in &rects[index + 1..] {
            let overlap = rect.intersect(*other);
            assert!(
                overlap.width() <= 0.5 || overlap.height() <= 0.5,
                "{name} {rect:?} overlaps {other_name} {other:?} at width {width}"
            );
        }
    }
}

#[test]
fn side_panel_header_never_overlaps_from_240_to_600() {
    let mut harness = harness(ThemePreference::Light, 240.0);
    for is_counting in [false, true] {
        harness.state_mut().is_counting = is_counting;
        for width in (240..=600).step_by(4) {
            harness.state_mut().width = width as f32;
            harness.run_steps(2);
            assert_no_overlap(&harness, width as f32);
        }
    }
}

/// The top of the first tree row.
fn tree_top(harness: &Harness<'_, Scene>) -> f32 {
    harness
        .query_all_by_role(Role::TreeItem)
        .map(|node| node.rect().top())
        .fold(f32::INFINITY, f32::min)
}

#[test]
fn side_panel_tree_stays_put_when_the_counting_ends_from_220_to_600() {
    let mut harness = harness(ThemePreference::Light, 220.0);
    // Five times the change list, most of it viewed: long texts beside the totals.
    let files: Vec<ChangedFile> = (0..5)
        .flat_map(|copy| {
            files()
                .into_iter()
                .map(move |file| ChangedFile { path: format!("copy_{copy}/{}", file.path), ..file })
        })
        .collect();
    harness.state_mut().viewed = files.iter().take(1_500).map(|file| file.path.clone()).collect();
    harness.state_mut().files = Arc::new(files);
    for width in 220..=600 {
        harness.state_mut().width = width as f32;
        harness.state_mut().is_counting = true;
        harness.run_steps(2);
        let counting = tree_top(&harness);
        harness.state_mut().is_counting = false;
        harness.run_steps(2);
        assert_eq!(tree_top(&harness), counting, "the tree moved at width {width}");
    }
}

#[test]
fn category_dropdown_filters_with_mouse_and_keyboard() {
    let mut harness = harness(ThemePreference::Light, 320.0);
    harness.run_steps(2);
    let dropdown = harness.get_by_role(Role::ComboBox);
    assert_eq!(dropdown.accesskit_node().value().as_deref(), Some("All files, 356 files"));

    // Open, then "Only" on Tests: just the tests remain.
    harness.click_at(dropdown.rect());
    harness.run_steps(2);
    harness.click_at(harness.get_by_label("Only Tests").rect());
    harness.run_steps(2);
    assert_eq!(
        harness.state().tree.filter.categories.iter().copied().collect::<Vec<_>>(),
        [FileCategory::Tests]
    );
    assert_eq!(
        harness.get_by_role(Role::ComboBox).accesskit_node().value().as_deref(),
        Some("Tests, 44 files")
    );

    // Checking Code as well combines both, the dropdown stays open for more picks.
    harness.click_at(harness.get_by_role_and_label(Role::CheckBox, "Code, 300 files").rect());
    harness.run_steps(2);
    assert_eq!(
        harness.get_by_role(Role::ComboBox).accesskit_node().value().as_deref(),
        Some("Code, Tests, 344 files")
    );

    // "All files" clears the filter.
    harness.click_at(harness.get_by_role_and_label(Role::CheckBox, "All files, 356 files").rect());
    harness.run_steps(2);
    assert!(harness.state().tree.filter.categories.is_empty());

    // Keyboard: Esc closes, Tab reaches the dropdown, Enter opens it on the first row, arrows move,
    // Space unchecks Code (from all: everything else stays).
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(
        harness.query_by_role_and_label(Role::CheckBox, "Code, 300 files").is_none(),
        "Esc closes the list"
    );
    harness.get_by_role(Role::TextInput).focus();
    harness.run_steps(1);
    harness.key_press(egui::Key::Tab);
    harness.run_steps(2);
    harness.key_press(egui::Key::Enter);
    harness.run_steps(3);
    harness.key_press(egui::Key::ArrowDown);
    harness.run_steps(2);
    harness.key_press(egui::Key::Space);
    harness.run_steps(2);
    let categories: Vec<_> = harness.state().tree.filter.categories.iter().copied().collect();
    assert_eq!(
        categories,
        [
            FileCategory::Tests,
            FileCategory::Docs,
            FileCategory::Config,
            FileCategory::Generated,
            FileCategory::Assets
        ]
    );

    // The hide-viewed toggle is a pressed/unpressed button.
    harness.click_at(harness.get_by_label("Hide viewed files").rect());
    harness.run_steps(2);
    assert!(harness.state().tree.filter.hide_viewed);
}
