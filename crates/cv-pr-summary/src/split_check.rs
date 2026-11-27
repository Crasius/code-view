//! The folder summary side by side, as the file view shows a diff with that layout: every changed
//! file's card puts the old lines left of the new ones (added and deleted files stay whole in one
//! pane), threads sit under their side, the header's layout choice switches all cards while the
//! line at the top of the view stays put and no diff loads twice, and a 10,000-file pull request
//! still costs only what is on screen.

use cv_testkit::Clicks;
use egui::Rect;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::ThreadAnchor;
use review_git::{ChangeKind, ChangedFile};
use review_store::{DiffLayout, ThemePreference};

use super::bodies::MAX_BODIES;
use super::loads::{MAX_RUNNING, MAX_WANTED};
use super::testing::{Scene, card_of, file, harness, settle};
use super::{MARGIN, SummaryEvent};
use cv_code::line_source::Pane;
use cv_pr_diff::diff_view::DiffView;
use cv_pr_diff::testing::thread_on_old_line;

/// Six hunks of three deleted and five added lines, each with three unchanged lines around them:
/// inline a hunk takes 14 rows, side by side 11, so the lines further down move up when it splits.
const PATH: &str = "src/app/cart.rs";
const ADDED: &str = "src/app/promo.rs";
const SIZE: [f32; 2] = [1180.0, 860.0];

fn files() -> Vec<ChangedFile> {
    vec![
        file("src/app/billing.rs", ChangeKind::Modified, Some((10, 6))),
        file(PATH, ChangeKind::Modified, Some((30, 18))),
        file(ADDED, ChangeKind::Added, Some((12, 0))),
        file("src/app/tax.rs", ChangeKind::Modified, Some((10, 6))),
        file("src/ui/theme.rs", ChangeKind::Modified, Some((4, 4))),
    ]
}

fn split_scene(theme: ThemePreference) -> Scene {
    let mut scene = Scene::new(files(), theme);
    scene.layout = DiffLayout::SideBySide;
    scene
}

fn view_of<'a>(scene: &'a Scene, path: &str) -> &'a DiffView {
    &scene.summary.bodies.get(path).expect("the card's diff is loaded").view
}

/// The card's row showing head line `new_line` (old line `old_line` when there is no head line).
fn row_of(scene: &Scene, path: &str, old_line: Option<u32>, new_line: Option<u32>) -> usize {
    let body = scene.summary.bodies.get(path).expect("the card's diff is loaded");
    body.view.row_of(&body.diff, old_line, new_line)
}

/// Where the code of head line `new_line` of [`PATH`] shows: the right pane side by side.
fn head_line(scene: &Scene, new_line: u32, pane: Pane) -> Rect {
    view_of(scene, PATH).text_area(pane, row_of(scene, PATH, None, Some(new_line))).rect
}

fn scroll_to_card(harness: &mut Harness<'_, Scene>, path: &str, below_top: f32) {
    let card = card_of(harness.state(), path);
    let top = harness.state().summary.index.top(card);
    harness.state_mut().summary.scroll_to = Some(top + MARGIN + below_top);
    settle(harness);
}

/// Side by side, a changed file's card shows the old lines in the left pane and the new ones in
/// the right, a replaced line beside the line that replaced it; an added file shows whole, in one
/// pane, as in the file view.
#[test]
fn split_cards_show_both_sides_and_whole_files_one_pane() {
    let mut harness = harness(split_scene(ThemePreference::Light), SIZE);
    settle(&mut harness);
    scroll_to_card(&mut harness, PATH, 0.0);
    let scene = harness.state();
    // Old line 4 was replaced by new line 4: one row, the old line left, the new one right.
    let row = row_of(scene, PATH, None, Some(4));
    assert_eq!(
        row,
        row_of(scene, PATH, Some(4), None),
        "the replaced and the replacing line share a row"
    );
    let view = view_of(scene, PATH);
    let (left, right) =
        (view.text_area(Pane::Left, row).rect, view.text_area(Pane::Right, row).rect);
    assert!((left.top() - right.top()).abs() < 0.5, "both sides on one row: {left:?} {right:?}");
    assert!(
        left.right() <= right.left() + 0.5 && left.left() < right.left(),
        "old left of new: {left:?} {right:?}"
    );
    let card_width = scene.summary.width;
    assert!(
        left.width() > card_width * 0.3 && right.width() > card_width * 0.3,
        "both sides share the card: {left:?} {right:?} in {card_width}"
    );

    scroll_to_card(&mut harness, ADDED, 0.0);
    let scene = harness.state();
    let whole =
        view_of(scene, ADDED).text_area(Pane::Inline, row_of(scene, ADDED, None, Some(1))).rect;
    assert!(
        whole.width() > scene.summary.width * 0.8,
        "an added file shows whole in one pane: {whole:?}"
    );
}

