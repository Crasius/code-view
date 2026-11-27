//! Markdown rendering for descriptions, comments and file previews. Links open in the browser,
//! unless the screen published a [`LinkResolver`] that knows them: then pull request links and
//! shorthand references (`#12`, `!12`) open in the app, and Ctrl+click or the context menu still
//! opens the web page. Images load through the [`ImageSource`] the screen published (see
//! `images`); without one they show as a card. Mermaid fences and Azure DevOps' `::: mermaid` containers
//! show as diagrams on every platform (see `mermaid`).
// RS-LINT-02: egui lays out in f32 points while lines, columns and counts are usize, so layout
// math converts between them (`lib.rs`, `render.rs`, `mermaid.rs`, `video.rs`, `images/`).
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "egui layout math converts between f32 points and usize counts"
)]
#![forbid(unsafe_code)]

mod images;
mod mermaid;
mod parse;
mod render;
mod video;

pub use images::{
    FileBase, ImageBase, ImageRequest, ImageSource, file_base, publish_images, show_lightbox,
};

/// Code size step of the rendered view, shared with the comment editor so fenced code has the
/// same size in Write and Preview.
pub use render::CODE_SIZE_DELTA;
/// Changed words of a block in a rendered diff.
pub use render::Marks;
/// Type and shape tokens of the rendered view, shared with the rich editor so writing and
/// reading look alike.
pub use render::typeset::SCRIPT_SCALE;
pub use render::{BULLETS, CHECKBOX_SIZE, HEADING_SIZES, QUOTE_BAR_WIDTH};

use std::borrow::Cow;
use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError};

use cv_base::mailbox;
use cv_base::platform::shortcut;
use cv_runtime::cache::memory::{self, BoundedCache, Holder};
use egui::Ui;
use review_core::ProviderKind;

/// How [`render()`] lays out a document.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderOptions {
    /// Follow this platform's dialect; `None` follows CommonMark/GFM.
    pub platform: Option<ProviderKind>,
    /// Multiplies every size (type, spacing, markers); 1.0 is the app's type scale.
    pub scale: f32,
    /// The repository file whose relative image paths resolve against it (see [`file_base`]);
    /// `None` for descriptions and comments.
    pub images: Option<ImageBase>,
    /// A change color drawn around every image (the rendered diff's added and removed blocks).
    pub image_border: Option<egui::Color32>,
    /// How wide a top-level table may grow, centered on the text, to show its columns without
    /// scrolling (the rendered diff's reading column); narrower than the text, none grow.
    pub breakout: f32,
}

impl RenderOptions {
    /// Descriptions and comments: the platform's dialect at the app's type scale.
    pub fn for_platform(platform: ProviderKind) -> Self {
        Self { platform: Some(platform), ..Self::default() }
    }
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self { platform: None, scale: 1.0, images: None, image_border: None, breakout: 0.0 }
    }
}

/// Renders markdown as `options.platform` does, at `options.scale`. An image that changes its
/// height above the view (it arrived) moves the enclosing scroll area with it.
pub fn render(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    text: &str,
    options: RenderOptions,
) {
    let id = ui.scope_id().with(id);
    let document = Document::load(ui.ctx(), text, options.platform);
    let palette = cv_base::theme::palette(ui.ctx());
    if document.blocks.is_empty() {
        ui.label(egui::RichText::new("No description.").italics().color(palette.text_secondary));
        return;
    }
    let links = current_links(ui.ctx());
    render::Renderer::new(palette, id, options.scale, links)
        .with_images(options.images, options.image_border, images::Anchor::ScrollsView)
        .show(ui, &document.blocks, document.key);
}

/// What a [`Unit`] is, for pairing an edited block with its old version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    Paragraph,
    Heading,
    ListItem,
    Quote,
    Code,
    Table,
    Rule,
}

/// One piece a rendered diff lines up: a top-level block, or one item of a top-level list (so an
/// item added to a long list is one added piece, not a changed list).
#[derive(Debug, Clone)]
pub struct Unit {
    block: usize,
    item: Option<usize>,
    pub kind: UnitKind,
    /// Source lines, 1-based, end exclusive.
    pub lines: Range<u32>,
    /// The text in reading order ([`Marks`] are byte ranges of it).
    pub text: String,
    /// Equal for pieces that render the same (an item's number aside).
    pub signature: u64,
    /// Images it shows, for its estimated height.
    images: usize,
}

