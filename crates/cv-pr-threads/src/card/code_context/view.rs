//! The excerpt as the card shows it: a toolbar with the shown lines and the New only / Diff / Split
//! switch, then the code in a bordered block ([`super::rows`]). "Show more" rows open the block
//! upwards and downwards. Split on a card too narrow for two readable sides shows Diff and says so
//! until the card is wide enough again. While the code loads a skeleton holds its place at the
//! height the code took last time, or else the one [`window::predict`] gives, so nothing below it
//! moves when the code lands; a failed load says why, with Retry.

use std::sync::Arc;

use cv_base::theme::{self, Palette, icons, motion, style};
use cv_code::diff_paint::Metrics;
use cv_widgets::{buttons, focus, glyph, row, skeleton, tabs};
use egui::{Align, Layout, Margin, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use review_core::ReviewThread;

use super::super::{hairline, section};
use super::code_file::CodeFile;
use super::rows::{self, Rows};
use super::window::{self, CONTEXT, Excerpt, Extra, Focus, Known, Mode, STEP, Shape, Shown};
use super::{CodeContexts, FileKey, LastShape};
use crate::{ThreadEvent, ThreadsUi, suggestion};

/// The excerpt's inset in the card: the comments' own.
const MARGIN: Margin = Margin { left: 16, right: 16, top: 12, bottom: 12 };
const TOOLBAR_HEIGHT: f32 = 26.0;
/// A "Show more" row, and the one-line message in place of the code.
const BAND_HEIGHT: f32 = 30.0;
const NEW_ONLY: &str = "New only";
const DIFF: &str = "Diff";
const SPLIT: &str = "Split";
/// Inner margin of the code block, which the rows' width leaves out.
const BLOCK_MARGIN: i8 = 1;

/// What the excerpt has to show this frame.
enum Status {
    /// The mirror or the file is on its way; the file to ask for once the mirror is there.
    Waiting(Option<FileKey>),
    /// Why there is no code; Retry loads the file again, or prepares the mirror again (`None`).
    Failed {
        message: String,
        key: Option<FileKey>,
    },
    Ready(Arc<CodeFile>),
}

/// The code `thread` is about, between its card's header and its comments, behind a hairline;
/// nothing for a thread without lines. Returns where the user asked to see the code.
pub fn show(ui: &mut Ui, thread: &ReviewThread, threads: &mut ThreadsUi) -> Option<ThreadEvent> {
    let code = &mut threads.code;
    let source = code.source(thread);
    let anchor = source.as_ref().map(|(_, anchor)| *anchor).or(thread.anchor.as_ref())?;
    let focus = Focus::of(anchor)?;
    let lines_title = suggestion::lines_title(anchor);
    let status = status(code, source.map(|(key, _)| key));
    let palette = theme::palette(ui.ctx());
    hairline(ui, &palette);
    let reveal = if thread.is_outdated && thread.origin.is_some() {
        ThreadEvent::ShowOrigin { thread_id: thread.id.clone() }
    } else {
        ThreadEvent::Reveal { thread_id: thread.id.clone() }
    };
    let mut is_revealed = false;
    section(ui, MARGIN, |ui| {
        ui.push_id(("thread_code", &thread.id), |ui| {
            let known = code.known(&anchor.path);
            let (picked, mode) = modes(ui, code, &thread.id, &status, &focus, known);
            let is_too_narrow = picked != mode;
            let extra = code.cards.get(&thread.id).map(|card| card.extra).unwrap_or_default();
            let excerpt = match &status {
                Status::Ready(file) => window::excerpt(file, mode, &focus, extra),
                Status::Waiting(_) | Status::Failed { .. } => None,
            };
            let caption = match (&status, &excerpt) {
                (Status::Ready(file), Some(excerpt)) => lines_caption(file, mode, excerpt),
                _ => lines_title.clone(),
            };
            let action = toolbar(ui, thread, &caption, picked, extra != Extra::default());
            match action {
                Some(ToolbarAction::Pick(picked)) => code.pick_mode(ui.ctx(), &thread.id, picked),
                Some(ToolbarAction::Collapse) => code.card(&thread.id).extra = Extra::default(),
                None => {}
            }
            if is_too_narrow {
                too_narrow(ui);
            }
            ui.add_space(style::SPACE_S);
            let preferred = code.preferred(ui.ctx(), &thread.id);
            code_block(ui, &palette, |ui| match status {
                Status::Waiting(key) => {
                    let shape = code
                        .last_shape(&thread.id, preferred, extra)
                        .unwrap_or_else(|| window::predict(&focus, mode, extra, known));
                    waiting(ui, code, &thread.id, key, shape);
                }
                Status::Failed { message, key } => {
                    if failure(ui, &message) {
                        code.retry(key.as_ref());
                    }
                }
                Status::Ready(file) => {
                    let card = code.card(&thread.id);
                    card.waiting_since = None;
                    // The one-line message in place of the code is one band high.
                    let shape = excerpt
                        .as_ref()
                        .map_or(Shape { above: true, lines: 0, below: false }, Excerpt::shape);
                    card.last = Some(LastShape { preferred, extra, shape });
                    let Some(excerpt) = excerpt else {
                        is_revealed = missing_lines(ui, &lines_title);
                        return;
                    };
                    let shown = Shown::new(&file, mode);
                    let card = code.card(&thread.id);
                    if excerpt.hidden_above > 0 && show_more(ui, excerpt.hidden_above, true) {
                        card.extra.above += STEP;
                    }
                    let rows =
                        Rows { file: &file, shown, focus: &focus, mode, font: code.font_size };
                    is_revealed |= rows.show(ui, &excerpt.lines);
                    let card = code.card(&thread.id);
                    if excerpt.hidden_below > 0 && show_more(ui, excerpt.hidden_below, false) {
                        card.extra.below += STEP;
                    }
                }
            });
        });
    });
    is_revealed.then_some(reveal)
}

/// The mode the card's switch shows and the one its code is shown in: Diff where Split was picked
/// but the card is too narrow for it. Before the file is read, what the list of changed files
/// says decides, so the code lands in the mode its skeleton had.
fn modes(
    ui: &Ui,
    code: &CodeContexts,
    thread_id: &str,
    status: &Status,
    focus: &Focus,
    known: Known,
) -> (Mode, Mode) {
    let is_on_removed_lines = match status {
        Status::Ready(file) => window::is_on_removed_lines(file, focus),
        // Every line of a deleted file is a removed one.
        Status::Waiting(_) => known.is_all_changed && focus.is_old_side(),
        Status::Failed { .. } => false,
    };
    let picked = code.mode(ui.ctx(), thread_id, is_on_removed_lines);
    // The gutter has room for four digits at least, so a file not read yet fits as one under
    // 10,000 lines does.
    let max_line = match status {
        Status::Ready(file) => Some(file.max_line()),
        Status::Waiting(_) => Some(0),
        Status::Failed { .. } => None,
    };
    let is_too_narrow = picked == Mode::Split
        && max_line.is_some_and(|max_line| {
            let metrics = Metrics::new(ui, code.font_size, max_line);
            let width = ui.available_width() - 2.0 * f32::from(BLOCK_MARGIN);
            !rows::fits_split(&metrics, width)
        });
    (picked, if is_too_narrow { Mode::Diff } else { picked })
}

fn status(code: &CodeContexts, key: Option<FileKey>) -> Status {
    let Some(key) = key else {
        return match &code.mirror_error {
            Some(error) => Status::Failed {
                message: format!("Could not prepare the repository: {error}"),
                key: None,
            },
            None => Status::Waiting(None),
        };
    };
    match code.files.get(&key) {
        Some(cv_runtime::load::Load::Ready(file)) => Status::Ready(file.clone()),
        Some(cv_runtime::load::Load::Failed(message)) => Status::Failed {
            message: format!("Could not load the code: {message}"),
            key: Some(key),
        },
        _ => Status::Waiting(Some(key)),
    }
}

/// "Lines 17–23" of what the excerpt shows.
fn lines_caption(file: &CodeFile, mode: Mode, excerpt: &Excerpt) -> String {
    let shown = Shown::new(file, mode);
    let numbers: Vec<u32> = excerpt
        .lines
        .clone()
        .flat_map(|position| shown.rows(position))
        .map(rows::number)
        .filter(|line| *line > 0)
        .collect();
    match (numbers.iter().min(), numbers.iter().max()) {
        (Some(first), Some(last)) if first == last => format!("Line {first}"),
        (Some(first), Some(last)) => format!("Lines {first}–{last}"),
        _ => String::new(),
    }
}

enum ToolbarAction {
    Pick(Mode),
    Collapse,
}

/// The shown lines (and which version they are from, for an outdated thread) on the left; the
/// mode switch and, once expanded, Collapse on the right.
fn toolbar(
    ui: &mut Ui,
    thread: &ReviewThread,
    caption: &str,
    mode: Mode,
    is_expanded: bool,
) -> Option<ToolbarAction> {
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), TOOLBAR_HEIGHT),
        Layout::right_to_left(Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let mut picked = mode;
            let options = [
                (Mode::NewOnly, NEW_ONLY, "The new version of the code, its changed lines marked"),
                (Mode::Diff, DIFF, "Removed and added lines"),
                (Mode::Split, SPLIT, "The old version on the left, the new one on the right"),
            ];
            // Its own left-to-right strip, so the options keep their order in this right-to-left row.
            let width = tabs::segmented_width(ui, &[NEW_ONLY, DIFF, SPLIT]);
            let switched = ui
                .allocate_ui_with_layout(
                    vec2(width, TOOLBAR_HEIGHT),
                    Layout::left_to_right(Align::Center),
                    |ui| tabs::segmented_with_hints(ui, &mut picked, &options),
                )
                .inner;
            if switched && picked != mode {
                action = Some(ToolbarAction::Pick(picked));
            }
            if is_expanded
                && ui
                    .link(RichText::new("Collapse").size(style::SMALL_SIZE + 1.0))
                    .on_hover_text(format!("Back to {CONTEXT} lines around the comment"))
                    .clicked()
            {
                action = Some(ToolbarAction::Collapse);
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let (prefix, hint) = match (&thread.origin, thread.is_outdated) {
                    (Some(origin), true) => (
                        format!("Version {} · ", short_sha(&origin.head_sha)),
                        "The code as it was when the comment was written",
                    ),
                    (None, true) => (
                        "Latest version · ".to_owned(),
                        "The comment was written on an earlier version: this is the code now",
                    ),
                    (_, false) => (String::new(), ""),
                };
                let text = RichText::new(format!("{prefix}{caption}"))
                    .size(style::SMALL_SIZE + 1.0)
                    .color(palette.text_secondary);
                let label = ui.add(egui::Label::new(text).truncate());
                if !hint.is_empty() {
                    label.on_hover_text(hint);
                }
            });
        },
    );
    action
}

