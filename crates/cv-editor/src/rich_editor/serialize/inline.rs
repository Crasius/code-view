//! Inline content to Markdown: escapes, code spans, atoms, and marks opened and closed as a
//! stack so they nest. Emphasis whose delimiters would not read back takes tags instead (see the
//! parent module).

use std::borrow::Cow;
use std::sync::Arc;

use review_core::ProviderKind;

use super::longest_run;
use super::prepare::{prepared, reads_back};
use crate::rich_editor::model::{ATOM_CHAR, Atom, HARD_BREAK, Inline, Link, Mark, Run};

/// Where inline content goes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Context {
    /// A paragraph: line breaks stay.
    Text,
    /// A heading: one line.
    Line,
    /// A table cell: one line, `|` escaped.
    Cell,
}

/// How emphasis is written.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Emphasis {
    Delimiters,
    Tags,
}

/// The inline content as Markdown; lines joined by `\n`.
pub(crate) fn inline_markdown(inline: &Inline, platform: ProviderKind, context: Context) -> String {
    let runs = prepared(inline, platform, context);
    let written = InlineWriter::write(&runs, platform, context, Emphasis::Delimiters);
    if context == Context::Cell || reads_back(&written, &runs, platform) {
        return written;
    }
    InlineWriter::write(&runs, platform, context, Emphasis::Tags)
}

/// An open mark and how it was opened.
#[derive(Clone)]
enum Open {
    Mark(Mark, &'static str),
    Link(Arc<Link>),
}

struct InlineWriter<'a> {
    runs: &'a [Run],
    platform: ProviderKind,
    context: Context,
    emphasis: Emphasis,
    /// The whole text, for the neighbours of a char across runs.
    chars: Vec<char>,
    out: String,
    stack: Vec<Open>,
}

impl<'a> InlineWriter<'a> {
    fn write(
        runs: &'a [Run],
        platform: ProviderKind,
        context: Context,
        emphasis: Emphasis,
    ) -> String {
        let chars = runs.iter().flat_map(|run| run.text.chars()).collect();
        let mut writer = Self {
            runs,
            platform,
            context,
            emphasis,
            chars,
            out: String::new(),
            stack: Vec::new(),
        };
        writer.runs_out();
        writer.out
    }

    fn runs_out(&mut self) {
        let mut index = 0;
        let mut start = 0;
        while let Some(run) = self.runs.get(index) {
            let wanted = Self::stacked(run);
            self.close_unwanted(&wanted);
            let mut missing: Vec<Open> =
                wanted.into_iter().filter(|open| !self.is_open(open)).collect();
            missing
                .sort_by_key(|open| (std::cmp::Reverse(self.extent(index, open)), priority(open)));
            let mut skipped = None;
            for open in missing {
                if let Open::Link(link) = &open
                    && let Some(span) = self.autolink(index, link)
                {
                    self.out.push('<');
                    self.out.push_str(&link.url);
                    self.out.push('>');
                    skipped = Some(span);
                    break;
                }
                self.open(&open);
            }
            if let Some((runs, chars)) = skipped {
                index += runs;
                start += chars;
                continue;
            }
            self.content(run, start);
            start += run.text.chars().count();
            index += 1;
        }
        while let Some(open) = self.stack.pop() {
            self.close(&open);
        }
    }

    /// The marks of `run` that open and close as tags or delimiters (not code).
    fn stacked(run: &Run) -> Vec<Open> {
        let mut wanted = Vec::new();
        if let Some(link) = &run.marks.link {
            wanted.push(Open::Link(link.clone()));
        }
        for mark in Mark::ALL {
            if mark != Mark::Code && run.marks.has(mark) {
                wanted.push(Open::Mark(mark, ""));
            }
        }
        wanted
    }

    fn is_open(&self, open: &Open) -> bool {
        self.stack.iter().any(|stacked| same(stacked, open))
    }

    fn close_unwanted(&mut self, wanted: &[Open]) {
        let Some(first) =
            self.stack.iter().position(|open| !wanted.iter().any(|wanted| same(open, wanted)))
        else {
            return;
        };
        while self.stack.len() > first {
            if let Some(open) = self.stack.pop() {
                self.close(&open);
            }
        }
    }

    /// How many runs from `index` on carry `open`.
    fn extent(&self, index: usize, open: &Open) -> usize {
        self.runs[index..]
            .iter()
            .take_while(|run| Self::stacked(run).iter().any(|wanted| same(wanted, open)))
            .count()
    }

