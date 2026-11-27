//! Block layout: one galley per block in the rendered comment's type (sizes from
//! `cv_markdown`, colours from the palette), the block's indent, panel inset and height,
//! and the y offset of every block.
//!
//! Cache (RS-PERF-01): galleys are kept per block, keyed by the block's pointer (the entry holds
//! the `Arc`, so a pointer is never reused while it is cached) and valid for one layout key: wrap
//! width, palette, pixels per point, font atlas epoch and the people list mention chips name.
//! A new key drops every entry. Entries of blocks that left the document drop when the offsets
//! are rebuilt after an edit, and above `MAX_ENTRIES` the least recently used half goes. Offsets
//! are rebuilt only after an edit or when a laid-out block's height differs from its estimate;
//! a frame without either costs a binary search plus the visible blocks' cache lookups.

mod look;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cv_base::theme::{Palette, style};
use cv_markdown::{HEADING_SIZES, QUOTE_BAR_WIDTH};
use cv_widgets::atlas_watch::AtlasWatch;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Context, FontId, Galley, Vec2, vec2};
use review_core::ProviderKind;

use self::look::{Look, chip_label};
use super::model::{Block, Container, Document, HARD_BREAK, Kind, ListKind, Marks};
use crate::markdown_editor::PullRequestContext;

/// Our budget of cached block layouts per editor: a few screens of a long document.
const MAX_ENTRIES: usize = 2048;
/// Indent of a quote's content: its bar and the gap after it, as the renderer spaces it.
const QUOTE_INDENT: f32 = QUOTE_BAR_WIDTH + style::SPACE_M;
/// Width of a list item's marker column, as the renderer's.
pub(crate) const ITEM_INDENT: f32 = style::SPACE_L + style::SPACE_M;
/// Inside a code, HTML or table panel.
pub(crate) const PANEL_INSET: Vec2 = vec2(style::SPACE_M, style::SPACE_S + 2.0);
const RULE_HEIGHT: f32 = style::SPACE_S * 2.0 + 1.0;
/// Room between a chip's text and its edges.
const CHIP_PAD: f32 = 4.0;

/// A laid-out block.
pub(crate) struct Laid {
    pub galley: Arc<Galley>,
    pub chips: Vec<Chip>,
    /// Left of the text column, from the content's left edge.
    pub indent: f32,
    /// Padding around the galley (panels only).
    pub inset: Vec2,
    pub height: f32,
}

/// An atom drawn over the space its char reserves.
pub(crate) struct Chip {
    /// Char index in the block.
    pub index: usize,
    pub label: String,
    pub kind: ChipKind,
    pub font: FontId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChipKind {
    Mention,
    Image,
    Raw,
}

struct Entry {
    block: Arc<Block>,
    laid: Arc<Laid>,
    used: u64,
}

/// A list marker in a block's indent column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Marker {
    Bullet { depth: usize },
    Number(u64),
    Task { checked: bool },
}

/// Where the blocks are, for one document state and layout key.
#[derive(Default)]
pub(crate) struct Offsets {
    /// Top of each block, from the content's top.
    pub tops: Vec<f32>,
    pub heights: Vec<f32>,
    pub total: f32,
    /// Per block, the markers of the items it starts: (x of the marker column, marker).
    pub markers: Vec<Vec<(f32, Marker)>>,
}

pub(crate) struct LayoutCache {
    key: u64,
    atlas: AtlasWatch,
    atlas_epoch: u64,
    width: f32,
    palette: Palette,
    platform: ProviderKind,
    people: Option<PullRequestContext>,
    entries: HashMap<usize, Entry>,
    frame: u64,
    offsets: Offsets,
    is_dirty: bool,
}

impl Default for LayoutCache {
    fn default() -> Self {
        Self {
            key: 0,
            atlas: AtlasWatch::default(),
            atlas_epoch: 0,
            width: 0.0,
            palette: Palette::LIGHT,
            platform: ProviderKind::GitHub,
            people: None,
            entries: HashMap::new(),
            frame: 0,
            offsets: Offsets::default(),
            is_dirty: false,
        }
    }
}

impl LayoutCache {
    /// Starts a frame at content width `width`.
    pub(crate) fn prepare(
        &mut self,
        ctx: &Context,
        palette: Palette,
        width: f32,
        platform: ProviderKind,
        people: Option<PullRequestContext>,
    ) {
        self.frame += 1;
        if self.atlas.is_rebuilt(ctx) {
            self.atlas_epoch += 1;
        }
        let people_id = people.as_ref().map(|context| Arc::as_ptr(&context.people) as usize);
        let key = egui::epaint::util::hash((
            self.atlas_epoch,
            palette.is_dark,
            width.to_bits(),
            ctx.pixels_per_point().to_bits(),
            people_id,
            platform,
        ));
        self.width = width;
        self.palette = palette;
        self.platform = platform;
        self.people = people;
        if key != self.key {
            self.key = key;
            self.entries.clear();
            self.is_dirty = true;
        }
    }

