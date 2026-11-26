//! Painting and pointer input. Only the blocks inside the clip rect are laid out and painted:
//! the first is found by binary search over the block offsets. Decorations match the rendered
//! comment (`cv_markdown`): quote bars, bullets, numbers and checkboxes in the marker
//! column, code on a tinted panel, a hairline rule, mentions as chips.

use cv_base::theme::{self, Palette, style};
use cv_markdown::{BULLETS, CHECKBOX_SIZE, QUOTE_BAR_WIDTH};
use egui::text::CCursor;
use egui::{
    Align2, Color32, CornerRadius, EventFilter, FontId, Id, Pos2, Rect, Response, Sense, Stroke,
    Ui, pos2, vec2,
};

use super::caret::{self, Placed};
use super::input::{self, Command, Frame, link_at};
use super::layout::{ChipKind, ITEM_INDENT, Marker, QUOTE_INDENT, indent};
use super::model::{Container, Kind, Pos, Selection};
use super::{MARGIN, Popup, RichOptions, State};
use crate::markdown_editor::PullRequestContext;

/// Width of the caret.
const CARET_WIDTH: f32 = 2.0;
/// How wide an empty selected line is drawn.
const EMPTY_LINE_SELECTION: f32 = 6.0;

/// Clicks, drags, double and triple clicks; a click on a task box toggles it.
pub fn pointer(ui: &Ui, response: &Response, state: &mut State, origin: Pos2) -> Vec<Command> {
    let Some(point) = response.interact_pointer_pos() else { return Vec::new() };
    let ctx = ui.ctx();
    let pressed =
        response.is_pointer_button_down_on() && ui.input(|input| input.pointer.primary_pressed());
    let pos = caret::pos_at(&mut state.layout, ctx, &state.doc, origin, point);
    if pressed {
        ui.memory_mut(|memory| memory.request_focus(response.id));
        if let Some(index) = task_at(ui, state, origin, point) {
            return vec![Command::ToggleTask(index)];
        }
        let modifiers = ui.input(|input| input.modifiers);
        if modifiers.command
            && let Some((_, link)) = link_at(&state.doc, pos)
        {
            ctx.open_url(egui::OpenUrl::new_tab(link.url));
        }
        state.selection = if modifiers.shift {
            Selection { anchor: state.selection.anchor, head: pos }
        } else {
            Selection::caret(pos)
        };
        state.history.break_group();
        state.stored = None;
        state.column = None;
        if !matches!(state.popup, Popup::None) {
            state.popup = Popup::None;
        }
    } else if response.dragged() && ui.input(|input| input.pointer.primary_down()) {
        state.selection.head = pos;
        ui.scroll_to_rect(Rect::from_center_size(point, vec2(1.0, 1.0)), None);
    }
    if response.triple_clicked() {
        let length = state.doc.block(pos.block).len();
        state.selection =
            Selection { anchor: Pos::new(pos.block, 0), head: Pos::new(pos.block, length) };
    } else if response.double_clicked() {
        state.selection = caret::word_at(&state.doc, pos);
    }
    Vec::new()
}

/// The marker column's rectangle of a block's item at column `x`: one text line high.
fn marker_rect(placed: &Placed, origin: Pos2, x: f32) -> Rect {
    let line = placed.laid.galley.pos_from_cursor(CCursor::new(0));
    let top = origin.y + placed.top + placed.laid.inset.y + line.min.y;
    Rect::from_min_size(
        pos2(origin.x + x, top),
        vec2(ITEM_INDENT, line.height().max(CHECKBOX_SIZE)),
    )
}

/// The block whose task box is under `point`.
fn task_at(ui: &Ui, state: &mut State, origin: Pos2, point: Pos2) -> Option<usize> {
    let index = caret::block_at_y(&mut state.layout, &state.doc, point.y - origin.y);
    let markers = state.layout.offsets(&state.doc).markers.get(index).cloned().unwrap_or_default();
    let placed = caret::place(&mut state.layout, ui.ctx(), &state.doc, origin, index);
    markers.iter().find_map(|(x, marker)| {
        let is_task = matches!(marker, Marker::Task { .. });
        (is_task && marker_rect(&placed, origin, *x).contains(point)).then_some(index)
    })
}

/// The range of blocks inside the clip rect.
fn visible(state: &mut State, origin: Pos2, clip: Rect) -> std::ops::Range<usize> {
    let offsets = state.layout.offsets(&state.doc);
    let top = clip.top() - origin.y;
    let bottom = clip.bottom() - origin.y;
    // The block starting above the top may reach into view.
    let first = offsets.tops.partition_point(|block_top| *block_top < top).saturating_sub(1);
    let end = offsets.tops.partition_point(|block_top| *block_top <= bottom);
    first.min(end)..end.max(first)
}

