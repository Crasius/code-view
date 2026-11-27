//! Markdown source to a small block tree the renderer lays out. Inline content of one block
//! becomes one run list, so a paragraph is laid out as a single galley: prose, links and code
//! spans then share rows, wrapping and baselines instead of being separate widgets.

// RS-DONT-04: parses pull request bodies and comments (with parse/builder and parse/html), so
// indexing, string slicing and narrowing casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing, clippy::cast_possible_truncation)]

mod builder;
mod html;
mod video;

use std::ops::Range;
use std::sync::Arc;

use cv_runtime::links::references;
use pulldown_cmark::{Alignment, BlockQuoteKind, Options, Parser};
use review_core::ProviderKind;

use self::builder::Builder;
pub(crate) use self::html::{HtmlMark, HtmlMarks};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Block {
    Paragraph(Inline),
    Heading(u8, Inline),
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Quote {
        alert: Option<BlockQuoteKind>,
        blocks: Vec<Block>,
    },
    /// `language` is the fence's info string (empty for indented code and bare fences).
    Code {
        language: String,
        code: String,
    },
    Table {
        alignments: Vec<Alignment>,
        header: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
    },
    Rule,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Item {
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

impl Block {
    /// The block's text in reading order, the way the renderer walks it: sibling blocks, list
    /// items and table cells are separated by one `\n`. Word-level change marks are byte ranges
    /// of this text.
    pub(crate) fn plain_text(&self, out: &mut String) {
        match self {
            Block::Paragraph(inline) | Block::Heading(_, inline) => inline.plain_text(out),
            Block::List { items, .. } => {
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push('\n');
                    }
                    item.plain_text(out);
                }
            }
            Block::Quote { blocks, .. } => plain_blocks(blocks, out),
            Block::Code { code, .. } => out.push_str(code),
            Block::Table { header, rows, .. } => {
                for (index, cell) in header.iter().chain(rows.iter().flatten()).enumerate() {
                    if index > 0 {
                        out.push('\n');
                    }
                    cell.plain_text(out);
                }
            }
            Block::Rule => {}
        }
    }
}

impl Item {
    /// Its blocks' text separated by `\n`, as [`Block::plain_text`].
    pub(crate) fn plain_text(&self, out: &mut String) {
        plain_blocks(&self.blocks, out);
    }
}

fn plain_blocks(blocks: &[Block], out: &mut String) {
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        block.plain_text(out);
    }
}

/// Where a top-level block is in the source, in bytes; for a list, each item too.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BlockSource {
    pub bytes: Range<usize>,
    pub items: Vec<Range<usize>>,
}

/// Top-level blocks with where each came from.
#[derive(Debug, Clone, Default)]
pub(crate) struct Parsed {
    pub blocks: Vec<Block>,
    /// One per block of `blocks`.
    pub sources: Vec<BlockSource>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Inline {
    pub runs: Vec<Run>,
}

impl Inline {
    /// Its runs' text, without the glyphs the renderer puts before some links.
    pub(crate) fn plain_text(&self, out: &mut String) {
        for run in &self.runs {
            out.push_str(&run.text);
        }
    }

    fn is_blank(&self) -> bool {
        self.runs.iter().all(|run| run.style.image.is_none() && run.text.trim().is_empty())
    }

    /// Whether any run is an image: such inlines flow text and images instead of being one galley.
    pub(crate) fn has_images(&self) -> bool {
        self.runs.iter().any(|run| run.style.image.is_some())
    }

    /// Plain runs (no link, code or image) become text, URL and reference pieces.
    fn link_plain_text(&mut self, platform: Option<ProviderKind>) {
        let mut runs = Vec::with_capacity(self.runs.len());
        for run in std::mem::take(&mut self.runs) {
            if !is_plain(&run.style) {
                runs.push(run);
                continue;
            }
            let pieces = plain_pieces(&run.text, platform);
            if let [(_, None)] = pieces.as_slice() {
                runs.push(run);
                continue;
            }
            for (piece, link) in pieces {
                let style = Style { link: link.map(str::to_owned), ..run.style.clone() };
                runs.push(Run { text: piece.to_owned(), style });
            }
        }
        self.runs = runs;
    }

