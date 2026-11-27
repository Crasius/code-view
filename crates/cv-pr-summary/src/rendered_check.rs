//! Markdown cards rendered in the folder summary: the header's Preview shows only for a folder
//! with Markdown files, switching it on turns their cards into rendered diffs (their versions
//! loaded once, no diff asked for), each card exactly as tall as its blocks, folding its unchanged
//! runs as the line cards do, and switching it off brings the lines back.

use cv_testkit::Clicks;
use egui_kittest::kittest::Queryable;
use review_git::{ChangeKind, ChangedFile};
use review_store::ThemePreference;

use super::SummaryEvent;
use super::testing::{Scene, card_of, file, harness, markdown_versions, settle};
use cv_pr_rendered::markdown_diff::embedded;

const README: &str = "docs/README.md";
const CODE: &str = "src/app/cart.rs";
const SIZE: [f32; 2] = [1180.0, 860.0];

fn files() -> Vec<ChangedFile> {
    vec![
        file(README, ChangeKind::Modified, Some((14, 14))),
        file(CODE, ChangeKind::Modified, Some((10, 6))),
    ]
}

/// The card of `path`: its slot's height in the index, less its header and the gap below it.
fn body_height(scene: &Scene, path: &str) -> f32 {
    let summary = &scene.summary;
    let card = card_of(scene, path);
    let geometry = summary.geometry.expect("laid out");
    summary.index.bottom(card) - summary.index.top(card) - geometry.header - geometry.gap
}

#[test]
fn preview_renders_the_markdown_cards_and_off_brings_their_lines_back() {
    let mut harness = harness(Scene::new(files(), ThemePreference::Light), SIZE);
    settle(&mut harness);
    assert!(harness.state().summary.bodies.get(README).is_some(), "off: the lines show");
    assert!(harness.state().source.rendered.is_empty(), "off: no versions load");

    harness.click_at(harness.get_by_label("Preview").rect());
    harness.run_steps(2);
    settle(&mut harness);
    let scene = harness.state();
    assert!(
        scene.events.iter().any(|event| matches!(event, SummaryEvent::SwitchPreview(true))),
        "the header asks the tab to save the choice"
    );
    assert_eq!(scene.source.rendered, [README], "only the Markdown card loads its versions, once");
    let rendered = scene.summary.rendered.get(README).expect("the README card is rendered");
    let height = embedded::height(&rendered.view);
    assert!(height > 200.0, "forty sections of blocks: {height}");
    assert!(
        (body_height(scene, README) - height).abs() < 0.5,
        "the card is as tall as its blocks: {} vs {height}",
        body_height(scene, README)
    );
    let code_height = body_height(scene, CODE);

    harness.click_at(harness.get_by_label("Preview").rect());
    harness.run_steps(2);
    settle(&mut harness);
    let scene = harness.state();
    let lines = scene.summary.bodies.get(README).expect("off: the lines show again").view.height();
    assert!((body_height(scene, README) - lines).abs() < 0.5, "back to the lines' height");
    assert!((body_height(scene, CODE) - code_height).abs() < 0.5, "a code card never changes");
}

#[test]
fn a_folder_without_markdown_has_no_preview_switch() {
    let mut harness = harness(
        Scene::new(vec![file(CODE, ChangeKind::Modified, Some((4, 2)))], ThemePreference::Light),
        SIZE,
    );
    settle(&mut harness);
    assert!(harness.query_by_label("Preview").is_none(), "it would switch nothing");
}

/// The rendered height of the README card, Preview on, its versions `versions` and the file view
/// keeping `context` unchanged lines around each change.
fn rendered_height(versions: (String, String), context: u32) -> f32 {
    let mut scene = Scene::new(files(), ThemePreference::Light);
    scene.source.versions = Some(versions);
    scene.context = context;
    scene.preview = true;
    let mut harness = harness(scene, SIZE);
    settle(&mut harness);
    let rendered =
        harness.state().summary.rendered.get(README).expect("the README card is rendered");
    embedded::height(&rendered.view)
}

/// Regression: a rendered card kept as many unchanged blocks as the file view's context choice
/// (50 here), where the line cards keep the folder's three, so it folded nothing.
#[test]
fn a_rendered_card_folds_its_unchanged_blocks_whatever_the_file_view_keeps() {
    let section = |index: usize, edited: bool| {
        let edit = if edited { " Edited." } else { "" };
        format!(
            "## Part {index}

Section {index} explains how the cart totals its lines.{edit}

"
        )
    };
    let one_edit = (
        (0..40).map(|index| section(index, false)).collect::<String>(),
        (0..40).map(|index| section(index, index == 20)).collect::<String>(),
    );
    let folded = rendered_height(one_edit.clone(), 50);
    assert!(
        (folded - rendered_height(one_edit, 3)).abs() < 0.5,
        "the file view's context does not reach the folder's cards"
    );
    let unfolded = rendered_height(markdown_versions(40), 50);
    assert!(folded * 3.0 < unfolded, "the runs around the one edit fold: {folded} vs {unfolded}");
}
