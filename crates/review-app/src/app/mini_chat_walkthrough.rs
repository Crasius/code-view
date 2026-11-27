//! The assistant's mini chat end to end: the real app against the mocked GitHub host
//! ([`super::github_mock`]) with a scripted agent. With "Keep the answer beside the code" on, a
//! citation opens the Files tab at its lines with the mini chat in the bottom-right corner; ▶ and
//! Alt+. step the diff through the references, a reference to a file the pull request does not
//! change shows its notice and "Skip" goes on; the card fades while it covers the cited lines and
//! not under the pointer; it shrinks to a pill and opens again in its corner; a drag released near
//! the bottom-left corner snaps there and is kept; a follow-up goes into the same conversation and
//! its answer replaces the old one; it asks for frames only while something of it moves. Off, a
//! citation only opens the file (on by default; the toggle turns it off). Asserts state and
//! geometry only. Needs `git` on PATH, no network, no real CLI.
//! `cargo test -p review-app mini_chat`

use egui::{Event, Key, Modifiers, PointerButton, Pos2, Role, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_agent::FakeAgent;

use super::ReviewApp;
use super::assistant_script::{
    answer_text, ask, install_claude, long_file, wait_for_prompts, wait_for_session,
};
use super::github_mock::{ExtraFile, GitHubHost, open_pull_request};
use super::walkthrough::{app, click_now, shows_file_view, type_into, wait_for_link, wait_until};
use cv_pr_assistant::mini::{
    self, ANSWERING, CARD_LABEL, FOLLOW_UP, NEXT, OPEN, OPEN_AT_HEAD, PREVIOUS, SHRINK, SKIP_NEXT,
};
use cv_pr_diff::diff_view::flashed_lines;
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

const LONG: &str = "src/long.rs";
/// In the checkout, not changed by the pull request.
const NOTES: &str = "docs/notes.md";
/// Four references, each alone in its paragraph so a click on the paragraph's middle hits it.
const ANSWER: &str = "The greeting:\n\nsrc/lib.rs:2\n\nThe long file:\n\nsrc/long.rs:60-62\n\n\
                      The notes:\n\ndocs/notes.md:2\n\nAnd its end:\n\nsrc/long.rs:100";
pub(super) const FIRST: &str = "src/lib.rs:2";

/// The pull request with the long file changed and the notes unchanged, its answer to the first
/// question in the Overview's card, the mini chat on or off.
pub(super) fn answered(
    name: &str,
    mini_chat: bool,
) -> (Harness<'static, ReviewApp>, FakeAgent, ScratchDir) {
    let scratch = ScratchDir::new(name);
    let notes = b"# Notes\nSee the greeting.\nNothing else.\n".to_vec();
    let files = [
        ExtraFile { path: LONG, base: long_file(false), head: long_file(true) },
        ExtraFile { path: NOTES, base: notes.clone(), head: notes },
    ];
    let host = GitHubHost::with_pull_request_files(scratch.path(), &files);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    install_claude(&harness, true);
    harness.state_mut().services.settings.assistant.mini_chat = mini_chat;
    open_pull_request(&mut harness);
    ask(&mut harness, "Where is the greeting?");
    let (_, mut agent) = wait_for_session(&mut harness);
    wait_for_prompts(&mut harness, &mut agent);
    answer_text(&agent, ANSWER);
    wait_for_link(&mut harness, FIRST);
    (harness, agent, scratch)
}

/// The whole tour with the mini chat on.
#[test]
fn mini_chat_keeps_the_answer_beside_the_code() {
    let (mut harness, mut agent, _scratch) = answered("mini-chat-on", true);
    let room = open_from_the_citation(&mut harness);
    step_and_fade(&mut harness, room);
    outside_the_diff(&mut harness);
    pill_and_back(&mut harness);
    drag_to_a_corner(&mut harness, room);
    follow_up(&mut harness, &mut agent);
}

/// The first citation opens its file with the mini chat in the bottom-right corner of the Files
/// tab's content, which is returned.
fn open_from_the_citation(harness: &mut Harness<'_, ReviewApp>) -> egui::Rect {
    let center = harness.get_by_label(FIRST).rect().center();
    click_now(harness, center);
    wait_until(harness, "the cited file with the mini chat", |harness| {
        shows_file_view(harness, "src/lib.rs") && has_card(harness)
    });
    assert!(harness.query_by_label("Reference 1 of 4").is_some());
    let room =
        cv_pr_session::assistant_link::last_room(&harness.ctx).expect("the Files tab's content");
    let shown = drawn(harness).rect;
    assert!(room.contains_rect(shown), "inside the content, off the tree and the tabs");
    assert_near(shown.max, room.max - egui::vec2(16.0, 16.0), "the bottom-right corner");
    assert!(is_disabled(harness, PREVIOUS), "no reference before the first");
    room
}

/// ▶ moves the diff to the next reference's file and lines; over the cited lines the card fades,
/// unless the pointer is on it.
fn step_and_fade(harness: &mut Harness<'_, ReviewApp>, room: egui::Rect) {
    harness.click_at(harness.get_by_label(NEXT).rect());
    wait_until(harness, "the second reference", |harness| {
        harness.query_by_label("Reference 2 of 4").is_some()
            && harness.query_by_label(&format!("Diff of {LONG}")).is_some()
            && flashed_lines(&harness.ctx).iter().any(|(_, line, _)| *line == 60)
    });

    let pointer_away = room.left_top() + egui::vec2(40.0, 40.0);
    harness.hover_at(pointer_away);
    wait_until(harness, "the covering fade", |harness| drawn(harness).opacity < 0.5);
    harness.hover_at(drawn(harness).rect.center());
    wait_until(harness, "full opacity under the pointer", |harness| {
        (drawn(harness).opacity - 1.0).abs() < 1e-3
    });
}

/// Alt+. reaches the notes, which the pull request does not change: a notice, never a dead end;
/// "Skip" goes on to the last reference.
fn outside_the_diff(harness: &mut Harness<'_, ReviewApp>) {
    harness.key_press_modifiers(Modifiers::ALT, Key::Period);
    let notice = format!("{NOTES} is not changed in this pull request.");
    wait_until(harness, "the notice", |harness| {
        harness.query_by_label("Reference 3 of 4").is_some()
            && harness.query_by_label(&notice).is_some()
    });
    assert!(harness.query_by_label(OPEN_AT_HEAD).is_some());
    assert!(
        harness.query_by_label(&format!("Diff of {LONG}")).is_some(),
        "the diff stays where it was"
    );
    harness.click_at(harness.get_by_label(SKIP_NEXT).rect());
    wait_until(harness, "the last reference", |harness| {
        harness.query_by_label("Reference 4 of 4").is_some()
            && harness.query_by_label(&notice).is_none()
            && flashed_lines(&harness.ctx).iter().any(|(_, line, _)| *line == 100)
    });
    assert!(is_disabled(harness, NEXT), "no reference after the last");
}

/// The pill keeps the arrows, then opens again as the card in the same corner.
fn pill_and_back(harness: &mut Harness<'_, ReviewApp>) {
    let card_rect = drawn(harness).rect;
    harness.click_at(harness.get_by_label(SHRINK).rect());
    wait_until(harness, "the pill", |harness| {
        let shown = drawn(harness);
        shown.is_pill && (shown.rect.height() - 44.0).abs() < 0.5
    });
    assert!(harness.query_by_label("Assistant · 4 of 4").is_some());
    assert_near(drawn(harness).rect.max, card_rect.max, "the pill stays in the corner");
    harness.key_press_modifiers(Modifiers::ALT, Key::Comma);
    wait_until(harness, "a step in the pill", |harness| {
        harness.query_by_label("Assistant · 3 of 4").is_some()
    });
    harness.click_at(harness.get_by_label(OPEN).rect());
    wait_until(harness, "the card again", |harness| {
        let shown = drawn(harness);
        !shown.is_pill && (shown.rect.height() - card_rect.height()).abs() < 0.5
    });
    assert_near(drawn(harness).rect.max, card_rect.max, "the same corner");
}

/// Dragged by its title line and released near the bottom-left corner, it snaps there and the
/// corner is kept; once still, it asks for no frames.
fn drag_to_a_corner(harness: &mut Harness<'_, ReviewApp>, room: egui::Rect) {
    let grip = drawn(harness).rect.left_top() + egui::vec2(60.0, 32.0);
    let release = pos2(room.left() + 200.0, room.bottom() - 120.0);
    drag(harness, grip, release);
    wait_until(harness, "the snap", |harness| {
        let shown = drawn(harness).rect;
        (shown.left() - (room.left() + 16.0)).abs() < 0.5
            && (shown.bottom() - (room.bottom() - 16.0)).abs() < 0.5
    });
    let corner = harness.state().services.settings.assistant.mini_chat_corner;
    assert!(corner.left && !corner.top, "the corner is kept");
    harness.run_steps(4);
    assert!(!mini::asks_for_frames(&harness.ctx), "a still mini chat asks for no frames");
}

/// A follow-up goes into the same conversation; while it runs the dot pulses and Send is Stop; its
/// answer replaces the old one.
fn follow_up(harness: &mut Harness<'_, ReviewApp>, agent: &mut FakeAgent) {
    type_into(harness, FOLLOW_UP, "And the farewell?");
    harness.key_press(Key::Enter);
    let asked = wait_for_prompts(harness, agent);
    assert_eq!(asked, [Some("And the farewell?".to_owned())], "the same agent's session");
    wait_until(harness, "the running turn", |harness| {
        harness.query_by(|node| node.placeholder() == Some(ANSWERING)).is_some()
            && harness.query_by_role_and_label(Role::Button, "Stop").is_some()
    });
    harness.run_steps(2);
    assert!(mini::asks_for_frames(&harness.ctx), "the working dot pulses");
    answer_text(agent, "Here:\n\nsrc/lib.rs:5");
    wait_until(harness, "the new answer", |harness| {
        harness.query_by_label("And the farewell?").is_some()
            && harness.query_by_label("1 reference").is_some()
            && harness.query_by_label("src/long.rs:100").is_none()
    });
}

/// Off, a citation opens the file and no mini chat; the chip menu turns it on again.
#[test]
fn mini_chat_off_leaves_the_citation_as_it_was() {
    let (mut harness, _agent, _scratch) = answered("mini-chat-off", false);
    harness.click_at(harness.get_by_label_contains("Claude · ").rect());
    wait_until(&mut harness, "the chip's menu", |harness| {
        harness.query_by_label("Keep the answer beside the code").is_some()
    });
    harness.key_press(Key::Escape);
    harness.run_steps(4);
    let center = harness.get_by_label(FIRST).rect().center();
    click_now(&mut harness, center);
    wait_until(&mut harness, "the cited file", |harness| shows_file_view(harness, "src/lib.rs"));
    harness.run_steps(4);
    assert!(!has_card(&harness) && mini::drawn(&harness.ctx).is_none(), "no mini chat");

    harness.click_at(harness.get_by_role_and_label(Role::Tab, "Overview").rect());
    wait_until(&mut harness, "the chip", |harness| {
        harness.query_by_label_contains("Claude · ").is_some()
    });
    harness.click_at(harness.get_by_label_contains("Claude · ").rect());
    wait_until(&mut harness, "the chip's menu", |harness| {
        harness.query_by_label("Keep the answer beside the code").is_some()
    });
    harness.click_at(harness.get_by_label("Keep the answer beside the code").rect());
    harness.run_steps(2);
    assert!(harness.state().services.settings.assistant.mini_chat, "switched on in place");
}

fn has_card(harness: &Harness<'_, ReviewApp>) -> bool {
    harness.query_by_role_and_label(Role::Group, CARD_LABEL).is_some()
}

fn drawn(harness: &Harness<'_, ReviewApp>) -> mini::Drawn {
    mini::drawn(&harness.ctx).expect("the mini chat")
}

fn is_disabled(harness: &Harness<'_, ReviewApp>, label: &str) -> bool {
    harness.get_by_label(label).accesskit_node().is_disabled()
}

fn assert_near(actual: Pos2, expected: Pos2, what: &str) {
    assert!(actual.distance(expected) < 0.5, "{what}: {actual:?}, expected {expected:?}");
}

/// Presses at `from`, moves the pointer to `to` over a few frames and releases there.
fn drag(harness: &mut Harness<'_, ReviewApp>, from: Pos2, to: Pos2) {
    harness.hover_at(from);
    harness.step();
    let button = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    harness.event(button(from, true));
    harness.step();
    for step in 1..=6 {
        harness.hover_at(from.lerp(to, step as f32 / 6.0));
        harness.step();
    }
    harness.event(button(to, false));
    harness.step();
}
