//! Structural edits: block kinds, lists (wrap, unwrap, change, nest, lift), quotes, rules and
//! task boxes. They act on whole blocks (`first..=last`); an item's blocks are its head and the
//! blocks after it that continue it at its depth (its further paragraphs and nested lists).

use std::sync::Arc;

use super::{continuation, split_block};
use crate::rich_editor::model::{
    Block, Container, Document, HARD_BREAK, Inline, Kind, ListKind, Marks, Pos, same_list,
};

/// The list a toolbar button or shortcut makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListStyle {
    Bullet,
    Ordered,
    Task,
}

impl ListStyle {
    fn matches(self, container: &Container) -> bool {
        matches!(
            (self, container),
            (ListStyle::Bullet, Container::Item { list: ListKind::Bullet, task: None, .. })
                | (ListStyle::Ordered, Container::Item { list: ListKind::Ordered(_), .. })
                | (ListStyle::Task, Container::Item { task: Some(_), .. })
        )
    }

    fn apply(self, container: &mut Container) {
        if let Container::Item { list, task, .. } = container {
            match self {
                ListStyle::Bullet => (*list, *task) = (ListKind::Bullet, None),
                ListStyle::Ordered => {
                    if !matches!(list, ListKind::Ordered(_)) {
                        *list = ListKind::Ordered(1);
                    }
                    *task = None;
                }
                ListStyle::Task => {
                    *list = ListKind::Bullet;
                    task.get_or_insert(false);
                }
            }
        }
    }

    fn container(self) -> Container {
        let mut container = Container::Item { list: ListKind::Bullet, task: None, head: true };
        self.apply(&mut container);
        container
    }
}

/// The index of the innermost item container of block `index`.
fn item_depth(doc: &Document, index: usize) -> Option<usize> {
    doc.block(index).item().map(|(depth, _)| depth)
}

/// The head block of the item at `depth` that block `index` is in.
fn item_head(doc: &Document, index: usize, depth: usize) -> usize {
    let mut head = index;
    while head > 0
        && matches!(doc.block(head).nesting.get(depth), Some(Container::Item { head: false, .. }))
    {
        head -= 1;
    }
    head
}

/// One past the last block of the item at `depth` whose head is `head`.
fn item_end(doc: &Document, head: usize, depth: usize) -> usize {
    let mut end = head + 1;
    while end < doc.blocks.len()
        && matches!(doc.block(end).nesting.get(depth), Some(Container::Item { head: false, .. }))
    {
        end += 1;
    }
    end
}

/// Takes block `index` (with its item, if its innermost container is one) out of its innermost
/// container: a nested item becomes an item of the parent list, a top-level item a paragraph, a
/// quoted block an unquoted one.
pub fn lift(doc: &mut Document, index: usize) {
    match doc.block(index).nesting.last() {
        Some(Container::Quote(_)) => {
            doc.block_mut(index).nesting.pop();
            doc.normalize();
        }
        Some(Container::Item { .. }) => {
            outdent(doc, index, index);
        }
        None => {}
    }
}

/// Shift+Tab: the items of `first..=last` move one level out. Returns whether anything moved.
pub fn outdent(doc: &mut Document, first: usize, last: usize) -> bool {
    let Some(depth) = item_depth(doc, first) else { return false };
    let head = item_head(doc, first, depth);
    let last_head =
        if doc.block(last).nesting.len() > depth { item_head(doc, last, depth) } else { head };
    let end = item_end(doc, last_head.max(head), depth);
    let parent = doc.block(head).nesting[..depth].iter().rposition(Container::is_item);
    let removed = parent.unwrap_or(depth);
    for index in head..end {
        let block = doc.block_mut(index);
        if block.nesting.len() > removed {
            block.nesting.remove(removed);
        }
    }
    doc.normalize();
    true
}

