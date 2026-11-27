//! The document to Markdown for one platform. Markdown comes first; inline HTML only for marks
//! Markdown lacks, using only tags the platform's sanitizer keeps:
//!
//! | Mark        | GitHub, GitLab | Azure DevOps        |
//! |-------------|----------------|---------------------|
//! | Underline   | `<ins>`        | `<u>`               |
//! | Subscript   | `<sub>`        | `<sub>`             |
//! | Superscript | `<sup>`        | `<sup>`             |
//! | Key         | `<kbd>`        | inline code         |
//!
//! Sources: GitHub renders Markdown through html-pipeline's sanitization allowlist, which keeps
//! `ins`, `del`, `sub`, `sup`, `kbd`, `strong`, `em`, `s`, `details` and `summary` but not `u`
//! (<https://github.com/gjtorikian/html-pipeline/blob/v2.14.3/lib/html/pipeline/sanitization_filter.rb>);
//! GitHub's own guide writes underline as `<ins>` and sub/superscript as `<sub>`/`<sup>`
//! (<https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax>).
//! GitLab uses the same `SanitizationFilter` allowlist and adds `span`, `abbr`, `details` and
//! `summary` (<https://docs.gitlab.com/user/markdown/#inline-html>). Azure DevOps documents `<u>`
//! for underline and shows `<sub>`, `<sup>`, `<ins>` and `<del>` in its HTML examples, but names no
//! `<kbd>` (<https://learn.microsoft.com/en-us/azure/devops/project/wiki/markdown-guidance?view=azure-devops#html-tags>),
//! so a key degrades to inline code there.
//!
//! Emphasis delimiters follow `CommonMark`'s flanking rules only for the usual cases; each block's
//! inline output is parsed back, and where that does not give the same marks, the block writes its
//! emphasis as `<strong>`, `<em>` and `<del>`, which all three keep. Mentions follow
//! `markdown_editor::PullRequestContext::mention`: `@login`, `@<id>` on Azure DevOps.

use std::collections::HashMap;
use std::sync::Arc;

use pulldown_cmark::Alignment;
use review_core::ProviderKind;

mod inline;
mod prepare;

pub use self::inline::run_source;
use self::inline::{Context, inline_markdown};
use super::model::{Alert, Block, Container, Document, Inline, Kind, ListKind};

/// Serialized inline content of unchanged blocks, keyed by block pointer (the entry holds the
/// block, so the pointer is never reused while cached). Entries of blocks that left the
/// document drop at the next serialization.
#[derive(Clone, Default)]
pub struct InlineCache {
    entries: HashMap<usize, (Arc<Block>, Arc<str>)>,
}

/// `doc` as Markdown for `platform`.
pub fn serialize(doc: &Document, platform: ProviderKind) -> String {
    serialize_with(doc, platform, &mut InlineCache::default())
}

/// [`serialize`], reusing the inline output of blocks `cache` saw before.
pub fn serialize_with(doc: &Document, platform: ProviderKind, cache: &mut InlineCache) -> String {
    let mut entries = HashMap::with_capacity(doc.blocks.len());
    let mut writer = BlockWriter::default();
    let mut previous: Option<&Block> = None;
    for block in &doc.blocks {
        if is_skipped(block, previous) {
            continue;
        }
        let key = Arc::as_ptr(block) as usize;
        let content = match cache.entries.remove(&key) {
            Some((kept, text)) if Arc::ptr_eq(&kept, block) => text,
            _ => Arc::from(content_markdown(block, platform)),
        };
        writer.block(block, previous, &content);
        entries.insert(key, (block.clone(), content));
        previous = Some(block);
    }
    cache.entries = entries;
    writer.out
}

/// An empty paragraph that starts no container shows as nothing in Markdown.
fn is_skipped(block: &Block, previous: Option<&Block>) -> bool {
    let is_empty_paragraph = block.kind == Kind::Paragraph && block.content.is_empty();
    is_empty_paragraph && shared_depth(block, previous) == block.nesting.len()
}

/// How many outer containers `block` shares with `previous`.
fn shared_depth(block: &Block, previous: Option<&Block>) -> usize {
    let Some(previous) = previous else { return 0 };
    block
        .nesting
        .iter()
        .zip(&previous.nesting)
        .take_while(|(here, there)| here.continues(there))
        .count()
}

/// A block's own Markdown, its lines joined by `\n` without container prefixes.
fn content_markdown(block: &Block, platform: ProviderKind) -> String {
    match &block.kind {
        Kind::Paragraph => inline_markdown(&block.content, platform, Context::Text),
        Kind::Heading(level) => {
            let text = inline_markdown(&block.content, platform, Context::Line);
            let hashes = "#".repeat(usize::from((*level).clamp(1, 6)));
            if text.is_empty() { hashes } else { format!("{hashes} {text}") }
        }
        Kind::Code(language) => {
            let code = source_text(&block.content, platform);
            let longest = longest_run(&code, '`');
            let (fence_char, longest) = if language.contains('`') {
                ('~', longest_run(&code, '~'))
            } else {
                ('`', longest)
            };
            let fence: String = std::iter::repeat_n(fence_char, (longest + 1).max(3)).collect();
            if code.is_empty() {
                format!("{fence}{language}\n{fence}")
            } else {
                format!("{fence}{language}\n{code}\n{fence}")
            }
        }
        Kind::Rule => "---".to_owned(),
        Kind::Html | Kind::Table => source_text(&block.content, platform),
    }
}

/// A source block's text as written: an atom (a mention moved into it) as its source.
fn source_text(content: &Inline, platform: ProviderKind) -> String {
    content.runs().iter().map(|run| run_source(run, platform)).collect()
}

