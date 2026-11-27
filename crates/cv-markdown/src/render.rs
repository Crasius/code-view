//! Lays out the block tree with the app's type scale, spacing tokens and palette, close to how
//! Azure DevOps and GitHub render a pull request description.
//!
//! Every paragraph is one galley. All its sections use `valign: BOTTOM`: Segoe UI and Cascadia
//! Mono have almost the same descent (0.25 vs 0.24 em) but different ascent and line gap, so
//! bottom-aligning the line boxes puts code spans on the prose baseline, where egui's default
//! centered alignment raises them.

pub(super) mod code;
pub(super) mod links;
mod table;
pub(super) mod typeset;

use std::ops::Range;
use std::sync::Arc;

use cv_base::theme::{self, Palette, icons, style as tokens};
use cv_syntax::code_colors;
use cv_widgets::{glyph, surfaces};
use egui::text_selection::LabelSelectionState;
use egui::{
    Align, Color32, CornerRadius, CursorIcon, FontId, Id, Layout, Margin, Rect, Sense, Stroke,
    TextWrapMode, Ui, WidgetInfo, pos2, vec2,
};
use pulldown_cmark::{Alignment, BlockQuoteKind};

use self::code::{CodeStyle, code_job, copy_button};
use self::links::{byte_at, link_menu};
use self::table::{Columns, Grid, cell_align, paint_lines};
use self::typeset::{Fill, Laid, Look, Typesetter, spoken_text};
use super::images::{self, Anchor, ImageBase};
use super::mermaid;
use super::parse::{Block, Inline, Item, Run};
use super::{InApp, LinkResolver, ResolvedLink, link_hint, request_in_app};

/// Heading sizes h1..h6: title, Azure DevOps' h2 (17 px), section, body, body, small.
pub const HEADING_SIZES: [f32; 6] = [
    tokens::TITLE_SIZE,
    17.0,
    tokens::SECTION_SIZE,
    tokens::BODY_SIZE,
    tokens::BODY_SIZE,
    tokens::SMALL_SIZE,
];
/// Code is set one point smaller than the text around it, like the app's other code.
pub const CODE_SIZE_DELTA: f32 = 1.0;
pub const QUOTE_BAR_WIDTH: f32 = 3.0;
pub const BULLETS: [&str; 3] = ["•", "◦", "▪"];
pub const CHECKBOX_SIZE: f32 = 14.0;
/// Ids below one top-level block: `next_id` counts within the block in the low bits.
const BLOCK_ID_BITS: u32 = 20;

/// The height of a thematic break at `scale`: room above and below its hairline.
pub(super) fn rule_height(scale: f32) -> f32 {
    tokens::SPACE_S * scale * 2.0 + 1.0
}

/// Laid-out heights of a document's top-level blocks for one width, scale and pixel density.
#[derive(Clone, Default)]
struct BlockHeights {
    key: u64,
    heights: Vec<Option<f32>>,
}

/// Words of one rendered block filled with `color` behind the text: byte ranges of its
/// [`Block::plain_text`]. A block takes several layers (its changed words, the find bar's hits
/// over them); where they overlap, a later layer is blended over an earlier one.
#[derive(Debug, Clone, PartialEq)]
pub struct Marks {
    pub ranges: Vec<Range<usize>>,
    pub color: Color32,
}

pub(crate) struct Renderer {
    palette: Palette,
    id: Id,
    next: usize,
    /// Multiplies every size: type, spacing, markers and frames.
    scale: f32,
    /// What the screen knows about link targets; `None` opens URLs in the browser.
    links: Option<Arc<dyn LinkResolver>>,
    /// Words to fill, layer by layer, and how far into the block's plain text the layout got.
    marks: Vec<Marks>,
    cursor: usize,
    /// Where relative image paths point (a repository file); `None` for comments.
    images: Option<ImageBase>,
    /// A change color drawn around every image (the rendered diff's added and removed blocks).
    image_border: Option<Color32>,
    /// Who keeps the reader's place when an image above the view changes height.
    anchor: Anchor,
    /// How wide a top-level table may grow, centered on the text.
    breakout: f32,
    /// The next block is a top-level one (its tables may grow to `breakout`).
    is_top: bool,
}

