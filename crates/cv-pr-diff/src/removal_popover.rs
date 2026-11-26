//! New only: the red wedges where lines were removed, and the popover that shows those lines with
//! their base line numbers and syntax colors. Resting the pointer on a wedge opens the popover,
//! clicking it (or Enter on the focused wedge) pins it; Esc, a click elsewhere or another click on
//! the wedge closes it.

use std::ops::Range;

use egui::{
    Align, Align2, CursorIcon, Id, Key, Layout, Modifiers, Order, Rect, RichText, Sense, Stroke,
    Ui, pos2, vec2,
};
use review_git::FileDiff;

use cv_base::theme::{self, icons, style};
use cv_code::code_job::{self, JobStyle};
use cv_code::contents::FileContents;
use cv_code::diff_paint::{self, Metrics, TEXT_END_PADDING};
use cv_code::line_source::{self, Pane};
use cv_syntax::highlight::LineSpans;
use cv_widgets::numbers::group_digits;
use cv_widgets::{buttons, focus};

/// Resting time on a wedge before its popover opens, so sweeping across the gutter stays quiet.
const HOVER_DELAY: f64 = 0.15;
const MAX_VISIBLE_LINES: usize = 14;
const MIN_WIDTH: f32 = 360.0;
const MAX_WIDTH: f32 = 960.0;
const HEADER_HEIGHT: f32 = 36.0;
/// Gap between the boundary the wedge points at and the popover.
const OFFSET: f32 = 6.0;
/// Space kept between the popover and the edges of the diff.
const MARGIN: f32 = 8.0;
/// Right padding of the line numbers, as in the diff gutter, and the gap between gutter and code.
const NUMBER_PADDING: f32 = 10.0;
const NUMBER_GAP: f32 = 8.0;
/// The removed text screen readers get is capped, so a huge removal stays cheap.
const MAX_ACCESSIBLE_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopoverAction {
    Copy,
    Comment,
}

#[derive(Default)]
pub struct RemovalPopover {
    open: Option<Open>,
    /// The wedge under the pointer and since when, until the hover delay has passed.
    resting: Option<(usize, f64)>,
    /// Where the open popover's wedge was painted this frame; `None` once it scrolled away.
    anchor: Option<Anchor>,
}

#[derive(Debug, Clone, Copy)]
struct Open {
    /// Index of the removal row.
    row: usize,
    /// The wedge's widget, where keyboard focus returns when the popover closes.
    wedge: Id,
    is_pinned: bool,
    /// Opened from the keyboard: focus moves to the popover's first button once.
    focus_inside: bool,
    /// The popover last frame, so moving the pointer onto it keeps a hover popover open.
    rect: Rect,
    /// Columns of the longest removed line, measured once.
    widest: Option<usize>,
}

impl Open {
    fn new(row: usize, wedge: Id, is_pinned: bool) -> Self {
        Self { row, wedge, is_pinned, focus_inside: false, rect: Rect::NOTHING, widest: None }
    }
}

#[derive(Debug, Clone, Copy)]
struct Anchor {
    y: f32,
    hit: Rect,
}

pub struct PopoverInputs<'a> {
    pub id: Id,
    pub diff: &'a FileDiff,
    pub contents: Option<&'a FileContents>,
    /// Syntax colors by diff line, standing in until the file's own colors reach a line.
    pub line_spans: Option<&'a LineSpans>,
    /// The removed lines (indices into the diff lines).
    pub lines: Range<usize>,
    pub metrics: &'a Metrics,
    pub can_comment: bool,
    /// The diff area; the popover stays inside it.
    pub bounds: Rect,
}

impl RemovalPopover {
    pub fn close(&mut self) {
        self.open = None;
        self.resting = None;
    }

    /// The removal row whose popover is open.
    pub fn open_row(&self) -> Option<usize> {
        self.open.map(|open| open.row)
    }

    /// Call once per frame before the wedges are painted.
    pub fn begin_frame(&mut self) {
        self.anchor = None;
    }

