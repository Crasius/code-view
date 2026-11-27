//! Edits on the document, checked through the Markdown they leave: Enter and Backspace in lists
//! and quotes as `TipTap` has them, nesting, marks, input rules, structure toggles and paste.

use review_core::ProviderKind;

use super::edit::structure::{self, ListStyle};
use super::edit::{self as edit_ops};
use super::history::{EditKind, History};
use super::input::type_into;
use super::model::{Document, Link, Mark, Marks, Pos, Selection};
use super::parse::parse;
use super::serialize::serialize;

fn github(source: &str) -> Document {
    parse(source, ProviderKind::GitHub)
}

fn markdown(doc: &Document) -> String {
    serialize(doc, ProviderKind::GitHub)
}

fn caret(block: usize, offset: usize) -> Selection {
    Selection::caret(Pos::new(block, offset))
}

/// Types `text` char by char at `at`, through the editor's typing step.
fn type_text(doc: &mut Document, mut at: Pos, text: &str) -> Pos {
    let mut stored = None;
    for ch in text.chars() {
        let typed = type_into(doc, Selection::caret(at), stored.take(), &ch.to_string());
        at = typed.caret;
        stored = typed.stored;
    }
    at
}

#[test]
fn enter_continues_a_list_and_leaves_it_from_an_empty_item() {
    let mut doc = github("- one");
    let at = edit_ops::enter(&mut doc, caret(0, 3));
    let at = type_text(&mut doc, at, "two");
    assert_eq!(markdown(&doc), "- one\n- two");
    let at = edit_ops::enter(&mut doc, Selection::caret(at));
    assert_eq!(markdown(&doc), "- one\n- two\n-");
    let at = edit_ops::enter(&mut doc, Selection::caret(at));
    type_text(&mut doc, at, "after");
    assert_eq!(markdown(&doc), "- one\n- two\n\nafter");
}

#[test]
fn enter_in_a_nested_empty_item_moves_it_out_one_level() {
    let mut doc = github("- a\n  - b\n  -");
    let at = edit_ops::enter(&mut doc, caret(2, 0));
    assert_eq!(markdown(&doc), "- a\n  - b\n-");
    assert_eq!(at, Pos::new(2, 0));
}

#[test]
fn backspace_at_an_items_start_lifts_it_and_then_joins() {
    let mut doc = github("- a\n- b");
    let at = edit_ops::backspace(&mut doc, caret(1, 0));
    assert_eq!(markdown(&doc), "- a\n\nb");
    edit_ops::backspace(&mut doc, Selection::caret(at));
    assert_eq!(markdown(&doc), "- ab");
}

#[test]
fn backspace_lifts_quotes_and_turns_headings_into_paragraphs() {
    let mut doc = github("> quoted");
    edit_ops::backspace(&mut doc, caret(0, 0));
    assert_eq!(markdown(&doc), "quoted");
    let mut doc = github("# Title");
    edit_ops::backspace(&mut doc, caret(0, 0));
    assert_eq!(markdown(&doc), "Title");
}

#[test]
fn enter_splits_text_and_a_heading_ends_in_a_paragraph() {
    let mut doc = github("# Title");
    let at = edit_ops::enter(&mut doc, caret(0, 5));
    type_text(&mut doc, at, "body");
    assert_eq!(markdown(&doc), "# Title\n\nbody");
    let mut doc = github("ab");
    edit_ops::enter(&mut doc, caret(0, 1));
    assert_eq!(markdown(&doc), "a\n\nb");
}

#[test]
fn three_enters_at_a_code_blocks_end_leave_it() {
    let mut doc = github("```\ncode\n```");
    let mut at = Pos::new(0, 4);
    for _ in 0..3 {
        at = edit_ops::enter(&mut doc, Selection::caret(at));
    }
    type_text(&mut doc, at, "text");
    assert_eq!(markdown(&doc), "```\ncode\n```\n\ntext");
}

#[test]
fn tab_nests_an_item_under_the_one_before_and_shift_tab_lifts_it() {
    let mut doc = github("- a\n- b\n- c");
    assert!(!structure::indent(&mut doc, 0, 0), "the first item has nothing to go into");
    assert!(structure::indent(&mut doc, 1, 1));
    assert_eq!(markdown(&doc), "- a\n  - b\n- c");
    assert!(structure::outdent(&mut doc, 1, 1));
    assert_eq!(markdown(&doc), "- a\n- b\n- c");
}

