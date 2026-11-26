//! The Markdown mode's edits: toolbar buttons and shortcuts, smart paste, Enter and Tab. Each
//! runs on the text and the text edit's selection as one undo step and keeps the caret where the
//! edit left it (`PendingCursor`).

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{Id, Ui};

use super::Options;
use super::context::FileContext;
use super::formatting::{self, Format, ListMarker, Selection};
use super::layout::WriteCache;
use super::structure::{self, InlineKind};
use super::toolbar::{Action, ActiveFormats};

/// The selection of the text edit in chars; the end of the text when it was never focused.
pub(super) fn selection_of(ui: &Ui, edit_id: Id, text: &str) -> Selection {
    let char_count = text.chars().count();
    TextEditState::load(ui.ctx(), edit_id).and_then(|state| state.cursor.char_range()).map_or(
        Selection::caret(char_count),
        |range| {
            let first = range.primary.index.0.min(range.secondary.index.0);
            let last = range.primary.index.0.max(range.secondary.index.0);
            Selection { start: first.min(char_count), end: last.min(char_count) }
        },
    )
}

/// Runs an edit on the text and selection; on change records an undo point for the text before
/// it (so Ctrl+Z steps back exactly one formatting action) and moves the caret. Returns whether
/// the text changed.
fn edit(
    ui: &Ui,
    edit_id: Id,
    text: &mut String,
    change: impl FnOnce(&mut String, Selection) -> Option<Selection>,
) -> bool {
    let mut state = TextEditState::load(ui.ctx(), edit_id).unwrap_or_default();
    let selection = selection_of(ui, edit_id, text);
    let before = text.clone();
    let Some(updated) = change(text, selection) else {
        return false;
    };
    let is_changed = *text != before;
    let previous = CCursorRange::two(CCursor::new(selection.start), CCursor::new(selection.end));
    let mut undoer = state.undoer();
    undoer.add_undo(&(previous, before));
    state.set_undoer(undoer);
    let range = CCursorRange::two(CCursor::new(updated.start), CCursor::new(updated.end));
    state.cursor.set_char_range(Some(range));
    state.store(ui.ctx(), edit_id);
    ui.memory_mut(|memory| memory.request_focus(edit_id));
    PendingCursor {
        selection: updated,
        text_hash: egui::epaint::util::hash(text.as_str()),
        frames_left: 2,
    }
    .store(ui.ctx(), edit_id);
    is_changed
}

/// The selection an edit produced, re-applied for the next frames. A toolbar click takes the
/// focus from the text edit for a frame, and an unfocused text edit collapses its selection, so
/// without this a placeholder like "bold text" would not stay selected.
#[derive(Clone, Copy)]
pub(super) struct PendingCursor {
    selection: Selection,
    text_hash: u64,
    frames_left: u8,
}

impl PendingCursor {
    fn id(edit_id: Id) -> Id {
        edit_id.with("pending_cursor")
    }

    fn store(self, ctx: &egui::Context, edit_id: Id) {
        ctx.data_mut(|data| data.insert_temp(Self::id(edit_id), self));
    }

    /// Restores the selection (and the focus) while the text is still the edited one.
    pub(super) fn restore(ctx: &egui::Context, edit_id: Id, text: &str) {
        let Some(mut pending) = ctx.data(|data| data.get_temp::<Self>(Self::id(edit_id))) else {
            return;
        };
        if pending.text_hash == egui::epaint::util::hash(text) {
            let mut state = TextEditState::load(ctx, edit_id).unwrap_or_default();
            let range = CCursorRange::two(
                CCursor::new(pending.selection.start),
                CCursor::new(pending.selection.end),
            );
            state.cursor.set_char_range(Some(range));
            state.store(ctx, edit_id);
            ctx.memory_mut(|memory| memory.request_focus(edit_id));
            pending.frames_left = pending.frames_left.saturating_sub(1);
        } else {
            pending.frames_left = 0;
        }
        if pending.frames_left == 0 {
            ctx.data_mut(|data| data.remove::<Self>(Self::id(edit_id)));
        } else {
            pending.store(ctx, edit_id);
        }
    }
}

pub(super) fn run_action(
    ui: &Ui,
    edit_id: Id,
    text: &mut String,
    cache: &mut WriteCache,
    action: Action,
    options: &Options<'_>,
) -> bool {
    match action {
        Action::Format(format) => {
            let model = cache.model_for(text);
            edit(ui, edit_id, text, |text, selection| Some(toggle(text, &model, selection, format)))
        }
        Action::Suggestion => options.suggestion.is_some_and(|lines| {
            edit(ui, edit_id, text, |text, selection| {
                Some(formatting::suggestion(text, selection, lines))
            })
        }),
        Action::Mention => edit(ui, edit_id, text, |text, selection| {
            let needs_space = selection.start > 0
                && text
                    .chars()
                    .nth(selection.start - 1)
                    .is_some_and(|character| !character.is_whitespace());
            Some(formatting::insert(text, selection, if needs_space { " @" } else { "@" }))
        }),
    }
}