impl Renderer {
    pub(crate) fn new(
        palette: Palette,
        id: Id,
        scale: f32,
        links: Option<Arc<dyn LinkResolver>>,
    ) -> Self {
        Self {
            palette,
            id,
            next: 0,
            scale,
            links,
            marks: Vec::new(),
            cursor: 0,
            images: None,
            image_border: None,
            anchor: Anchor::LeftToView,
            breakout: 0.0,
            is_top: false,
        }
    }

    /// Top-level tables may grow to `breakout` wide, centered on the text, rather than scroll.
    pub(crate) fn with_breakout(mut self, breakout: f32) -> Self {
        self.breakout = breakout;
        self
    }

    /// Relative image paths resolve against `base`; images get a `border` in that color; `anchor`
    /// says who keeps the reader's place when an image above the view changes height.
    pub(crate) fn with_images(
        mut self,
        base: Option<ImageBase>,
        border: Option<Color32>,
        anchor: Anchor,
    ) -> Self {
        self.images = base;
        self.image_border = border;
        self.anchor = anchor;
        self
    }

    /// Fills the words of the layers `marks` of the next block shown with [`Self::show_unit`].
    pub(crate) fn with_marks(mut self, marks: impl IntoIterator<Item = Marks>) -> Self {
        self.marks = marks.into_iter().collect();
        self
    }

