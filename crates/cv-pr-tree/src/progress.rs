//! The review progress line of the file tree pane: "3 of 12 viewed" and the line totals over a
//! hairline bar. The viewed count is a link to the next file not viewed yet (Shift+J). Once every
//! file is viewed the line says so and offers the vote and the Overview in the totals' place, so
//! the end of a review leads on to its verdict; the line keeps its height, nothing below moves.

use std::sync::Arc;

use egui::{Color32, FontId, Galley, Rect, Sense, Stroke, Ui, pos2, vec2};
use review_core::ChangeBreakdown;

use super::ReviewStep;
use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, motion, style};
use cv_widgets::numbers::group_digits;
use cv_widgets::{change_breakdown, focus, row};

/// Height of the progress strip; the bar itself is a 2 px hairline centred in it.
pub(crate) const PROGRESS_HEIGHT: f32 = 4.0;
pub(crate) const BAR_HEIGHT: f32 = 2.0;
/// Space between the viewed count and the line totals on one line.
const TOTALS_GAP: f32 = 12.0;
/// Space between the added and deleted totals.
const COUNT_GAP: f32 = 6.0;
/// Between the done line's icon and its text, and between its two links.
const LINK_GAP: f32 = 10.0;
const NEXT_UNVIEWED_HINT: &str =
    concat!("Open the next unviewed file (", shortcut!(shift "J"), ")");

/// What the done line offers for the vote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteOffer {
    /// "Approve" casts the approval; disabled while another write runs.
    Approve { is_enabled: bool },
    /// The user's vote is in already: its label, no second approval.
    Voted(&'static str),
    /// No vote here (no token, a closed pull request, no approval on this platform).
    None,
}

/// What the review progress line shows besides the line totals.
pub(crate) struct Progress {
    pub viewed: usize,
    pub total: usize,
    /// The line totals are still on their way.
    pub is_counting_lines: bool,
    pub vote: VoteOffer,
}

/// The line and its bar; returns the step a link asked for.
pub(crate) fn show(
    ui: &mut Ui,
    progress: &Progress,
    breakdown: &ChangeBreakdown,
) -> Option<ReviewStep> {
    let Progress { viewed, total, .. } = *progress;
    let is_done = total > 0 && viewed >= total;
    let step =
        if is_done { done_line(ui, progress, breakdown) } else { line(ui, progress, breakdown) };
    ui.add_space(style::SPACE_XS);
    progress_bar(ui, ui.available_width(), viewed, total);
    step
}

/// "3 of 12 viewed" and the line totals, on one line whatever the totals show, so the tree below
/// never moves when the counting ends. The totals sit right-aligned and keep their width; the
/// viewed text takes what is left and is cut with "…", its full text on hover. While lines are
/// being counted a small spinner and "412 of 1,504", the files counted so far, hold the totals'
/// place. Hovering or focusing the totals, or the files counted, shows their `breakdown` by kind
/// of file.
fn line(ui: &mut Ui, progress: &Progress, breakdown: &ChangeBreakdown) -> Option<ReviewStep> {
    let Progress { viewed, total, is_counting_lines, .. } = *progress;
    let lines = breakdown.total();
    let palette = theme::palette(ui.ctx());
    let font = FontId::proportional(style::SMALL_SIZE);
    let width = ui.available_width();
    let line_height = ui.fonts_mut(|fonts| fonts.row_height(&font));

    let totals = if is_counting_lines {
        let counted = lines.files.saturating_sub(lines.uncounted) as usize;
        let text = format!("{} of {}", group_digits(counted), group_digits(lines.files as usize));
        Totals::Counting(galley(ui, &text, &font, palette.text_secondary, width))
    } else if lines.files > lines.uncounted {
        let added = format!("+{}", group_digits(lines.added as usize));
        let deleted = format!("−{}", group_digits(lines.deleted as usize));
        Totals::Counted(
            galley(ui, &added, &font, palette.success, width),
            galley(ui, &deleted, &font, palette.error, width),
        )
    } else {
        Totals::None
    };
    let totals_width = match &totals {
        Totals::Counted(added, deleted) => added.size().x + COUNT_GAP + deleted.size().x,
        Totals::Counting(counted) => line_height + COUNT_GAP + counted.size().x,
        Totals::None => 0.0,
    };
    let viewed_width =
        if totals_width > 0.0 { (width - TOTALS_GAP - totals_width).max(0.0) } else { width };
    let viewed_text = format!("{} of {} viewed", group_digits(viewed), group_digits(total));
    // A link takes its color when painted, so it can ease to the accent on hover.
    let viewed_galley = galley(ui, &viewed_text, &font, Color32::PLACEHOLDER, viewed_width);
    let (line, response) = ui.allocate_exact_size(vec2(width, line_height), Sense::hover());

    let viewed_rect = Rect::from_min_size(line.min, viewed_galley.size());
    // With files left to review, the count leads to the next one.
    let step = if total > viewed {
        let link =
            Link { id: response.id.with("viewed"), name: &viewed_text, hint: NEXT_UNVIEWED_HINT };
        link.show(ui, viewed_rect, viewed_galley, palette.text_secondary)
            .then_some(ReviewStep::NextUnviewed)
    } else {
        let label = ui.interact(viewed_rect, response.id.with("viewed"), Sense::hover());
        label.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &viewed_text));
        let is_cut = viewed_galley.elided;
        ui.painter().galley(line.left_top(), viewed_galley, palette.text_secondary);
        if is_cut {
            label.on_hover_text(&viewed_text);
        }
        None
    };
    let at = pos2((line.right() - totals_width).max(line.left()), line.top());
    match totals {
        Totals::Counted(added, deleted) => {
            paint_totals(ui, response.id.with("totals"), at, (added, deleted), breakdown);
        }
        Totals::Counting(counted) => {
            paint_counting(ui, response.id.with("counting"), at, counted, breakdown);
        }
        Totals::None => {}
    }
    step
}