/// Split was picked but the card is too narrow for it: a quiet line under the toolbar says why the
/// excerpt shows Diff.
fn too_narrow(ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    ui.add_space(style::SPACE_XS);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        glyph::decorative(
            ui,
            RichText::new(icons::INFO)
                .font(icons::rest_font(&palette, icons::SIZE_SMALL))
                .color(palette.text_secondary),
        );
        ui.label(
            RichText::new("Too narrow for side by side")
                .size(style::SMALL_SIZE + 1.0)
                .color(palette.text_secondary),
        )
        .on_hover_text("Showing Diff until the card is wide enough for the old and the new side");
    });
}

/// The bordered block the code sits in.
fn code_block(ui: &mut Ui, palette: &Palette, content: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(style::RADIUS_CONTROL)
        .inner_margin(Margin::same(BLOCK_MARGIN))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            content(ui);
        });
}

/// The skeleton of the excerpt in its `shape` (its lines, with room for the "Show more" rows),
/// after the grace period. Asks for the file while it is on screen.
fn waiting(
    ui: &mut Ui,
    code: &mut CodeContexts,
    thread_id: &str,
    key: Option<FileKey>,
    shape: Shape,
) {
    let now = code.now;
    let started = *code.card(thread_id).waiting_since.get_or_insert(now);
    let metrics = Metrics::new(ui, code.font_size, 0);
    let above = if shape.above { BAND_HEIGHT } else { 0.0 };
    let below = if shape.below { BAND_HEIGHT } else { 0.0 };
    let height = above + shape.lines as f32 * metrics.line_height + below;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    skeleton::announce(&response, "Loading the code");
    if !ui.is_rect_visible(rect) {
        return;
    }
    // A new ask starts at the next frame.
    if let Some(key) = key
        && !code.files.contains_key(&key)
        && code.wanted.insert(key)
    {
        ui.ctx().request_repaint();
    }
    let shown = skeleton::reveal(ui.ctx(), started);
    if shown <= 0.0 || shape.lines == 0 {
        return;
    }
    let lines = Rect::from_x_y_ranges(rect.x_range(), rect.top() + above..=rect.bottom() - below);
    let mut lines_ui =
        ui.new_child(egui::UiBuilder::new().max_rect(lines).layout(Layout::top_down(Align::Min)));
    lines_ui.multiply_opacity(shown);
    lines_ui.spacing_mut().item_spacing.y = 0.0;
    skeleton::code_lines(&mut lines_ui, shape.lines, metrics.line_height, false);
}

