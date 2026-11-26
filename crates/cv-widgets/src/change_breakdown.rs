//! The breakdown behind a change's "+added −deleted" totals: hovering them (or focusing them with
//! the keyboard) shows how the files and lines split into Code, Tests, Docs, Mockups and Other
//! ([`ChangeGroup`]). A stacked bar of the groups' shares on top, then one row per group in a
//! fixed order (a swatch, the name, its files, its added and deleted lines and a bar of its share
//! of all changed lines) and the total. A group without files stays as a muted row, so the rows
//! never move; lines still being counted read "counting…".
//!
//! The columns are as wide as the total's numbers need (digits are tabular), so no number shifts
//! while the popover is open. The breakdown is built by the caller when its files change
//! ([`tally`]), never per frame.

use cv_base::theme::{self, FileHue, Palette, fonts, style};
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Response, Sense, Ui, pos2, vec2};
use review_core::{ChangeBreakdown, ChangeGroup, GroupChanges};
use review_git::ChangedFile;

use crate::focus;
use crate::numbers::group_digits;

/// Tall enough that a row's text keeps at least `style::SPACE_S` above and below it.
const ROW_HEIGHT: f32 = 36.0;
const SWATCH: f32 = 10.0;
const SWATCH_GAP: f32 = 10.0;
/// Between the name, the files and the line columns.
const COLUMN_GAP: f32 = 16.0;
/// Between the added and the deleted count.
const COUNT_GAP: f32 = 8.0;
const SHARE_WIDTH: f32 = 48.0;
const SHARE_HEIGHT: f32 = 4.0;
const STACK_HEIGHT: f32 = 6.0;
/// Inside the tooltip frame, on top of its own margin.
const PADDING: f32 = style::SPACE_S;

/// The breakdown of `files`: each one in its group, binary files with no lines.
pub fn tally<'a>(files: impl IntoIterator<Item = &'a ChangedFile>) -> ChangeBreakdown {
    let mut breakdown = ChangeBreakdown::default();
    for file in files {
        let lines = if file.is_binary { Some((0, 0)) } else { file.additions.zip(file.deletions) };
        breakdown.add(ChangeGroup::of(file.category, &file.path), lines);
    }
    breakdown
}

/// Shows `breakdown` under `response` after the tooltip delay while hovered, and at once while
/// it has visible keyboard focus.
pub fn attach(response: &Response, breakdown: &ChangeBreakdown) {
    let content = |ui: &mut Ui| show(ui, breakdown);
    if response.has_focus() && focus::is_visible(&response.ctx) {
        response.show_tooltip_ui(content);
    } else {
        egui::Tooltip::for_enabled(response).show(content);
    }
}

/// The swatch and bar color of `group`: a file-icon hue, apart from the added green and the
/// deleted red.
fn hue(palette: &Palette, group: ChangeGroup) -> Color32 {
    palette.file_hue(match group {
        ChangeGroup::Code => FileHue::Blue,
        ChangeGroup::Tests => FileHue::Purple,
        ChangeGroup::Docs => FileHue::Amber,
        ChangeGroup::Mockups => FileHue::Pink,
        ChangeGroup::Other => FileHue::Slate,
    })
}

/// Where a row's columns end, measured once for all rows from the total's numbers.
struct Columns {
    label_left: f32,
    files_right: f32,
    added_right: f32,
    deleted_right: f32,
    share: f32,
    width: f32,
}

impl Columns {
    fn measure(ui: &Ui, total: &GroupChanges) -> Self {
        let text = |text: String, font: FontId| {
            ui.painter().layout_no_wrap(text, font, Color32::WHITE).size().x
        };
        let label = ChangeGroup::ALL
            .iter()
            .map(|group| text(group.label().to_owned(), label_font()))
            .fold(text("Total".to_owned(), total_font()), f32::max);
        let files = text(files_text(total.files.max(10)), count_font());
        let added = text(template('+', total.added), count_font());
        let deleted = text(template('−', total.deleted), count_font());
        let counting = text("counting…".to_owned(), count_font());
        let lines = (added + COUNT_GAP + deleted).max(counting);
        let label_left = SWATCH + SWATCH_GAP;
        let files_right = label_left + label + COLUMN_GAP + files;
        let deleted_right = files_right + COLUMN_GAP + lines;
        Self {
            label_left,
            files_right,
            added_right: deleted_right - deleted - COUNT_GAP,
            deleted_right,
            share: deleted_right + COLUMN_GAP,
            width: deleted_right + COLUMN_GAP + SHARE_WIDTH,
        }
    }
}