/// A thread on a new line sits under the right pane, a thread on an old line under the left, and
/// both count into their card's height as inline.
#[test]
fn threads_sit_under_their_side_in_a_split_card() {
    let mut scene = split_scene(ThemePreference::Light);
    let mut on_new = thread_on_old_line(0, "Round before the tax.");
    on_new.id = "on-new".into();
    on_new.anchor = Some(ThreadAnchor {
        path: PATH.into(),
        old_path: None,
        old_line: None,
        new_line: Some(6),
        range: None,
    });
    let mut on_old = thread_on_old_line(0, "Why did the old check go?");
    on_old.id = "on-old".into();
    on_old.anchor = Some(ThreadAnchor {
        path: PATH.into(),
        old_path: None,
        old_line: Some(5),
        new_line: None,
        range: None,
    });
    scene.threads.extend([on_new, on_old]);
    let mut harness = harness(scene, SIZE);
    settle(&mut harness);
    scroll_to_card(&mut harness, PATH, 0.0);

    let new_card = harness.get_by_label_contains("Round before the tax.").rect();
    let old_card = harness.get_by_label_contains("Why did the old check go?").rect();
    let scene = harness.state();
    let new_pane = head_line(scene, 6, Pane::Right);
    let old_row = row_of(scene, PATH, Some(5), None);
    let old_pane = view_of(scene, PATH).text_area(Pane::Left, old_row).rect;
    assert!(
        new_card.left() >= new_pane.left() - 80.0 && new_card.left() > old_pane.right() - 1.0,
        "the new line's thread sits under the right pane: {new_card:?}, pane {new_pane:?}"
    );
    assert!(
        old_card.right() <= old_pane.right() + 1.0,
        "the old line's thread sits under the left pane: {old_card:?}, pane {old_pane:?}"
    );
    let (geometry, card) = (scene.summary.geometry.expect("laid out"), card_of(scene, PATH));
    let height = scene.summary.index.bottom(card) - scene.summary.index.top(card);
    assert!(
        height > geometry.slot(scene.summary.slots[card].shape, false) + 120.0,
        "the card counts both threads: {height}"
    );

    harness
        .click_at(harness.query_all_by_label("Resolve").next().expect("a thread's Resolve").rect());
    harness.run_steps(2);
    assert!(
        harness.state().events.iter().any(|event| matches!(event, SummaryEvent::Thread(_))),
        "the thread takes a resolve in the split card"
    );
}

/// The header's Inline / Side by side switches every card at once: the line at the top of the view
/// stays where it was (to the pixel), the cards keep their diffs (no diff loads twice; only cards
/// the shorter split cards bring into view load), and the tab is told so it saves the choice. Back
/// to inline returns to the same place.
#[test]
fn toggling_the_layout_keeps_the_line_at_the_top_and_loads_each_diff_once() {
    let mut harness = harness(Scene::new(files(), ThemePreference::Light), SIZE);
    settle(&mut harness);
    // Head line 125 is an added line of the fourth hunk: 9 rows further up side by side.
    scroll_to_card(&mut harness, PATH, 0.0);
    let line = head_line(harness.state(), 125, Pane::Inline);
    let view = harness.state().summary.screen;
    let delta = line.top() - view.top() - 6.0;
    let summary = &mut harness.state_mut().summary;
    summary.scroll_to = Some(summary.viewport.top() + MARGIN + delta);
    settle(&mut harness);
    let before = head_line(harness.state(), 125, Pane::Inline).top();
    let loads = harness.state().source.started.len();
    let offset_before = harness.state().summary.viewport.top();

    harness.click_at(harness.get_by_label("Side by side").rect());
    harness.run_steps(1);
    assert!(
        matches!(
            harness.state().events.last(),
            Some(SummaryEvent::SwitchLayout(DiffLayout::SideBySide))
        ),
        "the tab saves the choice"
    );
    // The same frame shows the cards split, the line where it was.
    let after = head_line(harness.state(), 125, Pane::Right).top();
    assert!((after - before).abs() < 1.0, "the line at the top moved from {before} to {after}");
    settle(&mut harness);
    let settled = head_line(harness.state(), 125, Pane::Right).top();
    assert!((settled - before).abs() < 1.0, "and stays there: {settled}");
    assert!(
        harness.state().summary.viewport.top() < offset_before,
        "the rows above it got fewer, so the view scrolled up"
    );
    assert_loaded_once(harness.state(), loads);

    let loads = harness.state().source.started.len();
    harness.click_at(harness.get_by_label("Inline").rect());
    settle(&mut harness);
    let back = head_line(harness.state(), 125, Pane::Inline).top();
    assert!(
        (back - before).abs() < 1.0,
        "back to inline, the line is where it was: {back} vs {before}"
    );
    assert_loaded_once(harness.state(), loads);
}