    /// How many block layouts are cached.
    #[cfg(test)]
    pub fn cached(&self) -> usize {
        self.entries.len()
    }

    /// The document changed: offsets are rebuilt at the next look.
    pub(crate) fn invalidate(&mut self) {
        self.is_dirty = true;
    }

    /// Block `index`, laid out.
    pub(crate) fn get(&mut self, ctx: &Context, doc: &Document, index: usize) -> Arc<Laid> {
        let block = &doc.blocks[index.min(doc.blocks.len() - 1)];
        let key = Arc::as_ptr(block) as usize;
        if let Some(entry) = self.entries.get_mut(&key)
            && Arc::ptr_eq(&entry.block, block)
        {
            entry.used = self.frame;
            return entry.laid.clone();
        }
        let laid = Arc::new(lay_out(
            ctx,
            block,
            &self.palette,
            self.width,
            self.people.as_ref(),
            self.platform,
        ));
        if self.offsets.heights.get(index).is_some_and(|height| (height - laid.height).abs() > 0.5)
        {
            self.is_dirty = true;
        }
        if self.entries.len() >= MAX_ENTRIES {
            self.evict_oldest();
        }
        self.entries
            .insert(key, Entry { block: block.clone(), laid: laid.clone(), used: self.frame });
        laid
    }

    fn evict_oldest(&mut self) {
        let mut used: Vec<u64> = self.entries.values().map(|entry| entry.used).collect();
        used.sort_unstable();
        let cut = used.get(used.len() / 2).copied().unwrap_or_default();
        self.entries.retain(|_, entry| entry.used > cut);
    }

    /// The blocks' offsets, rebuilt after an edit or a height change.
    pub(crate) fn offsets(&mut self, doc: &Document) -> &Offsets {
        if self.is_dirty || self.offsets.tops.len() != doc.blocks.len() {
            self.rebuild(doc);
        }
        &self.offsets
    }

    fn rebuild(&mut self, doc: &Document) {
        self.is_dirty = false;
        let present: HashSet<usize> =
            doc.blocks.iter().map(|block| Arc::as_ptr(block) as usize).collect();
        self.entries.retain(|key, _| present.contains(key));
        let mut offsets = Offsets {
            tops: Vec::with_capacity(doc.blocks.len()),
            heights: Vec::with_capacity(doc.blocks.len()),
            total: 0.0,
            markers: markers(doc),
        };
        let mut y = 0.0;
        for (index, block) in doc.blocks.iter().enumerate() {
            if let Some(previous) = index.checked_sub(1).map(|before| &doc.blocks[before]) {
                y += gap(previous, block);
            }
            let cached =
                self.entries.get(&(Arc::as_ptr(block) as usize)).map(|entry| entry.laid.height);
            let height = cached.unwrap_or_else(|| estimate(block, self.width));
            offsets.tops.push(y);
            offsets.heights.push(height);
            y += height;
        }
        offsets.total = y;
        self.offsets = offsets;
    }
}

/// Vertical space between two blocks: as the renderer spaces blocks, items of one list closer.
pub(crate) fn gap(previous: &Block, next: &Block) -> f32 {
    let same_list = next.nesting.iter().zip(&previous.nesting).any(|(here, there)| {
        here.is_item() && (here.continues(there) || here.is_next_item_of(there))
    });
    if same_list && next.kind != Kind::Heading(1) {
        return style::SPACE_XS;
    }
    match (&previous.kind, &next.kind) {
        (_, Kind::Heading(_)) => style::SPACE_L + style::SPACE_S,
        (Kind::Heading(_), _) => style::SPACE_S,
        _ => style::SPACE_L,
    }
}

/// How far `container` moves its content right: the text, the marker column and the quote bars.
pub(crate) fn container_indent(container: &Container) -> f32 {
    match container {
        Container::Quote(_) => QUOTE_INDENT,
        Container::Item { .. } => ITEM_INDENT,
    }
}

/// The text column's left edge for `nesting`.
pub(crate) fn indent(nesting: &[Container]) -> f32 {
    nesting.iter().map(container_indent).sum()
}

