//! Headless tests of the composer driven like a user: typing, shortcuts, paste, submit and cancel,
//! the three modes on one text and the editor each platform opens in.

use std::sync::Arc;

use cv_base::theme;
use cv_testkit::Clicks;
use egui::accesskit::{Role, Toggled};
use egui::{Event, Id, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::ProviderKind;
use review_git::FileDiff;
use review_store::{CommentEditor, CommentEditors, ThemePreference};

use super::toolbar_layout::TEXT_MARGIN;
use super::{FileContext, Options, Outcome, Person, PullRequestContext, show, take_chosen_editors};

const DIFF: &str = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,4 @@\n fn total(items: &[u32]) -> u32 {\n-    items.iter().sum()\n+    let sum = items.iter().sum();\n+    sum\n }\n";

#[derive(Default)]
struct Probe {
    text: String,
    submitted: usize,
    cancelled: usize,
    is_busy: bool,
    can_cancel: bool,
    leading_buttons: bool,
    platform: Option<ProviderKind>,
    /// The saved editor choice, published before each frame; `None` leaves the defaults.
    editors: Option<CommentEditors>,
    initial: Option<String>,
    suggestion: Option<String>,
    file: Option<FileContext>,
    people: Option<PullRequestContext>,
}

impl Probe {
    fn new() -> Self {
        Self { can_cancel: true, ..Self::default() }
    }
}

fn composer(probe: Probe, width: f32) -> Harness<'static, Probe> {
    let mut harness = Harness::builder()
        .with_size([width, 420.0])
        .with_max_steps(400)
        .with_context_setup(theme::fonts::install)
        .build_ui_state(
            |ui, probe: &mut Probe| {
                let palette = theme::activate(ui.ctx(), ThemePreference::Light);
                if let Some(file) = &probe.file {
                    file.clone().publish(ui.ctx());
                }
                if let Some(people) = &probe.people {
                    people.clone().publish(ui.ctx());
                }
                if let Some(editors) = probe.editors {
                    super::publish_editors(ui.ctx(), editors);
                }
                egui::Frame::new().fill(palette.ground).inner_margin(16).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let options = Options {
                        is_busy: probe.is_busy,
                        can_cancel: probe.can_cancel,
                        leading_buttons: probe.leading_buttons,
                        grab_focus: true,
                        platform: probe.platform,
                        initial: probe.initial.as_deref(),
                        suggestion: probe.suggestion.as_deref(),
                        ..Options::default()
                    };
                    let output = show(ui, Id::unique("composer"), &mut probe.text, &options);
                    let is_submitted = matches!(output, Some(Outcome::Submitted { .. }));
                    probe.submitted += usize::from(is_submitted);
                    probe.cancelled += usize::from(output == Some(Outcome::Cancelled));
                });
            },
            probe,
        );
    harness.run_steps(3);
    harness
}

fn type_text(harness: &mut Harness<'_, Probe>, text: &str) {
    harness.event(Event::Text(text.to_owned()));
    harness.run_steps(2);
}

fn press(harness: &mut Harness<'_, Probe>, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run_steps(2);
}

fn select_all(harness: &mut Harness<'_, Probe>) {
    press(harness, Modifiers::COMMAND, Key::A);
}

#[test]
fn enter_continues_and_ends_lists() {
    let mut harness = composer(Probe::new(), 720.0);
    type_text(&mut harness, "- one");
    press(&mut harness, Modifiers::NONE, Key::Enter);
    type_text(&mut harness, "two");
    press(&mut harness, Modifiers::NONE, Key::Enter);
    assert_eq!(harness.state().text, "- one\n- two\n- ");
    press(&mut harness, Modifiers::NONE, Key::Enter);
    type_text(&mut harness, "after");
    assert_eq!(harness.state().text, "- one\n- two\nafter");
}

#[test]
fn tab_nests_list_items_and_shift_tab_lifts_them() {
    let mut harness = composer(Probe::new(), 720.0);
    type_text(&mut harness, "- one");
    press(&mut harness, Modifiers::NONE, Key::Enter);
    type_text(&mut harness, "two");
    press(&mut harness, Modifiers::NONE, Key::Tab);
    assert_eq!(harness.state().text, "- one\n  - two");
    press(&mut harness, Modifiers::SHIFT, Key::Tab);
    assert_eq!(harness.state().text, "- one\n- two");
}

