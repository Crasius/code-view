//! Commands and the edits they make: from keys (`keys`), the toolbar, the popups, clicks and the
//! clipboard (`clipboard`). Ctrl+Enter and Esc (with no popup open) stay in the input: the caller
//! submits or cancels.

mod clipboard;
mod keys;

use egui::{Pos2, Ui};

pub(crate) use self::clipboard::is_url;
use self::clipboard::{copy, paste};
pub(crate) use self::keys::take_commands;
use super::caret::{self, Direction};
use super::edit::rules;
use super::edit::structure::{self, ListStyle};
use super::edit::{self as edit_ops};
use super::history::EditKind;
use super::model::{Atom, Document, Kind, Link, Mark, Marks, Pos, Selection};
use super::{Popup, State};

/// A caret move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    RowStart,
    RowEnd,
    DocStart,
    DocEnd,
    PageUp,
    PageDown,
}

/// What the editor does: from a key, a toolbar button, a menu entry or a click.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Command {
    Insert(String),
    Enter,
    LineBreak,
    Backspace,
    Delete,
    DeleteWord {
        forward: bool,
    },
    Move {
        motion: Motion,
        extend: bool,
    },
    SelectAll,
    Mark(Mark),
    /// Opens the link popover.
    EditLink,
    SetLink(Option<Link>),
    Heading(u8),
    List(ListStyle),
    Quote,
    CodeBlock,
    Rule,
    /// Types `@` and opens the mention list.
    Mention,
    Indent {
        outdent: bool,
    },
    ToggleTask(usize),
    Undo,
    Redo,
    Copy,
    Cut,
    Paste(String),
}

/// How the commands of one frame ran.
#[derive(Default)]
pub(crate) struct Ran {
    pub is_changed: bool,
    pub copied: Option<String>,
}

/// What a command runs with: the `Ui` for caret moves through the layout, the content's origin
/// and page height, the clock for undo grouping.
pub(crate) struct CommandContext<'a> {
    pub ui: &'a Ui,
    /// The content's top-left (screen coordinates).
    pub origin: Pos2,
    pub page_height: f32,
    pub now: f64,
}

/// Runs `commands` on the editor.
pub(crate) fn run(state: &mut State, context: &CommandContext<'_>, commands: Vec<Command>) -> Ran {
    let mut ran = Ran::default();
    for command in commands {
        let version = state.version;
        run_one(state, context, command, &mut ran);
        ran.is_changed |= state.version != version;
    }
    ran
}

fn selected_blocks(state: &State) -> (usize, usize) {
    let (from, to) = state.selection.ordered();
    (from.block, to.block)
}

/// Records an undo step of `kind`, then runs `change` and moves the caret where it says.
fn edit(
    state: &mut State,
    context: &CommandContext<'_>,
    kind: EditKind,
    change: impl FnOnce(&mut Document, Selection) -> Pos,
) {
    state.history.record(
        &state.doc,
        state.selection,
        kind,
        state.selection.head.block,
        context.now,
    );
    let caret = change(&mut state.doc, state.selection);
    state.selection = Selection::caret(state.doc.clamp(caret));
    state.stored = None;
    state.changed();
}

/// Records an undo step, then runs a block-level `change` over the selected blocks.
fn restructure(
    state: &mut State,
    context: &CommandContext<'_>,
    change: impl FnOnce(&mut Document, usize, usize),
) {
    let (first, last) = selected_blocks(state);
    state.history.record(&state.doc, state.selection, EditKind::Other, first, context.now);
    change(&mut state.doc, first, last);
    let (from, to) = state.selection.ordered();
    state.selection = Selection { anchor: state.doc.clamp(from), head: state.doc.clamp(to) };
    state.changed();
}