/// "All 12 files viewed" with the vote and "Overview" right-aligned where the totals were; the
/// totals stay on the text's hover. The links keep their width, the text is cut first.
fn done_line(ui: &mut Ui, progress: &Progress, breakdown: &ChangeBreakdown) -> Option<ReviewStep> {
    let palette = theme::palette(ui.ctx());
    let font = FontId::proportional(style::SMALL_SIZE);
    let width = ui.available_width();
    let line_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let (line, response) = ui.allocate_exact_size(vec2(width, line_height), Sense::hover());

    // Links take their color when painted, so they can ease to the accent on hover.
    let overview = galley(ui, "Overview", &font, Color32::PLACEHOLDER, width);
    let vote = match progress.vote {
        VoteOffer::Approve { .. } => Some("Approve"),
        VoteOffer::Voted(label) => Some(label),
        VoteOffer::None => None,
    }
    .map(|label| (galley(ui, label, &font, Color32::PLACEHOLDER, width), progress.vote));
    let vote_width = vote.as_ref().map_or(0.0, |(galley, _)| galley.size().x + LINK_GAP);
    let links_width = overview.size().x + vote_width;

    let mut step = None;
    let overview_rect =
        Rect::from_min_size(pos2(line.right() - overview.size().x, line.top()), overview.size());
    let link = Link {
        id: response.id.with("overview"),
        name: "Overview",
        hint: "The pull request's description, reviewers and checks",
    };
    if link.show(ui, overview_rect, overview, palette.accent_text) {
        step = Some(ReviewStep::Overview);
    }
    if let Some((galley, offer)) = vote {
        let rect = Rect::from_min_size(
            pos2(overview_rect.left() - LINK_GAP - galley.size().x, line.top()),
            galley.size(),
        );
        match offer {
            VoteOffer::Approve { is_enabled: true } => {
                let link = Link {
                    id: response.id.with("approve"),
                    name: "Approve",
                    hint: "Approve the pull request",
                };
                if link.show(ui, rect, galley, palette.accent_text) {
                    step = Some(ReviewStep::Approve);
                }
            }
            VoteOffer::Approve { is_enabled: false } => {
                let label = ui.interact(rect, response.id.with("approve"), Sense::hover());
                label.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, false, "Approve"));
                ui.painter().galley(rect.min, galley, palette.text_disabled);
                label.on_hover_text("Another change is on its way to the platform");
            }
            VoteOffer::Voted(label) => {
                let text = format!("Your vote: {label}");
                let label = ui.interact(rect, response.id.with("approve"), Sense::hover());
                label.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
                ui.painter().galley(rect.min, galley, palette.text_secondary);
                label.on_hover_text(text);
            }
            VoteOffer::None => {}
        }
    }

    // The check, then the text in what the links leave.
    let icon_font = icons::state_font(&palette, style::SMALL_SIZE, true);
    let icon = galley(ui, icons::CHECK_CIRCLE, &icon_font, palette.success, width);
    let text_left = line.left() + icon.size().x + style::SPACE_XS;
    let text_room = (line.right() - links_width - TOTALS_GAP - text_left).max(0.0);
    let text = format!("All {} files viewed", group_digits(progress.total));
    let text_galley = galley(ui, &text, &font, palette.text, text_room);
    let text_rect = Rect::from_min_size(
        line.min,
        vec2(text_left - line.left() + text_galley.size().x, line_height),
    );
    let label = ui.interact(text_rect, response.id.with("viewed"), Sense::hover());
    label.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
    ui.painter().galley(
        pos2(line.left(), line.center().y - icon.size().y / 2.0),
        icon,
        palette.success,
    );
    ui.painter().galley(pos2(text_left, line.top()), text_galley, palette.text);
    change_breakdown::attach(&label, breakdown);
    step
}