    fn push(&mut self, text: &str, style: &Style) {
        match self.runs.last_mut() {
            Some(last) if last.style == *style && style.image.is_none() => last.text.push_str(text),
            _ => self.runs.push(Run { text: text.to_owned(), style: style.clone() }),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Run {
    pub text: String,
    pub style: Style,
}

#[derive(Debug, Clone, Default, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "strong, emphasis, strike and code are independent inline marks that combine freely"
)]
pub(crate) struct Style {
    pub strong: bool,
    pub emphasis: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<String>,
    /// The run is this image; its text is the image's alt text. With a `link`, the image is a link.
    pub image: Option<Arc<ImageSpec>>,
    /// The inline HTML marks around the run (`<ins>`, `<sub>`, `<sup>`, `<kbd>`).
    pub html: HtmlMarks,
}

/// One image as written: `![alt](src "title")` or an HTML `<img>`; or a video ([`video`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ImageSpec {
    pub src: String,
    pub alt: String,
    pub title: String,
    /// The `width` and `height` attributes of an `<img>` or `<video>`.
    pub width: Option<Length>,
    pub height: Option<Length>,
    /// Written as a video (`<video>`, `<source>`, a bare GitHub attachment).
    pub video: bool,
}

impl ImageSpec {
    /// Whether it plays: written as a video, or its address names a video file.
    pub(crate) fn is_video(&self) -> bool {
        self.video || video::names_video(&self.src)
    }
}

/// An HTML length: CSS pixels (points here) or a share of the text width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Length {
    Points(f32),
    Percent(f32),
}

impl Length {
    /// `200`, `200px` or `50%`; `None` for anything else.
    fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some(percent) = value.strip_suffix('%') {
            return percent
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|share| *share > 0.0)
                .map(Length::Percent);
        }
        let points = value.strip_suffix("px").unwrap_or(value).trim();
        points
            .parse::<f32>()
            .ok()
            .filter(|points| *points > 0.0 && points.is_finite())
            .map(Length::Points)
    }
}

/// Blocks of `text`; with a `platform`, its shorthand references (`#12`, `!12`) become links whose
/// target is the reference text itself.
pub(crate) fn parse(text: &str, platform: Option<ProviderKind>) -> Vec<Block> {
    parse_with_sources(text, platform).blocks
}

/// [`parse`], keeping where each top-level block (and each item of a top-level list) is in `text`.
pub(crate) fn parse_with_sources(text: &str, platform: Option<ProviderKind>) -> Parsed {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let mut builder = Builder::default();
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        builder.event(event, range);
    }
    let mut parsed = builder.finish();
    link_plain_text(&mut parsed.blocks, platform);
    parsed
}

/// Every link target in `blocks`, in reading order: URLs and shorthand references (a linked image's
/// link too), not image sources.
pub(crate) fn link_targets(blocks: &[Block], targets: &mut Vec<String>) {
    for_each_inline(blocks, &mut |inline| {
        targets.extend(inline.runs.iter().filter_map(|run| run.style.link.clone()));
    });
}

/// Every image source in `blocks`, in reading order.
pub(crate) fn image_sources<'a>(blocks: &'a [Block], sources: &mut Vec<&'a str>) {
    let mut specs = Vec::new();
    image_specs(blocks, &mut specs);
    sources.extend(specs.into_iter().map(|image| image.src.as_str()));
}

/// Every image (and video) of `blocks` as written, in reading order.
pub(crate) fn image_specs<'a>(blocks: &'a [Block], specs: &mut Vec<&'a ImageSpec>) {
    for_each_inline(blocks, &mut |inline| {
        specs.extend(inline.runs.iter().filter_map(|run| run.style.image.as_deref()));
    });
}