    /// Paints the wedge of removal row `row`, pointing at boundary `y` of `pane`, and handles
    /// hovering and clicking it. `label` names it for screen readers.
    pub fn wedge(
        &mut self,
        ui: &mut Ui,
        row: usize,
        pane: Rect,
        y: f32,
        metrics: &Metrics,
        label: &str,
    ) {
        let palette = theme::palette(ui.ctx());
        let hit = metrics.wedge_rect(pane.left(), y);
        let id = ui.scope_id().with(("removal", row));
        let response =
            ui.interact(hit, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
        let is_open = |open: Option<Open>| open.is_some_and(|open| open.row == row);
        if response.hovered() {
            let now = ui.input(|input| input.time);
            let since = match self.resting {
                Some((resting, since)) if resting == row => since,
                _ => {
                    self.resting = Some((row, now));
                    now
                }
            };
            // A pinned popover stays until it is closed; hovering other wedges does not replace it.
            let can_open = !is_open(self.open) && self.open.is_none_or(|open| !open.is_pinned);
            let waited = now - since;
            if can_open && waited >= HOVER_DELAY {
                self.open = Some(Open::new(row, id, false));
            } else if can_open {
                ui.ctx().request_repaint_after_secs((HOVER_DELAY - waited) as f32);
            }
        } else if self.resting.is_some_and(|(resting, _)| resting == row) {
            self.resting = None;
        }
        if response.clicked() {
            // Enter or Space on the focused wedge clicks it too; then the popover takes the focus.
            let by_keyboard =
                ui.input(|input| input.key_pressed(Key::Enter) || input.key_pressed(Key::Space));
            self.open = match self.open {
                Some(open) if open.row == row && open.is_pinned => None,
                Some(open) if open.row == row => {
                    Some(Open { is_pinned: true, focus_inside: by_keyboard, ..open })
                }
                _ => Some(Open { focus_inside: by_keyboard, ..Open::new(row, id, true) }),
            };
        }
        let is_open = is_open(self.open);
        if is_open {
            self.anchor = Some(Anchor { y, hit });
        }
        diff_paint::paint_wedge(
            ui.painter(),
            metrics,
            pane,
            y,
            &palette,
            is_open || response.hovered(),
        );
        focus::ring(ui, &response, hit);
    }

    /// Shows the open popover above the rows; call after the rows were painted.
    pub fn show(&mut self, ui: &Ui, inputs: &PopoverInputs<'_>) -> Option<PopoverAction> {
        let mut open = self.open?;
        let Some(anchor) = self.anchor else {
            // Its wedge scrolled out of view.
            self.close();
            return None;
        };
        let ctx = ui.ctx().clone();
        if !stays_open(&ctx, &open, anchor) {
            self.close();
            return None;
        }

        let palette = theme::palette(&ctx);
        let place = Placement::new(inputs, &mut open, anchor);
        // Pressing inside a hover popover (to scroll it or use a button) keeps it open.
        if ctx.input(|input| {
            input.pointer.any_pressed()
                && input.pointer.press_origin().is_some_and(|origin| open.rect.contains(origin))
        }) {
            open.is_pinned = true;
        }
        let width = place.width;
        let mut clicked = None;
        let area = egui::Area::new(inputs.id)
            .order(Order::Foreground)
            .fixed_pos(place.left_top)
            .constrain(false)
            .show(&ctx, |ui| {
                egui::Frame::new()
                    .fill(palette.surface)
                    .stroke(Stroke::new(1.0, palette.border_strong))
                    .corner_radius(style::RADIUS_POPUP)
                    .shadow(style::popup_shadow(&palette))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        clicked = header(ui, inputs, &open, width);
                        let divider = ui.allocate_space(vec2(width, 1.0)).1;
                        ui.painter().hline(
                            divider.x_range(),
                            divider.center().y,
                            Stroke::new(1.0, palette.border),
                        );
                        body(ui, inputs, place.content_width.max(width), place.body_height);
                    });
            });
        open.rect = area.response.rect;
        open.focus_inside = false;
        self.open = Some(open);
        match clicked? {
            HeaderButton::Copy => Some(PopoverAction::Copy),
            HeaderButton::Comment => {
                self.close();
                Some(PopoverAction::Comment)
            }
            HeaderButton::Close => {
                ctx.memory_mut(|memory| memory.request_focus(open.wedge));
                self.close();
                None
            }
        }
    }
}