#[test]
fn shortcuts_format_and_undo_restores() {
    let mut harness = composer(Probe::new(), 720.0);
    type_text(&mut harness, "word");
    select_all(&mut harness);
    press(&mut harness, Modifiers::COMMAND, Key::B);
    assert_eq!(harness.state().text, "**word**");
    press(&mut harness, Modifiers::COMMAND, Key::I);
    assert_eq!(harness.state().text, "**_word_**");
    press(&mut harness, Modifiers::COMMAND, Key::Z);
    assert_eq!(harness.state().text, "**word**");
    press(&mut harness, Modifiers::COMMAND, Key::Y);
    assert_eq!(harness.state().text, "**_word_**");

    select_all(&mut harness);
    type_text(&mut harness, "call()");
    select_all(&mut harness);
    press(&mut harness, Modifiers::COMMAND, Key::E);
    assert_eq!(harness.state().text, "`call()`");

    select_all(&mut harness);
    type_text(&mut harness, "site");
    select_all(&mut harness);
    press(&mut harness, Modifiers::COMMAND, Key::K);
    assert_eq!(harness.state().text, "[site](https://)");

    select_all(&mut harness);
    type_text(&mut harness, "a\nb");
    select_all(&mut harness);
    press(&mut harness, Modifiers::COMMAND | Modifiers::SHIFT, Key::Num8);
    assert_eq!(harness.state().text, "- a\n- b");
    press(&mut harness, Modifiers::COMMAND | Modifiers::SHIFT, Key::Num7);
    assert_eq!(harness.state().text, "1. a\n2. b");
}

#[test]
fn bold_toggles_off_when_the_caret_is_inside() {
    let mut harness = composer(Probe::new(), 720.0);
    type_text(&mut harness, "a **bold** b");
    for _ in 0..4 {
        press(&mut harness, Modifiers::NONE, Key::ArrowLeft);
    }
    press(&mut harness, Modifiers::COMMAND, Key::B);
    assert_eq!(harness.state().text, "a bold b");
}

#[test]
fn pasting_a_url_over_selected_text_makes_a_link() {
    let mut harness = composer(Probe::new(), 720.0);
    type_text(&mut harness, "docs");
    select_all(&mut harness);
    harness.event(Event::Paste("https://example.com/guide".into()));
    harness.run_steps(2);
    assert_eq!(harness.state().text, "[docs](https://example.com/guide)");
    // Without a selection a URL is pasted as it is.
    harness.event(Event::Paste(" https://example.com".into()));
    harness.run_steps(2);
    assert_eq!(harness.state().text, "[docs](https://example.com/guide) https://example.com");
}

#[test]
fn pasting_lines_from_the_diff_makes_a_code_block() {
    let mut probe = Probe::new();
    probe.file = Some(file_context());
    let mut harness = composer(probe, 720.0);
    type_text(&mut harness, "Why not:");
    harness.event(Event::Paste("    let sum = items.iter().sum();\n    sum\n".into()));
    harness.run_steps(2);
    assert_eq!(
        harness.state().text,
        "Why not:\n```rust\n    let sum = items.iter().sum();\n    sum\n```\n"
    );
    // Text that is not from the diff stays plain.
    harness.event(Event::Paste("plain words".into()));
    harness.run_steps(2);
    assert!(harness.state().text.ends_with("```\nplain words"));
}

#[test]
fn suggestion_button_inserts_the_commented_line() {
    let mut probe = Probe::new();
    probe.suggestion = Some("    sum".into());
    let mut harness = composer(probe, 720.0);
    harness.click_at(harness.get_by_label("Insert suggestion").rect());
    harness.run_steps(2);
    assert_eq!(harness.state().text, "```suggestion\n    sum\n```\n");
    // The caret sits on the suggested line, ready to edit it.
    type_text(&mut harness, "total");
    assert_eq!(harness.state().text, "```suggestion\ntotal\n```\n");
}

#[test]
fn suggestion_is_offered_only_where_supported() {
    let harness = composer(Probe::new(), 720.0);
    assert!(harness.query_by_label("Insert suggestion").is_none());
}

#[test]
fn mentions_complete_from_participants() {
    let mut probe = Probe::new();
    probe.people = Some(people(ProviderKind::GitHub));
    probe.editors = Some(MARKDOWN);
    let mut harness = composer(probe, 720.0);
    type_text(&mut harness, "thanks @gr");
    harness.run_steps(2);
    press(&mut harness, Modifiers::NONE, Key::Enter);
    assert_eq!(harness.state().text, "thanks @ghopper ");
}