/// A Markdown text cut into [`Unit`]s, each of which renders on its own.
pub struct Units {
    blocks: Vec<parse::Block>,
    pub units: Vec<Unit>,
}

impl Units {
    /// The pieces of `text` in `platform`'s dialect, as [`render()`] would show them.
    pub fn parse(text: &str, platform: Option<ProviderKind>) -> Self {
        let Some(source) = dialect_source(text, platform) else {
            return Self { blocks: Vec::new(), units: Vec::new() };
        };
        let breaks: Vec<usize> = source.match_indices('\n').map(|(at, _)| at).collect();
        let line_of = |offset: usize| breaks.partition_point(|at| *at < offset) as u32 + 1;
        let lines_of = |bytes: &Range<usize>| {
            line_of(bytes.start)..line_of(bytes.end.max(bytes.start + 1) - 1) + 1
        };
        let parsed = parse::parse_with_sources(&source, platform);
        let mut units = Vec::with_capacity(parsed.blocks.len());
        for (index, (block, source)) in parsed.blocks.iter().zip(&parsed.sources).enumerate() {
            if let parse::Block::List { items, .. } = block {
                for (item_index, (item, bytes)) in items.iter().zip(&source.items).enumerate() {
                    let mut text = String::new();
                    item.plain_text(&mut text);
                    let mut sources = Vec::new();
                    parse::image_sources(&item.blocks, &mut sources);
                    units.push(Unit {
                        block: index,
                        item: Some(item_index),
                        kind: UnitKind::ListItem,
                        lines: lines_of(bytes),
                        signature: egui::epaint::util::hash(("item", format!("{item:?}"))),
                        text,
                        images: sources.len(),
                    });
                }
            } else {
                let mut text = String::new();
                block.plain_text(&mut text);
                let mut sources = Vec::new();
                parse::image_sources(std::slice::from_ref(block), &mut sources);
                units.push(Unit {
                    block: index,
                    item: None,
                    kind: unit_kind(block),
                    lines: lines_of(&source.bytes),
                    signature: egui::epaint::util::hash(format!("{block:?}")),
                    text,
                    images: sources.len(),
                });
            }
        }
        Self { blocks: parsed.blocks, units }
    }

    pub fn len(&self) -> usize {
        self.units.len()
    }

    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// Lays out unit `index` at `ui`'s cursor, its changed words `marks` filled. `id` must be
    /// unique and stable for this unit on screen.
    pub fn show(
        &self,
        ui: &mut Ui,
        id: egui::Id,
        index: usize,
        options: RenderOptions,
        marks: impl IntoIterator<Item = Marks>,
    ) {
        let Some(unit) = self.units.get(index) else {
            return;
        };
        let palette = cv_base::theme::palette(ui.ctx());
        let links = current_links(ui.ctx());
        // The rendered diff measures its rows again when an image arrives
        // (`cv_media::image_state::image_generation`) and keeps its own anchor.
        render::Renderer::new(palette, id, options.scale, links)
            .with_marks(marks)
            .with_images(options.images, options.image_border, images::Anchor::LeftToView)
            .with_breakout(options.breakout)
            .show_unit(ui, &self.blocks[unit.block], unit.item);
    }

    /// Every image source of unit `index`, as written, in reading order.
    pub(crate) fn image_sources(&self, index: usize) -> Vec<&str> {
        let mut sources = Vec::new();
        if let Some(unit) = self.units.get(index) {
            match (&self.blocks[unit.block], unit.item) {
                (parse::Block::List { items, .. }, Some(item)) => {
                    if let Some(item) = items.get(item) {
                        parse::image_sources(&item.blocks, &mut sources);
                    }
                }
                (block, _) => parse::image_sources(std::slice::from_ref(block), &mut sources),
            }
        }
        sources
    }

    /// Makes units differ that show an image whose content differs although its source does not
    /// (`./shot.png` replaced in the same pull request): `content` gives the content id of each
    /// such source in this version (the blob id). Units that use none keep their signature.
    pub fn distinguish_images(&mut self, content: &std::collections::BTreeMap<String, String>) {
        if content.is_empty() {
            return;
        }
        for index in 0..self.units.len() {
            let ids: Vec<&String> = self
                .image_sources(index)
                .into_iter()
                .filter_map(|source| content.get(source))
                .collect();
            if !ids.is_empty() {
                let salted = egui::epaint::util::hash((self.units[index].signature, ids));
                self.units[index].signature = salted;
            }
        }
    }

