//! Markdown input rules, as `TipTap` has them: typing Markdown syntax formats in place. Block rules
//! fire when a paragraph starts with their pattern and the typed char completes it (`# ` to
//! `###### `, `- `/`* `/`+ `, `1. `, `[ ] `/`[x] `, `> `, `` ```lang `` + space, `---`); mark
//! rules when the closing delimiter is typed after `**x**`, `__x__`, `*x*`, `_x_`, `` `x` `` or
//! `~~x~~` that starts the text or follows a space. Rules never fire in code. Backspace right
//! after a rule undoes it (the editor arms that, see `history`).

use super::structure::{self, ListStyle};
use crate::rich_editor::model::{
    Container, Document, Inline, Kind, ListKind, Mark, Marks, Pos, Selection,
};

/// What a rule did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fired {
    pub caret: Pos,
    /// Marks for the text typed next: a mark rule ends its mark at the caret.
    pub stored: Option<Marks>,
}

/// Runs the rule that the char just typed before `caret` completes, if any.
pub(crate) fn after_typing(doc: &mut Document, caret: Pos) -> Option<Fired> {
    let block = doc.block(caret.block);
    if block.kind.is_source() || block.kind == Kind::Rule {
        return None;
    }
    let before: Vec<char> = block.content.text().chars().take(caret.offset).collect();
    let marks = block.content.marks_at(caret.offset);
    if marks.has(Mark::Code) {
        return None;
    }
    if block.kind == Kind::Paragraph
        && let Some(fired) = block_rule(doc, caret, &before)
    {
        return Some(fired);
    }
    mark_rule(doc, caret, &before)
}

fn block_rule(doc: &mut Document, caret: Pos, before: &[char]) -> Option<Fired> {
    let text: String = before.iter().collect();
    let start = Pos::new(caret.block, 0);
    let index = caret.block;
    let strip = |doc: &mut Document, length: usize| {
        doc.block_mut(index).content.remove(0..length);
    };
    if text == "---" || text == "___ " || text == "*** " {
        strip(doc, before.len());
        let after = structure::insert_rule(doc, start);
        return Some(Fired { caret: after, stored: None });
    }
    let hashes = text.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&hashes) && text.len() == hashes + 1 && text.ends_with(' ') {
        strip(doc, before.len());
        doc.block_mut(index).kind = Kind::Heading(hashes as u8);
        return Some(Fired { caret: start, stored: None });
    }
    let list = match text.as_str() {
        "- " | "* " | "+ " => Some(ListStyle::Bullet),
        "[ ] " | "[] " | "[x] " | "[X] " => Some(ListStyle::Task),
        _ => ordered_start(&text).map(|_| ListStyle::Ordered),
    };
    if let Some(style) = list {
        strip(doc, before.len());
        structure::toggle_list(doc, index, index, style);
        if let Some(number) = ordered_start(&text)
            && let Some(Container::Item { list, .. }) = doc.block_mut(index).nesting.last_mut()
        {
            *list = ListKind::Ordered(number);
        }
        if text.starts_with("[x") || text.starts_with("[X") {
            structure::toggle_task(doc, index);
        }
        return Some(Fired { caret: start, stored: None });
    }
    if text == "> " {
        strip(doc, before.len());
        structure::toggle_quote(doc, index, index);
        return Some(Fired { caret: start, stored: None });
    }
    if let Some(language) = text.strip_prefix("```").and_then(|rest| rest.strip_suffix(' ')) {
        let is_language = !language.contains(['`', ' ']);
        if is_language && caret.offset == doc.block(index).len() {
            return Some(code_fence(doc, index, language));
        }
    }
    None
}

/// The paragraph at `index` (holding only `` ```lang ``) becomes an empty code block.
pub(crate) fn code_fence(doc: &mut Document, index: usize, language: &str) -> Fired {
    let block = doc.block_mut(index);
    block.content = Inline::default();
    block.kind = Kind::Code(language.to_owned());
    Fired { caret: Pos::new(index, 0), stored: None }
}

/// `n. ` at the start of a paragraph: the number.
fn ordered_start(text: &str) -> Option<u64> {
    let number = text.strip_suffix(". ")?;
    let is_number = !number.is_empty() && number.len() <= 9;
    if is_number && number.chars().all(|ch| ch.is_ascii_digit()) {
        number.parse().ok()
    } else {
        None
    }
}

const MARK_RULES: [(&str, Mark); 6] = [
    ("**", Mark::Bold),
    ("__", Mark::Bold),
    ("~~", Mark::Strike),
    ("*", Mark::Italic),
    ("_", Mark::Italic),
    ("`", Mark::Code),
];

fn mark_rule(doc: &mut Document, caret: Pos, before: &[char]) -> Option<Fired> {
    for (delimiter, mark) in MARK_RULES {
        let Some((open, content)) = delimited(before, delimiter) else { continue };
        let width = delimiter.chars().count();
        let index = caret.block;
        let content_marks = doc.block(index).content.marks_at(content.start + 1);
        {
            let inline = &mut doc.block_mut(index).content;
            inline.remove(content.end..content.end + width);
            inline.remove(open..open + width);
        }
        let start = Pos::new(index, open);
        let end = Pos::new(index, open + content.len());
        super::toggle_mark(doc, Selection { anchor: start, head: end }, mark);
        return Some(Fired { caret: end, stored: Some(content_marks.without(mark)) });
    }
    None
}

/// `before` ends with `delimiter` + content + `delimiter`, the first delimiter at the start or
/// after whitespace, the content not starting with whitespace and free of the delimiter's char.
/// Returns where the first delimiter starts and the content's range.
fn delimited(before: &[char], delimiter: &str) -> Option<(usize, std::ops::Range<usize>)> {
    let delimiter: Vec<char> = delimiter.chars().collect();
    let width = delimiter.len();
    let mark_char = delimiter[0];
    let length = before.len();
    if length < 2 * width + 1 || before[length - width..] != delimiter[..] {
        return None;
    }
    let content_end = length - width;
    let content_start =
        before[..content_end].iter().rposition(|ch| *ch == mark_char).map(|at| at + 1)?;
    let open = content_start.checked_sub(width)?;
    if before[open..content_start] != delimiter[..] || content_start >= content_end {
        return None;
    }
    let opens_cleanly = open == 0 || before[open - 1].is_whitespace();
    let content = &before[content_start..content_end];
    let has_text = !content[0].is_whitespace();
    (opens_cleanly && has_text).then_some((open, content_start..content_end))
}