#[test]
fn azure_mentions_use_the_identity_syntax() {
    let mut probe = Probe::new();
    probe.people = Some(people(ProviderKind::AzureDevOps));
    probe.editors = Some(MARKDOWN);
    let mut harness = composer(probe, 720.0);
    type_text(&mut harness, "@ada");
    harness.run_steps(2);
    press(&mut harness, Modifiers::NONE, Key::Enter);
    assert_eq!(harness.state().text, "@<0f8fad5b-d9cb-469f-a165-70867728950e> ");
}

#[test]
fn ctrl_enter_submits_only_with_text() {
    let mut harness = composer(Probe::new(), 720.0);
    press(&mut harness, Modifiers::COMMAND, Key::Enter);
    assert_eq!(harness.state().submitted, 0);
    type_text(&mut harness, "Looks good");
    press(&mut harness, Modifiers::COMMAND, Key::Enter);
    assert_eq!(harness.state().submitted, 1);
    assert_eq!(harness.state().text, "Looks good", "the caller decides when the text goes");
}

#[test]
fn busy_composer_does_not_submit_twice() {
    let mut probe = Probe::new();
    probe.is_busy = true;
    probe.text = "pending".into();
    let mut harness = composer(probe, 720.0);
    press(&mut harness, Modifiers::COMMAND, Key::Enter);
    assert_eq!(harness.state().submitted, 0);
    // The button keeps its label and reports the running post.
    harness.get_by_role_and_label(Role::Button, "Comment");
    let is_busy = harness.query_by(|node| {
        node.role() == Role::ProgressIndicator
            && node.label().is_some_and(|label| label.ends_with("in progress"))
    });
    assert!(is_busy.is_some());
}

#[test]
fn escape_cancels_empty_drafts_and_guards_written_ones() {
    let mut harness = composer(Probe::new(), 720.0);
    press(&mut harness, Modifiers::NONE, Key::Escape);
    assert_eq!(harness.state().cancelled, 1);

    type_text(&mut harness, "half a thought");
    press(&mut harness, Modifiers::NONE, Key::Escape);
    assert_eq!(harness.state().cancelled, 1, "a draft with text asks first");
    assert!(harness.query_by_label("Discard draft?").is_some());
    harness.click_at(harness.get_by_label("Keep editing").rect());
    harness.run_steps(2);
    assert_eq!(harness.state().text, "half a thought");
    assert!(harness.query_by_label("Discard draft?").is_none());

    press(&mut harness, Modifiers::NONE, Key::Escape);
    press(&mut harness, Modifiers::NONE, Key::Escape);
    assert!(harness.query_by_label("Discard draft?").is_none(), "Esc again keeps editing");
    assert_eq!(harness.state().cancelled, 1);

    harness.click_at(harness.get_by_label("Cancel").rect());
    harness.run_steps(2);
    harness.click_at(harness.get_by_label("Discard").rect());
    harness.run_steps(2);
    assert_eq!(harness.state().cancelled, 2);
}

#[test]
fn typing_answers_the_discard_question() {
    let mut harness = composer(Probe::new(), 720.0);
    type_text(&mut harness, "half a thought");
    press(&mut harness, Modifiers::NONE, Key::Escape);
    assert!(harness.query_by_label("Discard draft?").is_some());
    type_text(&mut harness, "!");
    assert!(harness.query_by_label("Discard draft?").is_none(), "typing means keep editing");
    assert_eq!(harness.state().text, "half a thought!");
    assert_eq!(harness.state().cancelled, 0);
}

#[test]
fn accepting_a_mention_answers_the_discard_question() {
    let mut probe = Probe::new();
    probe.people = Some(people(ProviderKind::GitHub));
    probe.editors = Some(MARKDOWN);
    let mut harness = composer(probe, 720.0);
    // The space closes the popup, so Esc goes to the composer and asks.
    type_text(&mut harness, "thanks @gr ");
    press(&mut harness, Modifiers::NONE, Key::Escape);
    assert!(harness.query_by_label("Discard draft?").is_some());
    // Moving the caret back behind `@gr` opens the popup again without changing the text.
    press(&mut harness, Modifiers::NONE, Key::ArrowLeft);
    assert!(harness.query_by_label("Discard draft?").is_some(), "a caret move is no answer");
    press(&mut harness, Modifiers::NONE, Key::Enter);
    assert_eq!(harness.state().text, "thanks @ghopper  ");
    assert!(harness.query_by_label("Discard draft?").is_none(), "the mention changed the text");
    assert_eq!(harness.state().cancelled, 0);
}