fn label_font() -> FontId {
    FontId::proportional(style::BODY_SIZE)
}

fn total_font() -> FontId {
    fonts::semibold_font(style::BODY_SIZE)
}

fn count_font() -> FontId {
    FontId::proportional(style::SMALL_SIZE)
}

/// "+0,000" for 1,270: as wide as the count (digits are tabular).
fn template(sign: char, value: u64) -> String {
    let digits: String = group_digits(value as usize)
        .chars()
        .map(|c| if c.is_ascii_digit() { '0' } else { c })
        .collect();
    format!("{sign}{digits}")
}

fn files_text(files: u32) -> String {
    match files {
        1 => "1 file".to_owned(),
        files => format!("{} files", group_digits(files as usize)),
    }
}

/// The popover's content.
fn show(ui: &mut Ui, breakdown: &ChangeBreakdown) {
    let palette = theme::palette(ui.ctx());
    let total = breakdown.total();
    let columns = Columns::measure(ui, &total);
    ui.spacing_mut().item_spacing.y = 0.0;
    let (stack, _) = ui.allocate_exact_size(
        vec2(columns.width + 2.0 * PADDING, STACK_HEIGHT + PADDING + style::SPACE_XS),
        Sense::hover(),
    );
    let track =
        Rect::from_min_size(stack.min + vec2(PADDING, PADDING), vec2(columns.width, STACK_HEIGHT));
    paint_stack(ui, &palette, track, breakdown);
    for group in ChangeGroup::ALL {
        let row = Row {
            name: group.label(),
            changes: breakdown.group(group),
            swatch: Some(hue(&palette, group)),
            share: Some(breakdown.share(group)),
        };
        row.show(ui, &columns);
    }
    let line_y = ui.cursor().top() + 0.5;
    ui.painter().hline(
        ui.cursor().left() + PADDING..=ui.cursor().left() + PADDING + columns.width,
        line_y,
        egui::Stroke::new(1.0, palette.border),
    );
    Row { name: "Total", changes: total, swatch: None, share: None }.show(ui, &columns);
    ui.add_space(style::SPACE_XS);
}

/// Every group's share of the changed lines, side by side in the groups' order.
fn paint_stack(ui: &Ui, palette: &Palette, track: Rect, breakdown: &ChangeBreakdown) {
    let radius = (STACK_HEIGHT / 2.0) as u8;
    ui.painter().rect_filled(track, radius, palette.border);
    let shown: Vec<(ChangeGroup, f32)> = ChangeGroup::ALL
        .iter()
        .map(|group| (*group, breakdown.share(*group)))
        .filter(|(_, share)| *share > 0.0)
        .collect();
    let mut left = track.left();
    for (index, (group, share)) in shown.iter().enumerate() {
        let right =
            if index + 1 == shown.len() { track.right() } else { left + track.width() * share };
        let is_first = index == 0;
        let is_last = index + 1 == shown.len();
        let (west, east) = (if is_first { radius } else { 0 }, if is_last { radius } else { 0 });
        let corners = CornerRadius { nw: west, sw: west, ne: east, se: east };
        let segment = Rect::from_x_y_ranges(left..=right, track.y_range());
        ui.painter().rect_filled(segment, corners, hue(palette, *group));
        left = right;
    }
}

/// One line of the breakdown: a group's, or the total without swatch and share.
struct Row<'a> {
    name: &'a str,
    changes: GroupChanges,
    swatch: Option<Color32>,
    share: Option<f32>,
}

