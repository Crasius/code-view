//! The editor driven as a person drives it, through egui input: typing, a toolbar button, an
//! input rule, Enter and Backspace in a list and undo, asserting the Markdown it writes and the
//! accessibility tree it reports.

use std::sync::Arc;

use cv_base::theme;
use cv_testkit::Clicks;
use egui::accesskit::Role;
use egui::{Event, ImeEvent, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::ProviderKind;
use review_store::ThemePreference;

use super::{RichOptions, show_with, text_id};
use crate::markdown_editor::{Person, PullRequestContext};

struct Probe {
    text: String,
    platform: ProviderKind,
    /// Frames whose output said the Markdown changed, and the last output's focus.
    changes: usize,
    has_focus: bool,
    /// The editor takes the focus back whenever it lost it.
    grab_focus: bool,
    /// Published as the pull request's people, for mentions.
    people: Option<Arc<Vec<Person>>>,
}

fn editor_id() -> egui::Id {
    egui::Id::unique("rich")
}

fn harness(text: &str, platform: ProviderKind) -> Harness<'static, Probe> {
    let probe = Probe {
        text: text.to_owned(),
        platform,
        changes: 0,
        has_focus: false,
        grab_focus: true,
        people: None,
    };
    let mut harness = Harness::builder()
        .with_size([760.0, 420.0])
        .with_max_steps(400)
        .with_step_dt(1.0 / 60.0)
        .with_context_setup(theme::fonts::install)
        .build_ui_state(
            |ui, probe: &mut Probe| {
                theme::activate(ui.ctx(), ThemePreference::Light);
                if let Some(people) = &probe.people {
                    PullRequestContext { kind: probe.platform, people: people.clone() }
                        .publish(ui.ctx());
                }
                let options = RichOptions {
                    platform: probe.platform,
                    label: "Comment",
                    grab_focus: probe.grab_focus,
                    ..RichOptions::default()
                };
                let id = editor_id();
                let output = show_with(ui, id, &mut probe.text, &options, |_| {});
                probe.changes += usize::from(output.is_changed);
                probe.has_focus = output.has_focus;
            },
            probe,
        );
    harness.run_steps(2);
    harness
}

fn type_text(harness: &mut Harness<'_, Probe>, text: &str) {
    for ch in text.chars() {
        harness.event(Event::Text(ch.to_string()));
        harness.run_steps(1);
    }
}

fn press(harness: &mut Harness<'_, Probe>, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run_steps(1);
}

#[test]
fn typing_formatting_lists_and_undo_write_markdown() {
    let mut harness = harness("", ProviderKind::GitHub);
    let focused = harness.ctx.memory(|memory| memory.has_focus(text_id(editor_id())));
    assert!(focused, "grab_focus focuses the text area");
    // The app's single-key shortcuts ask egui whether a text edit has the keys.
    assert!(harness.ctx.text_edit_focused(), "the focused area counts as a text edit");

    assert!(harness.state().has_focus);
    type_text(&mut harness, "Hello **world** ");
    assert!(harness.state().changes > 0, "the output reports the edits");
    assert_eq!(harness.state().text, "Hello **world**", "the input rule bolds the word");

    harness.click_at(harness.get_by_label("Italic").rect());
    harness.run_steps(2);
    type_text(&mut harness, "slanted");
    assert_eq!(harness.state().text, "Hello **world** *slanted*");

    press(&mut harness, Modifiers::NONE, Key::Enter);
    type_text(&mut harness, "- one");
    press(&mut harness, Modifiers::NONE, Key::Enter);
    type_text(&mut harness, "two");
    assert_eq!(harness.state().text, "Hello **world** *slanted*\n\n- one\n- two");

    press(&mut harness, Modifiers::NONE, Key::Enter);
    press(&mut harness, Modifiers::NONE, Key::Enter);
    type_text(&mut harness, "out");
    assert_eq!(
        harness.state().text,
        "Hello **world** *slanted*\n\n- one\n- two\n\nout",
        "Enter on an empty item leaves the list"
    );

    for _ in 0.."out".len() {
        press(&mut harness, Modifiers::NONE, Key::Backspace);
    }
    press(&mut harness, Modifiers::NONE, Key::Backspace);
    assert_eq!(
        harness.state().text,
        "Hello **world** *slanted*\n\n- one\n- two",
        "Backspace joins the empty line into the item"
    );

    press(&mut harness, Modifiers::COMMAND, Key::Z);
    assert_eq!(
        harness.state().text,
        "Hello **world** *slanted*\n\n- one\n- two\n\nout",
        "the deletes in a row undo as one step"
    );
    press(&mut harness, Modifiers::COMMAND, Key::Y);
    assert_eq!(harness.state().text, "Hello **world** *slanted*\n\n- one\n- two");

    let area = harness.get_by_label("Comment");
    assert_eq!(area.accesskit_node().role(), Role::MultilineTextInput);
    assert_eq!(area.accesskit_node().value().as_deref(), Some("Hello world slanted\none\ntwo"));
    let bold = harness.get_by_label("Bold");
    assert_eq!(bold.accesskit_node().role(), Role::Button);
}