/// Tab: the items of `first..=last` become children of the item before the first. Nothing
/// happens for the first item of a list (it has no item to go into).
pub fn indent(doc: &mut Document, first: usize, last: usize) -> bool {
    let Some(depth) = item_depth(doc, first) else { return false };
    let head = item_head(doc, first, depth);
    let Some(previous) = head.checked_sub(1).map(|before| doc.block(before).clone()) else {
        return false;
    };
    let here = doc.block(head).nesting.clone();
    let (Some(Container::Item { list, .. }), Some(Container::Item { list: before, task, .. })) =
        (here.get(depth), previous.nesting.get(depth))
    else {
        return false;
    };
    let shallower_continue =
        here[..depth].iter().zip(&previous.nesting[..depth]).all(|(a, b)| a.continues(b));
    if !same_list(*list, *before) || !shallower_continue {
        return false;
    }
    let last_head =
        if doc.block(last).nesting.len() > depth { item_head(doc, last, depth) } else { head };
    let end = item_end(doc, last_head.max(head), depth);
    let parent = Container::Item { list: *before, task: *task, head: false };
    for index in head..end {
        let block = doc.block_mut(index);
        if block.nesting.len() > depth {
            block.nesting.insert(depth, parent);
        }
    }
    doc.normalize();
    true
}

/// Turns `first..=last` into a list of `style`, changes the list they are in to it, or, when
/// they all are one already, takes them out of it.
pub fn toggle_list(doc: &mut Document, first: usize, last: usize, style: ListStyle) {
    let targets: Vec<usize> =
        (first..=last).filter(|&index| doc.block(index).kind != Kind::Rule).collect();
    let all_match = !targets.is_empty()
        && targets.iter().all(|&index| {
            doc.block(index).item().is_some_and(|(_, container)| style.matches(&container))
        });
    if all_match {
        for &index in &targets {
            if let Some((depth, _)) = doc.block(index).item() {
                doc.block_mut(index).nesting.remove(depth);
            }
        }
        doc.normalize();
        return;
    }
    for &index in &targets {
        match doc.block(index).item() {
            Some((depth, _)) => restyle_list(doc, index, depth, style),
            None => doc.block_mut(index).nesting.push(style.container()),
        }
    }
    doc.normalize();
}

/// Changes the whole list at `depth` around block `index` to `style`.
fn restyle_list(doc: &mut Document, index: usize, depth: usize, style: ListStyle) {
    let in_same_list = |doc: &Document, at: usize| {
        let (here, there) = (&doc.block(at).nesting, &doc.block(index).nesting);
        here.len() > depth
            && here[..depth].iter().zip(&there[..depth]).all(|(a, b)| a == b || a.continues(b))
            && matches!((here.get(depth), there.get(depth)),
                (Some(Container::Item { list: a, .. }), Some(Container::Item { list: b, .. })) if same_list(*a, *b))
    };
    let mut start = index;
    while start > 0 && in_same_list(doc, start - 1) {
        start -= 1;
    }
    let mut end = index + 1;
    while end < doc.blocks.len() && in_same_list(doc, end) {
        end += 1;
    }
    for at in start..end {
        if let Some(container) = doc.block_mut(at).nesting.get_mut(depth) {
            style.apply(container);
        }
    }
}

/// Quotes `first..=last`, or unquotes them when they all are quoted.
pub fn toggle_quote(doc: &mut Document, first: usize, last: usize) {
    let all_quoted = (first..=last).all(|index| {
        doc.block(index).nesting.iter().any(|container| matches!(container, Container::Quote(_)))
    });
    for index in first..=last {
        let nesting = &mut doc.block_mut(index).nesting;
        if all_quoted {
            if let Some(at) =
                nesting.iter().rposition(|container| matches!(container, Container::Quote(_)))
            {
                nesting.remove(at);
            }
        } else {
            nesting.push(Container::Quote(None));
        }
    }
    doc.normalize();
}

/// Makes `first..=last` headings of `level`, or paragraphs when they all are already.
pub fn toggle_heading(doc: &mut Document, first: usize, last: usize, level: u8) {
    let all_match = (first..=last).all(|index| doc.block(index).kind == Kind::Heading(level));
    let kind = if all_match { Kind::Paragraph } else { Kind::Heading(level) };
    set_kind(doc, first, last, &kind);
}