/// Calls `f` with every inline of `blocks` in reading order: paragraphs, headings and table cells,
/// inside lists and quotes too. Code and rules have none.
fn for_each_inline<'a>(blocks: &'a [Block], f: &mut impl FnMut(&'a Inline)) {
    for block in blocks {
        match block {
            Block::Paragraph(inline) | Block::Heading(_, inline) => f(inline),
            Block::List { items, .. } => {
                for item in items {
                    for_each_inline(&item.blocks, f);
                }
            }
            Block::Quote { blocks, .. } => for_each_inline(blocks, f),
            Block::Table { header, rows, .. } => {
                header.iter().chain(rows.iter().flatten()).for_each(&mut *f);
            }
            Block::Code { .. } | Block::Rule => {}
        }
    }
}

/// URLs and references in plain text (commit subjects, titles), in order.
pub(crate) fn plain_targets(text: &str, platform: ProviderKind) -> Vec<&str> {
    plain_pieces(text, Some(platform)).into_iter().filter_map(|(_, link)| link).collect()
}

/// Links bare URLs and, with a platform, its shorthand references in the plain runs of every
/// inline. Runs only once a block is complete: the parser splits text at characters like `_`,
/// and a URL such as `.../_git/repo` must be seen whole.
fn link_plain_text(blocks: &mut [Block], platform: Option<ProviderKind>) {
    for block in blocks {
        match block {
            Block::Paragraph(inline) => {
                inline.link_plain_text(platform);
                if let Some(video) = video::bare_attachment(inline) {
                    let style = Style { image: Some(Arc::new(video)), ..Style::default() };
                    inline.runs = vec![Run { text: String::new(), style }];
                }
            }
            Block::Heading(_, inline) => inline.link_plain_text(platform),
            Block::List { items, .. } => {
                for item in items {
                    link_plain_text(&mut item.blocks, platform);
                }
            }
            Block::Quote { blocks, .. } => link_plain_text(blocks, platform),
            Block::Table { header, rows, .. } => {
                for cell in header.iter_mut().chain(rows.iter_mut().flatten()) {
                    cell.link_plain_text(platform);
                }
            }
            Block::Code { .. } | Block::Rule => {}
        }
    }
}

/// Plain text cut into text, URL and reference pieces; `Some` carries the link target.
fn plain_pieces(text: &str, platform: Option<ProviderKind>) -> Vec<(&str, Option<&str>)> {
    let mut pieces = Vec::new();
    for (piece, url) in linkify(text) {
        let Some(platform) = platform.filter(|_| url.is_none()) else {
            pieces.push((piece, url));
            continue;
        };
        let mut last = 0;
        for range in references::find(piece, platform) {
            if let Some(before) = piece.get(last..range.start).filter(|before| !before.is_empty()) {
                pieces.push((before, None));
            }
            let Some(reference) = piece.get(range.clone()) else { continue };
            pieces.push((reference, Some(reference)));
            last = range.end;
        }
        if let Some(tail) = piece.get(last..).filter(|tail| !tail.is_empty()) {
            pieces.push((tail, None));
        }
    }
    pieces
}

fn is_plain(style: &Style) -> bool {
    style.link.is_none() && !style.code && style.image.is_none()
}

/// Splits plain text into pieces, marking bare `http(s)://` URLs as links like GitHub,
/// GitLab and Azure DevOps do.
fn linkify(text: &str) -> Vec<(&str, Option<&str>)> {
    let mut pieces = Vec::new();
    let mut rest = text;
    // The splits fall at ASCII matches, so they always succeed; a failed one ends the loop and
    // leaves the rest as plain text.
    while let Some((before, candidate)) =
        find_url_start(rest).and_then(|start| rest.split_at_checked(start))
    {
        let end = candidate
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '<' | '>' | '"'))
            .unwrap_or(candidate.len());
        let Some(word) = candidate.get(..end) else { break };
        let url = trim_url_end(word);
        if url.len() <= "https://".len() {
            let Some((plain, after)) = rest.split_at_checked(before.len() + end) else { break };
            pieces.push((plain, None));
            rest = after;
            continue;
        }
        let Some(after) = candidate.get(url.len()..) else { break };
        if !before.is_empty() {
            pieces.push((before, None));
        }
        pieces.push((url, Some(url)));
        rest = after;
    }
    if !rest.is_empty() {
        pieces.push((rest, None));
    }
    pieces
}