/// Applies `format`, or removes it when the caret sits inside that formatting already.
fn toggle(
    text: &mut String,
    model: &structure::Model,
    selection: Selection,
    format: Format,
) -> Selection {
    let start = formatting::byte_offset(text, selection.start);
    let end = formatting::byte_offset(text, selection.end);
    let inline = match format {
        Format::Bold => Some(InlineKind::Bold),
        Format::Italic => Some(InlineKind::Italic),
        Format::Strikethrough => Some(InlineKind::Strike),
        Format::InlineCode => Some(InlineKind::Code),
        Format::Link => Some(InlineKind::Link),
        _ => None,
    };
    if let Some(kind) = inline
        && let Some(span) = enclosing_span(model, kind, start, end)
    {
        let span = span.clone();
        return match kind {
            InlineKind::Link => formatting::unlink(text, span.range, selection),
            _ => formatting::unwrap_span(text, span.range, span.marker, selection),
        };
    }
    if format == Format::CodeBlock && model.is_in_code_block(start) {
        return selection;
    }
    formatting::apply(text, selection, format)
}

/// The span of `kind` around the selection; for a bare caret only when it is strictly inside.
fn enclosing_span(
    model: &structure::Model,
    kind: InlineKind,
    start: usize,
    end: usize,
) -> Option<&structure::InlineSpan> {
    let span = model.span_around(kind, start, end)?;
    let is_caret = start == end;
    let is_inside = !is_caret || (span.range.start < start && end < span.range.end);
    is_inside.then_some(span)
}

pub(super) fn active_formats(
    text: &str,
    model: &structure::Model,
    selection: Selection,
) -> ActiveFormats {
    let mut active = ActiveFormats::default();
    let start = formatting::byte_offset(text, selection.start);
    let end = formatting::byte_offset(text, selection.end);
    let pairs = [
        (InlineKind::Bold, Format::Bold),
        (InlineKind::Italic, Format::Italic),
        (InlineKind::Strike, Format::Strikethrough),
        (InlineKind::Code, Format::InlineCode),
        (InlineKind::Link, Format::Link),
    ];
    for (kind, format) in pairs {
        if enclosing_span(model, kind, start, end).is_some() {
            active.insert(format);
        }
    }
    if model.is_in_heading(start, text.len()) {
        active.insert(Format::Heading);
    }
    if model.is_in_code_block(start) {
        active.insert(Format::CodeBlock);
    }
    if formatting::is_quote_at(text, start) {
        active.insert(Format::Quote);
    }
    match formatting::list_at(text, start) {
        Some(ListMarker::Bullet(_)) => active.insert(Format::BulletList),
        Some(ListMarker::Numbered { .. }) => active.insert(Format::NumberedList),
        Some(ListMarker::Task { .. }) => active.insert(Format::TaskList),
        None => {}
    }
    active
}

/// Ctrl+V: a URL over selected text becomes a link; whole lines copied from the diff become a
/// code block in the file's language; anything else is pasted as it is.
pub(super) fn smart_paste(
    ui: &Ui,
    edit_id: Id,
    text: &mut String,
    cache: &mut WriteCache,
    pasted: &str,
) -> bool {
    let model = cache.model_for(text);
    let file = FileContext::current(ui.ctx());
    edit(ui, edit_id, text, |text, selection| {
        let start = formatting::byte_offset(text, selection.start);
        let end = formatting::byte_offset(text, selection.end);
        let is_in_code = model.is_in_code_block(start)
            || enclosing_span(&model, InlineKind::Code, start, end).is_some();
        let selected = &text[start..end];
        if !selection.is_empty()
            && is_url(pasted)
            && !selected.contains('\n')
            && !is_url(selected)
            && !is_in_code
        {
            return Some(formatting::link_to(text, selection, pasted.trim()));
        }
        if !is_in_code
            && let Some(file) = &file
            && file.contains_code(pasted)
        {
            return Some(formatting::fenced_paste(text, selection, pasted, &file.language()));
        }
        Some(formatting::insert(text, selection, pasted))
    })
}

fn is_url(text: &str) -> bool {
    let text = text.trim();
    (text.starts_with("https://") || text.starts_with("http://"))
        && text.len() > 8
        && !text.contains(char::is_whitespace)
}

/// Enter: continue or end a list or quote, keep the indentation inside code, else a plain newline.
pub(super) fn newline(ui: &Ui, edit_id: Id, text: &mut String, cache: &mut WriteCache) -> bool {
    let model = cache.model_for(text);
    edit(ui, edit_id, text, |text, selection| {
        let start = formatting::byte_offset(text, selection.start);
        if model.is_in_code_block(start) {
            return Some(formatting::newline_with_indent(text, selection));
        }
        formatting::continue_block(text, selection)
            .or_else(|| Some(formatting::insert(text, selection, "\n")))
    })
}

/// Tab / Shift+Tab: nest list items, or indent code inside a code block. Elsewhere Tab moves the
/// focus on (the text edit only locks Tab while the caret is in a list or code block).
pub(super) fn indent(
    ui: &Ui,
    edit_id: Id,
    text: &mut String,
    cache: &mut WriteCache,
    is_outdent: bool,
) -> bool {
    let model = cache.model_for(text);
    edit(ui, edit_id, text, |text, selection| {
        formatting::indent_items(text, selection, is_outdent).or_else(|| {
            let start = formatting::byte_offset(text, selection.start);
            (model.is_in_code_block(start) && !is_outdent)
                .then(|| formatting::insert(text, selection, "    "))
        })
    })
}
