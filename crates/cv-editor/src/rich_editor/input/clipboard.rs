//! Copy writes the selection as Markdown; paste reads Markdown as Markdown, links a selection
//! to a pasted URL and keeps other text plain.

use std::sync::Arc;

use super::{Frame, edit, set_link};
use crate::rich_editor::edit::{self as edit_ops};
use crate::rich_editor::history::EditKind;
use crate::rich_editor::model::{Block, Document, HARD_BREAK, Inline, Link, Marks};
use crate::rich_editor::{State, parse, serialize};

/// The selection as Markdown for the clipboard; `None` without a selection.
pub fn copy(state: &State) -> Option<String> {
    if state.selection.is_empty() {
        return None;
    }
    let fragment = edit_ops::fragment(&state.doc, state.selection);
    Some(serialize::serialize(&fragment, state.platform))
}

/// A URL pasted over a selection links it; Markdown is read as Markdown; other text is plain,
/// one paragraph per line.
pub fn paste(state: &mut State, frame: &Frame<'_>, text: &str) {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    if !state.selection.is_empty() && is_url(trimmed) {
        set_link(state, frame, Some(Link { url: trimmed.to_owned(), title: String::new() }));
        return;
    }
    let target = state.doc.block(state.selection.head.block);
    if target.kind.is_source() {
        edit(state, frame, EditKind::Other, |doc, selection| {
            edit_ops::insert_text(doc, selection, &text, &Marks::default())
        });
        return;
    }
    let fragment = if looks_like_markdown(&text) {
        parse::parse(&text, state.platform)
    } else {
        plain_fragment(&text)
    };
    edit(state, frame, EditKind::Other, |doc, selection| {
        edit_ops::insert_fragment(doc, selection, &fragment)
    });
}

pub fn is_url(text: &str) -> bool {
    (text.starts_with("https://") || text.starts_with("http://"))
        && !text.contains(char::is_whitespace)
}

/// Whether pasted text is written in Markdown: a block marker at a line start, or inline syntax.
fn looks_like_markdown(text: &str) -> bool {
    let block = text.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("# ")
            || line.starts_with("## ")
            || line.starts_with("### ")
            || line.starts_with("- ")
            || line.starts_with("* ")
            || line.starts_with("> ")
            || line.starts_with("```")
            || line.starts_with("| ")
            || line.split_once(". ").is_some_and(|(number, _)| {
                !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit())
            })
    });
    let inline = ["**", "](", "~~", "`"].iter().any(|syntax| text.contains(syntax));
    block || inline
}

/// Plain text as paragraphs, one per non-empty line; a single line stays inline.
fn plain_fragment(text: &str) -> Document {
    let blocks = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            Arc::new(Block::paragraph(Vec::new(), Inline::plain(&line.replace(HARD_BREAK, " "))))
        })
        .collect();
    let mut fragment = Document { blocks };
    fragment.normalize();
    fragment
}