fn run_one(state: &mut State, context: &CommandContext<'_>, command: Command, ran: &mut Ran) {
    match command {
        Command::Insert(text) => insert(state, context, &text),
        Command::Enter => enter(state, context),
        Command::LineBreak => edit(state, context, EditKind::Other, edit_ops::line_break),
        Command::Backspace => backspace(state, context),
        Command::Delete => edit(state, context, EditKind::Deleting, edit_ops::delete_forward),
        Command::DeleteWord { forward } => {
            if state.selection.is_empty() {
                let caret = state.selection.head;
                let to = if forward {
                    caret::word_right(&state.doc, caret)
                } else {
                    caret::word_left(&state.doc, caret)
                };
                state.selection.anchor = to;
            }
            edit(state, context, EditKind::Other, edit_ops::backspace);
        }
        Command::Move { motion, extend } => move_caret(state, context, motion, extend),
        Command::SelectAll => {
            state.selection = Selection { anchor: Pos::new(0, 0), head: state.doc.end() };
            state.history.break_group();
        }
        Command::Mark(mark) => toggle_mark(state, context, mark),
        Command::EditLink => open_link(state),
        Command::SetLink(link) => set_link(state, context, link),
        Command::Heading(level) => restructure(state, context, |doc, first, last| {
            structure::toggle_heading(doc, first, last, level);
        }),
        Command::List(style) => restructure(state, context, |doc, first, last| {
            structure::toggle_list(doc, first, last, style);
        }),
        Command::Quote => restructure(state, context, structure::toggle_quote),
        Command::CodeBlock => {
            edit(state, context, EditKind::Other, |doc, selection| {
                let (from, to) = selection.ordered();
                structure::toggle_code_block(doc, from.block, to.block)
            });
        }
        Command::Rule => edit(state, context, EditKind::Other, |doc, selection| {
            structure::insert_rule(doc, selection.head)
        }),
        Command::Mention => {
            insert(state, context, "@");
        }
        Command::Indent { outdent } => indent(state, context, outdent),
        Command::ToggleTask(index) => {
            let before = (state.doc.clone(), state.selection);
            if structure::toggle_task(&mut state.doc, index) {
                state.history.record(&before.0, before.1, EditKind::Other, index, context.now);
                state.changed();
            } else {
                state.history.break_group();
            }
        }
        Command::Undo => {
            if state.history.undo(&mut state.doc, &mut state.selection) {
                state.changed();
            }
        }
        Command::Redo => {
            if state.history.redo(&mut state.doc, &mut state.selection) {
                state.changed();
            }
        }
        Command::Copy => ran.copied = copy(state),
        Command::Cut => {
            ran.copied = copy(state);
            if ran.copied.is_some() {
                edit(state, context, EditKind::Other, edit_ops::backspace);
            }
        }
        Command::Paste(text) => paste(state, context, &text),
    }
}

/// What typing left: the caret, the marks for the text typed next, and the document and
/// selection an input rule rewrote, when one fired.
pub(crate) struct Typed {
    pub caret: Pos,
    pub stored: Option<Marks>,
    pub before_rule: Option<(Document, Selection)>,
}

/// Types `text` over `selection` with the `stored` marks (or the marks at the selection's start),
/// then runs the input rule the text completes.
pub(crate) fn type_into(
    doc: &mut Document,
    selection: Selection,
    stored: Option<Marks>,
    text: &str,
) -> Typed {
    let (from, _) = selection.ordered();
    let marks = stored.unwrap_or_else(|| doc.block(from.block).content.marks_at(from.offset));
    let caret = edit_ops::insert_text(doc, selection, text, &marks);
    let before = doc.clone();
    match rules::after_typing(doc, caret) {
        Some(fired) => Typed {
            caret: fired.caret,
            stored: fired.stored,
            before_rule: Some((before, Selection::caret(caret))),
        },
        None => Typed { caret, stored: None, before_rule: None },
    }
}

