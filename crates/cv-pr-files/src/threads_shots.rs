//! Threads inline in the diff (T1) and a new thread on picked lines (T2): the marked lines with
//! their accent bar, the gutter markers, the card with a suggested change and an open reply, a
//! resolved thread collapsed to one line, and the new-thread composer with its kept draft.

use cv_testkit::Clicks;
use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{ReviewThread, ThreadStatus};

use cv_base::theme;
use cv_code::diff_rows::Layout;
use cv_code::line_source::Pane;
use cv_pr_diff::testing::{self as check, Fixture, NEW, PATH, line_of, settle};
use cv_pr_threads::samples::{self, comment, erika, jonas, lines, minutes_ago};

fn click(harness: &mut Harness<'_, Fixture>, pos: Pos2, modifiers: Modifiers) {
    harness.hover_at(pos);
    harness.event(Event::ModifiersChanged(modifiers));
    harness.event(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed: true,
        modifiers,
    });
    harness.event(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed: false,
        modifiers,
    });
    harness.event(Event::ModifiersChanged(Modifiers::NONE));
    harness.step();
}

/// The discussion on `total`, a resolved thread on the end of `add` and a fresh one on `add`.
fn threads() -> Vec<ReviewThread> {
    let total = line_of(NEW, "pub fn total(");
    let mut discussion = samples::discussion("t-total", lines(PATH, total, total + 2));
    discussion.comments.truncate(2);
    let add_end = total - 2;
    let resolved = samples::resolved("t-empty", Some(lines(PATH, add_end, add_end)));
    let add = line_of(NEW, "pub fn add(");
    let fresh = samples::thread(
        "t-add",
        ThreadStatus::Active,
        Some(lines(PATH, add, add)),
        vec![comment("t-add-1", &jonas(), 9, "Callers in `checkout` still pass two arguments.")],
    );
    vec![fresh, discussion, resolved]
}

fn harness() -> Harness<'static, Fixture> {
    let mut fixture = check::fixture(Layout::SideBySide);
    fixture.threads = threads();
    let threads = &mut fixture.threads_ui;
    threads.context.author_id = Some(jonas().id);
    threads.context.web_url = Some(samples::PULL_REQUEST_URL.into());
    threads.set_me(erika());
    threads.visited_at(minutes_ago(30).unix_timestamp());
    Harness::builder()
        .with_size([1320.0, 1240.0])
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::motion::set_reduced(ctx, false);
        })
        .build_ui_state(
            |ui, state: &mut Fixture| {
                theme::activate(ui.ctx(), state.theme);
                check::show_diff(ui, state);
            },
            fixture,
        )
}

#[test]
fn threads_inline_in_the_diff() {
    let mut harness = harness();
    harness
        .state_mut()
        .threads_ui
        .open_reply_for_tests("t-total", "Agreed. A test for an empty cart would be good too.");
    harness.state_mut().view.reveal_thread("t-add");
    harness.run_steps(12);
    assert_eq!(
        harness.query_all_by_label("Status: Active").count(),
        2,
        "the status pill is a menu"
    );
    assert!(
        harness.query_by_label_contains("Resolved thread: ").is_some(),
        "the resolved thread is one line"
    );
    assert!(
        harness.query_by_label("Reply & resolve").is_some(),
        "the open reply can resolve the thread too"
    );
    assert!(
        harness.query_by_label_contains("Resolved thread by Erika Brandt").is_some(),
        "its gutter marker is a check"
    );
    assert!(
        harness.query_by_label_contains("Thread by Jonas Weber").is_some(),
        "an active thread's marker is the avatar"
    );
    assert!(
        harness.query_by_label("Apply as commit").is_none(),
        "no platform API applies a suggestion"
    );

    // A click on the marker collapses the card, another opens it again.
    harness
        .click_at(harness.get_by_label_contains("Thread by Jonas Weber, click to collapse").rect());
    harness.run_steps(4);
    assert!(harness.query_by_label_contains("Thread by Jonas Weber, click to show").is_some());
}

#[test]
fn a_new_thread_on_picked_lines_keeps_its_draft() {
    let mut harness = harness();
    harness.state_mut().threads.clear();
    settle(&mut harness);
    let first = line_of(NEW, "pub fn apply_discount(");
    let last = first + 3;
    let row = |harness: &Harness<'_, Fixture>, line: u32| {
        let state = harness.state();
        state.view.row_of(&state.diff, None, Some(line))
    };
    let (first_row, last_row) = (row(&harness, first), row(&harness, last));
    let first_number = harness.state().view.gutter_point(Pane::Right, first_row);
    let last_number = harness.state().view.gutter_point(Pane::Right, last_row);
    click(&mut harness, first_number, Modifiers::NONE);
    click(&mut harness, last_number, Modifiers::SHIFT);
    harness.click_at(harness.get_by_label(&format!("Comment on lines {first}–{last}")).rect());
    settle(&mut harness);
    harness
        .event(Event::Text("Lowercasing here and in `apply` twice? One place is enough.".into()));
    settle(&mut harness);
    assert!(harness.query_by_label("Draft saved").is_some(), "the draft is kept while typing");
    assert_eq!(harness.state().threads_ui.new_thread_drafts(PATH).len(), 1, "kept with its lines");
}