/// No diff loaded before the first `loads` is loaded again, and the ones after are cards near the view.
fn assert_loaded_once(scene: &Scene, loads: usize) {
    let started = &scene.source.started;
    let mut seen = std::collections::HashSet::new();
    assert!(started.iter().all(|path| seen.insert(path)), "a diff loaded twice: {started:?}");
    let summary = &scene.summary;
    let height = summary.viewport.height();
    for path in &started[loads..] {
        let card = card_of(scene, path);
        assert!(
            summary.index.bottom(card) > summary.viewport.top() - height
                && summary.index.top(card) < summary.viewport.bottom() + height,
            "{path} is not near the view"
        );
    }
}

/// Moving the boundary between the sides in one card moves it in every card and is saved once
/// the user lets go, as in the file view.
#[test]
fn the_boundary_between_the_sides_moves_in_every_card() {
    let mut harness = harness(split_scene(ThemePreference::Light), SIZE);
    settle(&mut harness);
    scroll_to_card(&mut harness, PATH, 0.0);
    let row = row_of(harness.state(), PATH, None, Some(4));
    // The boundary is the left pane's right edge (its text area runs up to it); drag it 120 points.
    let left = view_of(harness.state(), PATH).text_area(Pane::Left, row).rect;
    let edge = egui::pos2(left.right(), left.top() + 4.0);
    harness.hover_at(edge);
    harness.step();
    harness.event(egui::Event::PointerButton {
        pos: edge,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    harness.step();
    for step in 1..=6 {
        harness.hover_at(edge + egui::vec2(20.0 * step as f32, 0.0));
        harness.step();
    }
    harness.event(egui::Event::PointerButton {
        pos: edge + egui::vec2(120.0, 0.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    settle(&mut harness);
    let share = harness.state().split;
    assert!(share > 0.55, "the old side grew: {share}");
    let moved = view_of(harness.state(), PATH).text_area(Pane::Left, row).rect.right();
    assert!(
        (moved - edge.x - 120.0).abs() < 2.0,
        "the boundary followed the pointer: {} to {moved}",
        edge.x
    );
    assert!(
        harness.state().events.iter().any(|event| matches!(event, SummaryEvent::SplitResized)),
        "letting go saves it"
    );
    // Another card shows the same boundary.
    let other = "src/app/tax.rs";
    scroll_to_card(&mut harness, other, 0.0);
    let scene = harness.state();
    let row = row_of(scene, other, None, Some(4));
    let boundary = view_of(scene, other).text_area(Pane::Left, row).rect.right();
    assert!(
        (boundary - moved).abs() < 1.0,
        "the other card's boundary is in the same place: {boundary} vs {moved}"
    );
}

/// 10,000 files with about 250,000 changed lines, side by side: every frame still looks at the
/// cards and rows on screen only, a fling loads a handful of diffs and the bodies stay capped; and
/// switching layouts in the middle of it costs one frame of the cards on screen.
#[test]
fn ten_thousand_files_side_by_side_cost_what_is_on_screen() {
    let files: Vec<ChangedFile> = (0..10_000)
        .map(|index| {
            let kind = [
                ChangeKind::Modified,
                ChangeKind::Modified,
                ChangeKind::Added,
                ChangeKind::Deleted,
            ][index % 4];
            let counts = match kind {
                ChangeKind::Added => (10 + (index % 30) as u32, 0),
                ChangeKind::Deleted => (0, 8 + (index % 20) as u32),
                _ => (8 + (index % 25) as u32, 4 + (index % 15) as u32),
            };
            file(
                &format!(
                    "src/area_{}/module_{}/file_{index:05}.rs",
                    index / 1000,
                    index / 100 % 10
                ),
                kind,
                Some(counts),
            )
        })
        .collect();
    let mut scene = Scene::new(files, ThemePreference::Light);
    scene.layout = DiffLayout::SideBySide;
    let mut harness = harness(scene, [1280.0, 900.0]);
    harness.run_steps(40);
    let mut most = (0, 0);
    let mut check = |harness: &Harness<'_, Scene>| {
        let summary = &harness.state().summary;
        let stats = summary.stats();
        most = (most.0.max(stats.cards_shown), most.1.max(stats.rows_painted));
        assert!(stats.wanted <= MAX_WANTED);
        assert!(summary.bodies_kept() <= MAX_BODIES, "{} bodies kept", summary.bodies_kept());
        assert!(summary.loads.running() <= MAX_RUNNING);
    };
    let before = harness.state().source.started.len();
    let middle = harness.state().summary.index.total() / 2.0;
    harness.state_mut().summary.scroll_to = Some(middle);
    harness.run_steps(40);
    check(&harness);
    let settled = harness.state().source.started.len() - before;
    assert!(settled < 30, "the screen it lands on loads: {settled}");

    let loads = harness.state().source.started.len();
    let started = std::time::Instant::now();
    harness.click_at(harness.get_by_label("Inline").rect());
    harness.run_steps(1);
    let switch = started.elapsed();
    check(&harness);
    harness.run_steps(10);
    check(&harness);
    assert_eq!(harness.state().source.started.len(), loads, "the switch loads nothing");
    println!(
        "10k files side by side: per frame <= {} cards, {} rows; switching took {switch:?} (debug build)",
        most.0, most.1
    );
    assert!(most.0 < 30 && most.1 < 120, "per frame at most {} cards and {} rows", most.0, most.1);
}

/// Regression: embedded folder diffs used to clip long lines without handling sideways input.
#[test]
fn split_cards_scroll_only_the_hovered_code_pane() {
    for (theme, size) in
        [(ThemePreference::Light, [760.0, 600.0]), (ThemePreference::Dark, [1180.0, 860.0])]
    {
        let mut harness = harness(split_scene(theme), size);
        settle(&mut harness);
        scroll_to_card(&mut harness, PATH, 0.0);
        let row = row_of(harness.state(), PATH, None, Some(4));
        let view = view_of(harness.state(), PATH);
        let left = view.text_area(Pane::Left, row).rect;
        let right = view.text_area(Pane::Right, row).rect;
        let before = [painted_code_x(&harness, left), painted_code_x(&harness, right)];
        let top = harness.state().summary.viewport.top();
        harness.hover_at(right.center());
        harness.step();
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(-120.0, 0.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Wheel,
        });
        harness.run_steps(20);
        assert!(
            painted_code_x(&harness, right) < before[1] - 50.0,
            "the right pane's code must move left to reveal the end of long lines"
        );
        assert!(
            (painted_code_x(&harness, left) - before[0]).abs() < 1.0,
            "the other pane stays put"
        );
        assert!(
            (harness.state().summary.viewport.top() - top).abs() < 1.0,
            "horizontal scrolling must not move the folder vertically"
        );
    }
}

/// Read the painted code origin, so the check fails if input changes state but not the screen.
fn painted_code_x(harness: &Harness<'_, Scene>, row: Rect) -> f32 {
    fn find(shape: &egui::Shape, row: Rect) -> Option<f32> {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, row)),
            egui::Shape::Text(text)
                if text.galley.mesh_bounds.width() > 50.0
                    && text.pos.y >= row.top() - 1.0
                    && text.pos.y < row.bottom() =>
            {
                Some(text.pos.x)
            }
            _ => None,
        }
    }
    harness
        .output()
        .shapes
        .iter()
        .filter(|shape| shape.clip_rect.intersect(row).width() > row.width() / 2.0)
        .find_map(|shape| find(&shape.shape, row))
        .expect("the code line was painted")
}

/// The floating bar remains reachable when the bottom of a tall file is outside the viewport.
#[test]
fn split_card_scrollbar_drags_at_the_visible_bottom() {
    let mut harness = harness(split_scene(ThemePreference::Light), [760.0, 600.0]);
    settle(&mut harness);
    scroll_to_card(&mut harness, PATH, 0.0);
    let row = row_of(harness.state(), PATH, None, Some(4));
    let right = view_of(harness.state(), PATH).text_area(Pane::Right, row).rect;
    let before = painted_code_x(&harness, right);
    let start = egui::pos2(right.left() + 12.0, harness.state().summary.screen.bottom() - 4.0);
    harness.hover_at(start);
    harness.step();
    harness.event(egui::Event::PointerButton {
        pos: start,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    harness.step();
    harness.hover_at(start + egui::vec2(60.0, 0.0));
    harness.step();
    harness.event(egui::Event::PointerButton {
        pos: start + egui::vec2(60.0, 0.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    settle(&mut harness);
    assert!(
        painted_code_x(&harness, right) < before - 40.0,
        "dragging the visible scrollbar reveals more code"
    );
}