    /// Space above `next` after `previous` (each a text and a unit of it), at scale 1: as the
    /// whole document spaces its blocks, and items of one list as close as in the list. In a
    /// unified diff `previous` and `next` may come from the old and the new text.
    pub fn gap(previous: (&Units, usize), next: (&Units, usize)) -> f32 {
        let (Some(before), Some(after)) =
            (previous.0.units.get(previous.1), next.0.units.get(next.1))
        else {
            return 0.0;
        };
        let same_text = std::ptr::eq(previous.0, next.0);
        let same_list = before.kind == UnitKind::ListItem
            && after.kind == UnitKind::ListItem
            && (!same_text || before.block == after.block);
        if same_list {
            return cv_base::theme::style::SPACE_XS;
        }
        render::gap(&previous.0.blocks[before.block], &next.0.blocks[after.block])
    }

    /// A rough height of unit `index` at `width` and `scale`, for pieces not laid out yet.
    pub fn estimate_height(&self, index: usize, width: f32, scale: f32) -> f32 {
        let Some(unit) = self.units.get(index) else {
            return 0.0;
        };
        let body = cv_base::theme::style::BODY_SIZE * scale;
        let line_height = body * 1.4;
        let per_line = (width / (body * 0.5)).max(8.0);
        let lines: f32 = unit
            .text
            .split('\n')
            .map(|line| (line.chars().count() as f32 / per_line).ceil().max(1.0))
            .sum();
        if unit.images > 0 {
            // Images stand on rows of their own, at most a placeholder high until they arrive.
            return unit.images as f32 * images::placeholder_height(scale) + line_height * lines;
        }
        match unit.kind {
            UnitKind::Code => unit.text.split('\n').count() as f32 * line_height + body * 1.6,
            UnitKind::Heading => line_height * 1.4 * lines,
            UnitKind::Rule => render::rule_height(scale),
            UnitKind::Table => line_height * 1.6 * lines,
            UnitKind::Paragraph | UnitKind::ListItem | UnitKind::Quote => line_height * lines,
        }
    }
}

fn unit_kind(block: &parse::Block) -> UnitKind {
    match block {
        parse::Block::Paragraph(_) => UnitKind::Paragraph,
        parse::Block::Heading(..) => UnitKind::Heading,
        parse::Block::List { .. } => UnitKind::ListItem,
        parse::Block::Quote { .. } => UnitKind::Quote,
        parse::Block::Code { .. } => UnitKind::Code,
        parse::Block::Table { .. } => UnitKind::Table,
        parse::Block::Rule => UnitKind::Rule,
    }
}

/// Every link target of a Markdown text in reading order, without duplicates: URLs and, with a
/// `platform`, its shorthand references. Code and images are skipped.
pub fn link_targets(text: &str, platform: Option<ProviderKind>) -> Vec<String> {
    let mut targets = Vec::new();
    parse::link_targets(&parse_for(text, platform), &mut targets);
    dedupe(targets)
}

/// Every image source of a Markdown text in reading order, without duplicates.
pub fn image_sources(text: &str, platform: Option<ProviderKind>) -> Vec<String> {
    let blocks = parse_for(text, platform);
    let mut sources = Vec::new();
    parse::image_sources(&blocks, &mut sources);
    dedupe(sources.into_iter().map(str::to_owned).collect())
}

/// The sources of a Markdown text's still images (videos left out) in reading order, without
/// duplicates.
pub fn still_image_sources(text: &str, platform: Option<ProviderKind>) -> Vec<String> {
    let blocks = parse_for(text, platform);
    let mut specs = Vec::new();
    parse::image_specs(&blocks, &mut specs);
    let stills = specs.into_iter().filter(|image| !image.is_video());
    dedupe(stills.map(|image| image.src.clone()).collect())
}

/// URLs and shorthand references in plain text (commit subjects, titles, branch names).
pub fn plain_link_targets(text: &str, platform: ProviderKind) -> Vec<String> {
    dedupe(parse::plain_targets(text, platform).into_iter().map(str::to_owned).collect())
}

