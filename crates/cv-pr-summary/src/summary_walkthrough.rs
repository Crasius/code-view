//! The folder summary on its own, with a fake diff source that counts its loads: the root and a
//! folder, added and deleted files, a collapsed card, the large-file placeholder and the keyboard's
//! View; that only the cards near the view load; and a 10,000-file pull request with over 200,000
//! changed lines that costs what is on screen. No network, no tokens.

use cv_testkit::Clicks;
use egui::{
    Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, TouchPhase, ViewportId, pos2, vec2,
};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_git::{ChangeKind, ChangedFile};
use review_store::{DiffLayout, ThemePreference};

use super::SummaryEvent;
use super::bodies::MAX_BODIES;
use super::loads::{MAX_RUNNING, MAX_WANTED};
use super::testing::{
    SIZE, Scene, card_of, cards_shown, file, harness, key, settle, shop_files, ten_thousand_files,
};
use cv_runtime::background::Background;
use cv_testkit::frame_bench::{Bench, PATTERNS, SCALES};

#[test]
fn folder_summary_walkthrough() {
    let mut scene = Scene::new(shop_files(), ThemePreference::Light);
    scene.thread_counts.insert("src/app/checkout.rs".into(), 2);
    let mut harness = harness(scene, SIZE);
    settle(&mut harness);
    assert!(harness.query_by_label("9 changed files").is_some());
    assert!(harness.query_by_label("View src/app/cart.rs").is_some());
    assert!(
        harness.query_by_label("Show here data/fixtures.json").is_some(),
        "the large file waits for a click"
    );
    assert!(
        harness.query_by_label("Open file assets/logo.png").is_some(),
        "a binary file leads to its file view"
    );
    assert!(harness.query_by_label("Open file docs/guide.md").is_some(), "a pure rename too");
    let started = &harness.state().source.started;
    assert!(
        !started.iter().any(|path| path == "data/fixtures.json"
            || path == "assets/logo.png"
            || path == "docs/guide.md"),
        "placeholders load nothing: {started:?}"
    );

    harness.state_mut().open("src/app");
    settle(&mut harness);
    assert!(harness.query_by_label("4 changed files").is_some());

    // n moves to the cards one by one; the third and fourth are the deleted and added files.
    for _ in 0..3 {
        key(&mut harness, Key::N);
    }
    settle(&mut harness);
    assert_eq!(harness.state().summary.current, Some(2));

    key(&mut harness, Key::Enter);
    let index =
        harness.state().files.iter().position(|file| file.path == "src/app/legacy_payment.rs");
    assert!(
        matches!(harness.state().events.last(), Some(SummaryEvent::View(opened)) if Some(*opened) == index),
        "Enter opens the current card"
    );

    key(&mut harness, Key::Home);
    harness.click_at(harness.get_by_label("Collapse src/app/cart.rs").rect());
    settle(&mut harness);
    assert!(harness.query_by_label("Expand src/app/cart.rs").is_some());

    harness.click_at(harness.get_by_label("View src/app/checkout.rs").rect());
    harness.run_steps(2);
    let checkout = harness.state().files.iter().position(|file| file.path == "src/app/checkout.rs");
    assert!(
        matches!(harness.state().events.last(), Some(SummaryEvent::View(opened)) if Some(*opened) == checkout),
        "View opens the file view"
    );

    harness.click_at(harness.get_by_label("All files").rect());
    harness.run_steps(2);
    harness.state_mut().open("data");
    settle(&mut harness);
    harness.click_at(harness.get_by_label("Show here data/fixtures.json").rect());
    settle(&mut harness);
    assert!(
        harness.state().source.started.iter().any(|path| path == "data/fixtures.json"),
        "Show here loads it"
    );
}

/// The wait of every walkthrough here fails in place, with what is still loading, instead of
/// giving up silently and leaving a later assertion to fail.
#[test]
#[should_panic(expected = "400 frames did not settle the summary")]
fn settle_fails_when_a_load_never_finishes() {
    let mut scene = Scene::new(shop_files(), ThemePreference::Light);
    scene.source.hangs_on = Some(Background::new(egui::Context::default()));
    let mut harness = harness(scene, SIZE);
    settle(&mut harness);
}