    /// One piece of a rendered diff: a top-level block, or with `item` one item of a top-level
    /// list (numbered as in the whole list).
    pub(crate) fn show_unit(&mut self, ui: &mut Ui, block: &Block, item: Option<usize>) {
        let look =
            Look { size: self.px(tokens::BODY_SIZE), strong: false, color: self.palette.text };
        self.cursor = 0;
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            match (block, item) {
                (Block::List { start, items }, Some(index)) if index < items.len() => {
                    let numbering = Numbering { first: index, total: items.len() };
                    self.list(ui, *start, &items[index..=index], numbering, look, 0);
                }
                _ => {
                    self.is_top = true;
                    self.block(ui, block, look);
                }
            }
        });
    }

    /// The marks of the next `len` bytes of the block's plain text, from their start; moves past them.
    fn take_marks(&mut self, len: usize) -> Vec<Fill> {
        let start = self.cursor;
        self.cursor += len;
        typeset::layer(&self.marks, start..start + len)
    }

    /// The `\n` between two sibling blocks, list items or table cells in the plain text.
    fn separator(&mut self) {
        self.cursor += 1;
    }

    /// What setting this block's text needs.
    fn typesetter(&self) -> Typesetter<'_> {
        Typesetter {
            palette: &self.palette,
            scale: self.scale,
            links: self.links.as_deref(),
            images: self.images,
        }
    }

    /// A design-token length at this renderer's scale.
    fn px(&self, length: f32) -> f32 {
        length * self.scale
    }

    /// A margin token at this renderer's scale, as egui's integer margin.
    fn margin(&self, length: f32) -> i8 {
        self.px(length).round().clamp(0.0, f32::from(i8::MAX)) as i8
    }

    /// Lays out the document `blocks`, identified by `document` (a hash of its source).
    pub(crate) fn show(&mut self, ui: &mut Ui, blocks: &[Block], document: u64) {
        let look =
            Look { size: self.px(tokens::BODY_SIZE), strong: false, color: self.palette.text };
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            self.top_level(ui, blocks, look, document);
        });
    }

    /// The top-level blocks, skipping those outside the clip rect: a skipped block only
    /// reserves the height it had when it was last laid out at this width and scale, so a
    /// 2,000-line description costs what its visible part costs.
    fn top_level(&mut self, ui: &mut Ui, blocks: &[Block], look: Look, document: u64) {
        let cache_id = self.id.with("block_heights");
        let key = egui::epaint::util::hash((
            document,
            ui.available_width().to_bits(),
            self.scale.to_bits(),
            ui.ctx().pixels_per_point().to_bits(),
        ));
        let mut heights = ui
            .data(|data| data.get_temp::<BlockHeights>(cache_id))
            .filter(|cached| cached.key == key)
            .map(|cached| cached.heights)
            .unwrap_or_default();
        heights.resize(blocks.len(), None);
        let mut is_changed = false;
        for (index, block) in blocks.iter().enumerate() {
            if let Some(previous) = index.checked_sub(1).map(|i| &blocks[i]) {
                ui.add_space(self.px(gap(previous, block)));
            }
            // Ids depend on the block's index, not on how many blocks before it were skipped.
            self.next = index << BLOCK_ID_BITS;
            let top = ui.cursor().top();
            if let Some(height) = heights[index] {
                let rect = egui::Rect::from_min_size(
                    pos2(ui.cursor().left(), top),
                    vec2(ui.available_width(), height),
                );
                // Blocks just outside the view are laid out too, so their images start loading
                // before they are scrolled to.
                let ahead = ui
                    .clip_rect()
                    .expand2(vec2(0.0, ui.clip_rect().height() * images::PRELOAD_SCREENS));
                if !ui.is_visible() || !ahead.intersects(rect) {
                    ui.add_space(height);
                    continue;
                }
            }
            self.is_top = true;
            self.block(ui, block, look);
            let height = ui.cursor().top() - top;
            if heights[index] != Some(height) {
                heights[index] = Some(height);
                is_changed = true;
            }
        }
        if is_changed {
            ui.data_mut(|data| data.insert_temp(cache_id, BlockHeights { key, heights }));
        }
    }

    fn next_id(&mut self) -> Id {
        self.next += 1;
        self.id.with(self.next)
    }

    fn blocks(&mut self, ui: &mut Ui, blocks: &[Block], look: Look) {
        for (index, block) in blocks.iter().enumerate() {
            if let Some(previous) = index.checked_sub(1).map(|i| &blocks[i]) {
                ui.add_space(self.px(gap(previous, block)));
                self.separator();
            }
            self.block(ui, block, look);
        }
    }

    fn block(&mut self, ui: &mut Ui, block: &Block, look: Look) {
        let is_top = std::mem::take(&mut self.is_top);
        match block {
            Block::Paragraph(inline) => {
                self.paragraph(ui, inline, look, egui::Role::Label, TextWrapMode::Wrap);
            }
            Block::Heading(level, inline) => {
                let index = usize::from(level.saturating_sub(1)).min(HEADING_SIZES.len() - 1);
                let color = if index == 5 { self.palette.text_secondary } else { look.color };
                let heading = Look { size: self.px(HEADING_SIZES[index]), strong: true, color };
                self.paragraph(ui, inline, heading, egui::Role::Heading, TextWrapMode::Wrap);
            }
            Block::List { start, items } => {
                self.list(ui, *start, items, Numbering::whole(items), look, 0);
            }
            Block::Quote { alert, blocks } => self.quote(ui, *alert, blocks, look),
            Block::Code { language, code } if mermaid::is_mermaid(language) => {
                self.mermaid(ui, language, code);
            }
            Block::Code { language, code } => self.code_block(ui, language, code),
            Block::Table { alignments, header, rows } => {
                self.table(ui, alignments, header, rows, look, is_top);
            }
            Block::Rule => {
                let width = ui.available_width();
                let (rect, _) =
                    ui.allocate_exact_size(vec2(width, rule_height(self.scale)), Sense::hover());
                ui.painter().hline(
                    rect.x_range(),
                    rect.center().y,
                    Stroke::new(1.0, self.palette.border),
                );
            }
        }
    }

    /// One galley per block, or text and images flowing in rows when it has images.
    fn paragraph(
        &mut self,
        ui: &mut Ui,
        inline: &Inline,
        look: Look,
        role: egui::Role,
        wrap: TextWrapMode,
    ) {
        if inline.has_images() {
            self.flow(ui, inline, look, role, wrap);
        } else {
            self.text(ui, &inline.runs, look, role, wrap);
        }
    }

    /// Text and images in reading order: each stretch of text between images is a galley that
    /// continues the row the image before it is on, and an image too wide for the rest of a row
    /// starts the next one.
    fn flow(
        &mut self,
        ui: &mut Ui,
        inline: &Inline,
        look: Look,
        role: egui::Role,
        wrap: TextWrapMode,
    ) {
        let width = ui.available_width();
        let max_width = width.max(1.0);
        let layout = Layout::left_to_right(Align::Min).with_main_wrap(wrap == TextWrapMode::Wrap);
        ui.allocate_ui_with_layout(vec2(width, 0.0), layout, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, self.px(tokens::SPACE_XS));
            let runs = &inline.runs;
            let mut start = 0;
            for (index, run) in runs.iter().enumerate() {
                let Some(spec) = &run.style.image else {
                    continue;
                };
                if start < index {
                    self.text(ui, &runs[start..index], look, role, wrap);
                }
                start = index + 1;
                // The alt text stands for the image in the block's plain text.
                self.take_marks(run.text.len());
                let id = self.next_id();
                let link =
                    self.typesetter().resolve_links(std::slice::from_ref(run)).pop().flatten();
                let shown = images::Shown {
                    id,
                    spec,
                    link: run.style.link.as_deref().zip(link.as_ref()),
                    base: self.images,
                    border: self.image_border,
                    anchor: self.anchor,
                    max_width,
                    scale: self.scale,
                    palette: &self.palette,
                };
                images::show(ui, &shown);
            }
            if start < runs.len() {
                self.text(ui, &runs[start..], look, role, wrap);
            }
        });
    }

    /// One galley of `runs`; links are hit-tested on its glyphs.
    fn text(
        &mut self,
        ui: &mut Ui,
        runs: &[Run],
        look: Look,
        role: egui::Role,
        wrap: TextWrapMode,
    ) {
        let hover_id = self.next_id();
        let hovered: Option<usize> = ui.ctx().data(|data| data.get_temp(hover_id));
        let marks = self.take_marks(runs.iter().map(|run| run.text.len()).sum());
        let typesetter = self.typesetter();
        let links = typesetter.resolve_links(runs);
        let Laid { job, run_ends } =
            typesetter.layout_job(ui.ctx(), runs, look, hovered, &links, &marks);
        let (pos, galley, response) =
            egui::Label::new(job).sense(Sense::click()).wrap_mode(wrap).layout_in_ui(ui);
        // Named by the runs' words: the galley also holds the links' icon glyphs.
        response.widget_info(|| WidgetInfo::labeled(role, ui.is_enabled(), spoken_text(runs)));

        let link_under_pointer = response
            .hover_pos()
            .and_then(|pointer| byte_at(&galley, pointer - pos))
            .map(|byte| run_ends.partition_point(|&end| end <= byte))
            .filter(|&run| links.get(run).is_some_and(Option::is_some));
        if link_under_pointer != hovered {
            ui.ctx().data_mut(|data| data.insert_temp(hover_id, link_under_pointer));
            ui.ctx().request_repaint();
        }
        if ui.is_rect_visible(response.rect) {
            LabelSelectionState::label_text_selection(
                ui,
                &response,
                pos,
                galley,
                look.color,
                Stroke::NONE,
            );
        }
        let pointed = link_under_pointer
            .and_then(|run| Some((runs[run].style.link.clone()?, links[run].clone()?)));
        let menu_id = hover_id.with("menu");
        if response.secondary_clicked() {
            ui.ctx().data_mut(|data| match pointed.clone() {
                Some(menu) => {
                    data.insert_temp(menu_id, menu);
                }
                None => {
                    data.remove::<(String, ResolvedLink)>(menu_id);
                }
            });
        }
        if let Some((target, link)) =
            ui.ctx().data(|data| data.get_temp::<(String, ResolvedLink)>(menu_id))
        {
            response.context_menu(|ui| link_menu(ui, &target, &link));
            if !response.context_menu_opened() {
                ui.ctx().data_mut(|data| data.remove::<(String, ResolvedLink)>(menu_id));
            }
        }
        if let Some((target, link)) = pointed {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            if response.clicked() {
                let wants_browser = ui.input(|input| input.modifiers.command);
                if link.in_app != InApp::No && !wants_browser {
                    request_in_app(ui.ctx(), &target);
                } else {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(&link.web_url));
                }
            }
            response.on_hover_text_at_pointer(link_hint(&link));
        }
    }

    /// `items` are `numbering.first..` of a list of `numbering.total` items: the markers are
    /// numbered and sized as in the whole list.
    fn list(
        &mut self,
        ui: &mut Ui,
        start: Option<u64>,
        items: &[Item],
        numbering: Numbering,
        look: Look,
        depth: usize,
    ) {
        let marker_font = FontId::proportional(look.size);
        let row_height = ui.fonts_mut(|fonts| fonts.row_height(&marker_font));
        let last_number = start.map(|first| first + numbering.total.saturating_sub(1) as u64);
        let start = start.map(|first| first + numbering.first as u64);
        let number_width = last_number.map_or(0.0, |number| {
            ui.fonts_mut(|fonts| {
                fonts.layout_no_wrap(format!("{number}."), marker_font, look.color).size().x
            })
        });
        let marker_width = (number_width + self.px(tokens::SPACE_S))
            .max(self.px(tokens::SPACE_L + tokens::SPACE_M));

        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                ui.add_space(self.px(tokens::SPACE_XS));
                self.separator();
            }
            let width = ui.available_width();
            ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::left_to_right(Align::Min), |ui| {
                let (marker, _) =
                    ui.allocate_exact_size(vec2(marker_width, row_height), Sense::hover());
                let number = start.map(|first| first + index as u64);
                self.list_marker(ui, marker, item.task, number, look, depth);
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    self.item_blocks(ui, item, look, depth);
                });
            });
        }
    }

    /// The marker in `marker`: a task's checkbox, the item's `number`, or the bullet of `depth`.
    fn list_marker(
        &self,
        ui: &Ui,
        marker: Rect,
        task: Option<bool>,
        number: Option<u64>,
        look: Look,
        depth: usize,
    ) {
        if let Some(checked) = task {
            self.checkbox(ui, marker, checked);
            return;
        }
        let (text, right_inset) = match number {
            Some(number) => (format!("{number}."), tokens::SPACE_S),
            None => (BULLETS[depth.min(BULLETS.len() - 1)].to_owned(), tokens::SPACE_M),
        };
        let painter = ui.painter();
        let galley = painter.layout_no_wrap(text, FontId::proportional(look.size), look.color);
        let x = marker.right() - self.px(right_inset) - galley.size().x;
        painter.galley(pos2(x, marker.min.y), galley, look.color);
    }

    /// An item's blocks; a list inside it is nested one level deeper.
    fn item_blocks(&mut self, ui: &mut Ui, item: &Item, look: Look, depth: usize) {
        for (index, block) in item.blocks.iter().enumerate() {
            if index > 0 {
                ui.add_space(self.px(tokens::SPACE_XS));
                self.separator();
            }
            match block {
                Block::List { start, items } => {
                    self.list(ui, *start, items, Numbering::whole(items), look, depth + 1);
                }
                other => self.block(ui, other, look),
            }
        }
    }

    fn checkbox(&self, ui: &Ui, marker: egui::Rect, checked: bool) {
        let size = self.px(CHECKBOX_SIZE);
        let center =
            pos2(marker.right() - self.px(tokens::SPACE_S) - size / 2.0, marker.center().y);
        let rect = egui::Rect::from_center_size(center, vec2(size, size));
        let painter = ui.painter();
        let radius = CornerRadius::same(3);
        if checked {
            painter.rect_filled(rect, radius, self.palette.brand);
            let stroke = Stroke::new(self.px(1.6), self.palette.on_brand);
            let points = [
                pos2(rect.left() + self.px(3.5), rect.center().y),
                pos2(rect.left() + self.px(6.0), rect.bottom() - self.px(4.0)),
                pos2(rect.right() - self.px(3.5), rect.top() + self.px(4.0)),
            ];
            painter.line(points.to_vec(), stroke);
        } else {
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, self.palette.border_strong),
                egui::StrokeKind::Inside,
            );
        }
    }

    fn quote(&mut self, ui: &mut Ui, alert: Option<BlockQuoteKind>, blocks: &[Block], look: Look) {
        let (bar_color, title) = match alert {
            None => (self.palette.border_strong, None),
            Some(kind) => {
                let (color, icon, label) = match kind {
                    BlockQuoteKind::Note => (self.palette.accent, icons::INFO, "Note"),
                    BlockQuoteKind::Tip => (self.palette.success, icons::INFO, "Tip"),
                    BlockQuoteKind::Important => (self.palette.accent, icons::INFO, "Important"),
                    BlockQuoteKind::Warning => (self.palette.warning, icons::WARNING, "Warning"),
                    BlockQuoteKind::Caution => (self.palette.error, icons::WARNING, "Caution"),
                };
                (color, Some((color, icon, label)))
            }
        };
        let quoted = Look {
            color: if alert.is_some() { look.color } else { self.palette.text_secondary },
            ..look
        };
        let response = egui::Frame::new()
            .inner_margin(Margin {
                left: self.margin(QUOTE_BAR_WIDTH + tokens::SPACE_M),
                ..Margin::ZERO
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some((color, icon, label)) = title {
                    let title = egui::RichText::new(format!("{icon}  {label}"))
                        .font(theme::fonts::semibold_font(look.size))
                        .color(color);
                    glyph::speak_as(&ui.label(title), label);
                    ui.add_space(self.px(tokens::SPACE_XS));
                }
                self.blocks(ui, blocks, quoted);
            })
            .response;
        let bar = egui::Rect::from_min_size(
            response.rect.min,
            vec2(self.px(QUOTE_BAR_WIDTH), response.rect.height()),
        );
        ui.painter().rect_filled(bar, CornerRadius::same(1), bar_color);
    }

    /// A fenced block in its language's syntax colors (plain until they are worked out, in the
    /// same font, so nothing moves when they arrive) with a copy button in its corner.
    fn code_block(&mut self, ui: &mut Ui, language: &str, code: &str) {
        let id = self.next_id();
        let marks = self.take_marks(code.len());
        self.code_frame(ui, id, language, code, &marks);
    }

    /// A ```` ```mermaid ```` fence: its diagram, or its source (asked for, or when it cannot be
    /// drawn) as a code block with its changed words marked.
    fn mermaid(&mut self, ui: &mut Ui, language: &str, code: &str) {
        let id = self.next_id();
        let marks = self.take_marks(code.len());
        let diagram = mermaid::Diagram {
            id,
            code,
            palette: &self.palette,
            scale: self.scale,
            border: self.image_border,
            anchor: self.anchor,
        };
        mermaid::show(ui, &diagram, |ui| {
            self.code_frame(ui, id.with("source"), language, code, &marks)
        });
    }

    /// The frame, text and copy button of a code block; its rect.
    fn code_frame(&self, ui: &mut Ui, id: Id, language: &str, code: &str, marks: &[Fill]) -> Rect {
        let colors = code_colors::colors(ui.ctx(), language, code);
        let font = theme::fonts::code_font(self.px(tokens::BODY_SIZE - CODE_SIZE_DELTA));
        let job =
            code_job(code, colors.as_deref(), marks, &CodeStyle { font, palette: &self.palette });
        let frame = egui::Frame::new()
            .fill(self.palette.surface_subtle)
            .stroke(Stroke::new(1.0, self.palette.border))
            .corner_radius(tokens::RADIUS_CONTROL)
            .inner_margin(Margin::symmetric(
                self.margin(tokens::SPACE_M),
                self.margin(tokens::SPACE_S + 2.0),
            ));
        let response = frame
            .show(ui, |ui| {
                let width = ui.available_width();
                ui.set_width(width);
                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                if galley.size().x > width + 0.5 {
                    overflowing(ui, id, |ui| {
                        ui.add(egui::Label::new(galley).extend());
                    });
                } else {
                    ui.add(egui::Label::new(galley).extend());
                }
            })
            .response;
        copy_button(ui, id.with("copy"), response.rect, code, &self.palette);
        response.rect
    }

    /// Hairline cell borders and a tinted header row, like the web renderers. The columns share
    /// the width there is (see [`table::plan`]) and their text wraps; only a table that cannot
    /// fit with every column at its least width scrolls sideways, inside its own box. A top-level
    /// table too wide for the text may first grow to the renderer's breakout width, centered on
    /// the text.
    fn table(
        &mut self,
        ui: &mut Ui,
        alignments: &[Alignment],
        header: &[Inline],
        rows: &[Vec<Inline>],
        look: Look,
        is_top: bool,
    ) {
        let id = self.next_id();
        let head = Look { strong: true, ..look };
        let table: Vec<(&[Inline], Look)> = std::iter::once((header, head))
            .chain(rows.iter().map(|row| (row.as_slice(), look)))
            .collect();
        let column_count = table.iter().map(|(cells, _)| cells.len()).max().unwrap_or(0);
        let available = ui.available_width();
        let mut natural = vec![0.0_f32; column_count];
        let mut words = vec![0.0_f32; column_count];
        let typesetter = self.typesetter();
        for (cells, cell_look) in &table {
            for (column, cell) in cells.iter().enumerate() {
                let width = typesetter.measure_cell(ui.ctx(), cell, *cell_look, available).x.ceil();
                natural[column] = natural[column].max(width);
                let word = typesetter.longest_word(ui.ctx(), cell, *cell_look, available).ceil();
                words[column] = words[column].max(word);
            }
        }
        let padding = vec2(self.px(tokens::SPACE_M), self.px(tokens::SPACE_S));
        let columns = Columns { natural, words, frames: column_count as f32 * 2.0 * padding.x };
        let breakout = is_top.then_some(self.breakout);
        let table::TablePlan { width, widths, overflows } =
            table::plan(&columns, available, breakout, self.scale);
        let min_height = ui.fonts_mut(|fonts| fonts.row_height(&FontId::proportional(look.size)));
        let grid = Grid { alignments, table: &table, widths: &widths, padding, min_height };
        if width <= available {
            if overflows {
                overflowing(ui, id, |ui| self.grid(ui, &grid));
            } else {
                self.grid(ui, &grid);
            }
            return;
        }
        // Centered on the text; the text's own width is what it takes in the flow.
        let origin = ui.cursor().min - vec2((width - available) / 2.0, 0.0);
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(origin, vec2(width, f32::INFINITY)))
                .layout(Layout::top_down(Align::Min)),
        );
        child.spacing_mut().item_spacing = vec2(0.0, 0.0);
        if overflows {
            overflowing(&mut child, id, |ui| self.grid(ui, &grid));
        } else {
            self.grid(&mut child, &grid);
        }
        let height = child.min_rect().height();
        ui.allocate_exact_size(vec2(available, height), Sense::hover());
    }

    /// Lays out the cells of `grid` row by row, each row as tall as its tallest cell, then the
    /// borders and the header's tint behind them.
    fn grid(&mut self, ui: &mut Ui, grid: &Grid<'_>) {
        let Grid { alignments, table, widths, padding, min_height } = *grid;
        let origin = ui.cursor().min;
        let width: f32 = widths.iter().map(|width| width + 2.0 * padding.x).sum();
        let painter = ui.painter().clone();
        let header_fill = painter.add(egui::Shape::Noop);
        let mut row_tops = Vec::with_capacity(table.len() + 1);
        let mut y = origin.y;
        let mut is_first_cell = true;
        for (cells, cell_look) in table {
            row_tops.push(y);
            let mut height = min_height;
            let mut x = origin.x;
            for (column, column_width) in widths.iter().enumerate() {
                if let Some(cell) = cells.get(column) {
                    if !std::mem::take(&mut is_first_cell) {
                        self.separator();
                    }
                    let align = cell_align(alignments, column);
                    let cell_rect = Rect::from_min_size(
                        pos2(x, y) + padding,
                        vec2(*column_width, f32::INFINITY),
                    );
                    let mut child = ui.new_child(
                        egui::UiBuilder::new().max_rect(cell_rect).layout(Layout::top_down(align)),
                    );
                    child.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    self.paragraph(
                        &mut child,
                        cell,
                        *cell_look,
                        egui::Role::Label,
                        TextWrapMode::Wrap,
                    );
                    height = height.max(child.min_rect().height());
                }
                x += column_width + 2.0 * padding.x;
            }
            y += height + 2.0 * padding.y;
        }
        row_tops.push(y);
        let rect = Rect::from_min_max(origin, pos2(origin.x + width, y));
        ui.allocate_rect(rect, Sense::hover());

        paint_lines(&painter, grid, rect, &row_tops, header_fill, &self.palette);
    }
}