/// `word` without the punctuation of the sentence around it: `.,;:!?'` always, and a closing
/// parenthesis only while the URL has more closing than opening ones (GFM's rule), so
/// `https://en.wikipedia.org/wiki/Rust_(language)` stays whole and `(https://a.b/c)` loses its `)`.
fn trim_url_end(word: &str) -> &str {
    let mut url = word;
    loop {
        url = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
        let Some(inner) = url.strip_suffix(')') else {
            return url;
        };
        if url.matches('(').count() >= url.matches(')').count() {
            return url;
        }
        url = inner;
    }
}

fn find_url_start(text: &str) -> Option<usize> {
    let https = text.find("https://");
    let http = text.find("http://");
    match (https, http) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(inline: &Inline) -> Vec<(&str, &Style)> {
        inline.runs.iter().map(|run| (run.text.as_str(), &run.style)).collect()
    }

    #[test]
    fn paragraph_keeps_links_code_and_emphasis_in_one_inline() {
        let blocks = parse("A **bold** [link](https://x.test) and `code`.", None);
        let [Block::Paragraph(inline)] = blocks.as_slice() else { panic!("{blocks:?}") };
        let runs = texts(inline);
        assert_eq!(runs.len(), 7, "{runs:?}");
        assert!(runs[1].1.strong);
        assert_eq!(runs[3].1.link.as_deref(), Some("https://x.test"));
        assert!(runs[5].1.code);
    }

    #[test]
    fn html_marks_mark_their_text_and_other_tags_keep_it() {
        let blocks = parse(
            "<ins>u</ins> H<sub>2</sub>O x<sup>2</sup> <kbd>K</kbd> <u>v</u> <span>s</span>",
            None,
        );
        let [Block::Paragraph(inline)] = blocks.as_slice() else { panic!("{blocks:?}") };
        let marked = |text: &str, mark: HtmlMark| {
            texts(inline).iter().any(|(run, style)| *run == text && style.html.has(mark))
        };
        assert!(marked("u", HtmlMark::Underline), "{:?}", texts(inline));
        assert!(marked("v", HtmlMark::Underline));
        assert!(marked("2", HtmlMark::Subscript));
        assert!(marked("2", HtmlMark::Superscript));
        assert!(marked("K", HtmlMark::Kbd));
        let plain = texts(inline)
            .into_iter()
            .find(|(run, _)| run.contains('s'))
            .map(|(_, style)| style.html);
        assert_eq!(plain, Some(HtmlMarks::default()), "a tag outside the subset marks nothing");
        let blocks = parse("<p>a <kbd>Ctrl</kbd> b</p>", None);
        let [Block::Paragraph(inline)] = blocks.as_slice() else { panic!("{blocks:?}") };
        assert!(
            texts(inline)
                .iter()
                .any(|(run, style)| *run == "Ctrl" && style.html.has(HtmlMark::Kbd))
        );
    }

    #[test]
    fn tight_and_task_lists_become_items_with_paragraphs() {
        let blocks = parse("- [x] done\n- [ ] open\n  - nested\n", None);
        let [Block::List { start: None, items }] = blocks.as_slice() else { panic!("{blocks:?}") };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].task, Some(true));
        assert_eq!(items[1].task, Some(false));
        assert!(matches!(items[1].blocks.as_slice(), [Block::Paragraph(_), Block::List { .. }]));
    }

    #[test]
    fn gfm_tables_alerts_bare_fences_and_rules_become_their_blocks() {
        let blocks = parse(
            "| a | b |\n| - | - |\n| 1 | 2 |\n\n> [!NOTE]\n> quoted\n\n```\ncode\n```\n\n---\n",
            None,
        );
        assert!(
            matches!(&blocks[0], Block::Table { header, rows, .. } if header.len() == 2 && rows.len() == 1)
        );
        assert!(matches!(&blocks[1], Block::Quote { alert: Some(BlockQuoteKind::Note), .. }));
        assert_eq!(blocks[2], Block::Code { language: String::new(), code: "code".to_owned() });
        assert_eq!(blocks[3], Block::Rule);
    }

    #[test]
    fn code_blocks_keep_their_language() {
        let blocks = parse("```rust ignore\nfn main() {}\n```\n\n    indented\n", None);
        assert_eq!(
            blocks[0],
            Block::Code { language: "rust ignore".to_owned(), code: "fn main() {}".to_owned() }
        );
        assert_eq!(blocks[1], Block::Code { language: String::new(), code: "indented".to_owned() });
    }

    #[test]
    fn top_level_blocks_and_list_items_know_their_source() {
        let text = "# Title\n\nSome *text*\nover two lines.\n\n- one\n- two\n  - nested\n\n```\ncode\n```\n---\n";
        let parsed = parse_with_sources(text, None);
        assert_eq!(parsed.blocks.len(), parsed.sources.len());
        let sources: Vec<&str> = parsed
            .sources
            .iter()
            .map(|source| text.get(source.bytes.clone()).unwrap().trim_end())
            .collect();
        assert_eq!(
            sources,
            [
                "# Title",
                "Some *text*\nover two lines.",
                "- one\n- two\n  - nested",
                "```\ncode\n```",
                "---"
            ]
        );
        let items: Vec<&str> = parsed.sources[2]
            .items
            .iter()
            .map(|range| text.get(range.clone()).unwrap().trim_end())
            .collect();
        assert_eq!(items, ["- one", "- two\n  - nested"], "only the top-level list's own items");
    }

    #[test]
    fn plain_text_joins_blocks_items_and_cells_with_newlines() {
        let blocks = parse("- **a** b\n- c\n\n  d\n\n| h | i |\n| - | - |\n| 1 | 2 |\n", None);
        let texts: Vec<String> = blocks
            .iter()
            .map(|block| {
                let mut text = String::new();
                block.plain_text(&mut text);
                text
            })
            .collect();
        assert_eq!(texts, ["a b\nc\nd", "h\ni\n1\n2"]);
    }

    #[test]
    fn bare_urls_become_links_without_trailing_punctuation() {
        assert_eq!(
            linkify("see https://example.com/x. and (http://a.b/c)"),
            [
                ("see ", None),
                ("https://example.com/x", Some("https://example.com/x")),
                (". and (", None),
                ("http://a.b/c", Some("http://a.b/c")),
                (")", None),
            ]
        );
        assert_eq!(linkify("https:// alone"), [("https://", None), (" alone", None)]);
    }

    #[test]
    fn bare_urls_keep_their_balanced_parentheses() {
        let wiki = "https://en.wikipedia.org/wiki/Rust_(programming_language)";
        assert_eq!(
            linkify(&format!("see {wiki}.")),
            [("see ", None), (wiki, Some(wiki)), (".", None)]
        );
        assert_eq!(
            linkify(&format!("({wiki})")),
            [("(", None), (wiki, Some(wiki)), (")", None)],
            "the closing parenthesis around the link is text"
        );
    }

    #[test]
    fn references_become_links_outside_code_and_links() {
        let blocks = parse(
            "Fixes #59, see `#60` and [#61](https://x.test).\n\n| a |\n| - |\n| !7 |",
            Some(ProviderKind::GitLab),
        );
        let mut targets = Vec::new();
        link_targets(&blocks, &mut targets);
        assert_eq!(targets, ["#59", "https://x.test", "!7"]);
        let Block::Paragraph(inline) = &blocks[0] else { panic!("{blocks:?}") };
        assert_eq!(inline.runs[0].text, "Fixes ");
        assert_eq!(inline.runs[1].style.link.as_deref(), Some("#59"));
        let plain = parse("Fixes #59", None);
        let mut none = Vec::new();
        link_targets(&plain, &mut none);
        assert!(none.is_empty(), "without a platform there are no references");
    }

    #[test]
    fn bare_urls_with_underscores_stay_whole() {
        let blocks = parse("See https://dev.azure.com/org/Proj/_git/api/pullrequest/40 now", None);
        let mut targets = Vec::new();
        link_targets(&blocks, &mut targets);
        assert_eq!(targets, ["https://dev.azure.com/org/Proj/_git/api/pullrequest/40"]);
    }

    #[test]
    fn plain_targets_find_urls_and_references() {
        assert_eq!(
            plain_targets(
                "Merge !12 from https://gitlab.com/a/b/-/merge_requests/3 (#4)",
                ProviderKind::GitLab
            ),
            ["!12", "https://gitlab.com/a/b/-/merge_requests/3", "#4"]
        );
    }

    #[test]
    fn html_blocks_show_their_text_and_br_breaks_lines() {
        let blocks =
            parse("<details><summary>More</summary>\n\nBody\n\n</details>\n\nA<br>B", None);
        assert_eq!(blocks.len(), 3);
        let Block::Paragraph(last) = &blocks[2] else { panic!("{blocks:?}") };
        assert_eq!(last.runs[0].text, "A\nB");
    }

    fn image(run: &Run) -> &ImageSpec {
        run.style.image.as_deref().expect("an image run")
    }

    #[test]
    fn images_are_runs_of_their_own_and_keep_the_link_around_them() {
        let blocks = parse(
            "[![CI](https://x.test/ci.svg)](https://x.test/actions) ![a `b`](./img/a.png \"Title\")![c](c.png)",
            None,
        );
        let [Block::Paragraph(inline)] = blocks.as_slice() else { panic!("{blocks:?}") };
        assert!(inline.has_images());
        let images: Vec<&Run> =
            inline.runs.iter().filter(|run| run.style.image.is_some()).collect();
        assert_eq!(images.len(), 3, "adjacent images stay apart: {:?}", inline.runs);
        assert_eq!(
            (image(images[0]).src.as_str(), images[0].text.as_str()),
            ("https://x.test/ci.svg", "CI")
        );
        assert_eq!(images[0].style.link.as_deref(), Some("https://x.test/actions"));
        assert_eq!(
            (image(images[1]).alt.as_str(), image(images[1]).title.as_str()),
            ("a b", "Title")
        );
        assert_eq!(images[1].style.link, None);
        let mut sources = Vec::new();
        image_sources(&blocks, &mut sources);
        assert_eq!(sources, ["https://x.test/ci.svg", "./img/a.png", "c.png"]);
        let mut targets = Vec::new();
        link_targets(&blocks, &mut targets);
        assert_eq!(targets, ["https://x.test/actions"], "image sources are not link targets");
    }

    #[test]
    fn html_images_keep_their_size_and_links() {
        let html = "<p align=\"center\">\n  <a href=\"https://x.test\"><img src=\"docs/logo.png\" alt=\"Logo\" width=\"200\" height='50%'/></a>\n  <br>\n  <img src=https://img.shields.io/badge/a-b-green?x=1&amp;y=2>\n</p>\n";
        let blocks = parse(html, None);
        let [Block::Paragraph(inline)] = blocks.as_slice() else { panic!("{blocks:?}") };
        let images: Vec<&Run> =
            inline.runs.iter().filter(|run| run.style.image.is_some()).collect();
        assert_eq!(images.len(), 2, "{:?}", inline.runs);
        let logo = image(images[0]);
        assert_eq!((logo.src.as_str(), logo.alt.as_str()), ("docs/logo.png", "Logo"));
        assert_eq!(
            (logo.width, logo.height),
            (Some(Length::Points(200.0)), Some(Length::Percent(50.0)))
        );
        assert_eq!(images[0].style.link.as_deref(), Some("https://x.test"));
        assert_eq!(image(images[1]).src, "https://img.shields.io/badge/a-b-green?x=1&y=2");
        assert_eq!(images[1].style.link, None, "the link ended at </a>");
        assert!(
            inline.runs.iter().any(|run| run.text == "\n"),
            "the <br> breaks the line: {:?}",
            inline.runs
        );

        let inline_html = parse("Logo: <img src=\"a.png\" width=\"32px\"> done", None);
        let [Block::Paragraph(inline)] = inline_html.as_slice() else { panic!("{inline_html:?}") };
        assert_eq!(
            inline
                .runs
                .iter()
                .filter_map(|run| run.style.image.as_ref())
                .map(|image| image.width)
                .collect::<Vec<_>>(),
            [Some(Length::Points(32.0))]
        );
    }
}