#[test]
fn collapse_lasts_for_the_session_and_is_exactly_the_header() {
    let mut harness = harness(Scene::new(shop_files(), ThemePreference::Light), SIZE);
    settle(&mut harness);
    harness.click_at(harness.get_by_label("Collapse src/app/cart.rs").rect());
    harness.run_steps(3);
    let card = card_of(harness.state(), "src/app/cart.rs");
    let summary = &harness.state().summary;
    let geometry = summary.geometry.expect("laid out");
    assert_eq!(
        summary.index.bottom(card) - summary.index.top(card),
        geometry.header + geometry.gap
    );
    // Another folder and back: still collapsed.
    harness.state_mut().open("src");
    harness.run_steps(3);
    harness.state_mut().open("");
    harness.run_steps(3);
    assert!(harness.query_by_label("Expand src/app/cart.rs").is_some());
}

/// Loads stay with the cards on screen and one screen around them, nothing loads while the
/// window is in the background or the line counts are not there yet, and the request budget for
/// opening a summary is the cards near the view, not the folder.
#[test]
fn only_the_cards_near_the_view_load() {
    let files: Vec<ChangedFile> = (0..400)
        .map(|index| {
            file(
                &format!("src/m{:02}/f{index:04}.rs", index / 20),
                ChangeKind::Modified,
                Some((12, 8)),
            )
        })
        .collect();
    let mut scene = Scene::new(files, ThemePreference::Light);
    scene.can_load = false;
    let mut harness = harness(scene, [1100.0, 800.0]);
    harness.run_steps(10);
    assert!(harness.state().source.started.is_empty(), "nothing loads before the line counts");

    harness.state_mut().can_load = true;
    if let Some(viewport) = harness.input_mut().viewports.get_mut(&ViewportId::ROOT) {
        viewport.focused = Some(false);
    }
    harness.run_steps(10);
    assert!(
        harness.state().source.started.is_empty(),
        "nothing loads while the window is in the background"
    );

    if let Some(viewport) = harness.input_mut().viewports.get_mut(&ViewportId::ROOT) {
        viewport.focused = Some(true);
    }
    settle(&mut harness);
    let assert_near_view = |harness: &Harness<'_, Scene>, started: &[String]| {
        let summary = &harness.state().summary;
        let (top, bottom) = (summary.viewport.top(), summary.viewport.bottom());
        let height = summary.viewport.height();
        for path in started {
            let card = card_of(harness.state(), path);
            let (card_top, card_bottom) = (summary.index.top(card), summary.index.bottom(card));
            assert!(
                card_bottom > top - height && card_top < bottom + height,
                "{path} at {card_top}..{card_bottom} is beyond one screen around {top}..{bottom}"
            );
        }
    };
    let first = harness.state().source.started.clone();
    let on_screen = cards_shown(&harness.state().summary);
    assert!(first.len() >= on_screen, "every card on screen loads: {} of {on_screen}", first.len());
    assert!(
        first.len() <= MAX_WANTED && first.len() < 20,
        "only the cards near the view: {}",
        first.len()
    );
    assert_near_view(&harness, &first);

    // A jump to the end loads the end, once the view has settled there.
    key(&mut harness, Key::End);
    settle(&mut harness);
    let started = harness.state().source.started.clone();
    let after_jump = &started[first.len()..];
    assert_ne!(after_jump, [] as [String; 0]);
    assert_near_view(&harness, after_jump);
    assert!(
        started.len() < 40,
        "a jump loads the screen it lands on, not the way there: {}",
        started.len()
    );
}

