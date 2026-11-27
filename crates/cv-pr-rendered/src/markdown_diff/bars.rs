//! What frames the blocks: the bar of folded unchanged blocks, New only's wedge where blocks
//! were removed, the divider between Before and After and their header, the banner over a diff
//! without visible changes and the notice for two empty versions.

use std::ops::Range;

use egui::{
    Align, Align2, CursorIcon, FontId, Layout, Pos2, Rangef, Rect, Sense, Shape, Stroke, Ui,
    UiBuilder, pos2, vec2,
};
use review_core::DiffSide;

use super::align::Pair;
use super::blocks::lines_label;
use super::heights::RowHeights;
use super::lanes::Lanes;
use super::model::{Counts, Model, Row};
use super::{Frame, GUTTER, RIGHT_PADDING, RenderedEvent, STRIPE_WIDTH};
use cv_base::theme::{self, Palette, icons, style};
use cv_code::diff_paint::{self, mix};
use cv_pr_diff::gap_bar;
use cv_pr_diff::notice::{self, Action};
use cv_widgets::numbers::group_digits;
use cv_widgets::{buttons, focus, glyph};

const WEDGE_WIDTH: f32 = 7.0;
const WEDGE_HALF_HEIGHT: f32 = 5.0;
/// The fold bar's icon in the gutter, between `icons::SIZE_SMALL` and `icons::SIZE`.
const FOLD_ICON_SIZE: f32 = 15.0;
/// Removed blocks a New only wedge shows when hovered; the rest are counted.
const TOOLTIP_BLOCKS: usize = 6;
const TOOLTIP_WIDTH: f32 = 520.0;

/// The bar standing in for folded unchanged blocks, with the number of `comments` on them; true
/// when clicked open.
pub(super) fn show_fold(
    ui: &mut Ui,
    rect: Rect,
    frame: &Frame<'_>,
    pairs: Range<usize>,
    comments: usize,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let model = frame.model;
    let hint = if comments > 0 {
        "Show the unchanged blocks and the comments on them"
    } else {
        "Show the unchanged blocks"
    };
    let response = ui
        .interact(rect, frame.id.with(("fold", pairs.start)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(hint);
    let fill = if response.hovered() {
        mix(palette.surface, palette.accent, if palette.is_dark { 0.16 } else { 0.10 })
    } else {
        gap_bar::background(&palette)
    };
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 0, fill);
    let edge = Stroke::new(
        1.0,
        mix(palette.surface, palette.accent, if palette.is_dark { 0.22 } else { 0.16 }),
    );
    painter.hline(rect.x_range(), rect.top() + 0.5, edge);
    painter.hline(rect.x_range(), rect.bottom() - 0.5, edge);
    let label = fold_label(model, &pairs);
    painter.text(
        pos2(rect.left() + GUTTER / 2.0 + 1.0, rect.center().y),
        Align2::CENTER_CENTER,
        icons::ARROWS_OUT_LINE_VERTICAL,
        FontId::proportional(FOLD_ICON_SIZE),
        palette.accent,
    );
    let label_rect = painter.text(
        pos2(rect.left() + GUTTER, rect.center().y),
        Align2::LEFT_CENTER,
        &label,
        FontId::proportional(style::SMALL_SIZE),
        palette.text_secondary,
    );
    if response.hovered() {
        painter.hline(
            label_rect.x_range(),
            label_rect.bottom() + 1.0,
            Stroke::new(1.0, palette.text_secondary),
        );
    }
    let comments = match comments {
        0 => String::new(),
        1 => "1 comment".to_owned(),
        count => format!("{} comments", group_digits(count)),
    };
    if !comments.is_empty() {
        paint_comment_count(
            &painter,
            pos2(label_rect.right(), rect.center().y),
            &comments,
            &palette,
        );
    }
    let label = if comments.is_empty() { label } else { format!("{label} and {comments}") };
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Show {label}"))
    });
    focus::ring(ui, &response, rect.shrink(1.0));
    response.clicked()
}

