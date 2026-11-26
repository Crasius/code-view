//! Text edits on the document: insert, delete, split and join blocks, marks and links, pasted
//! fragments. Pure functions of the document and a selection that return the new selection;
//! structural edits (lists, quotes, block kinds) are in `structure`.

pub mod rules;
pub mod structure;

use std::sync::Arc;

use super::model::{
    Atom, Block, Container, Document, HARD_BREAK, Inline, Kind, Link, Mark, Marks, Pos, Selection,
};

/// Removes the text between `from` and `to` (in document order); returns where the caret goes.
pub fn delete_range(doc: &mut Document, from: Pos, to: Pos) -> Pos {
    let (from, to) = (doc.clamp(from), doc.clamp(to));
    if from >= to {
        return from;
    }
    if from.block == to.block {
        doc.block_mut(from.block).content.remove(from.offset..to.offset);
        return from;
    }
    let last = doc.block(to.block).clone();
    let tail = last.content.slice(to.offset..last.len());
    let first = doc.block_mut(from.block);
    if first.kind == Kind::Rule {
        first.kind.clone_from(&last.kind);
        first.nesting.clone_from(&last.nesting);
    }
    let keep = first.content.slice(0..from.offset);
    first.content = keep;
    append_content(first, tail, &last.kind);
    doc.blocks.drain(from.block + 1..=to.block);
    doc.normalize();
    from
}

/// Appends `content` from a block of kind `source` to `block`: unmarked into source blocks.
fn append_content(block: &mut Block, content: Inline, source: &Kind) {
    if block.kind.is_source() && !source.is_source() {
        block.content.append(content.unmarked(str::to_owned));
    } else if !block.kind.is_source() && source.is_source() {
        block.content.append(content.unmarked(|text| text.replace('\n', &HARD_BREAK.to_string())));
    } else {
        block.content.append(content);
    }
}

/// Replaces the selection with `text` in `marks`; returns the caret after it.
pub fn insert_text(doc: &mut Document, selection: Selection, text: &str, marks: &Marks) -> Pos {
    let (from, to) = selection.ordered();
    let at = delete_range(doc, from, to);
    let at = leave_rule(doc, at);
    let block = doc.block_mut(at.block);
    let inserted = if block.kind.is_source() {
        Inline::plain(text)
    } else {
        let mut inline = Inline::default();
        inline.push_text(text, marks);
        inline
    };
    let length = inserted.len();
    block.content.insert(at.offset, inserted);
    Pos::new(at.block, at.offset + length)
}

/// Inserts an atom (a mention) at the caret.
pub fn insert_atom(doc: &mut Document, selection: Selection, atom: Atom, marks: &Marks) -> Pos {
    let (from, to) = selection.ordered();
    let deleted = delete_range(doc, from, to);
    let at = leave_rule(doc, deleted);
    if doc.block(at.block).kind.is_source() {
        return at;
    }
    let mut inline = Inline::default();
    inline.push_atom(atom, marks);
    doc.block_mut(at.block).content.insert(at.offset, inline);
    Pos::new(at.block, at.offset + 1)
}

/// Text typed on a rule goes into a new paragraph after it.
fn leave_rule(doc: &mut Document, at: Pos) -> Pos {
    if doc.block(at.block).kind != Kind::Rule {
        return at;
    }
    let nesting = continuation(&doc.block(at.block).nesting);
    doc.blocks.insert(at.block + 1, Arc::new(Block::paragraph(nesting, Inline::default())));
    Pos::new(at.block + 1, 0)
}

/// The nesting of a block that continues `nesting`'s containers (no new item).
pub fn continuation(nesting: &[Container]) -> Vec<Container> {
    nesting
        .iter()
        .map(|container| match *container {
            Container::Item { list, task, .. } => Container::Item { list, task, head: false },
            quote @ Container::Quote(_) => quote,
        })
        .collect()
}

/// Enter: a new line in source blocks (three Enters at the end leave a code block), out of an
/// empty item or quote, else a new block (a new item inside a list).
pub fn enter(doc: &mut Document, selection: Selection) -> Pos {
    let (from, to) = selection.ordered();
    let at = delete_range(doc, from, to);
    let block = doc.block(at.block).clone();
    if let Kind::Code(_) = block.kind {
        let text = block.content.text();
        if at.offset == block.len() && text.ends_with("\n\n") {
            let length = block.len();
            doc.block_mut(at.block).content.remove(length - 2..length);
            let nesting = continuation(&block.nesting);
            doc.blocks.insert(at.block + 1, Arc::new(Block::paragraph(nesting, Inline::default())));
            return Pos::new(at.block + 1, 0);
        }
    }
    if block.kind.is_source() {
        return insert_text(doc, Selection::caret(at), "\n", &Marks::default());
    }
    if block.content.is_empty() && !block.nesting.is_empty() {
        structure::lift(doc, at.block);
        return Pos::new(at.block, 0);
    }
    split_block(doc, at)
}