/// A block wider than its column (a table whose columns cannot shrink further, code lines that do
/// not wrap) scrolls sideways inside its own box. The bar sits below the content and shows at
/// rest, so a cut edge always reads as one that scrolls.
fn overflowing<R>(ui: &mut Ui, id: Id, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = theme::palette(ui.ctx());
    ui.scope(|ui| {
        let scroll = &mut ui.spacing_mut().scroll;
        scroll.floating = false;
        scroll.foreground_color = false;
        // A solid bar takes the widgets' fills: a quiet handle that darkens under the pointer.
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.bg_fill = palette.border_strong;
        widgets.hovered.bg_fill = palette.text_secondary;
        widgets.active.bg_fill = palette.text_secondary;
        let output = egui::ScrollArea::horizontal()
            .id_salt(id)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show(ui, add_contents);
        // The output's inner rect does not follow the content's height on a sideways-only area.
        let shown = Rect::from_min_size(
            output.inner_rect.min,
            vec2(output.inner_rect.width(), output.content_size.y),
        );
        surfaces::mark_horizontal_scroll(ui, shown);
        output.inner
    })
    .inner
}

/// Which items of a list a [`Renderer::list`] call shows: from index `first` of `total` items.
#[derive(Clone, Copy)]
struct Numbering {
    first: usize,
    total: usize,
}

impl Numbering {
    fn whole(items: &[Item]) -> Self {
        Self { first: 0, total: items.len() }
    }
}

/// Vertical space between two sibling blocks, as Azure DevOps spaces them (headings 24 px
/// above and 8 px below, paragraphs and lists 16 px apart).
pub(super) fn gap(previous: &Block, next: &Block) -> f32 {
    match (previous, next) {
        (_, Block::Heading(..)) => tokens::SPACE_L + tokens::SPACE_S,
        (Block::Heading(..), _) => tokens::SPACE_S,
        _ => tokens::SPACE_L,
    }
}