/// Whether the popover stays: Esc (focus back on the wedge) or a press elsewhere closes it, and
/// an unpinned one closes when the pointer leaves the wedge, the popover and the line between.
fn stays_open(ctx: &egui::Context, open: &Open, anchor: Anchor) -> bool {
    // The popover keeps the line on each side of the boundary in view; the pointer crosses that
    // line on its way from the wedge to the popover.
    let bridge = Rect::from_x_y_ranges(
        anchor.hit.x_range(),
        open.rect.y_range().union(anchor.hit.y_range()),
    );
    let is_inside = |position: egui::Pos2| {
        anchor.hit.contains(position) || open.rect.contains(position) || bridge.contains(position)
    };
    let is_hovering = ctx.pointer_hover_pos().is_some_and(is_inside);
    let pressed_elsewhere = ctx.input(|input| {
        input.pointer.any_pressed()
            && input.pointer.press_origin().is_some_and(|origin| !is_inside(origin))
    });
    let escape = ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    if escape {
        ctx.memory_mut(|memory| memory.request_focus(open.wedge));
    }
    !escape && !pressed_elsewhere && (open.is_pinned || is_hovering)
}

/// Where the popover goes and how large it is.
struct Placement {
    left_top: egui::Pos2,
    width: f32,
    /// The widest line's width; wider than `width`, the body scrolls sideways.
    content_width: f32,
    body_height: f32,
}

impl Placement {
    /// As wide as the widest removed line within the bounds, above the boundary when it fits.
    /// Measures the widest line once per opening, in `open`.
    fn new(inputs: &PopoverInputs<'_>, open: &mut Open, anchor: Anchor) -> Self {
        let metrics = inputs.metrics;
        let diff = inputs.diff;
        let lines = diff.lines();
        let widest = *open.widest.get_or_insert_with(|| {
            inputs
                .lines
                .clone()
                .map(|index| code_job::columns(diff.text(&lines[index])))
                .max()
                .unwrap_or(0)
        });
        let count = inputs.lines.len();
        let text_inset = metrics.number_width + NUMBER_GAP;
        let content_width = text_inset + widest as f32 * metrics.char_width + TEXT_END_PADDING;
        let max_width = MAX_WIDTH
            .min(inputs.bounds.width() - MARGIN * 2.0)
            .max(MIN_WIDTH.min(inputs.bounds.width()));
        let width = content_width.clamp(MIN_WIDTH.min(max_width), max_width);
        let body_height = count.min(MAX_VISIBLE_LINES) as f32 * metrics.line_height;
        let height = HEADER_HEIGHT + 1.0 + body_height + 2.0;
        // Above the boundary when it fits, so the lines that replaced the removed ones stay visible
        // below; one line of distance keeps the line right before the removal readable too.
        let distance = metrics.line_height + OFFSET;
        let room_above = anchor.y - inputs.bounds.top() - MARGIN;
        let room_below = inputs.bounds.bottom() - anchor.y - MARGIN;
        let top = if room_above >= height + distance || room_above > room_below {
            anchor.y - distance - height
        } else {
            anchor.y + distance
        };
        let left = anchor
            .hit
            .left()
            .min(inputs.bounds.right() - width - MARGIN)
            .max(inputs.bounds.left() + MARGIN);
        Self { left_top: pos2(left, top), width, content_width, body_height }
    }
}

#[derive(Clone, Copy)]
enum HeaderButton {
    Copy,
    Comment,
    Close,
}

/// "2 lines removed", where they were in the base version, and the actions.
fn header(
    ui: &mut Ui,
    inputs: &PopoverInputs<'_>,
    open: &Open,
    width: f32,
) -> Option<HeaderButton> {
    let palette = theme::palette(ui.ctx());
    let lines = inputs.diff.lines();
    let count = inputs.lines.len();
    let first = lines[inputs.lines.start].old_line;
    let last = lines[inputs.lines.end - 1].old_line;
    let title = if count == 1 {
        "1 line removed".to_owned()
    } else {
        format!("{} lines removed", group_digits(count))
    };
    let place = if first == last {
        format!("base line {first}")
    } else {
        format!("base lines {first}–{last}")
    };
    let mut clicked = None;
    ui.allocate_ui_with_layout(
        vec2(width, HEADER_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.add_space(style::SPACE_M);
            ui.label(
                RichText::new(title)
                    .font(theme::fonts::semibold_font(style::SMALL_SIZE))
                    .color(palette.text),
            );
            ui.label(RichText::new(place).size(style::SMALL_SIZE).color(palette.text_secondary));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(style::SPACE_XS);
                ui.spacing_mut().item_spacing.x = 2.0;
                if open.is_pinned && buttons::icon(ui, icons::X, "Close (Esc)").clicked() {
                    clicked = Some(HeaderButton::Close);
                }
                if inputs.can_comment
                    && buttons::icon(ui, icons::CHAT_CIRCLE_TEXT, "Comment on the removed lines")
                        .clicked()
                {
                    clicked = Some(HeaderButton::Comment);
                }
                // Laid out right to left, so Copy is the first button from the left.
                let copy = buttons::icon(ui, icons::COPY, "Copy the removed lines");
                if open.focus_inside {
                    copy.request_focus();
                }
                if copy.clicked() {
                    clicked = Some(HeaderButton::Copy);
                }
            });
        },
    );
    clicked
}