    /// A link whose text is its URL and that has no other marks: `<url>`; the runs and chars
    /// it covers.
    fn autolink(&self, index: usize, link: &Link) -> Option<(usize, usize)> {
        let count = self.extent(index, &Open::Link(Arc::new(link.clone())));
        let span = &self.runs[index..index + count];
        let text: String = span.iter().map(|run| run.text.as_str()).collect();
        let is_plain = span.iter().all(|run| {
            let mut others = run.marks.clone();
            others.link = None;
            run.atom.is_none() && others.is_plain()
        });
        let is_url = text == link.url
            && link.title.is_empty()
            && (link.url.starts_with("https://") || link.url.starts_with("http://"))
            && !link.url.contains(|ch: char| ch.is_whitespace() || ch == '<' || ch == '>');
        (is_plain && is_url).then(|| (count, text.chars().count()))
    }

    fn open(&mut self, open: &Open) {
        match open {
            Open::Link(_) => {
                self.out.push('[');
                self.stack.push(open.clone());
            }
            Open::Mark(mark, _) => {
                let opener = self.opener(*mark);
                self.out.push_str(opener);
                self.stack.push(Open::Mark(*mark, opener));
            }
        }
    }

    fn opener(&self, mark: Mark) -> &'static str {
        let after_star = self.out.ends_with('*');
        match (mark, self.emphasis) {
            (Mark::Bold, Emphasis::Delimiters) => {
                if after_star {
                    "__"
                } else {
                    "**"
                }
            }
            (Mark::Italic, Emphasis::Delimiters) => {
                if after_star {
                    "_"
                } else {
                    "*"
                }
            }
            (Mark::Strike, Emphasis::Delimiters) => "~~",
            (Mark::Bold, Emphasis::Tags) => "<strong>",
            (Mark::Italic, Emphasis::Tags) => "<em>",
            (Mark::Strike, Emphasis::Tags) => "<del>",
            (Mark::Underline, _) => {
                if self.platform == ProviderKind::AzureDevOps {
                    "<u>"
                } else {
                    "<ins>"
                }
            }
            (Mark::Subscript, _) => "<sub>",
            (Mark::Superscript, _) => "<sup>",
            (Mark::Kbd, _) => "<kbd>",
            (Mark::Code, _) => "",
        }
    }

    fn close(&mut self, open: &Open) {
        match open {
            Open::Link(link) => {
                self.out.push_str("](");
                self.out.push_str(&destination(&link.url));
                if !link.title.is_empty() {
                    self.out.push_str(" \"");
                    self.out.push_str(&link.title.replace('"', "\\\""));
                    self.out.push('"');
                }
                self.out.push(')');
            }
            Open::Mark(_, opener) => {
                let closer = match *opener {
                    tag if tag.starts_with('<') => format!("</{}", &tag[1..]),
                    delimiter => delimiter.to_owned(),
                };
                self.out.push_str(&closer);
            }
        }
    }

    fn in_link(&self) -> bool {
        self.stack.iter().any(|open| matches!(open, Open::Link(_)))
    }

    /// The run's text, code span or atom; `start` is its first char in `chars`.
    fn content(&mut self, run: &Run, start: usize) {
        if let Some(atom) = &run.atom {
            let source = atom_source(atom, self.platform);
            self.out.push_str(&source);
            return;
        }
        if run.marks.has(Mark::Code) {
            self.code_span(&run.text);
            return;
        }
        let mut url_end = 0;
        for (offset, ch) in run.text.chars().enumerate() {
            let at = start + offset;
            if at >= url_end && self.starts_url(at) {
                url_end = self.url_end(at);
            }
            self.text_char(ch, at, at < url_end);
        }
    }

    fn starts_url(&self, at: usize) -> bool {
        let previous = at.checked_sub(1).and_then(|before| self.chars.get(before));
        if previous.is_some_and(|ch| ch.is_alphanumeric()) {
            return false;
        }
        let ahead: String = self.chars[at..].iter().take(8).collect();
        ahead.starts_with("https://") || ahead.starts_with("http://")
    }

    fn url_end(&self, at: usize) -> usize {
        self.chars[at..]
            .iter()
            .position(|ch| ch.is_whitespace() || matches!(*ch, '<' | '>' | ATOM_CHAR | HARD_BREAK))
            .map_or(self.chars.len(), |length| at + length)
    }

    fn text_char(&mut self, ch: char, at: usize, in_url: bool) {
        let previous = at.checked_sub(1).and_then(|before| self.chars.get(before)).copied();
        let next = self.chars.get(at + 1).copied();
        match ch {
            '\n' => {
                self.out.push('\n');
                return;
            }
            HARD_BREAK => {
                self.out.push_str("\\\n");
                return;
            }
            _ => {}
        }
        let line_start = self.out.rsplit('\n').next().is_none_or(str::is_empty);
        let line: &str = self.out.rsplit('\n').next().unwrap_or_default();
        let escape = if in_url {
            matches!(ch, '*' | '`' | '[' | ']' | '<' | '\\')
        } else {
            match ch {
                '\\' => next.is_none_or(|next| next.is_ascii_punctuation()),
                '*' | '`' | '[' | '~' => true,
                ']' => self.in_link(),
                '_' => {
                    !(previous.is_some_and(char::is_alphanumeric)
                        && next.is_some_and(char::is_alphanumeric))
                }
                '<' => next.is_some_and(|next| {
                    next.is_ascii_alphabetic() || matches!(next, '/' | '!' | '?')
                }),
                '&' => self.starts_entity(at),
                '|' => self.context == Context::Cell || line_start,
                '#' | '>' => line_start,
                '-' | '+' | '=' => line_start && self.opens_block_line(at, ch),
                '.' | ')' => {
                    let is_number = !line.is_empty()
                        && line.len() <= 9
                        && line.chars().all(|digit| digit.is_ascii_digit());
                    is_number && next.is_none_or(|next| next == ' ' || next == '\n')
                }
                _ => false,
            }
        };
        if escape {
            self.out.push('\\');
        }
        self.out.push(ch);
    }

    /// A `-`, `+` or `=` at the start of a line that would read as a list marker, a rule or a
    /// heading underline.
    fn opens_block_line(&self, at: usize, ch: char) -> bool {
        let next = self.chars.get(at + 1).copied();
        if matches!(ch, '-' | '+') && next.is_none_or(|next| next == ' ' || next == '\n') {
            return true;
        }
        let rest = self.chars[at..].iter().take_while(|other| **other != '\n');
        let mut rest = rest.filter(|other| **other != ' ');
        rest.all(|other| *other == ch)
    }

    fn starts_entity(&self, at: usize) -> bool {
        let rest: String = self.chars[at + 1..].iter().take(33).collect();
        let name_len =
            rest.find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '#')).unwrap_or(rest.len());
        name_len > 0 && rest[name_len..].starts_with(';')
    }

    fn code_span(&mut self, text: &str) {
        let fence = "`".repeat(longest_run(text, '`') + 1);
        let starts_or_ends_with_tick = text.starts_with('`') || text.ends_with('`');
        let has_space_edges =
            text.starts_with(' ') && text.ends_with(' ') && !text.chars().all(|ch| ch == ' ');
        let pad = if starts_or_ends_with_tick || has_space_edges { " " } else { "" };
        self.out.push_str(&fence);
        self.out.push_str(pad);
        self.out.push_str(text);
        self.out.push_str(pad);
        self.out.push_str(&fence);
    }
}

