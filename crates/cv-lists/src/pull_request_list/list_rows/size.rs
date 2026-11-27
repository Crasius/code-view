//! The size column of a row: the counted lines over a bar and the changed files, placeholders
//! while they are counted, or what is known with "Count lines".

use cv_base::theme::{self, Palette, icons, motion, style};
use cv_services::diff_stats::{self, Shown};
use cv_widgets::numbers::{compact_count, digit_template, group_digits};
use cv_widgets::{row, skeleton};
use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Painter, Rect, Sense, Stroke, Ui, pos2, vec2,
};
use review_core::{DiffStat, LineCounts};

use super::LINE_OFFSET;

/// The bar: a track whose fill grows from the right with the changed lines.
const BAR_WIDTH: f32 = 52.0;
const BAR_HEIGHT: f32 = 6.0;
const BAR_GAP: f32 = 6.0;
/// The changed files, right-aligned after the bar.
const FILE_COUNT_WIDTH: f32 = 38.0;
const _: () = assert!(
    FILE_COUNT_WIDTH + BAR_GAP + BAR_WIDTH <= super::SIZE_WIDTH,
    "the bar and the file count fit the size column"
);
/// Between the added and the deleted count, and between the file icon and its count.
const COUNT_GAP: f32 = 6.0;
const FILE_ICON_GAP: f32 = 3.0;
/// The file count and "Count lines", a step below the meta line.
const DETAIL_SIZE: f32 = 11.0;
const DETAIL_ICON_SIZE: f32 = 12.0;

/// Where the size column's parts sit. The counts end at fixed places: the deleted one at the
/// column's right edge, in a slot as wide as the widest it shows ("−0,000"), the added one before
/// that slot; so both line up from row to row and across cards, whatever the numbers.
struct SizeLayout {
    added_right: f32,
    deleted_right: f32,
    track: Rect,
    /// The file icon and count, right-aligned in it.
    files: Rect,
    title_y: f32,
    meta_y: f32,
}

impl SizeLayout {
    fn new(column: Rect, deleted_slot: f32) -> Self {
        let right = column.right();
        let (title_y, meta_y) = (column.center().y - LINE_OFFSET, column.center().y + LINE_OFFSET);
        let files = Rect::from_x_y_ranges(
            (right - FILE_COUNT_WIDTH)..=right,
            (meta_y - 7.0)..=(meta_y + 7.0),
        );
        let track = Rect::from_min_size(
            pos2(files.left() - BAR_GAP - BAR_WIDTH, meta_y - BAR_HEIGHT / 2.0),
            vec2(BAR_WIDTH, BAR_HEIGHT),
        );
        Self {
            added_right: right - deleted_slot - COUNT_GAP,
            deleted_right: right,
            track,
            files,
            title_y,
            meta_y,
        }
    }

    fn measure(painter: &Painter, column: Rect) -> Self {
        let slot = painter
            .layout_no_wrap(digit_template('−', 9_999), count_font(), Color32::WHITE)
            .size()
            .x;
        Self::new(column, slot)
    }
}

fn count_font() -> FontId {
    theme::fonts::semibold_font(style::SMALL_SIZE)
}

/// The size column's placeholders, in the places of the counts, the bar and the file count.
pub fn paint_placeholder(ui: &Ui, column: Rect) {
    let layout = SizeLayout::measure(ui.painter(), column);
    let block = |right: f32, y: f32, size: egui::Vec2| {
        Rect::from_min_size(pos2(right - size.x, y - size.y / 2.0), size)
    };
    skeleton::paint_bar(ui, block(layout.added_right, layout.title_y, vec2(34.0, 10.0)));
    skeleton::paint_bar(ui, block(layout.deleted_right, layout.title_y, vec2(26.0, 10.0)));
    skeleton::paint_bar(ui, layout.track);
    skeleton::paint_bar(ui, block(layout.files.right(), layout.meta_y, vec2(24.0, 8.0)));
}

