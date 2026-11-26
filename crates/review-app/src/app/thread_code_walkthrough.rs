//! The code at the top of the overview's thread cards, end to end: the real GitHub provider
//! against the mocked host, whose pull request was force pushed after its first thread was
//! written. The current thread's card reads its lines from the mirror and grows on "Show more";
//! the outdated thread's card shows the version it was written on, in Diff with the removed line,
//! and its line numbers lead to that version in the Files tab. No network, no real token.

use std::fmt::Write as _;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::ReviewApp;
use super::action_e2e::{app, open_pull_request, press, wait_until};
use super::clicks::Clicks;
use super::github_mock::{CURRENT_LINE, GitHubHost};
use cv_testkit::ScratchDir;

/// The new head: forty lines of documentation over the renamed greeting, so the current thread on
/// line 20 has lines to show on both sides.
fn rewritten_file() -> String {
    let mut text = (1..=40).fold(String::new(), |mut text, line| {
        writeln!(text, "/// Line {line} of the greeting's documentation.").unwrap();
        text
    });
    text.push_str("pub fn greeting() -> &'static str {\n    \"Hello, world\"\n}\n");
    text
}

#[test]
fn thread_cards_show_their_code_from_the_mirror() {
    let scratch = ScratchDir::new("thread-code");
    let mut host = GitHubHost::with_pull_request(scratch.path());
    let replaced = host.force_push_outdating_the_thread(&rewritten_file(), true);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);

    // The current thread: its code loads once its card is on screen, in New only, its line marked
    // and three lines on each side.
    let location = format!("src/lib.rs:{CURRENT_LINE}");
    wait_until(&mut harness, "the current thread", |harness| {
        harness.query_by_label(&location).is_some()
    });
    harness.get_by_label(&location).scroll_to_me();
    settle(&mut harness);
    let commented = format!("Changed line {CURRENT_LINE}, commented: /// Line {CURRENT_LINE} of");
    wait_until(&mut harness, "the current thread's code", |harness| {
        harness.query_by_label_contains(&commented).is_some()
    });
    assert!(harness.query_by_label("Lines 17–23").is_some(), "three lines on each side");
    press(&mut harness, "Show 10 more lines above");
    settle(&mut harness);
    assert!(
        harness.query_by_label_contains("Changed line 7: /// Line 7 of").is_some(),
        "ten more lines above"
    );

    // The outdated thread: the version it was written on, where line 1 replaced the old greeting.
    let version = format!("Version {} · ", &replaced[..7]);
    wait_until(&mut harness, "the outdated thread's code", |harness| {
        harness.query_by_label_contains(&version).is_some()
            && harness
                .query_by_label_contains("Changed line 1, commented: pub fn greeting()")
                .is_some()
    });
    let diff = harness.get_all_by_label("Diff").next().expect("the outdated card's switch");
    diff.scroll_to_me();
    settle(&mut harness);
    let target =
        harness.get_all_by_label("Diff").next().expect("the outdated card's switch").rect();
    harness.click_at(target);
    settle(&mut harness);
    assert_eq!(
        harness.query_all_by_label_contains("Removed line 1: pub fn greet() ->").count(),
        2,
        "Diff shows the line the change removed, on the current card too: it follows the pick"
    );

    // Its line numbers open the version it was written on in the Files tab.
    press(&mut harness, "Show line 1 in the file");
    let banner = format!("Showing the version from {}", &replaced[..7]);
    wait_until(&mut harness, "the original version", |harness| {
        harness.query_by_label_contains(&banner).is_some()
    });
}

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    for _ in 0..20 {
        harness.step();
    }
}