/// Shift+Enter: a hard line break (a newline in source blocks).
pub fn line_break(doc: &mut Document, selection: Selection) -> Pos {
    let (from, to) = selection.ordered();
    let at = delete_range(doc, from, to);
    let marks = doc.block(at.block).content.marks_at(at.offset);
    let text = if doc.block(at.block).kind.is_source() { '\n' } else { HARD_BREAK };
    insert_text(doc, Selection::caret(at), &text.to_string(), &marks)
}

/// Splits the block at `at`; the part after it becomes the next block (the next item in a
/// list, a paragraph after a heading's end).
pub fn split_block(doc: &mut Document, at: Pos) -> Pos {
    let at = doc.clamp(at);
    let block = doc.block_mut(at.block);
    if block.kind == Kind::Rule {
        return leave_rule(doc, at);
    }
    let tail = block.content.split_off(at.offset);
    let kind = match &block.kind {
        Kind::Heading(_) if tail.is_empty() => Kind::Paragraph,
        other => other.clone(),
    };
    let mut nesting = continuation(&block.nesting);
    if let Some(Container::Item { task, head, .. }) =
        nesting.iter_mut().rev().find(|container| container.is_item())
    {
        *head = true;
        if task.is_some() {
            *task = Some(false);
        }
    }
    doc.blocks.insert(at.block + 1, Arc::new(Block { kind, nesting, content: tail }));
    Pos::new(at.block + 1, 0)
}

/// Backspace: the selection, the char before the caret, or at a block's start: lift the block
/// out of its innermost container, turn a heading or an empty code block into a paragraph, or
/// join the block to the one before.
pub fn backspace(doc: &mut Document, selection: Selection) -> Pos {
    let (from, to) = selection.ordered();
    if from != to {
        return delete_range(doc, from, to);
    }
    let at = doc.clamp(from);
    if at.offset > 0 {
        return delete_range(doc, Pos::new(at.block, at.offset - 1), at);
    }
    let block = doc.block(at.block).clone();
    match block.nesting.last() {
        Some(Container::Item { head: true, .. } | Container::Quote(_)) => {
            structure::lift(doc, at.block);
            return at;
        }
        Some(Container::Item { head: false, .. }) | None => {}
    }
    let is_empty_source = block.kind.is_source() && block.content.is_empty();
    if matches!(block.kind, Kind::Heading(_)) || is_empty_source {
        doc.block_mut(at.block).kind = Kind::Paragraph;
        return at;
    }
    if block.kind.is_source() || at.block == 0 {
        return at;
    }
    join_backward(doc, at.block)
}

/// Joins block `index` into the one before it; a rule before it is deleted instead.
fn join_backward(doc: &mut Document, index: usize) -> Pos {
    let previous = doc.block(index - 1).clone();
    if previous.kind == Kind::Rule {
        doc.blocks.remove(index - 1);
        doc.normalize();
        return Pos::new(index - 1, 0);
    }
    let end = Pos::new(index - 1, previous.len());
    delete_range(doc, end, Pos::new(index, 0))
}

/// Delete: the selection, the char after the caret, or at a block's end the next block joined in.
pub fn delete_forward(doc: &mut Document, selection: Selection) -> Pos {
    let (from, to) = selection.ordered();
    if from != to {
        return delete_range(doc, from, to);
    }
    let at = doc.clamp(from);
    let length = doc.block(at.block).len();
    if at.offset < length {
        return delete_range(doc, at, Pos::new(at.block, at.offset + 1));
    }
    if at.block + 1 >= doc.blocks.len() {
        return at;
    }
    if doc.block(at.block + 1).kind == Kind::Rule {
        doc.blocks.remove(at.block + 1);
        doc.normalize();
        return at;
    }
    delete_range(doc, at, Pos::new(at.block + 1, 0))
}

/// The char ranges of each text block the selection covers, as (block, start..end).
pub fn text_ranges(doc: &Document, selection: Selection) -> Vec<(usize, std::ops::Range<usize>)> {
    let (from, to) = selection.ordered();
    let (from, to) = (doc.clamp(from), doc.clamp(to));
    (from.block..=to.block)
        .filter(|&index| !doc.block(index).kind.is_source())
        .map(|index| {
            let start = if index == from.block { from.offset } else { 0 };
            let end = if index == to.block { to.offset } else { doc.block(index).len() };
            (index, start..end)
        })
        .filter(|(_, range)| !range.is_empty())
        .collect()
}

/// Whether every selected char carries `mark`.
pub fn has_mark(doc: &Document, selection: Selection, mark: Mark) -> bool {
    let ranges = text_ranges(doc, selection);
    !ranges.is_empty()
        && ranges.iter().all(|(index, range)| {
            doc.block(*index).content.all_marked(range.clone(), |m| m.has(mark))
        })
}