#[test]
fn thread_composer_cancel_asks_like_escape() {
    let mut probe = Probe::new();
    probe.leading_buttons = true;
    let mut harness = composer(probe, 720.0);
    harness.click_at(harness.get_by_label("Cancel").rect());
    harness.run_steps(2);
    assert_eq!(harness.state().cancelled, 1, "an empty draft closes at once");

    type_text(&mut harness, "half a thought");
    harness.click_at(harness.get_by_label("Cancel").rect());
    harness.run_steps(2);
    assert_eq!(harness.state().cancelled, 1, "a draft with text is never dropped silently");
    assert!(harness.query_by_label("Discard draft?").is_some());
    harness.click_at(harness.get_by_label("Keep editing").rect());
    harness.run_steps(2);
    assert_eq!(harness.state().text, "half a thought");
    assert!(harness.query_by_label("Discard draft?").is_none());

    harness.click_at(harness.get_by_label("Cancel").rect());
    harness.run_steps(2);
    harness.click_at(harness.get_by_label("Discard").rect());
    harness.run_steps(2);
    assert_eq!(harness.state().cancelled, 2);
}

#[test]
fn markdown_round_trips_unchanged() {
    let source = "## Review notes\n\nThis is **bold**, _italic_, ~~gone~~ and `code` with a [link](https://example.com).\n\n- item\n  - nested\n- [ ] open task\n- [x] done task\n\n1. first\n2. second\n\n> quoted text\n\n```rust\nfn main() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nThanks @octocat, see #12 and !34.\n";
    let mut probe = Probe::new();
    probe.text = source.to_owned();
    let mut harness = composer(probe, 720.0);
    harness.run_steps(5);
    assert_eq!(harness.state().text, source);
    // Switching to the preview and back changes nothing either.
    harness.click_at(harness.get_by_label("Preview").rect());
    harness.run_steps(3);
    harness.click_at(harness.get_by_label("Markdown").rect());
    harness.run_steps(3);
    assert_eq!(harness.state().text, source);
}

/// In Write, a click on `[ ]` places the caret there instead of checking the task.
#[test]
fn clicking_a_task_marker_places_the_caret() {
    let mut probe = Probe::new();
    probe.text = "- [ ] ship it".into();
    let mut harness = composer(probe, 720.0);
    let area = harness.get_by(|node| node.role() == Role::MultilineTextInput);
    // The text starts inside the area's margin; `[ ]` follows the two chars `- `.
    let at = area.rect().left_top() + TEXT_MARGIN.left_top() + egui::vec2(14.0, 8.0);
    harness.event(Event::PointerMoved(at));
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    harness.run_steps(2);
    assert_eq!(harness.state().text, "- [ ] ship it");
    let edit_id = Id::unique("composer").with("text");
    let caret = egui::text_edit::TextEditState::load(&harness.ctx, edit_id)
        .and_then(|state| state.cursor.char_range())
        .map(|range| range.primary.index.0);
    assert!(caret.is_some_and(|caret| (1..=5).contains(&caret)), "caret at {caret:?}");
}

/// Every platform's comments in the Markdown editor, for the tests of its text edit.
const MARKDOWN: CommentEditors = CommentEditors {
    github: CommentEditor::Markdown,
    gitlab: CommentEditor::Markdown,
    azure_devops: CommentEditor::Markdown,
};

fn platform_probe(platform: ProviderKind) -> Probe {
    Probe { platform: Some(platform), ..Probe::new() }
}

fn is_mode_selected(harness: &Harness<'_, Probe>, mode: &str) -> bool {
    let tab = harness.get_by_role_and_label(Role::Tab, mode);
    tab.accesskit_node().toggled() == Some(Toggled::True)
}

/// The value of the text area the current mode shows.
fn area_value(harness: &Harness<'_, Probe>) -> Option<String> {
    harness.get_by_role(Role::MultilineTextInput).accesskit_node().value()
}