impl Row<'_> {
    fn show(&self, ui: &mut Ui, columns: &Columns) {
        let palette = theme::palette(ui.ctx());
        let (rect, _) =
            ui.allocate_exact_size(vec2(columns.width + 2.0 * PADDING, ROW_HEIGHT), Sense::hover());
        let left = rect.left() + PADDING;
        let y = rect.center().y;
        let is_empty = self.changes.files == 0;
        let is_total = self.swatch.is_none();
        let painter = ui.painter();
        if let Some(color) = self.swatch {
            let swatch = Rect::from_center_size(pos2(left + SWATCH / 2.0, y), vec2(SWATCH, SWATCH));
            let color = if is_empty { palette.border_strong } else { color };
            painter.rect_filled(swatch, 3, color);
        }
        let (font, name_color) = match (is_total, is_empty) {
            (true, _) => (total_font(), palette.text),
            (false, true) => (label_font(), palette.text_disabled),
            (false, false) => (label_font(), palette.text),
        };
        let name = painter.layout_no_wrap(self.name.to_owned(), font, name_color);
        let name_rect = Rect::from_min_size(
            pos2(left + columns.label_left, y - name.size().y / 2.0),
            name.size(),
        );
        painter.galley(name_rect.min, name, name_color);

        let text = |right: f32, value: String, color: Color32| {
            painter.text(pos2(left + right, y), Align2::RIGHT_CENTER, value, count_font(), color);
        };
        let lines = self.lines_text();
        if is_empty {
            text(columns.files_right, "—".to_owned(), palette.text_disabled);
            text(columns.deleted_right, "—".to_owned(), palette.text_disabled);
        } else {
            text(columns.files_right, files_text(self.changes.files), palette.text_secondary);
            match &lines {
                None => text(columns.deleted_right, "counting…".to_owned(), palette.text_secondary),
                Some((added, deleted)) => {
                    let tint = |value: u64, color: Color32| {
                        if value == 0 { palette.text_disabled } else { color }
                    };
                    text(
                        columns.added_right,
                        added.clone(),
                        tint(self.changes.added, palette.success),
                    );
                    text(
                        columns.deleted_right,
                        deleted.clone(),
                        tint(self.changes.deleted, palette.error),
                    );
                }
            }
        }
        if let (Some(share), Some(color)) = (self.share, self.swatch) {
            let track = Rect::from_min_size(
                pos2(left + columns.share, y - SHARE_HEIGHT / 2.0),
                vec2(SHARE_WIDTH, SHARE_HEIGHT),
            );
            let radius = (SHARE_HEIGHT / 2.0) as u8;
            painter.rect_filled(track, radius, palette.border);
            if share > 0.0 {
                let fill = Rect::from_min_size(track.min, vec2(SHARE_WIDTH * share, SHARE_HEIGHT));
                painter.rect_filled(fill, radius, color);
            }
        }
        self.describe(ui, rect, name_rect, lines.as_ref());
    }

    /// "+1,234" and "−56", or `None` while some of the files are still counted.
    fn lines_text(&self) -> Option<(String, String)> {
        (self.changes.uncounted == 0).then(|| {
            (
                format!("+{}", group_digits(self.changes.added as usize)),
                format!("−{}", group_digits(self.changes.deleted as usize)),
            )
        })
    }

    /// The row in words for assistive technology, and its name as a label of its own, so the
    /// walkthrough can measure where the text sits in the row.
    fn describe(&self, ui: &Ui, rect: Rect, name_rect: Rect, lines: Option<&(String, String)>) {
        let summary = match (self.changes.files, lines) {
            (0, _) => format!("{}: no files", self.name),
            (files, None) => format!("{}: {}, counting lines", self.name, files_text(files)),
            (files, Some((added, deleted))) => {
                format!("{}: {}, {added} {deleted}", self.name, files_text(files))
            }
        };
        let id = ui.scope_id().with(("change_breakdown_row", self.name));
        ui.interact(rect, id, Sense::hover())
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Row, true, &summary));
        ui.interact(name_rect, id.with("name"), Sense::hover())
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, self.name));
    }
}