/// Typed text, then the input rule it completes and the mention list an `@` opens.
fn insert(state: &mut State, context: &CommandContext<'_>, text: &str) {
    let is_space = text.chars().all(char::is_whitespace);
    let (from, _) = state.selection.ordered();
    state.history.record(
        &state.doc,
        state.selection,
        EditKind::Typing { is_space },
        from.block,
        context.now,
    );
    let typed = type_into(&mut state.doc, state.selection, state.stored.take(), text);
    if let Some((doc, selection)) = &typed.before_rule {
        state.history.record_rule(doc, *selection);
    }
    state.selection = Selection::caret(typed.caret);
    state.stored = typed.stored;
    state.changed();
    if text == "@" && opens_mention(&state.doc, typed.caret) {
        state.popup = Popup::Mention {
            start: Pos::new(typed.caret.block, typed.caret.offset - 1),
            selected: 0,
        };
    }
}

/// An `@` opens the mention list at a word start.
fn opens_mention(doc: &Document, caret: Pos) -> bool {
    let block = doc.block(caret.block);
    if block.kind.is_source() || caret.offset == 0 {
        return false;
    }
    let before =
        caret.offset.checked_sub(2).and_then(|index| block.content.text().chars().nth(index));
    before.is_none_or(|ch| ch.is_whitespace() || matches!(ch, '(' | '[' | '"' | '\''))
}

fn enter(state: &mut State, context: &CommandContext<'_>) {
    let caret = state.selection.head;
    let block = state.doc.block(caret.block);
    let fence = block.content.text();
    if block.kind == Kind::Paragraph
        && state.selection.is_empty()
        && caret.offset == block.len()
        && let Some(language) = fence.strip_prefix("```").filter(|rest| !rest.contains(['`', ' ']))
    {
        state.history.record_rule(&state.doc, state.selection);
        let fired = rules::code_fence(&mut state.doc, caret.block, language);
        state.selection = Selection::caret(fired.caret);
        state.changed();
        return;
    }
    edit(state, context, EditKind::Other, edit_ops::enter);
}

fn backspace(state: &mut State, context: &CommandContext<'_>) {
    if state.history.takes_backspace() && state.selection.is_empty() {
        if state.history.undo(&mut state.doc, &mut state.selection) {
            state.changed();
        }
        return;
    }
    edit(state, context, EditKind::Deleting, edit_ops::backspace);
}

fn indent(state: &mut State, context: &CommandContext<'_>, outdent: bool) {
    let caret = state.selection.head;
    if matches!(state.doc.block(caret.block).kind, Kind::Code(_)) {
        if !outdent {
            insert(state, context, "    ");
        }
        return;
    }
    let (first, last) = selected_blocks(state);
    let before = (state.doc.clone(), state.selection);
    let moved = if outdent {
        structure::outdent(&mut state.doc, first, last)
    } else {
        structure::indent(&mut state.doc, first, last)
    };
    if moved {
        state.history.record(&before.0, before.1, EditKind::Other, first, context.now);
        state.changed();
    } else {
        state.history.break_group();
    }
}

fn toggle_mark(state: &mut State, context: &CommandContext<'_>, mark: Mark) {
    if state.selection.is_empty() {
        let caret = state.selection.head;
        let marks = state
            .stored
            .clone()
            .unwrap_or_else(|| state.doc.block(caret.block).content.marks_at(caret.offset));
        state.stored = Some(edit_ops::toggled(&marks, mark));
        return;
    }
    let before = (state.doc.clone(), state.selection);
    if edit_ops::toggle_mark(&mut state.doc, state.selection, mark) {
        let block = state.selection.head.block;
        state.history.record(&before.0, before.1, EditKind::Other, block, context.now);
        state.changed();
    } else {
        state.history.break_group();
    }
}

/// The link at the caret: its range in the caret's block and the link.
pub(crate) fn link_at(doc: &Document, caret: Pos) -> Option<(std::ops::Range<usize>, Link)> {
    let content = &doc.block(caret.block).content;
    let found = content
        .link_range(caret.offset)
        .or_else(|| caret.offset.checked_sub(1).and_then(|before| content.link_range(before)));
    found.map(|(range, link)| (range, (*link).clone()))
}