/// Why the code is missing, with Retry; true when Retry was clicked.
fn failure(ui: &mut Ui, message: &str) -> bool {
    let palette = theme::palette(ui.ctx());
    band(ui, |ui| {
        let mut is_retried = false;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            is_retried = buttons::subtle(ui, icons::ARROW_CLOCKWISE, "Retry").clicked();
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                glyph::decorative(
                    ui,
                    RichText::new(icons::WARNING)
                        .font(icons::rest_font(&palette, icons::SIZE_SMALL))
                        .color(palette.warning),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(message).size(style::SMALL_SIZE + 1.0).color(palette.text),
                    )
                    .truncate(),
                )
                .on_hover_text(message);
            });
        });
        is_retried
    })
}

/// The commented lines are not in the file the excerpt read: say so and lead to the file. True
/// when the link was clicked.
fn missing_lines(ui: &mut Ui, lines_title: &str) -> bool {
    let palette = theme::palette(ui.ctx());
    band(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(
            RichText::new(format!("{lines_title} is not in this version of the file."))
                .size(style::SMALL_SIZE + 1.0)
                .color(palette.text_secondary),
        );
        ui.link(RichText::new("Show in the file").size(style::SMALL_SIZE + 1.0)).clicked()
    })
}

/// A one-line row in the block, its content centered vertically.
fn band<R>(ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), BAND_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.add_space(style::SPACE_M);
            ui.set_max_width(ui.available_width() - style::SPACE_S);
            content(ui)
        },
    )
    .inner
}