fn longest_run(text: &str, wanted: char) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for ch in text.chars() {
        current = if ch == wanted { current + 1 } else { 0 };
        longest = longest.max(current);
    }
    longest
}

/// A list level open while blocks are written: its marker width and its next number.
#[derive(Clone, Copy)]
struct Level {
    container: Container,
    width: usize,
    number: u64,
}

#[derive(Default)]
struct BlockWriter {
    out: String,
    levels: Vec<Level>,
}

impl BlockWriter {
    fn block(&mut self, block: &Block, previous: Option<&Block>, content: &str) {
        let shared = shared_depth(block, previous);
        let sibling = previous.is_some_and(|previous| {
            match (block.nesting.get(shared), previous.nesting.get(shared)) {
                (Some(here), Some(there)) => here.is_next_item_of(there),
                _ => false,
            }
        });
        if previous.is_some() {
            self.separator(block, shared, sibling);
        }
        self.levels.truncate(shared);
        let mut first_prefix = self.continuation(shared);
        for container in block.nesting.iter().skip(shared) {
            match *container {
                Container::Quote(alert) => {
                    if let Some(alert) = alert {
                        let name = alert_name(alert);
                        self.line(&format!("{first_prefix}> [!{name}]"));
                        first_prefix = self.continuation(self.levels.len());
                    }
                    first_prefix.push_str("> ");
                    self.levels.push(Level { container: *container, width: 2, number: 0 });
                }
                Container::Item { list, task, .. } => {
                    let number = match (sibling, self.levels.len() == shared, list) {
                        (true, true, _) => self.next_number(shared, previous),
                        (_, _, ListKind::Ordered(start)) => start,
                        (_, _, ListKind::Bullet) => 0,
                    };
                    let marker = match list {
                        ListKind::Bullet => "-".to_owned(),
                        ListKind::Ordered(_) => format!("{number}."),
                    };
                    let width = marker.len() + 1;
                    first_prefix.push_str(&marker);
                    first_prefix.push(' ');
                    match task {
                        Some(true) => first_prefix.push_str("[x] "),
                        Some(false) => first_prefix.push_str("[ ] "),
                        None => {}
                    }
                    self.levels.push(Level { container: *container, width, number });
                }
            }
        }
        let rest_prefix = self.continuation(self.levels.len());
        let is_source = block.kind.is_source();
        for (index, line) in content.split('\n').enumerate() {
            let prefix = if index == 0 { first_prefix.as_str() } else { rest_prefix.as_str() };
            if line.is_empty() || (!is_source && line.trim().is_empty()) {
                self.line(prefix.trim_end());
            } else {
                self.line(&format!("{prefix}{line}"));
            }
        }
    }

    /// The number of the next item in the list at `depth`.
    fn next_number(&self, depth: usize, previous: Option<&Block>) -> u64 {
        let open = self.levels.get(depth).filter(|level| level.container.is_item());
        let fallback =
            previous.and_then(|previous| previous.nesting.get(depth)).map_or(1, |c| match c {
                Container::Item { list: ListKind::Ordered(start), .. } => *start,
                _ => 1,
            });
        open.map_or(fallback, |level| level.number) + 1
    }

    /// Between two blocks: a line break between items of one list and before a bullet list
    /// nested in an item's text, a blank line (inside the shared quotes) everywhere else.
    fn separator(&mut self, block: &Block, shared: usize, sibling: bool) {
        let nests_list = shared > 0
            && shared == self.levels.len()
            && matches!(
                block.nesting.get(shared),
                Some(Container::Item {
                    list: ListKind::Bullet | ListKind::Ordered(1),
                    head: true,
                    ..
                })
            );
        if sibling || nests_list {
            return;
        }
        let blank = self.continuation(shared);
        self.line(blank.trim_end());
    }

    /// The prefix that continues the first `depth` open levels on a further line.
    fn continuation(&self, depth: usize) -> String {
        let mut prefix = String::new();
        for level in self.levels.iter().take(depth) {
            match level.container {
                Container::Quote(_) => prefix.push_str("> "),
                Container::Item { .. } => prefix.extend(std::iter::repeat_n(' ', level.width)),
            }
        }
        prefix
    }

    fn line(&mut self, line: &str) {
        if !self.out.is_empty() {
            self.out.push('\n');
        }
        self.out.push_str(line);
    }
}

fn alert_name(alert: Alert) -> &'static str {
    match alert {
        Alert::Note => "NOTE",
        Alert::Tip => "TIP",
        Alert::Important => "IMPORTANT",
        Alert::Warning => "WARNING",
        Alert::Caution => "CAUTION",
    }
}

/// A table as canonical GFM source.
pub fn table_source(
    alignments: &[Alignment],
    header: &[Inline],
    rows: &[Vec<Inline>],
    platform: ProviderKind,
) -> String {
    let columns = alignments.len().max(header.len()).max(1);
    let row_line = |cells: &[Inline]| {
        let mut line = String::from("|");
        for column in 0..columns {
            let cell = cells
                .get(column)
                .map(|cell| inline_markdown(cell, platform, Context::Cell))
                .unwrap_or_default();
            line.push(' ');
            line.push_str(&cell);
            line.push_str(" |");
        }
        line
    };
    let mut lines = vec![row_line(header)];
    let mut delimiter = String::from("|");
    for column in 0..columns {
        delimiter.push_str(match alignments.get(column) {
            Some(Alignment::Left) => " :--- |",
            Some(Alignment::Right) => " ---: |",
            Some(Alignment::Center) => " :---: |",
            Some(Alignment::None) | None => " --- |",
        });
    }
    lines.push(delimiter);
    lines.extend(rows.iter().map(|row| row_line(row)));
    lines.join("\n")
}