fn dedupe(targets: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    targets.into_iter().filter(|target| seen.insert(target.clone())).collect()
}

/// Whether a link opens in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InApp {
    /// Opens in the browser.
    No,
    /// A pull request the app can show.
    Yes,
    /// Opens in the app if the platform says it is a pull request (GitHub's `#12`).
    Maybe,
}

/// How the renderer shows and opens one link target.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLink {
    /// The page Ctrl+click and "Open in browser" open.
    pub web_url: String,
    pub in_app: InApp,
    /// A glyph drawn before the link text. Only for targets whose kind is known from the text
    /// alone, so it never appears later and moves the line.
    pub icon: Option<&'static str>,
    /// What the target is called, when known, for the tooltip.
    pub title: Option<String>,
}

/// Turns link targets (URLs and shorthand references like `#12`) into what the renderer shows.
/// A screen publishes one for each frame; without it URLs open in the browser and references
/// stay plain text.
pub trait LinkResolver: Send + Sync {
    /// `None` leaves the target as plain text (a reference no connection can resolve).
    fn resolve(&self, target: &str) -> Option<ResolvedLink>;
}

const LINKS_ID: &str = "review.markdown.links";
const IN_APP_ID: &str = "review.markdown.open_in_app";

/// Makes `links` resolve the links of every Markdown text drawn this frame.
pub fn publish_links(ctx: &egui::Context, links: Arc<dyn LinkResolver>) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| data.insert_temp(egui::Id::unique(LINKS_ID), (pass, links)));
}

/// The resolver published this frame, for a view that adds its own targets around it.
pub fn current_links(ctx: &egui::Context) -> Option<Arc<dyn LinkResolver>> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|data| data.get_temp::<(u64, Arc<dyn LinkResolver>)>(egui::Id::unique(LINKS_ID)))
        .filter(|(published, _)| *published == pass)
        .map(|(_, links)| links)
}

/// The link target the user clicked to open in the app this frame, once.
pub fn take_in_app_request(ctx: &egui::Context) -> Option<String> {
    mailbox::take(ctx, egui::Id::unique(IN_APP_ID))
}

fn request_in_app(ctx: &egui::Context, target: &str) {
    mailbox::post(ctx, egui::Id::unique(IN_APP_ID), target.to_owned());
}

/// Tooltip text for a link: what it is, then how it opens.
fn link_hint(link: &ResolvedLink) -> String {
    let what = link.title.as_deref().unwrap_or(&link.web_url);
    match link.in_app {
        InApp::Yes => format!("{what}\nOpen in Code View ({}: browser)", shortcut!(command click)),
        InApp::Maybe => format!(
            "{what}\nOpens in Code View if it is a pull request ({}: browser)",
            shortcut!(command click)
        ),
        InApp::No => what.to_owned(),
    }
}

/// Parsed documents kept for every rendered text, as estimated by [`memory::debug_weight`]: our
/// budget, a few hundred comments or a few long READMEs.
const DOCUMENTS_BUDGET_BYTES: usize = 8 * 1024 * 1024;
const DOCUMENTS_ID: &str = "review.markdown.documents";

/// The parsed blocks of the rendered texts by text and dialect, shared by every view of the
/// context, the least recently shown dropped first.
type Documents = Arc<Mutex<BoundedCache<u64, Arc<Vec<parse::Block>>>>>;

/// The parsed blocks of one rendered text, kept so a long description is parsed once, not on
/// every frame.
struct Document {
    key: u64,
    blocks: Arc<Vec<parse::Block>>,
}

impl Document {
    fn load(ctx: &egui::Context, text: &str, platform: Option<ProviderKind>) -> Self {
        let key = egui::epaint::util::hash((text, platform));
        let documents: Documents = ctx.data_mut(|data| {
            data.get_temp_mut_or_insert_with(egui::Id::unique(DOCUMENTS_ID), || {
                Arc::new(Mutex::new(BoundedCache::new(Holder::Markdown, DOCUMENTS_BUDGET_BYTES)))
            })
            .clone()
        });
        let mut documents = documents.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(blocks) = documents.get(&key) {
            return Self { key, blocks: blocks.clone() };
        }
        let blocks = Arc::new(parse_for(text, platform));
        documents.insert(key, blocks.clone(), memory::debug_weight(&*blocks));
        Self { key, blocks }
    }
}

