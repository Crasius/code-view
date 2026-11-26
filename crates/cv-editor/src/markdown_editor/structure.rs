//! The Markdown structure the editing logic needs: which inline spans surround the caret (so the
//! toolbar shows a format as active and toggles it off), which lines are headings, and where code
//! blocks are (Enter keeps their indentation, Tab indents, smart paste leaves them alone, and the
//! write area sets fenced code in the code font). Nothing here styles the text.
//!
//! Parsing uses pulldown-cmark's offset iterator (GitHub-flavoured: tables, strikethrough, task
//! lists), so the structure agrees with how GitHub, GitLab and Azure DevOps render the comment.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};

/// Inline formatting the toolbar can toggle off again when the cursor is inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineKind {
    Bold,
    Italic,
    Strike,
    Code,
    Link,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineSpan {
    pub kind: InlineKind,
    /// Byte range including the markers.
    pub range: Range<usize>,
    /// Bytes of the opening and the closing marker (`**` = 2); zero for links.
    pub marker: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlock {
    /// Byte range of the whole block, fences included, without the trailing newline.
    pub block: Range<usize>,
    /// Byte range of the content lines (for Enter's auto indent, Tab and smart paste).
    pub content: Range<usize>,
    pub is_fenced: bool,
}

#[derive(Debug, Default)]
pub struct Model {
    pub spans: Vec<InlineSpan>,
    /// Byte ranges of the headings.
    pub headings: Vec<Range<usize>>,
    pub code_blocks: Vec<CodeBlock>,
    /// An unclosed fence or HTML block runs to the end of the text, so text after it would
    /// continue that block rather than start fresh.
    pub ends_open: bool,
    /// The text defines link references (`[name]: url`), which links anywhere may use.
    pub has_definitions: bool,
}

impl Model {
    /// Appends `other`, parsed from the part of a longer text that starts at `byte`.
    pub fn append(&mut self, other: &Model, byte: usize) {
        let shift = |range: &Range<usize>| range.start + byte..range.end + byte;
        self.spans.extend(
            other.spans.iter().map(|span| InlineSpan { range: shift(&span.range), ..span.clone() }),
        );
        self.headings.extend(other.headings.iter().map(shift));
        self.code_blocks.extend(other.code_blocks.iter().map(|code| CodeBlock {
            block: shift(&code.block),
            content: shift(&code.content),
            is_fenced: code.is_fenced,
        }));
        self.ends_open = other.ends_open;
        self.has_definitions |= other.has_definitions;
    }

    /// The innermost span of `kind` that contains the byte range `start..end`.
    pub fn span_around(&self, kind: InlineKind, start: usize, end: usize) -> Option<&InlineSpan> {
        self.spans
            .iter()
            .filter(|span| span.kind == kind && span.range.start <= start && end <= span.range.end)
            .min_by_key(|span| span.range.len())
    }

    /// Whether the character at `byte` (the last one when `byte` is the end) is in a heading.
    pub fn is_in_heading(&self, byte: usize, text_len: usize) -> bool {
        let probe = if byte < text_len { byte } else { byte.saturating_sub(1) };
        self.headings.iter().any(|heading| heading.contains(&probe))
    }

    pub fn is_in_code_block(&self, byte: usize) -> bool {
        self.code_blocks.iter().any(|code| code.content.start <= byte && byte <= code.content.end)
    }
}

pub fn parse(text: &str) -> Model {
    let mut builder = Builder { text, model: Model::default() };
    builder.walk();
    builder.model
}

struct Builder<'a> {
    text: &'a str,
    model: Model,
}

