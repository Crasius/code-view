//! The keyboard workflow, offline against the mocked GitHub Enterprise host
//! ([`super::github_mock`]).
//!
//! On a pull request Ctrl+1…4 (⌘1…4) pick the tabs and their tooltips name the keys; `?` opens
//! Settings at Keyboard and mouse with the page's shortcuts in view, while in a text field it is
//! typed. On a long list J and K move the keyboard focus from row to row, scrolling the row into
//! view, and Enter opens it.
//! `cargo test -p review-app --bin CodeView app::keyboard_walkthrough`

use egui::{Event, Key, Modifiers, Role, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_store::PullRequestTab;

use super::ReviewApp;
use super::github_mock::{GitHubHost, OWNER, REPOSITORY, open_pull_request, search_row};
use super::walkthrough::{app, hover_until, type_into, wait_until, wait_until_still};
use cv_base::platform::shortcut;
use cv_runtime::route::Route;
use cv_testkit::ScratchDir;

#[test]
fn pick_tabs_and_open_the_shortcuts() {
    let scratch = ScratchDir::new("keyboard-tabs");
    let host = GitHubHost::with_pull_request(scratch.path());
    let connection = host.connection("gh", "Acme");
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    open_pull_request(&mut harness);
    assert_eq!(
        tab(&harness),
        Some(PullRequestTab::Overview),
        "a pull request opens on the Overview"
    );

    pick_tab(&mut harness, Key::Num2, PullRequestTab::Files);
    pick_tab(&mut harness, Key::Num4, PullRequestTab::Pipelines);

    // The tooltip names the key.
    let commits = harness
        .get_by(|node| {
            node.role() == Role::Tab
                && node.label().is_some_and(|label| label.starts_with("Commits"))
        })
        .accesskit_node()
        .label()
        .expect("the Commits tab's label");
    let tooltip = concat!("Commits (", shortcut!(command "3"), ")");
    hover_until(&mut harness, Role::Tab, &commits, "the tab's tooltip", |harness| {
        harness.query_by_label(tooltip).is_some()
    });
    pick_tab(&mut harness, Key::Num1, PullRequestTab::Overview);

    // `?` opens the one list of shortcuts, at the pull request's.
    harness.event(Event::Text("?".to_owned()));
    wait_until(&mut harness, "Keyboard and mouse", |harness| {
        harness.state().route == Route::Settings && harness.query_by_label("Pull request").is_some()
    });
    let heading = wait_until_still(&mut harness, "the pull request's shortcuts", |harness| {
        harness.query_by_label("Pull request").map(|node| node.rect())
    });
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
    assert!(matches!(harness.state().route, Route::PullRequest(_)), "the page stays");
    let field = harness.get_by_role(Role::SearchInput);
    assert_eq!(field.value().as_deref(), Some("?"), "the field took the question mark");
}

#[test]
fn move_through_the_list_rows() {
    let scratch = ScratchDir::new("keyboard-list");
    let rows: Vec<_> = (1..=30)
        .map(|number| search_row(number, &format!("Change {number}"), REPOSITORY))
        .collect();
    let host = GitHubHost::start(OWNER, &rows);
    let connection = host.connection("gh", "Acme");
    let mut harness = app(&scratch, std::slice::from_ref(&connection));
    harness.set_size(Vec2::new(1440.0, 600.0));
    wait_until(&mut harness, "the rows", |harness| harness.query_by_label("1 Change 1").is_some());

    press(&mut harness, Key::J, 1, "the first J focusing the first row");
    press(&mut harness, Key::J, 2, "J moving to the next row");
    press(&mut harness, Key::K, 1, "K moving back");

    // Far below the window: the list scrolls the row into view and it takes the keys.
    for number in 2..=20 {
        press(&mut harness, Key::J, number, "J reaching a row that was off screen");
    }
    let row = wait_until_still(&mut harness, "row 20 scrolled into view", |harness| {
        harness.query_by_label("20 Change 20").map(|node| node.rect())
    });
    let window = harness.ctx.content_rect();
    assert!(row.bottom() <= window.bottom() && row.top() >= window.top(), "{row:?} in view");

    harness.key_press(Key::Enter);
    wait_until(
        &mut harness,
        "the pull request",
        |harness| matches!(&harness.state().route, Route::PullRequest(key) if key.number == 20),
    );
}

/// Ctrl+`key` (⌘ on macOS) and the wait for the pull request's tab `shown`.
fn pick_tab(harness: &mut Harness<'_, ReviewApp>, key: Key, shown: PullRequestTab) {
    harness.key_press_modifiers(Modifiers::COMMAND, key);
    wait_until(harness, &format!("Ctrl+{key:?} to show the {shown:?} tab"), |harness| {
        tab(harness) == Some(shown)
    });
}

/// Presses `key` and waits until the row of pull request `number` has the keyboard focus (`what`).
fn press(harness: &mut Harness<'_, ReviewApp>, key: Key, number: u64, what: &str) {
    harness.key_press(key);
    wait_until(harness, what, |harness| is_focused(harness, number));
}

fn is_focused(harness: &Harness<'_, ReviewApp>, number: u64) -> bool {
    harness.query_by_label(&format!("{number} Change {number}")).is_some_and(|row| row.is_focused())
}

fn tab(harness: &Harness<'_, ReviewApp>) -> Option<PullRequestTab> {
    harness.state().current_tab()
}