/// 10,000 files with about 250,000 changed lines (and one 20,000-line file): every frame looks at
/// the cards and rows on screen only, a fling through all of it loads a handful of diffs, and the
/// bodies kept never pass their cap.
#[test]
fn ten_thousand_files_cost_what_is_on_screen() {
    let mut files = ten_thousand_files();
    // The fourth file becomes the one with 20,000 added lines.
    files[3].additions = Some(20_000);
    files[3].kind = ChangeKind::Added;
    let total: u64 = files
        .iter()
        .map(|file| u64::from(file.additions.unwrap_or(0) + file.deletions.unwrap_or(0)))
        .sum();
    assert!(total > 200_000, "{total} changed lines");

    let mut harness = harness(Scene::new(files, ThemePreference::Light), [1280.0, 900.0]);
    settle(&mut harness);
    let (mut most_cards, mut most_rows) = (0, 0);
    let mut check = |harness: &Harness<'_, Scene>| {
        let summary = &harness.state().summary;
        most_cards = most_cards.max(cards_shown(summary));
        most_rows = most_rows.max(summary.rows_painted);
        assert!(summary.loads.wanted().len() <= MAX_WANTED);
        assert!(summary.bodies_kept() <= MAX_BODIES, "{} bodies kept", summary.bodies_kept());
        assert!(summary.loads.running() <= MAX_RUNNING);
    };
    // The 20,000-line card fills the screen: it paints the rows in view, not its 20,000.
    let inside = harness.state().summary.index.top(3) + 5_000.0;
    harness.state_mut().summary.scroll_to = Some(inside);
    settle(&mut harness);
    check(&harness);
    let summary = &harness.state().summary;
    let (shown, rows) = (cards_shown(summary), summary.rows_painted);
    assert!(shown == 1 && rows <= 50, "inside the tall card: {shown} cards, {rows} rows");

    let before = harness.state().source.started.len();
    let pointer = pos2(640.0, 500.0);
    harness.input_mut().events.push(Event::PointerMoved(pointer));
    let wheel = |delta: f32, phase: TouchPhase| Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: vec2(0.0, delta),
        phase,
        modifiers: Modifiers::NONE,
        source: MouseWheelSource::Wheel,
    };
    harness.input_mut().events.push(wheel(0.0, TouchPhase::Start));
    for _ in 0..240 {
        harness.input_mut().events.push(wheel(-6_000.0, TouchPhase::Move));
        harness.run_steps(1);
        check(&harness);
    }
    harness.input_mut().events.push(wheel(0.0, TouchPhase::End));
    harness.run_steps(1);
    let during = harness.state().source.started.len() - before;
    let travelled = harness.state().summary.viewport.top();
    assert!(travelled > 200_000.0, "the fling went far: {travelled}");
    assert!(during <= 2 * MAX_RUNNING, "a fling through {travelled} points loaded {during} diffs");

    harness.run_steps(40);
    check(&harness);
    let settled = harness.state().source.started.len() - before - during;
    assert!(settled > 0 && settled < 20, "once it settles, the screen loads: {settled}");
    assert!(
        most_cards < 30 && most_rows < 120,
        "per frame at most {most_cards} cards and {most_rows} rows"
    );
}

/// The summary of the 10,000-file pull request at 4K, inline and side by side, scrolled at the
/// standard patterns.
#[test]
#[ignore = "release-mode frame budget benchmark; see cv_testkit::frame_bench"]
fn frame_budget_summary() {
    let files: Vec<ChangedFile> = (0..10_000)
        .map(|index| {
            file(
                &format!(
                    "src/area_{}/module_{}/file_{index:05}.rs",
                    index / 1000,
                    index / 100 % 10
                ),
                ChangeKind::Modified,
                Some((8 + (index % 25) as u32, 4 + (index % 15) as u32)),
            )
        })
        .collect();
    let mut reports = Vec::new();
    for (layout, layout_name) in
        [(DiffLayout::Inline, "inline"), (DiffLayout::SideBySide, "side by side")]
    {
        for pixels_per_point in SCALES {
            let mut scene = Scene::new(files.clone(), ThemePreference::Light);
            scene.layout = layout;
            let mut bench =
                Bench::new(pixels_per_point, scene, |ui, scene: &mut Scene| scene.show(ui));
            let pointer = bench.ctx.content_rect().center();
            bench.warm_up(pointer);
            for (pattern, speed) in PATTERNS {
                let name = format!(
                    "summary 10k files {layout_name} @{:.0}% {pattern}",
                    pixels_per_point * 100.0
                );
                reports.push(bench.scroll_best(&name, pointer, vec2(0.0, -speed)));
            }
            println!(
                "loads {layout_name} at {:.0}%: {}",
                pixels_per_point * 100.0,
                bench.state.source.started.len()
            );
        }
    }
    for report in &reports {
        report.print();
    }
    for report in &reports {
        report.assert_within_budget();
    }
}