/// Paints the visible blocks, the selection, the caret and the placeholder.
pub fn paint(ui: &Ui, state: &mut State, origin: Pos2, has_focus: bool, hint: &str) {
    let palette = theme::palette(ui.ctx());
    let clip = ui.clip_rect();
    let mut range = visible(state, origin, clip);
    // Laying out a block can correct its estimated height; the range is then found again.
    for index in range.clone() {
        state.layout.get(ui.ctx(), &state.doc, index);
    }
    let corrected = visible(state, origin, clip);
    if corrected != range {
        range = corrected;
        ui.ctx().request_repaint();
    }
    let width = ui.max_rect().width() - 2.0 * (origin.x - ui.max_rect().left());
    for index in range {
        let placed = caret::place(&mut state.layout, ui.ctx(), &state.doc, origin, index);
        decorations(ui, state, &palette, origin, width, index, &placed);
        selection(ui, state, &palette, index, &placed);
        ui.painter().galley(placed.galley_origin, placed.laid.galley.clone(), palette.text);
        chips(ui, &palette, &placed);
    }
    let is_empty = state.doc.blocks.len() == 1
        && state.doc.block(0).content.is_empty()
        && state.doc.block(0).kind == Kind::Paragraph;
    if is_empty && state.preedit.is_none() {
        let font = FontId::proportional(style::BODY_SIZE);
        ui.painter().text(origin, Align2::LEFT_TOP, hint, font, palette.text_secondary);
    }
    if has_focus && state.preedit.is_none() {
        let rect = caret::caret_rect(
            &mut state.layout,
            ui.ctx(),
            &state.doc,
            origin,
            state.selection.head,
        );
        let caret = Rect::from_min_size(rect.min, vec2(CARET_WIDTH, rect.height()));
        ui.painter().rect_filled(caret, 1.0, palette.accent);
    }
}

/// Quote bars, list markers, code panels and rules of block `index`.
fn decorations(
    ui: &Ui,
    state: &mut State,
    palette: &Palette,
    origin: Pos2,
    width: f32,
    index: usize,
    placed: &Placed,
) {
    let painter = ui.painter();
    let block = state.doc.block(index).clone();
    let offsets = state.layout.offsets(&state.doc);
    let top = origin.y + placed.top;
    let bottom = top + placed.laid.height;
    let previous_bottom = index
        .checked_sub(1)
        .map(|before| origin.y + offsets.tops[before] + offsets.heights[before]);
    let previous = index.checked_sub(1).map(|before| state.doc.block(before).nesting.clone());
    let markers = offsets.markers.get(index).cloned().unwrap_or_default();
    let mut x = origin.x;
    let mut continues = true;
    for (depth, container) in block.nesting.iter().enumerate() {
        let before = previous.as_ref().and_then(|nesting| nesting.get(depth));
        continues = continues && before.is_some_and(|before| container.continues(before));
        if let Container::Quote(alert) = container {
            let start = if continues { previous_bottom.unwrap_or(top) } else { top };
            let color = if alert.is_some() { palette.accent } else { palette.border_strong };
            let bar = Rect::from_min_max(pos2(x, start), pos2(x + QUOTE_BAR_WIDTH, bottom));
            painter.rect_filled(bar, CornerRadius::same(1), color);
        }
        x += match container {
            Container::Quote(_) => QUOTE_INDENT,
            Container::Item { .. } => ITEM_INDENT,
        };
    }
    for (column, marker) in markers {
        list_marker(ui, palette, marker_rect(placed, origin, column), marker, placed);
    }
    let left = origin.x + indent(&block.nesting);
    let right = origin.x + width;
    match &block.kind {
        Kind::Code(_) | Kind::Html | Kind::Table => {
            let panel = Rect::from_min_max(pos2(left, top), pos2(right, bottom));
            painter.rect(
                panel,
                style::RADIUS_CONTROL,
                palette.surface_subtle,
                Stroke::new(1.0, palette.border),
                egui::StrokeKind::Inside,
            );
            let label = match &block.kind {
                Kind::Code(language) => language.as_str(),
                Kind::Html => "HTML",
                _ => "Table",
            };
            if !label.is_empty() {
                let font = FontId::proportional(style::SMALL_SIZE);
                let at = pos2(panel.right() - style::SPACE_S, panel.top() + style::SPACE_XS);
                painter.text(at, Align2::RIGHT_TOP, label, font, palette.text_secondary);
            }
        }
        Kind::Rule => {
            let y = f32::midpoint(top, bottom);
            painter.hline(left..=right, y, Stroke::new(1.0, palette.border));
        }
        Kind::Paragraph | Kind::Heading(_) => {}
    }
}

