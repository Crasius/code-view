//! Markdown edits on a text and a selection, in character offsets (what egui's cursor uses).
//! Every edit produces plain Markdown that GitHub, GitLab and Azure DevOps render the same way.

use std::ops::Range;

use super::structure::InlineKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Bold,
    Italic,
    Strikethrough,
    InlineCode,
    Link,
    Heading,
    Quote,
    BulletList,
    NumberedList,
    TaskList,
    CodeBlock,
}

impl Format {
    /// The span this format writes, `None` for the line and block formats.
    pub fn inline_kind(self) -> Option<InlineKind> {
        match self {
            Self::Bold => Some(InlineKind::Bold),
            Self::Italic => Some(InlineKind::Italic),
            Self::Strikethrough => Some(InlineKind::Strike),
            Self::InlineCode => Some(InlineKind::Code),
            Self::Link => Some(InlineKind::Link),
            Self::Heading
            | Self::Quote
            | Self::BulletList
            | Self::NumberedList
            | Self::TaskList
            | Self::CodeBlock => None,
        }
    }
}

/// Which way Tab moves list items: one level in, or (Shift+Tab) one level out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indent {
    In,
    Out,
}

/// Selection in character offsets, `start <= end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub start: usize,
    pub end: usize,
}

impl Selection {
    pub fn caret(at: usize) -> Self {
        Self { start: at, end: at }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// Applies `format` and returns the new selection.
pub fn apply(text: &mut String, selection: Selection, format: Format) -> Selection {
    match format {
        Format::Bold => wrap(text, selection, "**", "**", "bold text"),
        Format::Italic => wrap(text, selection, "_", "_", "italic text"),
        Format::Strikethrough => wrap(text, selection, "~~", "~~", "struck text"),
        Format::InlineCode => wrap(text, selection, "`", "`", "code"),
        Format::Link => link(text, selection),
        Format::CodeBlock => code_block(text, selection),
        Format::Heading => heading(text, selection),
        Format::Quote => prefix_lines(text, selection, Prefix::Stacks, |_| "> ".into()),
        Format::BulletList => {
            prefix_lines(text, selection, Prefix::ReplacesMarker, |_| "- ".into())
        }
        Format::NumberedList => prefix_lines(text, selection, Prefix::ReplacesMarker, |index| {
            format!("{}. ", index + 1)
        }),
        Format::TaskList => {
            prefix_lines(text, selection, Prefix::ReplacesMarker, |_| "- [ ] ".into())
        }
    }
}

pub fn byte_offset(text: &str, char_offset: usize) -> usize {
    // Most comments are ASCII; skip the char walk over long ones.
    if text.is_ascii() {
        return char_offset.min(text.len());
    }
    text.char_indices().nth(char_offset).map_or(text.len(), |(byte, _)| byte)
}

pub fn char_offset(text: &str, byte_offset: usize) -> usize {
    text[..byte_offset.min(text.len())].chars().count()
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

fn byte_range(text: &str, selection: Selection) -> Range<usize> {
    byte_offset(text, selection.start)..byte_offset(text, selection.end)
}

/// Wraps the selection; with nothing selected inserts a placeholder and selects it.
/// Wrapping an already wrapped selection removes the markers again.
fn wrap(
    text: &mut String,
    selection: Selection,
    before: &str,
    after: &str,
    placeholder: &str,
) -> Selection {
    let Range { start, end } = byte_range(text, selection);
    let is_wrapped = text[..start].ends_with(before) && text[end..].starts_with(after);
    if is_wrapped && start != end {
        text.replace_range(end..end + after.len(), "");
        text.replace_range(start - before.len()..start, "");
        let shift = char_len(before);
        return Selection { start: selection.start - shift, end: selection.end - shift };
    }
    let inner = if start == end { placeholder.to_owned() } else { text[start..end].to_owned() };
    let replacement = format!("{before}{inner}{after}");
    text.replace_range(start..end, &replacement);
    let inner_start = selection.start + char_len(before);
    Selection { start: inner_start, end: inner_start + char_len(&inner) }
}

/// Removes the `marker` bytes at both ends of `span` (a byte range), keeping the caret on the
/// same character of the content.
pub fn unwrap_span(
    text: &mut String,
    span: Range<usize>,
    marker: usize,
    selection: Selection,
) -> Selection {
    let span_start_char = char_offset(text, span.start);
    let marker_chars = char_len(&text[span.start..span.start + marker]);
    text.replace_range(span.end - marker..span.end, "");
    text.replace_range(span.start..span.start + marker, "");
    let content_end_char = span_start_char + char_len(&text[span.start..span.end - 2 * marker]);
    let shift = |position: usize| {
        position.saturating_sub(marker_chars).clamp(span_start_char, content_end_char)
    };
    Selection { start: shift(selection.start), end: shift(selection.end) }
}

/// Turns a link back into its label (`[label](url)` → `label`).
pub fn unlink(text: &mut String, span: Range<usize>, selection: Selection) -> Selection {
    let source = &text[span.clone()];
    let label = source
        .strip_prefix('[')
        .and_then(|rest| rest.rfind("](").map(|end| rest[..end].to_owned()))
        .unwrap_or_else(|| source.trim_matches(['<', '>']).to_owned());
    let start_char = char_offset(text, span.start);
    text.replace_range(span, &label);
    let end_char = start_char + char_len(&label);
    let clamp = |position: usize| position.saturating_sub(1).clamp(start_char, end_char);
    Selection { start: clamp(selection.start), end: clamp(selection.end) }
}

fn link(text: &mut String, selection: Selection) -> Selection {
    let Range { start, end } = byte_range(text, selection);
    let label = if start == end { "link text".to_owned() } else { text[start..end].to_owned() };
    let replacement = format!("[{label}](https://)");
    text.replace_range(start..end, &replacement);
    let url_start = selection.start + char_len(&label) + 3;
    Selection { start: url_start, end: url_start + char_len("https://") }
}

/// `[selection](url)`, for pasting a URL over selected text. The caret lands after the link.
pub fn link_to(text: &mut String, selection: Selection, url: &str) -> Selection {
    let Range { start, end } = byte_range(text, selection);
    let replacement = format!("[{}]({url})", &text[start..end]);
    text.replace_range(start..end, &replacement);
    Selection::caret(selection.start + char_len(&replacement))
}

/// Replaces the selection with `insert` and puts the caret after it.
pub fn insert(text: &mut String, selection: Selection, insert: &str) -> Selection {
    let range = byte_range(text, selection);
    text.replace_range(range, insert);
    Selection::caret(selection.start + char_len(insert))
}

/// Puts `block` (several lines) on lines of its own in place of the selection, adding the line
/// breaks it needs before and after. Returns the char range of `block` in the new text.
fn insert_block(text: &mut String, selection: Selection, block: &str) -> Range<usize> {
    let Range { start, end } = byte_range(text, selection);
    let needs_break_before = start > 0 && !text[..start].ends_with('\n');
    let needs_break_after = !text[end..].starts_with('\n');
    let before = if needs_break_before { "\n" } else { "" };
    let after = if needs_break_after { "\n" } else { "" };
    text.replace_range(start..end, &format!("{before}{block}{after}"));
    let block_start = selection.start + before.len();
    block_start..block_start + char_len(block)
}

/// Puts a fence tagged `tag` around `body` on lines of its own in place of the selection.
/// Returns the char ranges of the whole fenced block and of `body` in the new text.
fn fence(
    text: &mut String,
    selection: Selection,
    tag: &str,
    body: &str,
) -> (Range<usize>, Range<usize>) {
    let inserted = insert_block(text, selection, &format!("```{tag}\n{body}\n```"));
    let body_start = inserted.start + char_len("```") + char_len(tag) + 1;
    let body = body_start..body_start + char_len(body);
    (inserted, body)
}

/// A fenced code block around the selection (or an empty line, with the caret on it).
fn code_block(text: &mut String, selection: Selection) -> Selection {
    let Range { start, end } = byte_range(text, selection);
    let code = text[start..end].trim_end_matches('\n').to_owned();
    let (_, body) = fence(text, selection, "", &code);
    Selection { start: body.start, end: body.end }
}

/// Pasted code: a fence with the file's language; the caret lands after the block.
pub fn fenced_paste(
    text: &mut String,
    selection: Selection,
    code: &str,
    language: &str,
) -> Selection {
    let (inserted, _) = fence(text, selection, language, code.trim_end_matches(['\n', '\r']));
    Selection::caret((inserted.end + 1).min(char_len(text)))
}

/// A ```` ```suggestion ```` block holding `lines`, with the caret on its first line so the reviewer
/// can start editing the proposed code right away.
pub fn suggestion(text: &mut String, selection: Selection, lines: &str) -> Selection {
    let (_, body) = fence(text, selection, "suggestion", lines.trim_end_matches(['\n', '\r']));
    Selection { start: body.start, end: body.end }
}

/// How a line prefix meets a list marker the line starts with already.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Prefix {
    /// Goes in front of it (a quote of a list).
    Stacks,
    /// Takes its place: bullets become numbers, never a bullet before a number.
    ReplacesMarker,
}

/// Prefixes every line touched by the selection; removes the prefix when all lines already have it.
/// A list marker goes inside the line's quote markers (`> - item`), so a list in a quote keeps its
/// quote when it is switched or removed.
fn prefix_lines(
    text: &mut String,
    selection: Selection,
    kind: Prefix,
    prefix_for: impl Fn(usize) -> String,
) -> Selection {
    let Range { start: line_start, end: line_end } =
        lines_around(text, byte_range(text, selection));
    let block: Vec<(&str, &str)> = text[line_start..line_end]
        .split('\n')
        .map(|line| match kind {
            Prefix::Stacks => ("", line),
            Prefix::ReplacesMarker => line.split_at(LinePrefix::parse(line).quote.len()),
        })
        .collect();
    let all_prefixed =
        block.iter().enumerate().all(|(index, (_, line))| line.starts_with(&prefix_for(index)));
    let rewritten: Vec<String> = block
        .iter()
        .enumerate()
        .map(|(index, (quote, line))| {
            let prefix = prefix_for(index);
            if all_prefixed {
                format!("{quote}{}", &line[prefix.len()..])
            } else if kind == Prefix::ReplacesMarker {
                format!("{quote}{prefix}{}", strip_list_prefix(line))
            } else {
                format!("{prefix}{line}")
            }
        })
        .collect();
    let is_single_line = block.len() == 1;
    let replacement = rewritten.join("\n");
    text.replace_range(line_start..line_end, &replacement);
    let block_start = char_len(&text[..line_start]);
    if selection.is_empty() && is_single_line {
        let caret = block_start + char_len(&replacement);
        return Selection::caret(caret);
    }
    Selection { start: block_start, end: block_start + char_len(&replacement) }
}

/// Makes the touched lines `###` headings, or plain text again when they already are headings of
/// any level.
fn heading(text: &mut String, selection: Selection) -> Selection {
    let Range { start: line_start, end: line_end } =
        lines_around(text, byte_range(text, selection));
    let heading_prefix = |line: &str| {
        let hashes = line.bytes().take_while(|&byte| byte == b'#').count();
        ((1..=6).contains(&hashes) && line[hashes..].starts_with(' ')).then_some(hashes + 1)
    };
    let lines: Vec<&str> = text[line_start..line_end].split('\n').collect();
    let all_headings = lines.iter().all(|line| heading_prefix(line).is_some());
    let replacement = lines
        .iter()
        .map(|line| match heading_prefix(line) {
            Some(prefix) if all_headings => line[prefix..].to_owned(),
            Some(_) => (*line).to_owned(),
            None => format!("### {line}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    text.replace_range(line_start..line_end, &replacement);
    let block_start = char_len(&text[..line_start]);
    Selection::caret(block_start + char_len(&replacement))
}

/// A line without its list marker, so switching bullets to numbers does not stack markers.
fn strip_list_prefix(line: &str) -> &str {
    let prefix = LinePrefix::parse(line);
    match prefix.list {
        Some(_) => &line[prefix.len..],
        None => line,
    }
}

/// The list or quote syntax at the start of a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinePrefix {
    /// Leading spaces before the marker.
    pub indent: usize,
    /// `> ` quote markers before the list marker.
    pub quote: String,
    pub list: Option<ListMarker>,
    /// Bytes of the whole prefix, including the space after the marker.
    pub len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListMarker {
    Bullet(char),
    Numbered { number: u64, delimiter: char },
    Task { bullet: char, is_checked: bool },
}

impl LinePrefix {
    pub fn parse(line: &str) -> Self {
        let mut position = 0;
        let mut quote = String::new();
        loop {
            let rest = &line[position..];
            let spaces = rest.len() - rest.trim_start_matches(' ').len();
            if rest[spaces..].starts_with('>') {
                let marker_end = position + spaces + 1;
                let has_space = line[marker_end..].starts_with(' ');
                let consumed = marker_end + usize::from(has_space);
                quote.push_str(&line[position..consumed]);
                position = consumed;
                continue;
            }
            break;
        }
        let rest = &line[position..];
        let indent = rest.len() - rest.trim_start_matches(' ').len();
        let body = &rest[indent..];
        let marker_start = position + indent;
        let mut list = None;
        let mut len = position;
        if let Some(bullet) =
            body.chars().next().filter(|character| matches!(character, '-' | '*' | '+'))
            && (body.len() == 1 || body[1..].starts_with(' '))
        {
            let after_bullet = &body[1..];
            let task = after_bullet.strip_prefix(' ').and_then(|after| {
                ["[ ] ", "[x] ", "[X] "]
                    .iter()
                    .find(|box_text| after.starts_with(**box_text) || after == box_text.trim_end())
                    .map(|box_text| (box_text.len(), box_text.contains(['x', 'X'])))
            });
            if let Some((box_len, is_checked)) = task {
                list = Some(ListMarker::Task { bullet, is_checked });
                len = (marker_start + 2 + box_len).min(line.len());
            } else {
                list = Some(ListMarker::Bullet(bullet));
                len = (marker_start + 2).min(line.len());
            }
        } else {
            let digits = body.bytes().take_while(u8::is_ascii_digit).count();
            let delimiter =
                body[digits..].chars().next().filter(|character| matches!(character, '.' | ')'));
            if (1..=9).contains(&digits)
                && let Some(delimiter) = delimiter
                && (body.len() == digits + 1 || body[digits + 1..].starts_with(' '))
            {
                let number = body[..digits].parse().unwrap_or(1);
                list = Some(ListMarker::Numbered { number, delimiter });
                len = (marker_start + digits + 2).min(line.len());
            }
        }
        Self { indent: if list.is_some() { indent } else { 0 }, quote, list, len }
    }

    pub fn is_empty(&self) -> bool {
        self.quote.is_empty() && self.list.is_none()
    }

    /// The prefix for the next line: same quote, next number, an unchecked box.
    fn continuation(&self) -> String {
        let indent = " ".repeat(self.indent);
        let marker = match &self.list {
            None => String::new(),
            Some(ListMarker::Bullet(bullet)) => format!("{indent}{bullet} "),
            Some(ListMarker::Numbered { number, delimiter }) => {
                format!("{indent}{}{delimiter} ", number + 1)
            }
            Some(ListMarker::Task { bullet, .. }) => format!("{indent}{bullet} [ ] "),
        };
        format!("{}{marker}", self.quote)
    }

    /// Width of the marker, the indent a nested item needs to belong to this one.
    fn marker_width(&self) -> usize {
        match &self.list {
            Some(ListMarker::Numbered { number, .. }) => number.to_string().len() + 2,
            Some(_) => 2,
            None => 0,
        }
    }
}

fn line_bounds(text: &str, byte: usize) -> Range<usize> {
    let start = text[..byte].rfind('\n').map_or(0, |index| index + 1);
    let end = text[byte..].find('\n').map_or(text.len(), |index| byte + index);
    start..end
}

/// The whole lines the byte range `bytes` touches.
fn lines_around(text: &str, bytes: Range<usize>) -> Range<usize> {
    line_bounds(text, bytes.start).start..line_bounds(text, bytes.end).end
}

/// Enter inside a list or quote: continues it on the next line, or ends it when the current item
/// is empty. `None` when the caret is not in a list or quote (a plain newline then).
pub fn continue_block(text: &mut String, selection: Selection) -> Option<Selection> {
    let Range { start, end } = byte_range(text, selection);
    let line = line_bounds(text, start);
    let prefix = LinePrefix::parse(&text[line.clone()]);
    if prefix.is_empty() || start < line.start + prefix.len {
        return None;
    }
    let content = text[line.start + prefix.len..line.end].trim();
    if content.is_empty() && end >= line.end {
        // Enter on an empty item ends the list (or one quote level) and leaves an empty line.
        let kept = if prefix.list.is_some() { prefix.quote } else { String::new() };
        text.replace_range(line.clone(), &kept);
        let caret = char_offset(text, line.start) + char_len(&kept);
        return Some(Selection::caret(caret));
    }
    let continuation = format!("\n{}", prefix.continuation());
    text.replace_range(start..end, &continuation);
    Some(Selection::caret(selection.start + char_len(&continuation)))
}

/// Enter inside a code block keeps the current line's indentation.
pub fn newline_with_indent(text: &mut String, selection: Selection) -> Selection {
    let Range { start, end } = byte_range(text, selection);
    let line = line_bounds(text, start);
    let indent: String = text[line.start..start]
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect();
    let insert = format!("\n{indent}");
    text.replace_range(start..end, &insert);
    Selection::caret(selection.start + char_len(&insert))
}

/// Tab / Shift+Tab on list items: nests the selected items one level under the item above, or
/// moves them one level out. `None` when no selected line is a list item.
pub fn indent_items(
    text: &mut String,
    selection: Selection,
    direction: Indent,
) -> Option<Selection> {
    let block = lines_around(text, byte_range(text, selection));
    let previous: Vec<LinePrefix> = text[..block.start]
        .lines()
        .map(LinePrefix::parse)
        .filter(|prefix| prefix.list.is_some())
        .collect();
    let mut any_item = false;
    let mut caret_shift: isize = 0;
    let mut rewritten = Vec::new();
    let mut parents = previous;
    for (index, line) in text[block.clone()].split('\n').enumerate() {
        let prefix = LinePrefix::parse(line);
        if prefix.list.is_none() {
            rewritten.push(line.to_owned());
            continue;
        }
        any_item = true;
        let quote_len = prefix.quote.len();
        let target = if direction == Indent::Out {
            parents
                .iter()
                .rev()
                .find(|parent| parent.indent < prefix.indent)
                .map_or(0, |parent| parent.indent)
        } else {
            parents
                .iter()
                .rev()
                .find(|parent| parent.indent <= prefix.indent)
                .map_or(prefix.indent, |parent| {
                    (parent.indent + parent.marker_width()).max(prefix.indent)
                })
        };
        let delta = target as isize - prefix.indent as isize;
        if index == 0 {
            caret_shift = delta;
        }
        let rest = &line[quote_len + prefix.indent..];
        rewritten.push(format!("{}{}{rest}", &line[..quote_len], " ".repeat(target)));
        parents.push(LinePrefix { indent: target, ..prefix });
    }
    if !any_item {
        return None;
    }
    let replacement = rewritten.join("\n");
    text.replace_range(block.clone(), &replacement);
    let block_start = char_offset(text, block.start);
    let shift =
        |position: usize| (position as isize + caret_shift).max(block_start as isize) as usize;
    if selection.is_empty() {
        return Some(Selection::caret(shift(selection.start)));
    }
    Some(Selection { start: block_start, end: block_start + char_len(&replacement) })
}

/// The list kind of the line holding `byte`, for the toolbar's active state.
pub fn list_at(text: &str, byte: usize) -> Option<ListMarker> {
    let line = line_bounds(text, byte.min(text.len()));
    LinePrefix::parse(&text[line]).list
}

pub fn is_quote_at(text: &str, byte: usize) -> bool {
    let line = line_bounds(text, byte.min(text.len()));
    !LinePrefix::parse(&text[line]).quote.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, start: usize, end: usize, format: Format) -> (String, Selection) {
        let mut text = text.to_owned();
        let selection = apply(&mut text, Selection { start, end }, format);
        (text, selection)
    }

    fn enter(text: &str, at: usize) -> Option<(String, Selection)> {
        let mut text = text.to_owned();
        continue_block(&mut text, Selection::caret(at)).map(|selection| (text, selection))
    }

    #[test]
    fn wraps_and_unwraps_bold() {
        let (text, selection) = run("make it bold", 8, 12, Format::Bold);
        assert_eq!(text, "make it **bold**");
        assert_eq!(selection, Selection { start: 10, end: 14 });
        let mut again = text;
        let unwrapped = apply(&mut again, selection, Format::Bold);
        assert_eq!(again, "make it bold");
        assert_eq!(unwrapped, Selection { start: 8, end: 12 });
    }

    #[test]
    fn unwraps_the_span_around_the_caret() {
        let mut text = "a **bold** b".to_owned();
        let selection = unwrap_span(&mut text, 2..10, 2, Selection::caret(6));
        assert_eq!(text, "a bold b");
        assert_eq!(selection, Selection::caret(4));
    }

    #[test]
    fn unlinks_to_the_label() {
        let mut text = "see [docs](https://x) now".to_owned();
        unlink(&mut text, 4..21, Selection::caret(6));
        assert_eq!(text, "see docs now");
    }

    #[test]
    fn inserts_placeholder_without_selection() {
        let (text, selection) = run("x ", 2, 2, Format::InlineCode);
        assert_eq!(text, "x `code`");
        assert_eq!(selection, Selection { start: 3, end: 7 });
    }

    #[test]
    fn numbers_and_unnumbers_lines() {
        let (text, _) = run("one\ntwo", 0, 7, Format::NumberedList);
        assert_eq!(text, "1. one\n2. two");
        let (back, _) = run(&text, 0, 13, Format::NumberedList);
        assert_eq!(back, "one\ntwo");
    }

    #[test]
    fn switching_list_kinds_replaces_the_marker() {
        let (text, _) = run("- one", 0, 0, Format::NumberedList);
        assert_eq!(text, "1. one");
    }

    #[test]
    fn list_buttons_keep_the_quote_around_a_list() {
        assert_eq!(run("> - one", 0, 0, Format::NumberedList).0, "> 1. one");
        assert_eq!(run("> - one", 0, 0, Format::BulletList).0, "> one", "toggled off");
        assert_eq!(run("> one\n> two", 0, 11, Format::TaskList).0, "> - [ ] one\n> - [ ] two");
        assert_eq!(run("- one", 0, 0, Format::Quote).0, "> - one", "a quote goes around a list");
    }

    #[test]
    fn builds_links_and_selects_url() {
        let (text, selection) = run("see docs", 4, 8, Format::Link);
        assert_eq!(text, "see [docs](https://)");
        assert_eq!(&text[selection.start..selection.end], "https://");
    }

    #[test]
    fn handles_multibyte_text() {
        let (text, selection) = run("grüße", 0, 5, Format::Italic);
        assert_eq!(text, "_grüße_");
        assert_eq!(selection, Selection { start: 1, end: 6 });
    }

    #[test]
    fn code_block_sits_on_its_own_lines() {
        let (text, selection) = run("see foo()", 4, 9, Format::CodeBlock);
        assert_eq!(text, "see \n```\nfoo()\n```\n");
        assert_eq!(&text[selection.start..selection.end], "foo()");
    }

    #[test]
    fn enter_continues_lists_and_quotes() {
        assert_eq!(enter("- one", 5), Some(("- one\n- ".to_owned(), Selection::caret(8))));
        assert_eq!(enter("9. nine", 7).map(|(text, _)| text), Some("9. nine\n10. ".to_owned()));
        assert_eq!(
            enter("- [x] done", 10).map(|(text, _)| text),
            Some("- [x] done\n- [ ] ".to_owned())
        );
        assert_eq!(enter("> said", 6).map(|(text, _)| text), Some("> said\n> ".to_owned()));
        assert_eq!(
            enter("  - nested", 10).map(|(text, _)| text),
            Some("  - nested\n  - ".to_owned())
        );
        assert_eq!(enter("plain", 5), None);
    }

    #[test]
    fn enter_on_an_empty_item_ends_the_list() {
        assert_eq!(enter("- one\n- ", 8), Some(("- one\n".to_owned(), Selection::caret(6))));
        assert_eq!(enter("> - a\n> - ", 10).map(|(text, _)| text), Some("> - a\n> ".to_owned()));
        assert_eq!(enter("> a\n> ", 6).map(|(text, _)| text), Some("> a\n".to_owned()));
    }

    #[test]
    fn tab_nests_items_under_the_previous_one() {
        let mut text = "- one\n- two".to_owned();
        let selection =
            indent_items(&mut text, Selection::caret(11), Indent::In).expect("list item");
        assert_eq!(text, "- one\n  - two");
        assert_eq!(selection, Selection::caret(13));
        let selection = indent_items(&mut text, selection, Indent::Out).expect("list item");
        assert_eq!(text, "- one\n- two");
        assert_eq!(selection, Selection::caret(11));
        let mut numbered = "1. one\n1. two".to_owned();
        indent_items(&mut numbered, Selection::caret(13), Indent::In);
        assert_eq!(numbered, "1. one\n   1. two");
        let mut plain = "text".to_owned();
        assert_eq!(indent_items(&mut plain, Selection::caret(2), Indent::In), None);
    }

    #[test]
    fn pastes_links_fences_and_suggestions() {
        let mut text = "read the docs".to_owned();
        let selection = link_to(&mut text, Selection { start: 9, end: 13 }, "https://example.com");
        assert_eq!(text, "read the [docs](https://example.com)");
        assert_eq!(selection, Selection::caret(36));

        let mut text = "look:".to_owned();
        fenced_paste(&mut text, Selection::caret(5), "fn a() {}\n", "rust");
        assert_eq!(text, "look:\n```rust\nfn a() {}\n```\n");

        let mut text = String::new();
        let selection = suggestion(&mut text, Selection::caret(0), "let x = 1;");
        assert_eq!(text, "```suggestion\nlet x = 1;\n```\n");
        assert_eq!(&text[selection.start..selection.end], "let x = 1;");
    }

    #[test]
    fn parses_line_prefixes() {
        let prefix = LinePrefix::parse("> 12) item");
        assert_eq!(prefix.quote, "> ");
        assert_eq!(prefix.list, Some(ListMarker::Numbered { number: 12, delimiter: ')' }));
        assert_eq!(prefix.len, 6);
        assert_eq!(LinePrefix::parse("-not a list").list, None);
        assert_eq!(LinePrefix::parse("- [ ] t").len, 6);
    }
}