impl Builder<'_> {
    /// Whether a block that was not closed reaches the end of the text.
    fn reaches_end(&self, range: &Range<usize>) -> bool {
        trim_trailing_newline(self.text, range.clone())
            >= self.text.trim_end_matches(['\n', '\r']).len()
    }

    fn walk(&mut self) {
        let options =
            Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
        // The first pass, which collects link reference definitions, runs when the parser is made.
        let parser = Parser::new_ext(self.text, options).into_offset_iter();
        self.model.has_definitions = parser.reference_definitions().iter().next().is_some();
        for (event, range) in parser {
            match event {
                Event::Start(tag) => match tag {
                    Tag::Strong => self.emphasis(range, InlineKind::Bold),
                    Tag::Emphasis => self.emphasis(range, InlineKind::Italic),
                    Tag::Strikethrough => self.emphasis(range, InlineKind::Strike),
                    Tag::Link { .. } => {
                        self.model.spans.push(InlineSpan {
                            kind: InlineKind::Link,
                            range,
                            marker: 0,
                        });
                    }
                    Tag::Heading { .. } => self.model.headings.push(range),
                    Tag::CodeBlock(kind) => self.code_block(range, &kind),
                    Tag::HtmlBlock => {
                        self.model.ends_open |=
                            self.reaches_end(&range) && is_unclosed_html(&self.text[range]);
                    }
                    _ => {}
                },
                Event::Code(_) => self.inline_code(range),
                _ => {}
            }
        }
    }

    fn emphasis(&mut self, range: Range<usize>, kind: InlineKind) {
        let source = &self.text[range.clone()];
        let first = source.as_bytes().first().copied().unwrap_or(b'*');
        let marker = source.bytes().take_while(|&byte| byte == first).count().min(2);
        let marker = if kind == InlineKind::Italic { 1 } else { marker };
        if range.len() < marker * 2 {
            return;
        }
        self.model.spans.push(InlineSpan { kind, range, marker });
    }

    fn code_block(&mut self, range: Range<usize>, kind: &CodeBlockKind<'_>) {
        let end = trim_trailing_newline(self.text, range.clone());
        let mut content = range.start..end;
        let is_fenced = matches!(kind, CodeBlockKind::Fenced(_));
        if is_fenced {
            let first_line_end =
                self.text[range.start..end].find('\n').map_or(end, |offset| range.start + offset);
            content.start = (first_line_end + 1).min(end);
            let last_line_start = self.text[range.start..end]
                .rfind('\n')
                .map_or(range.start, |offset| range.start + offset + 1);
            let last_line = self.text[last_line_start..end].trim();
            let is_closed = last_line_start > first_line_end
                && (last_line.starts_with("```") || last_line.starts_with("~~~"));
            if is_closed {
                content.end = last_line_start.saturating_sub(1).max(content.start);
            } else {
                self.model.ends_open |= self.reaches_end(&range);
            }
        }
        self.model.code_blocks.push(CodeBlock { block: range.start..end, content, is_fenced });
    }

    fn inline_code(&mut self, range: Range<usize>) {
        let source = &self.text[range.clone()];
        let ticks = source.bytes().take_while(|&byte| byte == b'`').count();
        self.model.spans.push(InlineSpan { kind: InlineKind::Code, range, marker: ticks });
    }
}

/// `CommonMark` HTML blocks of kinds 1 to 5 (`<pre>`, `<script>`, `<style>`, `<textarea>`,
/// comments, `<?`, `<!X`, CDATA) run on over blank lines until their end marker; the other
/// kinds end at a blank line.
fn is_unclosed_html(block: &str) -> bool {
    let start = block.trim_start_matches(' ');
    let lower = start.to_ascii_lowercase();
    for tag in ["script", "pre", "style", "textarea"] {
        if let Some(rest) = lower.strip_prefix('<').and_then(|rest| rest.strip_prefix(tag))
            && (rest.is_empty() || rest.starts_with([' ', '\t', '>', '\n', '\r']))
        {
            return !rest.contains(&format!("</{tag}>"));
        }
    }
    let end_marker = if start.starts_with("<!--") {
        "-->"
    } else if start.starts_with("<?") {
        "?>"
    } else if start.starts_with("<![CDATA[") {
        "]]>"
    } else if start.starts_with("<!")
        && start[2..].starts_with(|character: char| character.is_ascii_alphabetic())
    {
        ">"
    } else {
        return false;
    };
    !start[2..].contains(end_marker)
}

fn trim_trailing_newline(text: &str, range: Range<usize>) -> usize {
    let source = &text[range.clone()];
    range.start + source.trim_end_matches(['\n', '\r']).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds_at(model: &Model, text: &str, needle: &str) -> Vec<InlineKind> {
        let start = text.find(needle).expect("needle in text");
        model
            .spans
            .iter()
            .filter(|span| span.range.start < start && start < span.range.end)
            .map(|span| span.kind)
            .collect()
    }

    #[test]
    fn finds_inline_spans_with_their_markers() {
        let text = "a **bold** and _it_ and `code` and [docs](https://x.y) and https://bare.test";
        let model = parse(text);
        assert_eq!(kinds_at(&model, text, "old"), [InlineKind::Bold]);
        assert_eq!(kinds_at(&model, text, "t_"), [InlineKind::Italic]);
        assert_eq!(kinds_at(&model, text, "ode"), [InlineKind::Code]);
        assert_eq!(kinds_at(&model, text, "ocs"), [InlineKind::Link]);
        assert!(kinds_at(&model, text, "bare").is_empty(), "a bare URL has nothing to unlink");
        let bold = model.span_around(InlineKind::Bold, 5, 5).expect("bold span");
        assert_eq!((&text[bold.range.clone()], bold.marker), ("**bold**", 2));
    }

    #[test]
    fn finds_code_blocks_and_their_content() {
        let text = "- one\n\n```rust\nlet x = 1;\n```\n\n    indented\n";
        let model = parse(text);
        assert_eq!(model.code_blocks.len(), 2);
        let fenced = &model.code_blocks[0];
        assert!(fenced.is_fenced);
        assert_eq!(&text[fenced.content.clone()], "let x = 1;");
        assert_eq!(&text[fenced.block.clone()], "```rust\nlet x = 1;\n```");
        assert!(!model.code_blocks[1].is_fenced);
        assert!(model.is_in_code_block(text.find("x =").expect("code")));
        assert!(!model.is_in_code_block(0));
    }

    #[test]
    fn knows_heading_lines() {
        let text = "## Title\nbody";
        let model = parse(text);
        assert!(model.is_in_heading(3, text.len()));
        assert!(!model.is_in_heading(text.len(), text.len()));
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        assert!(parse("text\n\n```\nopen").ends_open);
        assert!(!parse("```\nclosed\n```").ends_open);
    }
}