/// A small text link painted in a rect its line measured: the accent on hover with an underline,
/// a focus ring for the keyboard.
struct Link<'a> {
    id: egui::Id,
    /// What screen readers hear.
    name: &'a str,
    hint: &'a str,
}

impl Link<'_> {
    /// Paints `galley` at `rect` in `rest` (the accent on hover); returns whether it was clicked.
    fn show(&self, ui: &Ui, rect: Rect, galley: Arc<Galley>, rest: Color32) -> bool {
        let palette = theme::palette(ui.ctx());
        let response = ui.interact(rect, self.id, Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, self.name));
        let hover = row::hover_amount(ui, &response);
        let color = motion::mix(rest, palette.accent_hover, hover);
        let is_cut = galley.elided;
        ui.painter().galley(rect.min, galley, color);
        if response.hovered() {
            ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, color));
        }
        focus::ring(ui, &response, rect.expand(2.0));
        let response = if is_cut {
            response.on_hover_text(format!("{}. {}", self.name, self.hint))
        } else {
            response.on_hover_text(self.hint)
        };
        response.clicked()
    }
}

/// What holds the place of the line totals in [`line`].
enum Totals {
    /// "+added" and "−deleted".
    Counted(Arc<Galley>, Arc<Galley>),
    /// The files counted so far while the lines are counted: "412 of 1,504".
    Counting(Arc<Galley>),
    None,
}

/// "+added −deleted" from `at`, focusable, with the breakdown on hover and focus.
fn paint_totals(
    ui: &Ui,
    id: egui::Id,
    at: egui::Pos2,
    (added, deleted): (Arc<Galley>, Arc<Galley>),
    breakdown: &ChangeBreakdown,
) {
    let palette = theme::palette(ui.ctx());
    let added_rect = Rect::from_min_size(at, added.size());
    let deleted_rect =
        Rect::from_min_size(pos2(added_rect.right() + COUNT_GAP, at.y), deleted.size());
    let description = format!("{} added, {} deleted", added.text(), deleted.text());
    let totals_rect = added_rect.union(deleted_rect);
    let totals = ui.interact(totals_rect, id, Sense::hover() | Sense::focusable_noninteractive());
    totals.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &description));
    ui.painter().galley(added_rect.min, added, palette.success);
    ui.painter().galley(deleted_rect.min, deleted, palette.error);
    focus::ring(ui, &totals, totals_rect.expand(2.0));
    change_breakdown::attach(&totals, breakdown);
}

/// The spinner and the files counted so far from `at`, the count focusable with the breakdown
/// counted so far on hover and focus.
fn paint_counting(
    ui: &Ui,
    id: egui::Id,
    at: egui::Pos2,
    counted: Arc<Galley>,
    breakdown: &ChangeBreakdown,
) {
    let palette = theme::palette(ui.ctx());
    let line_height = counted.size().y;
    let spinner_rect = Rect::from_min_size(at, vec2(line_height, line_height));
    let spinner = ui.interact(spinner_rect, id, Sense::hover());
    spinner.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, true, "Counting lines")
    });
    egui::Spinner::new()
        .size(line_height - 4.0)
        .color(palette.text_secondary)
        .paint_at(ui, spinner_rect);
    spinner.on_hover_text("Counting lines and detecting renames");
    let counted_rect =
        Rect::from_min_size(pos2(spinner_rect.right() + COUNT_GAP, at.y), counted.size());
    let description = format!("{} files counted", counted.text());
    let label = ui.interact(
        counted_rect,
        id.with("files"),
        Sense::hover() | Sense::focusable_noninteractive(),
    );
    label.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &description));
    ui.painter().galley(counted_rect.min, counted, palette.text_secondary);
    focus::ring(ui, &label, counted_rect.expand(2.0));
    change_breakdown::attach(&label, breakdown);
}

/// A hairline track inside the reserved strip: progress reads at a glance without a grey slab.
fn progress_bar(ui: &mut Ui, width: f32, viewed: usize, total: usize) {
    let palette = theme::palette(ui.ctx());
    let (strip, _) = ui.allocate_exact_size(vec2(width, PROGRESS_HEIGHT), Sense::hover());
    let track = Rect::from_center_size(strip.center(), vec2(strip.width(), BAR_HEIGHT));
    ui.painter().rect_filled(track, BAR_HEIGHT / 2.0, palette.border);
    let share = if total == 0 { 0.0 } else { viewed as f32 / total as f32 };
    let share =
        motion::glide(ui.ctx(), egui::Id::unique("files_viewed_progress"), share, motion::MEDIUM);
    if share > 0.0 {
        let filled = Rect::from_min_size(track.min, vec2(track.width() * share, track.height()));
        ui.painter().rect_filled(filled, BAR_HEIGHT / 2.0, palette.success);
    }
}

/// One line of text, cut with "…" at `max_width`.
fn galley(ui: &Ui, text: &str, font: &FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font.clone(), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width);
    ui.painter().layout_job(job)
}