#[test]
fn list_buttons_wrap_change_and_unwrap() {
    let mut doc = github("one\n\ntwo");
    structure::toggle_list(&mut doc, 0, 1, ListStyle::Bullet);
    assert_eq!(markdown(&doc), "- one\n- two");
    structure::toggle_list(&mut doc, 0, 1, ListStyle::Ordered);
    assert_eq!(markdown(&doc), "1. one\n2. two");
    structure::toggle_list(&mut doc, 0, 0, ListStyle::Task);
    assert_eq!(markdown(&doc), "- [ ] one\n- [ ] two");
    assert!(structure::toggle_task(&mut doc, 1));
    assert_eq!(markdown(&doc), "- [ ] one\n- [x] two");
    structure::toggle_list(&mut doc, 0, 1, ListStyle::Task);
    assert_eq!(markdown(&doc), "one\n\ntwo");
}

#[test]
fn quote_heading_code_block_and_rule_toggle() {
    let mut doc = github("a\n\nb");
    structure::toggle_quote(&mut doc, 0, 1);
    assert_eq!(markdown(&doc), "> a\n>\n> b");
    structure::toggle_quote(&mut doc, 0, 1);
    structure::toggle_heading(&mut doc, 0, 0, 2);
    assert_eq!(markdown(&doc), "## a\n\nb");
    structure::toggle_heading(&mut doc, 0, 0, 2);
    structure::toggle_code_block(&mut doc, 0, 1);
    assert_eq!(markdown(&doc), "```\na\nb\n```");
    let mut doc = github("text");
    let at = structure::insert_rule(&mut doc, Pos::new(0, 4));
    assert_eq!(at, Pos::new(2, 0));
    assert_eq!(markdown(&doc), "text\n\n---");
}

/// A mention that moves into a code block (joined in by Backspace, turned into code, pasted) is
/// written as its source there, never as the editor's placeholder char, and comes back out as a
/// mention.
#[test]
fn mentions_moved_into_code_keep_their_source() {
    let mut doc = github("```\ncode\n```\n\nhi @ada");
    edit_ops::backspace(&mut doc, caret(1, 0));
    assert_eq!(markdown(&doc), "```\ncodehi @ada\n```");

    let mut doc = github("hi @ada");
    structure::toggle_code_block(&mut doc, 0, 0);
    assert_eq!(markdown(&doc), "```\nhi @ada\n```");
    structure::toggle_code_block(&mut doc, 0, 0);
    assert_eq!(markdown(&doc), "hi @ada");
    assert!(doc.block(0).content.runs().iter().any(|run| run.atom.is_some()), "still a mention");

    let mut doc = github("```\n\n```");
    let fragment = parse("one @ada\n\ntwo", ProviderKind::GitHub);
    edit_ops::insert_fragment(&mut doc, caret(0, 0), &fragment);
    assert_eq!(markdown(&doc), "```\none @ada\ntwo\n```");
    let mut doc = parse("@<6A1B2C3D-0000-1111-2222-333344445555> hi", ProviderKind::AzureDevOps);
    structure::toggle_code_block(&mut doc, 0, 0);
    assert_eq!(
        serialize(&doc, ProviderKind::AzureDevOps),
        "```\n@<6A1B2C3D-0000-1111-2222-333344445555> hi\n```"
    );
}

#[test]
fn marks_toggle_over_words_and_at_a_bare_caret() {
    let mut doc = github("one two three");
    let words = Selection { anchor: Pos::new(0, 4), head: Pos::new(0, 13) };
    edit_ops::toggle_mark(&mut doc, words, Mark::Bold);
    assert_eq!(markdown(&doc), "one **two three**");
    edit_ops::toggle_mark(&mut doc, words, Mark::Bold);
    assert_eq!(markdown(&doc), "one two three");
    edit_ops::toggle_mark(&mut doc, words, Mark::Underline);
    assert_eq!(markdown(&doc), "one <ins>two three</ins>");
    let stored = edit_ops::toggled(&Marks::default(), Mark::Italic);
    let at = edit_ops::insert_text(&mut doc, caret(0, 0), "new ", &stored);
    assert_eq!(at, Pos::new(0, 4));
    assert_eq!(markdown(&doc), "*new* one <ins>two three</ins>");
}