/// Gives `first..=last` the text kind `kind` (paragraph or heading); rules stay.
pub fn set_kind(doc: &mut Document, first: usize, last: usize, kind: &Kind) {
    for index in first..=last {
        if doc.block(index).kind == Kind::Rule {
            continue;
        }
        let block = doc.block_mut(index);
        if block.kind.is_source() {
            block.content =
                block.content.unmarked(|text| text.replace('\n', &HARD_BREAK.to_string()));
        }
        block.kind = kind.clone();
    }
}

/// Makes `first..=last` one code block, or turns code blocks back into paragraphs. Returns the
/// caret.
pub fn toggle_code_block(doc: &mut Document, first: usize, last: usize) -> Pos {
    let all_code = (first..=last).all(|index| matches!(doc.block(index).kind, Kind::Code(_)));
    if all_code {
        set_kind(doc, first, last, &Kind::Paragraph);
        return Pos::new(last, doc.block(last).len());
    }
    let content = source_lines(
        (first..=last).map(|index| doc.block(index)).filter(|block| block.kind != Kind::Rule),
    );
    let nesting = doc.block(first).nesting.clone();
    doc.blocks.drain(first + 1..=last);
    doc.blocks[first] = Arc::new(Block { kind: Kind::Code(String::new()), nesting, content });
    doc.normalize();
    Pos::new(first, doc.block(first).len())
}

/// The content of `blocks` unmarked for a source block, each block on a line of its own.
pub fn source_lines<'a>(blocks: impl IntoIterator<Item = &'a Block>) -> Inline {
    let mut lines = Inline::default();
    for (index, block) in blocks.into_iter().enumerate() {
        if index > 0 {
            lines.push_text("\n", &Marks::default());
        }
        lines.append(block.content.unmarked(|text| text.replace(HARD_BREAK, "\n")));
    }
    lines
}

/// Inserts a horizontal rule at the caret and puts the caret in the paragraph after it.
pub fn insert_rule(doc: &mut Document, at: Pos) -> Pos {
    let at = doc.clamp(at);
    let block = doc.block(at.block).clone();
    let rule_at = if block.content.is_empty() && block.kind == Kind::Paragraph {
        doc.block_mut(at.block).kind = Kind::Rule;
        at.block
    } else if at.offset == 0 {
        let nesting = block.nesting;
        doc.blocks.insert(
            at.block,
            Arc::new(Block { kind: Kind::Rule, nesting, content: Inline::default() }),
        );
        doc.normalize();
        return Pos::new(at.block + 1, 0);
    } else {
        let after = split_block(doc, at);
        if doc.block(after.block).content.is_empty() {
            doc.block_mut(after.block).kind = Kind::Rule;
            after.block
        } else {
            let nesting = continuation(&block.nesting);
            doc.blocks.insert(
                after.block,
                Arc::new(Block { kind: Kind::Rule, nesting, content: Inline::default() }),
            );
            doc.normalize();
            return Pos::new(after.block + 1, 0);
        }
    };
    let nesting = continuation(&doc.block(rule_at).nesting);
    doc.blocks.insert(rule_at + 1, Arc::new(Block::paragraph(nesting, Inline::default())));
    doc.normalize();
    Pos::new(rule_at + 1, 0)
}

/// Checks or clears the task box of the item block `index` is in. Returns whether it had one.
pub fn toggle_task(doc: &mut Document, index: usize) -> bool {
    let Some((depth, Container::Item { task: Some(checked), .. })) = doc.block(index).item() else {
        return false;
    };
    let head = item_head(doc, index, depth);
    let end = item_end(doc, head, depth);
    for at in head..end {
        if let Some(Container::Item { task, .. }) = doc.block_mut(at).nesting.get_mut(depth) {
            *task = Some(!checked);
        }
    }
    true
}