/// "12 unchanged blocks (lines 40–96)": how many blocks `pairs` folds and where they are in the
/// new version (the old one for blocks only it has), so the user knows what is hidden.
fn fold_label(model: &Model, pairs: &Range<usize>) -> String {
    let lines = |pair: &Pair| {
        pair.new.or(pair.old).map(|unit| {
            let units = if pair.new.is_some() { &model.new } else { &model.old };
            units.units[unit].lines.clone()
        })
    };
    let first = model.pairs.get(pairs.start).and_then(lines);
    let last = model.pairs.get(pairs.end.saturating_sub(1)).and_then(lines);
    let span = match (first, last) {
        (Some(first), Some(last)) => format!(" ({})", lines_label(&(first.start..last.end))),
        _ => String::new(),
    };
    format!("{} unchanged blocks{span}", group_digits(pairs.len()))
}

/// A fold's count of `comments` after its label, which ends at `after` (vertically centered).
fn paint_comment_count(painter: &egui::Painter, after: Pos2, comments: &str, palette: &Palette) {
    let icon_center = pos2(after.x + style::SPACE_M, after.y);
    painter.text(
        icon_center,
        Align2::CENTER_CENTER,
        icons::CHAT_CIRCLE_TEXT,
        icons::font(icons::SIZE_SMALL, icons::Weight::Bold),
        palette.accent,
    );
    let left = icon_center.x + icons::SIZE_SMALL / 2.0 + style::SPACE_XS;
    painter.text(
        pos2(left, after.y),
        Align2::LEFT_CENTER,
        comments,
        FontId::proportional(style::SMALL_SIZE),
        palette.accent_text,
    );
}

/// Side by side: the divider between Before and After over the `shown` rows of `viewport`,
/// broken by the fold bars and the cards' bands that span both lanes.
pub(super) fn paint_column_divider(
    ui: &Ui,
    frame: &Frame<'_>,
    rows: &[Row],
    heights: &RowHeights,
    shown: &[usize],
    viewport: Rect,
    palette: &Palette,
) {
    let x = frame.origin.x + frame.lanes.first.max;
    let paint = |top: f32, bottom: f32| {
        let segment = Rangef::new(frame.origin.y + top, frame.origin.y + bottom);
        diff_paint::paint_divider(ui.painter(), x, Rect::from_x_y_ranges(x..=x, segment), palette);
    };
    let mut top = viewport.top();
    for &index in shown {
        let is_band = match rows[index] {
            Row::Folded { .. } => true,
            Row::Thread(_) => heights.height(index) > 0.0,
            Row::Blocks { .. } | Row::Removal { .. } => false,
        };
        if is_band {
            paint(top, heights.top(index));
            top = heights.top(index) + heights.height(index);
        }
    }
    paint(top, viewport.bottom());
}

/// New only: a red wedge where blocks were removed, at `y`; hovering it shows them, a click
/// (true) shows them in place, side by side.
pub(super) fn show_removal(
    ui: &mut Ui,
    frame: &Frame<'_>,
    y: f32,
    pairs: Range<usize>,
    palette: &Palette,
) -> bool {
    let model = frame.model;
    let lane = frame.lanes.column(DiffSide::New);
    let left = frame.origin.x + lane.min;
    let hit = Rect::from_center_size(
        pos2(left + GUTTER / 2.0, y),
        vec2(GUTTER, WEDGE_HALF_HEIGHT * 2.0 + 8.0),
    );
    let id = frame.id.with(("removal", pairs.start));
    let removed: Vec<usize> = model.pairs[pairs].iter().filter_map(|pair| pair.old).collect();
    let label = match removed.len() {
        1 => "1 removed block".to_owned(),
        count => format!("{} removed blocks", group_digits(count)),
    };
    let response = ui.interact(hit, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            true,
            format!("Show the {label} side by side"),
        )
    });
    focus::ring(ui, &response, hit);
    let grow = if response.hovered() { 1.5 } else { 0.0 };
    if response.hovered() {
        let text_left = left + GUTTER;
        ui.painter().hline(
            text_left..=frame.origin.x + lane.max - RIGHT_PADDING,
            y,
            Stroke::new(1.0, palette.error.gamma_multiply(0.6)),
        );
    }
    let tip = left + STRIPE_WIDTH;
    let points = vec![
        pos2(tip, y - WEDGE_HALF_HEIGHT - grow),
        pos2(tip + WEDGE_WIDTH + grow, y),
        pos2(tip, y + WEDGE_HALF_HEIGHT + grow),
    ];
    ui.painter().add(Shape::convex_polygon(points, palette.error, Stroke::NONE));
    let clicked = response.clicked();
    response.on_hover_ui(|ui| {
        ui.set_max_width(TOOLTIP_WIDTH);
        ui.label(egui::RichText::new(&label).size(style::SMALL_SIZE).color(palette.error));
        ui.add_space(style::SPACE_XS);
        for unit in removed.iter().take(TOOLTIP_BLOCKS) {
            model.old.show(
                ui,
                frame.id.with(("removed", *unit)),
                *unit,
                model.options(frame.options, DiffSide::Old, None),
                [],
            );
            ui.add_space(style::SPACE_S);
        }
        let more = removed.len().saturating_sub(TOOLTIP_BLOCKS);
        let hint = if more > 0 {
            format!("and {} more. Click to show them all side by side.", group_digits(more))
        } else {
            "Click to show them side by side.".to_owned()
        };
        ui.label(egui::RichText::new(hint).size(style::SMALL_SIZE).color(palette.text_secondary));
    });
    clicked
}