#[test]
fn shortcuts_toolbar_and_platform_html() {
    let mut harness = harness("", ProviderKind::AzureDevOps);
    press(&mut harness, Modifiers::COMMAND, Key::U);
    type_text(&mut harness, "under");
    assert_eq!(harness.state().text, "<u>under</u>", "Azure DevOps keeps <u>, not <ins>");

    press(&mut harness, Modifiers::NONE, Key::Enter);
    harness.click_at(harness.get_by_label("Numbered list").rect());
    harness.run_steps(2);
    type_text(&mut harness, "first");
    press(&mut harness, Modifiers::NONE, Key::Enter);
    type_text(&mut harness, "second");
    press(&mut harness, Modifiers::NONE, Key::Tab);
    assert_eq!(
        harness.state().text,
        "<u>under</u>\n\n1. first\n   1. second",
        "Tab nests the item"
    );
    press(&mut harness, Modifiers::SHIFT, Key::Tab);
    assert_eq!(harness.state().text, "<u>under</u>\n\n1. first\n2. second");
}

/// A Tab the first item of a list refuses records no undo step: the next Ctrl+Z undoes the last
/// typed text, and a refused Tab after it leaves the redo in place.
#[test]
fn a_refused_tab_leaves_undo_to_the_last_real_edit() {
    let mut harness = harness("", ProviderKind::GitHub);
    type_text(&mut harness, "- one");
    assert_eq!(harness.state().text, "- one");
    press(&mut harness, Modifiers::NONE, Key::Tab);
    assert_eq!(harness.state().text, "- one", "the first item has nothing to nest under");
    press(&mut harness, Modifiers::COMMAND, Key::Z);
    assert_eq!(harness.state().text, "-", "Ctrl+Z undoes the typed word, not the refused Tab");
    press(&mut harness, Modifiers::NONE, Key::Tab);
    press(&mut harness, Modifiers::COMMAND, Key::Y);
    assert_eq!(harness.state().text, "- one", "the redo survives a refused Tab");
}

/// Tab takes the mention list's pick (as Enter does) in a paragraph too, where Tab otherwise moves
/// the focus on: the text keeps the focus and the next words land after the mention.
#[test]
fn tab_takes_a_mention_and_keeps_the_focus() {
    let mut harness = harness("", ProviderKind::GitHub);
    let ada =
        Person { id: "1".into(), display_name: "Ada Lovelace".into(), login: Some("ada".into()) };
    harness.state_mut().grab_focus = false;
    harness.state_mut().people = Some(Arc::new(vec![ada]));
    let text_area = text_id(editor_id());
    harness.ctx.memory_mut(|memory| memory.request_focus(text_area));
    harness.run_steps(2);
    type_text(&mut harness, "hi @ad");
    press(&mut harness, Modifiers::NONE, Key::Tab);
    harness.run_steps(2);
    assert!(harness.state().has_focus, "the text keeps the focus");
    type_text(&mut harness, "thanks");
    assert_eq!(harness.state().text, "hi @ada thanks");
}

/// An IME composition started with the caret on a rule goes into one new paragraph after it:
/// each update replaces the last, and the commit replaces the composition.
#[test]
fn an_ime_composition_on_a_rule_writes_one_paragraph() {
    let mut harness = harness("a\n\n---", ProviderKind::GitHub);
    for text in ["k", "ka"] {
        let preedit = ImeEvent::Preedit { text: text.into(), active_range_chars: None };
        harness.event(Event::Ime(preedit));
        harness.run_steps(1);
    }
    harness.event(Event::Ime(ImeEvent::Commit("kana".into())));
    harness.run_steps(1);
    assert_eq!(harness.state().text, "a\n\n---\n\nkana");
}

#[test]
fn text_changed_from_outside_is_read_again() {
    let mut harness = harness("# Title", ProviderKind::GitHub);
    type_text(&mut harness, " more");
    assert_eq!(harness.state().text, "# Title more");
    harness.state_mut().text = "- [ ] task".to_owned();
    harness.run_steps(2);
    type_text(&mut harness, " done");
    assert_eq!(harness.state().text, "- [ ] task done");
}

#[test]
fn reload_reads_a_same_length_change_in_the_middle() {
    let mut harness = harness("one two three", ProviderKind::GitHub);
    // A caller that rewrites the text in place asks for a fresh read; the undo history goes.
    harness.state_mut().text = "one TWO three".to_owned();
    super::reload(&harness.ctx, editor_id());
    harness.run_steps(2);
    type_text(&mut harness, "!");
    assert_eq!(harness.state().text, "one TWO three!");
}

/// A 5,000-line comment lays out only the blocks in view, on open, after typing at its end and
/// after jumping back to its start.
#[test]
fn a_long_comment_lays_out_only_what_is_visible() {
    let long: Vec<String> =
        (0..5_000).map(|line| format!("Line {line} of a long comment")).collect();
    let mut harness = harness(&long.join("\n\n"), ProviderKind::GitHub);
    let cached = |harness: &Harness<'_, Probe>| {
        let id = editor_id().with("rich_state");
        let handle = harness.ctx.data(|data| data.get_temp::<super::Handle>(id)).expect("state");
        handle.lock().layout.cached()
    };
    assert!(cached(&harness) < 60, "{} blocks laid out on open", cached(&harness));
    type_text(&mut harness, "!");
    assert!(
        harness.state().text.ends_with("Line 4999 of a long comment!"),
        "the caret starts at the end"
    );
    assert!(cached(&harness) < 60, "{} blocks laid out after typing", cached(&harness));
    press(&mut harness, Modifiers::COMMAND, Key::Home);
    harness.run_steps(4);
    type_text(&mut harness, "#");
    assert!(harness.state().text.starts_with("\\#Line 0"), "a typed `#` stays text");
    assert!(cached(&harness) < 120, "{} blocks laid out after the jump", cached(&harness));
}