/// Whether two open marks are the same mark (a link: the same target).
fn same(a: &Open, b: &Open) -> bool {
    match (a, b) {
        (Open::Mark(a, _), Open::Mark(b, _)) => a == b,
        (Open::Link(a), Open::Link(b)) => a == b,
        _ => false,
    }
}

/// Order among marks of equal reach: links outermost, then emphasis, then tags.
fn priority(open: &Open) -> u8 {
    match open {
        Open::Link(_) => 0,
        Open::Mark(mark, _) => *mark as u8 + 1,
    }
}

/// A link destination; `<...>` when it holds spaces or parentheses.
fn destination(url: &str) -> String {
    if url.contains(|ch: char| ch.is_whitespace() || ch == '(' || ch == ')') {
        format!("<{}>", url.replace('<', "%3C").replace('>', "%3E"))
    } else {
        url.to_owned()
    }
}

/// An atom as Markdown.
fn atom_source(atom: &Atom, platform: ProviderKind) -> String {
    match atom {
        Atom::Mention { key } if platform == ProviderKind::AzureDevOps => format!("@<{key}>"),
        Atom::Mention { key } => format!("@{key}"),
        Atom::Image { src, alt, title } => {
            let alt = alt.replace('\\', "\\\\").replace('[', "\\[").replace(']', "\\]");
            let title = if title.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", title.replace('"', "\\\""))
            };
            format!("![{alt}]({}{title})", destination(src))
        }
        Atom::Raw(source) => source.clone(),
    }
}

/// A run as Markdown source: an atom as its source, text as it is.
pub(crate) fn run_source(run: &Run, platform: ProviderKind) -> Cow<'_, str> {
    match &run.atom {
        Some(atom) => Cow::Owned(atom_source(atom, platform)),
        None => Cow::Borrowed(&run.text),
    }
}