#[test]
fn input_rules_format_as_typed() {
    for (typed, expected) in [
        ("**bold** ", "**bold**"),
        ("*it* x", "*it* x"),
        ("`code` x", "`code` x"),
        ("~~gone~~ x", "~~gone~~ x"),
        ("# Title", "# Title"),
        ("### Small", "### Small"),
        ("- item", "- item"),
        ("* item", "- item"),
        ("1. first", "1. first"),
        ("3. third", "3. third"),
        ("[ ] task", "- [ ] task"),
        ("[x] done", "- [x] done"),
        ("> quote", "> quote"),
        ("---", "---"),
        ("```rust code", "```rust\ncode\n```"),
    ] {
        let mut doc = Document::default();
        type_text(&mut doc, Pos::new(0, 0), typed);
        assert_eq!(markdown(&doc).trim_end(), expected, "typed {typed:?}");
    }
}

#[test]
fn input_rules_wait_for_a_word_start_and_skip_code() {
    let mut doc = Document::default();
    type_text(&mut doc, Pos::new(0, 0), "snake_case_name");
    assert_eq!(
        markdown(&doc),
        "snake_case_name",
        "an intraword `_` neither italicizes nor needs escaping"
    );
    let mut doc = github("```\n\n```");
    type_text(&mut doc, Pos::new(0, 0), "**x**");
    assert_eq!(markdown(&doc), "```\n**x**\n```");
}

#[test]
fn links_set_on_a_selection_and_insert_at_a_caret() {
    let mut doc = github("see docs");
    let link = Link { url: "https://docs.test".into(), title: String::new() };
    edit_ops::set_link(
        &mut doc,
        Selection { anchor: Pos::new(0, 4), head: Pos::new(0, 8) },
        Some(link.clone()),
    );
    assert_eq!(markdown(&doc), "see [docs](https://docs.test)");
    edit_ops::set_link(&mut doc, Selection { anchor: Pos::new(0, 4), head: Pos::new(0, 8) }, None);
    assert_eq!(markdown(&doc), "see docs");
    edit_ops::insert_link(&mut doc, caret(0, 0), "https://docs.test", link);
    assert_eq!(markdown(&doc), "<https://docs.test>see docs");
}

#[test]
fn paste_of_blocks_goes_between_the_halves_of_the_caret_block() {
    let mut doc = github("- ab");
    let fragment = parse("x\n\n- y", ProviderKind::GitHub);
    edit_ops::insert_fragment(&mut doc, caret(0, 1), &fragment);
    assert_eq!(markdown(&doc), "- ax\n  - y\n\n  b");
    let mut doc = github("ab");
    let inline = parse("**x**", ProviderKind::GitHub);
    let at = edit_ops::insert_fragment(&mut doc, caret(0, 1), &inline);
    assert_eq!(at, Pos::new(0, 2));
    assert_eq!(markdown(&doc), "a**x**b");
}

#[test]
fn copy_takes_the_selected_part_with_its_containers() {
    let doc = github("- one\n- **two** three");
    let selection = Selection { anchor: Pos::new(0, 1), head: Pos::new(1, 3) };
    assert_eq!(markdown(&edit_ops::fragment(&doc, selection)), "- ne\n- **two**");
}

#[test]
fn undo_groups_a_word_and_redo_restores_it() {
    let mut doc = Document::default();
    let mut history = History::default();
    let mut selection = caret(0, 0);
    for (index, ch) in "ab cd".chars().enumerate() {
        history.record(
            &doc,
            selection,
            EditKind::Typing { is_space: ch == ' ' },
            0,
            index as f64 * 0.1,
        );
        let at = edit_ops::insert_text(&mut doc, selection, &ch.to_string(), &Marks::default());
        selection = Selection::caret(at);
    }
    assert!(history.undo(&mut doc, &mut selection));
    assert_eq!(markdown(&doc), "ab");
    assert!(history.undo(&mut doc, &mut selection));
    assert_eq!(markdown(&doc), "");
    assert!(!history.undo(&mut doc, &mut selection));
    assert!(history.redo(&mut doc, &mut selection));
    assert_eq!(markdown(&doc), "ab");
}