/// Adds `mark` to the selection, or removes it where all of the selection has it. Subscript and
/// superscript exclude each other. Returns whether anything changed.
pub fn toggle_mark(doc: &mut Document, selection: Selection, mark: Mark) -> bool {
    let ranges = text_ranges(doc, selection);
    if ranges.is_empty() {
        return false;
    }
    let is_on = !has_mark(doc, selection, mark);
    for (index, range) in ranges {
        doc.block_mut(index).content.map_marks(range, |marks| {
            marks.set(mark, is_on);
            if is_on {
                match mark {
                    Mark::Subscript => marks.set(Mark::Superscript, false),
                    Mark::Superscript => marks.set(Mark::Subscript, false),
                    _ => {}
                }
            }
        });
    }
    true
}

/// Stored marks after a toggle at a bare caret.
pub fn toggled(marks: &Marks, mark: Mark) -> Marks {
    let is_on = !marks.has(mark);
    let mut marks = marks.clone();
    marks.set(mark, is_on);
    if is_on {
        match mark {
            Mark::Subscript => marks.set(Mark::Superscript, false),
            Mark::Superscript => marks.set(Mark::Subscript, false),
            _ => {}
        }
    }
    marks
}

/// Links the selection to `link`, or unlinks it with `None`.
pub fn set_link(doc: &mut Document, selection: Selection, link: Option<Link>) {
    let link = link.map(Arc::new);
    for (index, range) in text_ranges(doc, selection) {
        doc.block_mut(index).content.map_marks(range, |marks| marks.link.clone_from(&link));
    }
}

/// Inserts `text` linked to `link` at the caret (Ctrl+K without a selection).
pub fn insert_link(doc: &mut Document, selection: Selection, text: &str, link: Link) -> Pos {
    let (from, to) = selection.ordered();
    let at = delete_range(doc, from, to);
    let mut marks = doc.block(at.block).content.marks_at(at.offset);
    marks.link = Some(Arc::new(link));
    insert_text(doc, Selection::caret(at), text, &marks)
}

/// The selected part of the document as a document of its own (for copy).
pub fn fragment(doc: &Document, selection: Selection) -> Document {
    let (from, to) = selection.ordered();
    let (from, to) = (doc.clamp(from), doc.clamp(to));
    let mut blocks = Vec::new();
    for index in from.block..=to.block {
        let block = doc.block(index);
        let start = if index == from.block { from.offset } else { 0 };
        let end = if index == to.block { to.offset } else { block.len() };
        let mut part = block.clone();
        part.content = block.content.slice(start..end);
        blocks.push(Arc::new(part));
    }
    let mut fragment = Document { blocks };
    fragment.normalize();
    fragment
}

/// Replaces the selection with `fragment` (a paste). One plain paragraph goes into the text at
/// the caret; more blocks split the block at the caret and go between its halves, inside its
/// containers. Returns the caret after the pasted content.
pub fn insert_fragment(doc: &mut Document, selection: Selection, fragment: &Document) -> Pos {
    let (from, to) = selection.ordered();
    let deleted = delete_range(doc, from, to);
    let at = leave_rule(doc, deleted);
    let Some((first, rest)) = fragment.blocks.split_first() else { return at };
    let target = doc.block(at.block).clone();
    let is_inline = rest.is_empty() && first.kind == Kind::Paragraph && first.nesting.is_empty();
    if is_inline || target.kind.is_source() {
        let content =
            if target.kind.is_source() { source_lines(fragment) } else { first.content.clone() };
        let length = content.len();
        doc.block_mut(at.block).content.insert(at.offset, content);
        return Pos::new(at.block, at.offset + length);
    }
    let tail = doc.block_mut(at.block).content.split_off(at.offset);
    let base = continuation(&target.nesting);
    let mut index = at.block;
    let mut caret = at;
    let starts_inline = first.kind == Kind::Paragraph && first.nesting.is_empty();
    let pasted = if starts_inline {
        let block = doc.block_mut(at.block);
        let length = first.content.len();
        block.content.append(first.content.clone());
        caret = Pos::new(at.block, at.offset + length);
        rest
    } else {
        &fragment.blocks[..]
    };
    for block in pasted {
        index += 1;
        let mut nesting = base.clone();
        nesting.extend(block.nesting.iter().copied());
        let length = block.len();
        doc.blocks.insert(index, Arc::new(Block { nesting, ..(**block).clone() }));
        caret = Pos::new(index, length);
    }
    if !tail.is_empty() {
        doc.blocks.insert(index + 1, Arc::new(Block { content: tail, ..target }));
        if let Some(block) = doc.blocks.get_mut(index + 1) {
            Arc::make_mut(block).nesting = base;
        }
    }
    doc.normalize();
    caret
}

/// A fragment's content unmarked, its blocks on lines of their own (pasting into code).
fn source_lines(fragment: &Document) -> Inline {
    structure::source_lines(fragment.blocks.iter().map(|block| &**block))
}