/// The blocks of `text` in `platform`'s dialect; none when only whitespace or comments remain.
fn parse_for(text: &str, platform: Option<ProviderKind>) -> Vec<parse::Block> {
    dialect_source(text, platform).map_or_else(Vec::new, |source| parse::parse(&source, platform))
}

/// `text` as `platform`'s dialect hands it to the parser; `None` when only whitespace or comments
/// remain. Comments keep their line breaks, diagram containers their length and Azure DevOps
/// headings only gain a space: lines of the result are lines of `text`.
fn dialect_source(text: &str, platform: Option<ProviderKind>) -> Option<String> {
    let visible = without_html_comments(text);
    if visible.trim().is_empty() {
        return None;
    }
    let visible = mermaid::containers_as_fences(&visible);
    let source = match platform {
        Some(ProviderKind::AzureDevOps) => azure_headings(&visible),
        Some(ProviderKind::GitHub | ProviderKind::GitLab) | None => visible,
    };
    Some(source.into_owned())
}

/// HTML comments are hidden on every platform (templates use them for instructions). A comment
/// over several lines leaves its line breaks, so every block stays on its source lines. A `<!--`
/// inside a code span or code block is code and stays, with everything after it.
fn without_html_comments(text: &str) -> Cow<'_, str> {
    if !text.contains("<!--") {
        return Cow::Borrowed(text);
    }
    let code = code_ranges(text);
    let mut visible = String::with_capacity(text.len());
    let mut position = 0;
    while let Some(found) = text[position..].find("<!--") {
        let start = position + found;
        if let Some(code) = code.iter().find(|code| code.contains(&start)) {
            visible.push_str(&text[position..code.end]);
            position = code.end;
            continue;
        }
        visible.push_str(&text[position..start]);
        let end = text[start..].find("-->").map_or(text.len(), |end| start + end + 3);
        visible.extend(std::iter::repeat_n('\n', text[start..end].matches('\n').count()));
        position = end;
    }
    visible.push_str(&text[position..]);
    Cow::Owned(visible)
}

/// Byte ranges of the code spans and code blocks of `text`.
fn code_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    use pulldown_cmark::{Event, Options, Parser, Tag};
    Parser::new_ext(text, Options::ENABLE_TABLES | Options::ENABLE_GFM)
        .into_offset_iter()
        .filter(|(event, _)| matches!(event, Event::Code(_) | Event::Start(Tag::CodeBlock(_))))
        .map(|(_, range)| range)
        .collect()
}

/// Azure DevOps renders `#Title` and `##1. Step` as headings; `CommonMark` (and so GitHub and
/// GitLab) needs a space after the hashes. A single `#` before a digit stays text: that is an
/// Azure DevOps work item mention (`#123`). Fenced code is left alone.
fn azure_headings(text: &str) -> Cow<'_, str> {
    let mut fence: Option<&str> = None;
    let mut changed = false;
    let mut out = String::with_capacity(text.len() + 16);
    for line in text.split_inclusive('\n') {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let body = &line[indent..];
        if let Some(marker) = fence {
            if body.starts_with(marker) {
                fence = None;
            }
        } else if body.starts_with("```") || body.starts_with("~~~") {
            fence = Some(&body[..3]);
        } else if indent <= 3 {
            let hashes = body.len() - body.trim_start_matches('#').len();
            let next = body[hashes..].chars().next();
            let glued = (1..=6).contains(&hashes)
                && next.is_some_and(|ch| !ch.is_whitespace() && ch != '#')
                && !(hashes == 1 && next.is_some_and(|ch| ch.is_ascii_digit()));
            if glued {
                out.push_str(&line[..indent + hashes]);
                out.push(' ');
                out.push_str(&body[hashes..]);
                changed = true;
                continue;
            }
        }
        out.push_str(line);
    }
    if changed { Cow::Owned(out) } else { Cow::Borrowed(text) }
}