/// A GitHub composer opens in Rich text; its toolbar writes Markdown into the one String that
/// Markdown shows as typed, the switch remembers the platform's editor, and Ctrl+Enter submits.
#[test]
fn rich_text_and_markdown_edit_the_same_comment() {
    let mut harness = composer(platform_probe(ProviderKind::GitHub), 720.0);
    assert!(is_mode_selected(&harness, "Rich text"), "GitHub comments open as rich text");
    harness.click_at(harness.get_by_label("Bold").rect());
    harness.run_steps(2);
    type_text(&mut harness, "x");
    assert_eq!(harness.state().text, "**x**");
    assert_eq!(area_value(&harness).as_deref(), Some("x"), "the rich text area shows no markers");

    harness.click_at(harness.get_by_role_and_label(Role::Tab, "Markdown").rect());
    harness.run_steps(2);
    assert_eq!(area_value(&harness).as_deref(), Some("**x**"), "Markdown shows the source");
    assert_eq!(harness.state().text, "**x**", "switching rewrites nothing");
    let chosen = take_chosen_editors(&harness.ctx).expect("the switch chose an editor");
    assert_eq!(chosen.get(ProviderKind::GitHub), CommentEditor::Markdown);
    assert_eq!(
        chosen.get(ProviderKind::GitLab),
        CommentEditor::Rich,
        "other platforms keep theirs"
    );

    harness.click_at(harness.get_by_role_and_label(Role::Tab, "Rich text").rect());
    harness.run_steps(2);
    assert_eq!(harness.state().text, "**x**");
    assert_eq!(area_value(&harness).as_deref(), Some("x"));
    let chosen = take_chosen_editors(&harness.ctx).expect("the switch chose again");
    assert_eq!(chosen.get(ProviderKind::GitHub), CommentEditor::Rich);

    press(&mut harness, Modifiers::COMMAND, Key::Enter);
    assert_eq!(harness.state().submitted, 1, "Ctrl+Enter submits from Rich text");
    assert!(is_mode_selected(&harness, "Rich text"), "after the post it stays on the editor");
    assert!(take_chosen_editors(&harness.ctx).is_none(), "a post chooses nothing");
}

/// A Markdown edit that keeps the text's length still reaches Rich text.
#[test]
fn rich_text_reads_a_markdown_edit_of_the_same_length() {
    let mut probe = platform_probe(ProviderKind::GitHub);
    probe.text = "one two".into();
    let mut harness = composer(probe, 720.0);
    harness.click_at(harness.get_by_role_and_label(Role::Tab, "Markdown").rect());
    harness.run_steps(2);
    harness.state_mut().text = "one TWO".into();
    harness.run_steps(1);
    harness.click_at(harness.get_by_role_and_label(Role::Tab, "Rich text").rect());
    harness.run_steps(2);
    assert_eq!(area_value(&harness).as_deref(), Some("one TWO"));
}

#[test]
fn azure_devops_comments_open_in_markdown() {
    let harness = composer(platform_probe(ProviderKind::AzureDevOps), 720.0);
    assert!(is_mode_selected(&harness, "Markdown"));
    assert!(
        harness.query_by_role_and_label(Role::Tab, "Rich text").is_some(),
        "Rich stays offered"
    );
    let plain = composer(Probe::new(), 720.0);
    assert!(
        plain.query_by_role_and_label(Role::Tab, "Rich text").is_none(),
        "without a platform there is no rich text"
    );
}

/// Editing a posted comment: cancelling the unchanged text closes at once, in either editor;
/// a changed text asks first.
#[test]
fn cancelling_an_unchanged_edit_closes_at_once() {
    for platform in [None, Some(ProviderKind::GitHub)] {
        let probe =
            Probe { text: "LGTM".into(), initial: Some("LGTM".into()), platform, ..Probe::new() };
        let mut harness = composer(probe, 720.0);
        press(&mut harness, Modifiers::NONE, Key::Escape);
        assert_eq!(harness.state().cancelled, 1, "{platform:?}");
        assert!(harness.query_by_label("Discard draft?").is_none());

        type_text(&mut harness, "!");
        press(&mut harness, Modifiers::NONE, Key::Escape);
        assert_eq!(harness.state().cancelled, 1, "{platform:?}: a changed edit asks first");
        assert!(harness.query_by_label("Discard draft?").is_some());
    }
}

fn file_context() -> FileContext {
    FileContext {
        path: "src/lib.rs".into(),
        diff: Arc::new(FileDiff::parse_unified(DIFF.as_bytes())),
    }
}

fn people(kind: ProviderKind) -> PullRequestContext {
    PullRequestContext {
        kind,
        people: Arc::new(vec![
            Person {
                id: "0f8fad5b-d9cb-469f-a165-70867728950e".into(),
                display_name: "Ada Lovelace".into(),
                login: Some("ada".into()),
            },
            Person {
                id: "7c9e6679-7425-40de-944b-e07fc1f90ae7".into(),
                display_name: "Grace Hopper".into(),
                login: Some("ghopper".into()),
            },
        ]),
    }
}