/// The size column: counted lines over the bar and the changed files; placeholders while they
/// are counted; or a dash, the files where known and "Count lines". True when that was clicked.
pub fn paint(
    ui: &mut Ui,
    painter: &Painter,
    id: Id,
    column: Rect,
    shown: &Shown,
    number: &str,
) -> bool {
    let layout = SizeLayout::measure(painter, column);
    let response = ui.interact(column, id, Sense::hover());
    // Numbers that replace the placeholders fade in; numbers there at first sight show at once.
    let mut painter = painter.clone();
    painter.multiply_opacity(motion::reveal(ui.ctx(), id, !matches!(shown, Shown::Counting)));
    match shown {
        Shown::Counting => {
            skeleton::announce(&response, &format!("Counting the lines of {number}"));
            // Still: counting may wait for the window to be active again, and must not keep it busy.
            skeleton::still(ui, |ui| paint_placeholder(ui, column));
            false
        }
        Shown::Counted(stat) => {
            paint_counted(ui, &painter, &layout, response, stat);
            false
        }
        Shown::Uncounted { files, why } => {
            let palette = theme::palette(ui.ctx());
            painter.text(
                pos2(layout.deleted_right, layout.title_y),
                Align2::RIGHT_CENTER,
                "—",
                count_font(),
                palette.text_secondary,
            );
            if let Some(files) = *files {
                paint_file_count(&painter, &palette, layout.added_right, layout.title_y, files);
            }
            let detail = match files {
                Some(files) => format!("{}\n{why}", files_changed(*files)),
                None => why.clone(),
            };
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Label,
                    true,
                    format!("Lines of {number} not counted"),
                )
            });
            response.on_hover_text(&detail);
            paint_count_link(
                ui,
                &painter,
                id.with("count"),
                column.right(),
                layout.meta_y,
                number,
                &detail,
            )
        }
    }
}

/// The counted lines over the bar, the changed files under them; the split in words on hover.
fn paint_counted(
    ui: &Ui,
    painter: &Painter,
    layout: &SizeLayout,
    response: egui::Response,
    stat: &DiffStat,
) {
    let palette = theme::palette(ui.ctx());
    let lines = stat.lines.unwrap_or_default();
    let text = |right: f32, value: String, color: Color32| {
        painter.text(pos2(right, layout.title_y), Align2::RIGHT_CENTER, value, count_font(), color);
    };
    text(
        layout.added_right,
        format!("+{}", compact_count(lines.added)),
        count_color(lines.added, palette.success, &palette),
    );
    text(
        layout.deleted_right,
        format!("−{}", compact_count(lines.deleted)),
        count_color(lines.deleted, palette.error, &palette),
    );
    paint_bar(painter, &palette, layout.track, lines);
    if let Some(files) = stat.files {
        paint_file_count(painter, &palette, layout.files.right(), layout.meta_y, files);
    }
    let (summary, split) = size_summary(lines, stat.files);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{summary}: {split}"))
    });
    response.on_hover_ui(|ui| {
        ui.label(&summary);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let added = format!("+{} added", group_digits(lines.added as usize));
            ui.label(egui::RichText::new(added).color(palette.success));
            ui.label(", ");
            let deleted = format!("−{} deleted", group_digits(lines.deleted as usize));
            ui.label(egui::RichText::new(deleted).color(palette.error));
        });
    });
}