fn list_marker(ui: &Ui, palette: &Palette, rect: Rect, marker: Marker, placed: &Placed) {
    let painter = ui.painter();
    let size = placed.laid.galley.pos_from_cursor(CCursor::new(0)).height().max(style::BODY_SIZE);
    let font = FontId::proportional((size / 1.45).clamp(style::SMALL_SIZE, style::BODY_SIZE));
    match marker {
        Marker::Bullet { depth } => {
            let bullet = BULLETS[depth.min(BULLETS.len() - 1)];
            let at = pos2(rect.right() - style::SPACE_M, rect.center().y);
            painter.text(at, Align2::RIGHT_CENTER, bullet, font, palette.text);
        }
        Marker::Number(number) => {
            let at = pos2(rect.right() - style::SPACE_S, rect.center().y);
            painter.text(at, Align2::RIGHT_CENTER, format!("{number}."), font, palette.text);
        }
        Marker::Task { checked } => {
            let center = pos2(rect.right() - style::SPACE_S - CHECKBOX_SIZE / 2.0, rect.center().y);
            let rect = Rect::from_center_size(center, vec2(CHECKBOX_SIZE, CHECKBOX_SIZE));
            let radius = CornerRadius::same(3);
            if checked {
                painter.rect_filled(rect, radius, palette.brand);
                let points = vec![
                    pos2(rect.left() + 3.5, rect.center().y),
                    pos2(rect.left() + 6.0, rect.bottom() - 4.0),
                    pos2(rect.right() - 3.5, rect.top() + 4.0),
                ];
                painter.line(points, Stroke::new(1.6, palette.on_brand));
            } else {
                painter.rect_stroke(
                    rect,
                    radius,
                    Stroke::new(1.0, palette.border_strong),
                    egui::StrokeKind::Inside,
                );
            }
        }
    }
}

/// The selection's rectangles in block `index`, row by row.
fn selection(ui: &Ui, state: &State, palette: &Palette, index: usize, placed: &Placed) {
    let (from, to) = state.selection.ordered();
    if from == to || index < from.block || index > to.block {
        return;
    }
    let length = state.doc.block(index).len();
    let start = if index == from.block { from.offset } else { 0 };
    let end = if index == to.block { to.offset } else { length };
    let galley = &placed.laid.galley;
    let offset = placed.galley_origin.to_vec2();
    let painter = ui.painter();
    let mut row_start = 0;
    for row in &galley.rows {
        let count = row.char_count_including_newline().0;
        let row_end = row_start + count;
        let (a, b) = (start.max(row_start), end.min(row_end));
        let reaches_end = index != to.block && b == length && row_end >= length;
        if a < b || (a == b && reaches_end && a >= row_start) {
            let left = galley.pos_from_cursor(CCursor::new(a)).min.x;
            let mut right = galley.pos_from_cursor(CCursor::new(b)).min.x;
            if reaches_end || (b == row_end && count > row.char_count_excluding_newline().0) {
                right += EMPTY_LINE_SELECTION;
            }
            let rect =
                Rect::from_min_max(pos2(left, row.pos.y), pos2(right, row.pos.y + row.height()));
            painter.rect_filled(rect.translate(offset), 2.0, palette.text_selection);
        }
        row_start = row_end;
    }
    if galley.rows.is_empty() && index != to.block {
        let rect =
            Rect::from_min_size(placed.galley_origin, vec2(EMPTY_LINE_SELECTION, style::BODY_SIZE));
        painter.rect_filled(rect, 2.0, palette.text_selection);
    }
}

/// Mentions, images and raw HTML tags drawn over the room their char reserves.
fn chips(ui: &Ui, palette: &Palette, placed: &Placed) {
    let galley = &placed.laid.galley;
    let offset = placed.galley_origin.to_vec2();
    for chip in &placed.laid.chips {
        let start = galley.pos_from_cursor(CCursor::new(chip.index));
        let end = galley.pos_from_cursor(CCursor::new(chip.index + 1));
        let rect = Rect::from_min_max(start.min, pos2(end.min.x, start.max.y))
            .translate(offset)
            .shrink2(vec2(1.0, 1.0));
        let (fill, color) = match chip.kind {
            ChipKind::Mention => (palette.selection, palette.accent_text),
            ChipKind::Image | ChipKind::Raw => (palette.surface_subtle, palette.text_secondary),
        };
        ui.painter().rect(
            rect,
            style::RADIUS_CONTROL,
            fill,
            Stroke::new(1.0, Color32::TRANSPARENT),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            &chip.label,
            chip.font.clone(),
            color,
        );
    }
}