#[cfg(test)]
mod images_tests;
#[cfg(test)]
mod mermaid_tests;
#[cfg(test)]
mod render_tests;
#[cfg(test)]
mod test_document;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ops::Range;

    use review_core::ProviderKind;

    use super::{
        UnitKind, Units, azure_headings, link_targets, plain_link_targets, without_html_comments,
    };

    #[test]
    fn link_targets_follow_the_platform_dialect() {
        let text = "#12 fixes it, see !4 and https://x.test/a\n\n<!-- #99 hidden -->\n\n#12 again";
        assert_eq!(
            link_targets(text, Some(ProviderKind::AzureDevOps)),
            ["#12", "!4", "https://x.test/a"]
        );
        assert_eq!(link_targets(text, None), ["https://x.test/a"]);
        assert_eq!(plain_link_targets("Fix #3 (#3)", ProviderKind::GitHub), ["#3"]);
    }

    #[test]
    fn hides_html_comments() {
        assert_eq!(without_html_comments("a <!-- x --> b"), "a  b");
        assert_eq!(without_html_comments("<!-- a -->\nText\n<!-- b"), "\nText\n");
        assert_eq!(without_html_comments("plain"), "plain");
        assert_eq!(
            without_html_comments("a\n<!--\nhidden\n-->\nb"),
            "a\n\n\n\nb",
            "lines keep their numbers"
        );
    }

    #[test]
    fn keeps_comment_markers_inside_code() {
        let inline = "Open one with `<!--` and close it.\n\nThe rest stays.";
        assert_eq!(without_html_comments(inline), inline);
        assert_eq!(
            without_html_comments("```html\n<!-- shown -->\n```\n<!-- hidden -->after"),
            "```html\n<!-- shown -->\n```\nafter"
        );
    }

    #[test]
    fn azure_headings_get_their_space() {
        assert_eq!(azure_headings("#🚀 What's Changing?\n"), "# 🚀 What's Changing?\n");
        assert_eq!(azure_headings("##1. Review the YAML file(s)"), "## 1. Review the YAML file(s)");
        assert_eq!(azure_headings("  ###Deep"), "  ### Deep");
    }

    #[test]
    fn azure_headings_leave_other_text_alone() {
        for text in [
            "# Already spaced",
            "#123 fixes the bug",
            "####### seven",
            "    #indented code",
            "a #tag in text",
            "```\n#include <x>\n```",
            "#",
        ] {
            assert_eq!(azure_headings(text), text);
        }
    }

    /// A rendered diff anchors comments and change marks to these lines: hidden comments, Azure
    /// DevOps headings and diagram containers must not shift them.
    #[test]
    fn units_keep_their_source_lines() {
        let text = concat!(
            "#Title\n",
            "\n",
            "<!--\n",
            "hidden\n",
            "-->\n",
            "Para one\n",
            "\n",
            "- a\n",
            "- b\n",
            "\n",
            "::: mermaid\n",
            "graph TD\n",
            ":::\n",
            "\n",
            "last",
        );
        let units = Units::parse(text, Some(ProviderKind::AzureDevOps));
        let found: Vec<(UnitKind, u32)> =
            units.units.iter().map(|unit| (unit.kind, unit.lines.start)).collect();
        assert_eq!(
            found,
            [
                (UnitKind::Heading, 1),
                (UnitKind::Paragraph, 6),
                (UnitKind::ListItem, 8),
                (UnitKind::ListItem, 9),
                (UnitKind::Code, 11),
                (UnitKind::Paragraph, 15),
            ]
        );
        let ranges: Vec<Range<u32>> = units.units.iter().map(|unit| unit.lines.clone()).collect();
        assert_eq!(ranges[0], 1..2, "the heading is one line");
        assert_eq!(ranges[4], 11..14, "the diagram spans its container's three lines");
        assert_eq!(units.units[2].text, "a", "each list item is a piece of its own");
        assert!(Units::parse("<!-- only a note -->", None).is_empty());
    }

    #[test]
    fn replaced_images_change_only_the_units_that_show_them() {
        let text = "![shot](./shot.png)\n\nplain\n\n- ![logo](logo.png)";
        let plain = Units::parse(text, None);
        let content = BTreeMap::from([("./shot.png".to_owned(), "blob-2".to_owned())]);
        let mut replaced = Units::parse(text, None);
        replaced.distinguish_images(&content);
        let changed: Vec<bool> = plain
            .units
            .iter()
            .zip(&replaced.units)
            .map(|(before, after)| before.signature != after.signature)
            .collect();
        assert_eq!(changed, [true, false, false]);
        let mut same = Units::parse(text, None);
        same.distinguish_images(&content);
        assert_eq!(
            same.units[0].signature, replaced.units[0].signature,
            "same content, same piece"
        );
    }
}