/// The removed lines as the inline layout shows deleted lines; only the visible ones are laid out.
fn body(ui: &mut Ui, inputs: &PopoverInputs<'_>, content_width: f32, height: f32) {
    let palette = theme::palette(ui.ctx());
    let metrics = inputs.metrics;
    let count = inputs.lines.len();
    let colors = diff_paint::line_colors(review_git::LineKind::Deleted, &palette);
    // The area's size is remembered from the frame before, so the body asks for its full height.
    egui::ScrollArea::both()
        .id_salt("removed_lines")
        .max_height(height)
        .min_scrolled_height(height)
        .auto_shrink([false, true])
        .show_viewport(ui, |ui, viewport| {
            let size = vec2(content_width, count as f32 * metrics.line_height);
            let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Document, true, accessible_text(inputs))
            });
            let painter = ui.painter();
            painter.rect_filled(rect, 0, colors.background);
            let gutter = Rect::from_min_size(rect.min, vec2(metrics.number_width, rect.height()));
            painter.rect_filled(gutter, 0, colors.gutter);
            painter.rect_filled(
                Rect::from_min_size(rect.min, vec2(3.0, rect.height())),
                0,
                colors.stripe,
            );
            let first = (viewport.top() / metrics.line_height).floor().max(0.0) as usize;
            let last = ((viewport.bottom() / metrics.line_height).ceil() as usize).min(count);
            for offset in first..last {
                let index = inputs.lines.start + offset;
                let mut code =
                    line_source::diff_line(inputs.diff, inputs.contents, index, Pane::Left);
                if !code.has_file_spans
                    && let Some(spans) = inputs.line_spans.and_then(|spans| spans.line(index))
                {
                    code.spans = spans;
                }
                let top = rect.top() + offset as f32 * metrics.line_height;
                let center_y = top + metrics.line_height / 2.0;
                painter.text(
                    pos2(rect.left() + metrics.number_width - NUMBER_PADDING, center_y),
                    Align2::RIGHT_CENTER,
                    code.number.to_string(),
                    metrics.number_font.clone(),
                    palette.text_secondary,
                );
                let style = JobStyle {
                    font: &metrics.font,
                    palette: &palette,
                    word_background: egui::Color32::TRANSPARENT,
                    line_height: metrics.line_height,
                    wrap_width: None,
                    syntax_mix: 1.0,
                };
                let galley =
                    painter.layout_job(code_job::layout_job(code.text, code.spans, &[], &style));
                let text_left = (rect.left() + metrics.number_width + NUMBER_GAP).round();
                painter.galley(pos2(text_left, top.round()), galley, palette.text);
            }
        });
}

/// The removed text for screen readers (and tests), cut at a line end once it gets long.
fn accessible_text(inputs: &PopoverInputs<'_>) -> String {
    let lines = inputs.diff.lines();
    let mut text = String::from("Removed lines:");
    for index in inputs.lines.clone() {
        let line = inputs.diff.text(&lines[index]);
        if text.len() + line.len() > MAX_ACCESSIBLE_BYTES {
            text.push_str("\n…");
            break;
        }
        text.push('\n');
        text.push_str(line);
    }
    text
}

/// The removed lines as plain text, for the clipboard.
pub fn removed_text(diff: &FileDiff, lines: Range<usize>) -> String {
    let diff_lines = diff.lines();
    lines.map(|index| diff.text(&diff_lines[index])).collect::<Vec<_>>().join("\n")
}

/// Screen reader name of a wedge.
pub fn wedge_label(diff: &FileDiff, lines: Range<usize>) -> String {
    let diff_lines = diff.lines();
    let count = lines.len();
    let first = diff_lines[lines.start].old_line;
    let last = diff_lines[lines.end - 1].old_line;
    if count == 1 {
        format!("Show 1 removed line (base line {first})")
    } else {
        format!("Show {} removed lines (base lines {first}–{last})", group_digits(count))
    }
}
