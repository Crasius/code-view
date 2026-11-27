//! The keyboard workflow, offline against the mocked GitHub Enterprise host
//! ([`super::github_mock`]).
//!
//! On a pull request Ctrl+1…4 (⌘1…4) pick the tabs and their tooltips name the keys; `?` opens
//! Settings at Keyboard and mouse with the page's shortcuts in view, while in a text field it is
//! typed. On a long list J and K move the keyboard focus from row to row, scrolling the row into
//! view, and Enter opens it.
//! `cargo test -p review-app ux_keys`

use egui::{Event, Key, Modifiers, Role, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::ReviewApp;
use super::github_mock::{GitHubHost, OWNER, REPOSITORY, open_pull_request, search_row};
use super::walkthrough::{app, type_into, wait_until};
use cv_base::platform::shortcut;
use cv_runtime::route::{PullRequestTab, Route};
use cv_testkit::ScratchDir;

#[test]
fn ux_keys_pick_tabs_and_open_the_shortcuts() {
    let scratch = ScratchDir::new("ux-keys-tabs");
    let host = GitHubHost::with_pull_request(scratch.path());
    let connection = host.connection("gh", "Acme");
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    open_pull_request(&mut harness);
    assert!(
        tab(&harness) == Some(PullRequestTab::Overview),
        "a pull request opens on the Overview"
    );

    harness.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    harness.run_steps(4);
    assert!(tab(&harness) == Some(PullRequestTab::Files), "Ctrl+2 shows the Files tab");
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Num4);
    harness.run_steps(4);
    assert!(tab(&harness) == Some(PullRequestTab::Pipelines), "Ctrl+4 shows the Pipelines tab");

    // The tooltip names the key.
    harness
        .get_by(|node| {
            node.role() == Role::Tab
                && node.label().is_some_and(|label| label.starts_with("Commits"))
        })
        .hover();
    harness.run_steps(40);
    let tooltip = concat!("Commits (", shortcut!(command "3"), ")");
    assert!(harness.query_by_label(tooltip).is_some(), "the tab's tooltip: {tooltip}");
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    harness.run_steps(4);
    assert!(tab(&harness) == Some(PullRequestTab::Overview), "Ctrl+1 goes back to the Overview");

    // `?` opens the one list of shortcuts, at the pull request's.
    harness.event(Event::Text("?".to_owned()));
    wait_until(&mut harness, "Keyboard and mouse", |harness| {
        harness.state().route == Route::Settings && harness.query_by_label("Pull request").is_some()
    });
    harness.run_steps(40);
    let heading = harness.get_by_label("Pull request").rect();
    let window = harness.ctx.content_rect();
    assert!(
        heading.top() > window.top() && heading.top() < window.top() + 200.0,
        "the pull request's shortcuts lead the view: {heading:?} in {window:?}"
    );

    // In a text field `?` is typed, and the page stays.
    harness.key_press_modifiers(Modifiers::ALT, Key::ArrowLeft);
    wait_until(&mut harness, "the pull request again", |harness| {
        matches!(harness.state().route, Route::PullRequest(_))
    });
    type_into(&mut harness, "Search", "?");
    harness.run_steps(4);
    assert!(matches!(harness.state().route, Route::PullRequest(_)), "the page stays");
    let field = harness.get_by_role(Role::SearchInput);
    assert_eq!(field.value().as_deref(), Some("?"), "the field took the question mark");
}

#[test]
fn ux_keys_move_through_the_list_rows() {
    let scratch = ScratchDir::new("ux-keys-list");
    let rows: Vec<_> = (1..=30)
        .map(|number| search_row(number, &format!("Change {number}"), REPOSITORY))
        .collect();
    let host = GitHubHost::start(OWNER, &rows);
    let connection = host.connection("gh", "Acme");
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    harness.set_size(Vec2::new(1440.0, 600.0));
    wait_until(&mut harness, "the rows", |harness| harness.query_by_label("1 Change 1").is_some());

    press(&mut harness, Key::J);
    assert!(is_focused(&harness, 1), "the first J focuses the first row");
    press(&mut harness, Key::J);
    assert!(is_focused(&harness, 2), "J moves to the next row");
    press(&mut harness, Key::K);
    assert!(is_focused(&harness, 1), "K moves back");

    // Far below the window: the list scrolls the row into view and it takes the keys.
    for _ in 0..19 {
        press(&mut harness, Key::J);
    }
    assert!(is_focused(&harness, 20), "J reaches a row that was off screen");
    let row = harness.get_by_label("20 Change 20").rect();
    let window = harness.ctx.content_rect();
    assert!(row.bottom() <= window.bottom() && row.top() >= window.top(), "{row:?} in view");

    press(&mut harness, Key::Enter);
    wait_until(
        &mut harness,
        "the pull request",
        |harness| matches!(&harness.state().route, Route::PullRequest(key) if key.number == 20),
    );
}

fn press(harness: &mut Harness<'_, ReviewApp>, key: Key) {
    harness.key_press(key);
    harness.run_steps(6);
}

fn is_focused(harness: &Harness<'_, ReviewApp>, number: u64) -> bool {
    harness.query_by_label(&format!("{number} Change {number}")).is_some_and(|row| row.is_focused())
}

fn tab(harness: &Harness<'_, ReviewApp>) -> Option<PullRequestTab> {
    harness.state().current_tab()
}