fn open_link(state: &mut State) {
    let url = match link_at(&state.doc, state.selection.head) {
        Some((_, link)) => link.url,
        None => String::new(),
    };
    state.popup = Popup::Link { url, is_new: true };
}

fn set_link(state: &mut State, context: &CommandContext<'_>, link: Option<Link>) {
    let caret = state.selection.head;
    let existing = link_at(&state.doc, caret);
    let target = match (&existing, state.selection.is_empty()) {
        (Some((range, _)), true) => Selection {
            anchor: Pos::new(caret.block, range.start),
            head: Pos::new(caret.block, range.end),
        },
        _ => state.selection,
    };
    state.history.record(&state.doc, state.selection, EditKind::Other, caret.block, context.now);
    match link {
        Some(link) if target.is_empty() => {
            let text = link.url.clone();
            let end = edit_ops::insert_link(&mut state.doc, target, &text, link);
            state.selection = Selection::caret(end);
        }
        link => edit_ops::set_link(&mut state.doc, target, link),
    }
    state.changed();
}

fn move_caret(state: &mut State, context: &CommandContext<'_>, motion: Motion, extend: bool) {
    let selection = state.selection;
    let head = state.doc.clamp(selection.head);
    let (from, to) = selection.ordered();
    let ctx = context.ui.ctx();
    let doc = &state.doc;
    let layout = &mut state.layout;
    let keeps_column =
        matches!(motion, Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown);
    let target = match motion {
        Motion::Left if !extend && !selection.is_empty() => from,
        Motion::Right if !extend && !selection.is_empty() => to,
        Motion::Left => caret::step(doc, head, Direction::Backward),
        Motion::Right => caret::step(doc, head, Direction::Forward),
        Motion::WordLeft => caret::word_left(doc, head),
        Motion::WordRight => caret::word_right(doc, head),
        Motion::Up | Motion::Down => {
            let direction =
                if motion == Motion::Down { Direction::Forward } else { Direction::Backward };
            let (pos, column) =
                caret::vertical(layout, ctx, doc, context.origin, head, state.column, direction);
            state.column = Some(column);
            pos
        }
        Motion::RowStart => caret::row_edge(layout, ctx, doc, head, Direction::Backward),
        Motion::RowEnd => caret::row_edge(layout, ctx, doc, head, Direction::Forward),
        Motion::DocStart => Pos::new(0, 0),
        Motion::DocEnd => doc.end(),
        Motion::PageUp | Motion::PageDown => {
            let direction =
                if motion == Motion::PageDown { Direction::Forward } else { Direction::Backward };
            caret::page(layout, ctx, doc, context.origin, head, context.page_height, direction)
        }
    };
    if !keeps_column {
        state.column = None;
    }
    state.selection = if extend {
        Selection { anchor: selection.anchor, head: target }
    } else {
        Selection::caret(target)
    };
    state.stored = None;
    state.history.break_group();
    state.scroll_to_caret = true;
}

/// The person chosen in the mention list replaces `@query`.
pub(crate) fn choose_mention(
    state: &mut State,
    context: &CommandContext<'_>,
    start: Pos,
    key: String,
) {
    let caret = state.selection.head;
    if caret.block != start.block || caret.offset <= start.offset {
        return;
    }
    let range = Selection { anchor: start, head: caret };
    state.history.record(&state.doc, state.selection, EditKind::Other, caret.block, context.now);
    let marks = state.doc.block(caret.block).content.marks_at(start.offset);
    let at = edit_ops::insert_atom(&mut state.doc, range, Atom::Mention { key }, &marks);
    let after = edit_ops::insert_text(&mut state.doc, Selection::caret(at), " ", &marks);
    state.selection = Selection::caret(after);
    state.popup = Popup::None;
    state.changed();
}