/// "Before" and "After" over the lanes, each with what changed on its side.
pub(super) fn paint_header(ui: &Ui, rect: Rect, lanes: &Lanes, model: &Model, palette: &Palette) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0, palette.surface);
    painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, palette.border));
    let Counts { added, removed, edited } = model.counts;
    let summary =
        |count: usize, what: &str| (count > 0).then(|| format!("{} {what}", group_digits(count)));
    let edited = summary(edited, "edited");
    let sides = [
        (lanes.column(DiffSide::Old), "Before", [summary(removed, "removed"), edited.clone()]),
        (lanes.column(DiffSide::New), "After", [summary(added, "added"), edited]),
    ];
    for (lane, title, parts) in sides {
        let left = rect.left() + lane.min + GUTTER;
        let title_rect = painter.text(
            pos2(left, rect.center().y),
            Align2::LEFT_CENTER,
            title,
            theme::fonts::semibold_font(style::BODY_SIZE),
            palette.text,
        );
        let detail: Vec<String> = parts.into_iter().flatten().collect();
        if !detail.is_empty() {
            let clip = Rect::from_x_y_ranges(
                title_rect.right()..=rect.left() + lane.max - RIGHT_PADDING,
                rect.y_range(),
            );
            painter.with_clip_rect(clip).text(
                pos2(title_rect.right() + style::SPACE_S, rect.center().y),
                Align2::LEFT_CENTER,
                detail.join(", "),
                FontId::proportional(style::SMALL_SIZE),
                palette.text_secondary,
            );
        }
    }
}

/// Over a rendered diff without visible changes: why, and the text diff that shows them.
pub(super) fn show_banner(ui: &mut Ui, rect: Rect, palette: &Palette) -> Option<RenderedEvent> {
    ui.painter().rect_filled(
        rect,
        0,
        mix(palette.surface, palette.accent, if palette.is_dark { 0.10 } else { 0.06 }),
    );
    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, palette.border));
    let inner = rect.shrink2(vec2(style::SPACE_M, 0.0));
    let clicked = ui
        .scope_builder(UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)), |ui| {
            glyph::decorative(ui, egui::RichText::new(icons::INFO).font(icons::rest_font(palette, icons::SIZE)).color(palette.accent));
            ui.label(egui::RichText::new("The rendered text is the same in both versions: the changes are in whitespace or markup that does not show.").color(palette.text));
            buttons::secondary_icon(ui, icons::FILE_TEXT, "Show text diff").clicked()
        })
        .inner;
    clicked.then_some(RenderedEvent::ShowSource { side: DiffSide::New, lines: 1..2 })
}

/// Neither version renders anything: say so and offer the text diff.
pub(super) fn show_empty(ui: &mut Ui, palette: &Palette) -> Option<RenderedEvent> {
    let actions = [Action { icon: icons::FILE_TEXT, label: "Show text diff", is_primary: true }];
    let detail = "Both versions are empty or hold only comments, which Markdown does not show.";
    notice::show(
        ui,
        icons::FILE_TEXT,
        palette.text_secondary,
        "Nothing to render",
        detail,
        &actions,
    )
    .map(|_| RenderedEvent::ShowSource { side: DiffSide::New, lines: 1..2 })
}
