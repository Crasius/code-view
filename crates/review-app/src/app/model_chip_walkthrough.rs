//! Where the assistant's agent and model chip sits, end to end on the mocked GitHub host
//! ([`super::github_mock`]) with a scripted agent. It is always in the title line, left of "New
//! conversation" and centred with those buttons, in the card, in the panel and in the mini chat's
//! title line, with no setting involved; its menu switches the model from there. Asserts state and
//! geometry only. Needs `git` on PATH, no network, no real CLI.
//! `cargo test -p review-app model_chip`

use egui::{Rect, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::ReviewApp;
use super::action_e2e::wait_until;
use super::clicks::Clicks;
use super::mini_chat_walkthrough::{FIRST, answered, primary_click};
use cv_pr_assistant::mini::SHRINK;

const CHIP: &str = "Claude · ";
const NEW: &str = "New conversation (";

#[test]
fn the_model_chip_sits_in_the_title_line_of_card_panel_and_mini_chat() {
    let (mut harness, _agent, _scratch) = answered("model-chip", true);
    assert!(
        is_in_line(chip(&harness), button(&harness, NEW)),
        "the chip is in the card's title line"
    );

    pick(&mut harness, "CLI default");
    wait_until(&mut harness, "the model switched from the title line", |harness| {
        chip_label(harness).contains("CLI default")
    });
    assert!(is_in_line(chip(&harness), button(&harness, NEW)), "still in the title line");

    harness.click_at(harness.get_by_label_contains("Expand (").rect());
    wait_until(&mut harness, "the chip in the panel's title line", |harness| {
        harness.query_by_label("Collapse (Esc)").is_some()
            && is_in_line(chip(harness), button(harness, NEW))
    });
    harness.click_at(harness.get_by_label("Collapse (Esc)").rect());
    wait_until(&mut harness, "the card again", |harness| {
        harness.query_by_label_contains("Expand (").is_some()
    });

    let center = harness.get_by_label(FIRST).rect().center();
    primary_click(&mut harness, center);
    wait_until(&mut harness, "the chip in the mini chat's title line", |harness| {
        harness.query_by_label(SHRINK).is_some()
            && is_in_line(chip(harness), button(harness, SHRINK))
    });
    pick(&mut harness, "Haiku");
    wait_until(&mut harness, "the model switched from the mini chat", |harness| {
        chip_label(harness).starts_with("Claude · claude-haiku")
    });
}

/// The one chip on screen: a single button, wherever it sits.
fn chip(harness: &Harness<'_, ReviewApp>) -> Rect {
    let mut chips = chips(harness);
    let found = chips.next().expect("the chip");
    assert!(chips.next().is_none(), "one chip, not one in the title line and one below");
    found.rect()
}

fn chip_label(harness: &Harness<'_, ReviewApp>) -> String {
    chips(harness).next().and_then(|node| node.accesskit_node().label()).unwrap_or_default()
}

fn chips<'a>(
    harness: &'a Harness<'_, ReviewApp>,
) -> impl Iterator<Item = egui_kittest::Node<'a>> + 'a {
    harness
        .query_all_by_role(Role::Button)
        .filter(|node| node.accesskit_node().label().is_some_and(|label| label.starts_with(CHIP)))
}

fn button(harness: &Harness<'_, ReviewApp>, label: &str) -> Rect {
    harness.get_by_label_contains(label).rect()
}

/// Left of `button`, in its row: the same vertical centre within a point.
fn is_in_line(chip: Rect, button: Rect) -> bool {
    (chip.center().y - button.center().y).abs() <= 1.0 && chip.right() <= button.left()
}

/// Opens the chip's menu and picks `item`.
fn pick(harness: &mut Harness<'_, ReviewApp>, item: &str) {
    let at = chip(harness).center();
    primary_click(harness, at);
    wait_until(harness, "the chip's menu", |harness| harness.query_by_label(item).is_some());
    harness.click_at(harness.get_by_label(item).rect());
    harness.run_steps(4);
}