fn estimate(block: &Block, width: f32) -> f32 {
    let line = style::BODY_SIZE * 1.45;
    let per_line = ((width - indent(&block.nesting)) / (style::BODY_SIZE * 0.5)).max(8.0);
    let text = block.content.text();
    let lines: f32 = text
        .split(['\n', HARD_BREAK])
        .map(|part| (part.chars().count() as f32 / per_line).ceil().max(1.0))
        .sum();
    match block.kind {
        Kind::Rule => RULE_HEIGHT,
        Kind::Heading(level) => heading_size(level) * 1.45 * lines,
        Kind::Code(_) | Kind::Html | Kind::Table => lines * line + 2.0 * PANEL_INSET.y,
        Kind::Paragraph => lines * line,
    }
}

pub(crate) fn heading_size(level: u8) -> f32 {
    HEADING_SIZES[usize::from(level.clamp(1, 6)) - 1]
}

/// The markers each block's new items show, numbered as the serializer numbers them.
fn markers(doc: &Document) -> Vec<Vec<(f32, Marker)>> {
    let mut numbers: Vec<u64> = Vec::new();
    let mut out = Vec::with_capacity(doc.blocks.len());
    let mut previous: Option<&Block> = None;
    for block in &doc.blocks {
        let mut here = Vec::new();
        let mut x = 0.0;
        let mut list_depth = 0;
        let mut continues = true;
        numbers.truncate(block.nesting.len());
        for (depth, container) in block.nesting.iter().enumerate() {
            let before = previous.and_then(|previous| previous.nesting.get(depth));
            if let Container::Item { list, task, head } = *container {
                if head {
                    let is_next =
                        continues && before.is_some_and(|before| container.is_next_item_of(before));
                    let number = match list {
                        ListKind::Ordered(start) if !is_next => start,
                        _ => numbers.get(depth).map_or(1, |number| number + 1),
                    };
                    numbers.resize(depth + 1, 0);
                    numbers[depth] = number;
                    let marker = match (task, list) {
                        (Some(checked), _) => Marker::Task { checked },
                        (None, ListKind::Ordered(_)) => Marker::Number(number),
                        (None, ListKind::Bullet) => Marker::Bullet { depth: list_depth },
                    };
                    here.push((x, marker));
                }
                list_depth += 1;
            }
            continues = continues && before.is_some_and(|before| container.continues(before));
            x += container_indent(container);
        }
        out.push(here);
        previous = Some(block);
    }
    out
}

fn lay_out(
    ctx: &Context,
    block: &Block,
    palette: &Palette,
    width: f32,
    people: Option<&PullRequestContext>,
    platform: ProviderKind,
) -> Laid {
    let indent = indent(&block.nesting);
    let is_panel = block.kind.is_source();
    let inset = if is_panel { PANEL_INSET } else { Vec2::ZERO };
    let wrap = (width - indent - 2.0 * inset.x).max(24.0);
    if block.kind == Kind::Rule {
        let galley = ctx.fonts_mut(|fonts| fonts.layout_job(LayoutJob::default()));
        return Laid { galley, chips: Vec::new(), indent, inset, height: RULE_HEIGHT };
    }
    let look = Look::of(block, palette);
    let mut job = LayoutJob {
        first_row_min_height: ctx
            .fonts_mut(|fonts| fonts.row_height(&FontId::proportional(look.size))),
        ..LayoutJob::default()
    };
    job.wrap.max_width = wrap;
    job.wrap.break_anywhere = is_panel;
    let mut chips = Vec::new();
    let mut index = 0;
    for run in block.content.runs() {
        let format = if is_panel { look.code_format() } else { look.format(&run.marks) };
        if let Some(atom) = &run.atom {
            let (label, kind) = chip_label(atom, people, platform);
            let font = FontId::proportional(look.size.min(style::BODY_SIZE));
            let label_width = ctx.fonts_mut(|fonts| {
                fonts.layout_no_wrap(label.clone(), font.clone(), Color32::PLACEHOLDER).size().x
            });
            let space = TextFormat { color: Color32::TRANSPARENT, ..format };
            job.append("\u{A0}", label_width + 2.0 * CHIP_PAD, space);
            chips.push(Chip { index, label, kind, font });
        } else {
            let text = run.text.replace(HARD_BREAK, "\n");
            job.append(&text, 0.0, format);
        }
        index += run.text.chars().count();
    }
    if block.content.is_empty() {
        job.append("", 0.0, look.format(&Marks::default()));
    }
    let galley = ctx.fonts_mut(|fonts| fonts.layout_job(job));
    let height = galley.size().y + 2.0 * inset.y;
    Laid { galley, chips, indent, inset, height }
}