/// Makes `Context::text_edit_focused` true while the text area has the focus. egui answers it by
/// whether the focused id has a `TextEditState`, and the app's single-key shortcuts (N, P, R, ...)
/// rely on it to stay out of the way while the user types.
fn mark_as_text_edit(ctx: &egui::Context, text_id: Id) {
    if egui::text_edit::TextEditState::load(ctx, text_id).is_none() {
        egui::text_edit::TextEditState::default().store(ctx, text_id);
    }
}

/// The text area of one frame.
pub struct Body<'a> {
    pub text_id: Id,
    pub options: &'a RichOptions<'a>,
    pub has_focus: bool,
    pub page_height: f32,
    pub min_height: f32,
}

impl Body<'_> {
    /// Lays out, takes input, runs commands and paints. Returns the caret's rectangle and the
    /// content's origin.
    pub fn show(&self, ui: &mut Ui, state: &mut State, clicked: Option<Command>) -> (Rect, Pos2) {
        let ctx = ui.ctx().clone();
        let palette = theme::palette(&ctx);
        let width = ui.available_width();
        let people = PullRequestContext::current(&ctx);
        state.layout.prepare(&ctx, palette, width - 2.0 * MARGIN.x, state.platform, people);
        let total = state.layout.offsets(&state.doc).total;
        let size = vec2(width, (total + 2.0 * MARGIN.y).max(self.min_height));
        let rect = Rect::from_min_size(ui.cursor().min, size);
        ui.allocate_rect(rect, Sense::hover());
        let sense = if self.options.is_enabled { Sense::click_and_drag() } else { Sense::hover() };
        let response = ui.interact(rect, self.text_id, sense);
        let origin = rect.min + MARGIN;
        let frame =
            Frame { ui, origin, page_height: self.page_height, now: ui.input(|input| input.time) };

        let mut commands: Vec<Command> = clicked.into_iter().collect();
        commands.extend(pointer(ui, &response, state, origin));
        let takes_input = self.has_focus && self.options.is_enabled;
        if takes_input {
            mark_as_text_edit(&ctx, self.text_id);
            commands.extend(input::take_commands(ui, state));
        }
        let ran = input::run(state, &frame, commands);
        if takes_input {
            // For the next frame's keys, so it follows where this frame's input left the caret.
            // Tab stays in the text where it nests, indents or takes the open mention list's pick.
            let keeps_tab = {
                let block = state.doc.block(state.selection.head.block);
                block.item().is_some()
                    || matches!(block.kind, Kind::Code(_))
                    || matches!(state.popup, Popup::Mention { .. })
            };
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    self.text_id,
                    EventFilter {
                        tab: keeps_tab,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        // Esc stays with the caller (cancel), the focus with the text.
                        escape: true,
                    },
                );
            });
        }
        if let Some(copied) = ran.copied {
            ctx.copy_text(copied);
        }
        if response.hovered() && self.options.is_enabled {
            ctx.set_cursor_icon(egui::CursorIcon::Text);
        }
        let caret_rect =
            caret::caret_rect(&mut state.layout, &ctx, &state.doc, origin, state.selection.head);
        if state.scroll_to_caret {
            state.scroll_to_caret = false;
            ui.scroll_to_rect(caret_rect.expand2(caret::SCROLL_MARGIN), None);
        }
        paint(ui, state, origin, self.has_focus, self.options.hint);
        self.report(ui, &response, state, caret_rect, rect);
        (caret_rect, origin)
    }

    /// The IME area and the accessibility node.
    fn report(
        &self,
        ui: &Ui,
        response: &Response,
        state: &mut State,
        caret_rect: Rect,
        rect: Rect,
    ) {
        if self.has_focus && ui.memory(|memory| memory.owns_ime_events(self.text_id)) {
            let to_global = ui.ctx().layer_transform_to_global(ui.layer_id()).unwrap_or_default();
            ui.output_mut(|output| {
                output.ime = Some(egui::output::IMEOutput {
                    purpose: egui::IMEPurpose::Normal,
                    rect: to_global * rect,
                    cursor_rect: to_global * caret_rect,
                    should_interrupt_composition: false,
                });
            });
        }
        let value = state.value();
        let enabled = self.options.is_enabled;
        let hint = self.options.hint;
        response.widget_info(|| egui::WidgetInfo::text_edit(enabled, &*value, &*value, hint));
        let label = self.options.label;
        ui.ctx().accesskit_node_builder(self.text_id, |node| {
            node.set_role(egui::accesskit::Role::MultilineTextInput);
            node.set_label(label);
        });
    }
}