/// The bar under the counts: a neutral track, filled from the right as far as
/// [`diff_stats::bar_fraction`] says, green for the added and red for the deleted share.
fn paint_bar(painter: &Painter, palette: &Palette, track: Rect, lines: LineCounts) {
    let radius = (BAR_HEIGHT / 2.0) as u8;
    painter.rect_filled(track, radius, palette.border);
    let total = lines.total();
    let length = BAR_WIDTH * diff_stats::bar_fraction(total);
    if total == 0 || length <= 0.0 {
        return;
    }
    let left = track.right() - length;
    let split = left + length * (lines.added as f32 / total as f32);
    let has_added = lines.added > 0;
    let has_deleted = lines.deleted > 0;
    if has_added {
        let inner = if has_deleted { 0 } else { radius };
        let corners = CornerRadius { nw: radius, sw: radius, ne: inner, se: inner };
        painter.rect_filled(
            Rect::from_x_y_ranges(left..=split, track.y_range()),
            corners,
            palette.success,
        );
    }
    if has_deleted {
        let inner = if has_added { 0 } else { radius };
        let corners = CornerRadius { nw: inner, sw: inner, ne: radius, se: radius };
        painter.rect_filled(
            Rect::from_x_y_ranges(split..=track.right(), track.y_range()),
            corners,
            palette.error,
        );
    }
}

/// A file icon and the count after it, ending at `right`.
fn paint_file_count(painter: &Painter, palette: &Palette, right: f32, y: f32, files: u64) {
    let count = painter.text(
        pos2(right, y),
        Align2::RIGHT_CENTER,
        compact_count(files),
        FontId::proportional(DETAIL_SIZE),
        palette.text_secondary,
    );
    let icon = icons::rest_font(palette, DETAIL_ICON_SIZE);
    painter.text(
        pos2(count.left() - FILE_ICON_GAP, y),
        Align2::RIGHT_CENTER,
        icons::FILE,
        icon,
        palette.text_secondary,
    );
}

/// "Count lines" with its refresh icon, a link ending at `right`. True when clicked.
fn paint_count_link(
    ui: &mut Ui,
    painter: &Painter,
    id: Id,
    right: f32,
    y: f32,
    number: &str,
    tooltip: &str,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let galley = painter.layout_no_wrap(
        "Count lines".to_owned(),
        FontId::proportional(DETAIL_SIZE),
        palette.accent_text,
    );
    let text = Rect::from_min_size(
        pos2(right - galley.size().x, y - galley.size().y / 2.0),
        galley.size(),
    );
    let icon_right = text.left() - FILE_ICON_GAP;
    let hit = Rect::from_x_y_ranges((icon_right - DETAIL_ICON_SIZE)..=right, (y - 9.0)..=(y + 9.0))
        .expand2(vec2(2.0, 0.0));
    let response = ui.interact(hit, id, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Count lines of {number}"))
    });
    let color =
        motion::mix(palette.accent_text, palette.accent_hover, row::hover_amount(ui, &response));
    painter.text(
        pos2(icon_right, y),
        Align2::RIGHT_CENTER,
        icons::ARROW_CLOCKWISE,
        icons::rest_font(&palette, DETAIL_ICON_SIZE),
        color,
    );
    painter.galley(text.min, galley, color);
    if response.hovered() {
        painter.hline(text.x_range(), text.bottom(), Stroke::new(1.0, color));
    }
    cv_widgets::focus::ring(ui, &response, hit);
    response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tooltip).clicked()
}

/// "1,552 lines changed in 18 files" and "1,240 added, 312 deleted" (for screen readers).
fn size_summary(lines: LineCounts, files: Option<u64>) -> (String, String) {
    let changed = match lines.total() {
        1 => "1 line changed".to_owned(),
        total => format!("{} lines changed", group_digits(total as usize)),
    };
    let summary = match files {
        Some(1) => format!("{changed} in 1 file"),
        Some(files) => format!("{changed} in {} files", group_digits(files as usize)),
        None => changed,
    };
    let split = format!(
        "{} added, {} deleted",
        group_digits(lines.added as usize),
        group_digits(lines.deleted as usize)
    );
    (summary, split)
}

fn files_changed(files: u64) -> String {
    if files == 1 {
        "1 file changed".to_owned()
    } else {
        format!("{} files changed", group_digits(files as usize))
    }
}

/// A count in its tint; a zero carries no news, so it steps back (as in the file tree).
pub fn count_color(value: u64, tint: Color32, palette: &Palette) -> Color32 {
    if value == 0 { palette.text_disabled } else { tint }
}