/// "Show 10 more lines" above (`is_above`) or below the code; true when clicked.
fn show_more(ui: &mut Ui, hidden: usize, is_above: bool) -> bool {
    let palette = theme::palette(ui.ctx());
    let count = hidden.min(STEP);
    let text =
        if count == 1 { "Show 1 more line".to_owned() } else { format!("Show {count} more lines") };
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), BAND_HEIGHT), Sense::click());
    let side = if is_above { "above" } else { "below" };
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{text} {side}"))
    });
    let fill = row::state_fill(ui, &response, palette.surface_subtle);
    ui.painter().rect_filled(rect, 0, fill);
    let edge = if is_above { rect.bottom() - 0.5 } else { rect.top() + 0.5 };
    ui.painter().hline(rect.x_range(), edge, Stroke::new(1.0, palette.border));
    let hover = row::hover_amount(ui, &response);
    let color = motion::mix(palette.text_secondary, palette.accent_text, hover);
    let icon = if is_above { icons::CARET_UP } else { icons::CARET_DOWN };
    let icon_center = pos2(rect.left() + style::SPACE_M + icons::SIZE_SMALL / 2.0, rect.center().y);
    ui.painter().text(
        icon_center,
        egui::Align2::CENTER_CENTER,
        icon,
        icons::rest_font(&palette, icons::SIZE_SMALL),
        color,
    );
    ui.painter().text(
        pos2(icon_center.x + icons::SIZE_SMALL / 2.0 + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(style::SMALL_SIZE + 1.0),
        color,
    );
    focus::ring(ui, &response, rect);
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}
