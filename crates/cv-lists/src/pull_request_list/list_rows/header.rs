//! A card's header: chevron (overview only), the card's mark and name, the row count and, when
//! the last refresh failed, a retry. The Favorites card's context menu hides its lane.

use cv_base::theme::{self, Palette, icons, style};
use cv_widgets::{buttons, elide, provider_icon, row, star};
use egui::{Align2, Color32, CornerRadius, FontId, Painter, Rect, Sense, Ui, pos2, vec2};

use super::{CHIP_HEIGHT, Group, Header, ListAction, ROW_PADDING, chip_width, paint_chip};

/// The Favorites card's menu entry; the overview's star tool and Settings bring the lane back.
const HIDE_LANE: &str = "Hide the Favorites lane";
/// The size of a header's caret glyph.
const CARET_SIZE: f32 = 14.0;
/// From the caret's left edge to the mark after it.
const CARET_TO_MARK: f32 = 24.0;
/// From a header's mark to its title.
pub(super) const MARK_TO_TITLE: f32 = 26.0;

/// In the overview the whole header toggles the card (click, Enter, Space). A connection's card
/// names the connection and its platform; the Favorites card leads with a filled star and counts
/// its repositories.
pub(crate) fn paint(
    ui: &mut Ui,
    rect: Rect,
    group: &Group<'_>,
    is_collapsible: bool,
) -> Option<ListAction> {
    let palette = theme::palette(ui.ctx());
    let key = group.key();
    let (name, detail, refresh_error) = text(group);
    let action = if is_collapsible { toggle(ui, rect, group, &name, &detail) } else { None };
    let painter = ui.painter_at(rect);
    let mut left = rect.left() + ROW_PADDING;
    if is_collapsible {
        left = paint_caret(&painter, &palette, left, rect.center().y, group.opening > 0.5);
    }
    let mark = Rect::from_center_size(
        pos2(left + icons::SIZE / 2.0, rect.center().y),
        vec2(icons::SIZE, icons::SIZE),
    );
    match group.header {
        Header::Connection(result) => {
            provider_icon::paint(&painter, &palette, mark.center(), icons::SIZE, result.kind);
            provider_icon::interact(
                ui,
                mark,
                ui.scope_id().with(("group_platform", key)),
                result.kind,
                &result.base_url,
            );
        }
        Header::Favorites { .. } => {
            star::paint(&painter, &palette, mark.center(), icons::SIZE, true, 0.0);
        }
    }
    let mut right = paint_count(&painter, &palette, rect, group.count);
    let mut retry = None;
    if let Some(message) = refresh_error {
        let failed = Failed { name: &name, key, message };
        let (left_end, picked) = refresh_failed(ui, &painter, rect, right, &failed);
        right = left_end - style::SPACE_S;
        retry = picked;
    }
    let title_left = left + MARK_TO_TITLE;
    paint_title(ui, &painter, title_left..=right, rect.center().y, (&name, &detail), key);
    retry.or(action)
}

/// The caret of a header that opens and closes, at `left`; returns the left of the mark after
/// it. The card headers and the section headers share it, so their marks and titles line up.
pub(super) fn paint_caret(
    painter: &Painter,
    palette: &Palette,
    left: f32,
    center_y: f32,
    is_open: bool,
) -> f32 {
    let caret = if is_open { icons::CARET_DOWN } else { icons::CARET_RIGHT };
    painter.text(
        pos2(left + CARET_SIZE / 2.0, center_y),
        Align2::CENTER_CENTER,
        caret,
        FontId::proportional(CARET_SIZE),
        palette.text_secondary,
    );
    left + CARET_TO_MARK
}

/// The card's name and the detail after it, between `room`'s ends: a wide font or a narrow
/// window drops the detail first, then cuts the name, which reads in full on hover.
fn paint_title(
    ui: &Ui,
    painter: &Painter,
    room: std::ops::RangeInclusive<f32>,
    center_y: f32,
    (name, detail): (&str, &str),
    key: &str,
) {
    let palette = theme::palette(ui.ctx());
    let (left, right) = (*room.start(), *room.end());
    let font = theme::fonts::semibold_font(style::BODY_SIZE);
    let title = elide::line(painter, name, font, palette.text, right - left);
    let title_rect = Rect::from_min_size(pos2(left, center_y - title.size().y / 2.0), title.size());
    let is_cut = title.elided;
    painter.galley(title_rect.min, title, palette.text);
    if is_cut {
        let id = ui.scope_id().with(("group_title", key));
        ui.interact(title_rect, id, Sense::hover()).on_hover_text(name);
        return;
    }
    let detail = painter.layout_no_wrap(
        detail.to_owned(),
        FontId::proportional(style::SMALL_SIZE),
        palette.text_secondary,
    );
    let detail_left = title_rect.right() + style::SPACE_S;
    if detail_left + detail.size().x <= right {
        let top = center_y - detail.size().y / 2.0;
        painter.galley(pos2(detail_left, top), detail, palette.text_secondary);
    }
}

/// The card's name, the detail after it, and why its last refresh failed.
fn text<'a>(group: &Group<'a>) -> (String, String, Option<&'a str>) {
    match &group.header {
        Header::Connection(result) => (
            result.connection_name.clone(),
            result.kind.label().to_owned(),
            result.load.refresh_error(),
        ),
        Header::Favorites { repositories, hidden, refresh_error } => {
            let detail = match (repositories, hidden) {
                (0, 0) => "Starred repositories from every connection".to_owned(),
                (listed, 0) => repositories_text(*listed),
                (listed, hidden) => format!("{}, {hidden} hidden", repositories_text(*listed)),
            };
            ("Favorites".to_owned(), detail, *refresh_error)
        }
    }
}

/// "1 repository", "3 repositories".
pub(crate) fn repositories_text(count: usize) -> String {
    if count == 1 { "1 repository".to_owned() } else { format!("{count} repositories") }
}

/// The whole header as the button that collapses or expands the card.
fn toggle(ui: &Ui, rect: Rect, group: &Group<'_>, name: &str, detail: &str) -> Option<ListAction> {
    let is_open = group.opening > 0.5;
    let response =
        ui.interact(rect, ui.scope_id().with(("group_header", group.key())), Sense::click());
    let count = group.count.map_or_else(String::new, |count| format!(", {count} pull requests"));
    let state = if is_open { "expanded" } else { "collapsed" };
    // The Favorites card's detail (how many repositories) is part of what it is; a connection's
    // platform is in its mark.
    let detail = if matches!(group.header, Header::Favorites { .. }) {
        format!(" ({detail})")
    } else {
        String::new()
    };
    let accessible = format!("{name}{detail}{count}, {state}");
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &accessible));
    let fill = row::state_fill(ui, &response, Color32::TRANSPARENT);
    if fill != Color32::TRANSPARENT {
        let bottom = if group.opening > 0.0 { 0 } else { style::RADIUS_CARD };
        let corner =
            CornerRadius { nw: style::RADIUS_CARD, ne: style::RADIUS_CARD, sw: bottom, se: bottom };
        ui.painter().rect_filled(rect.shrink(1.0), corner, fill);
    }
    cv_widgets::focus::ring(ui, &response, rect);
    let tooltip = if is_open { "Collapse" } else { "Expand" };
    let response = response.on_hover_text(tooltip);
    if matches!(group.header, Header::Favorites { .. }) && hides_lane(&response) {
        return Some(ListAction::HideFavoritesLane);
    }
    if !response.clicked() {
        return None;
    }
    Some(match group.header {
        Header::Connection(result) => {
            ListAction::ToggleConnectionCard(result.connection_id.clone())
        }
        Header::Favorites { .. } => ListAction::ToggleFavoritesCard,
    })
}

/// The Favorites header's context menu; true when "Hide the Favorites lane" was picked.
fn hides_lane(response: &egui::Response) -> bool {
    let mut hides = false;
    response.context_menu(|ui| {
        if buttons::menu_item(ui, icons::EYE_SLASH, HIDE_LANE, None, None).clicked() {
            hides = true;
            ui.close();
        }
    });
    hides
}

/// The row count in a chip at the header's right end; returns where the room before it ends.
fn paint_count(painter: &Painter, palette: &Palette, rect: Rect, count: Option<usize>) -> f32 {
    let right = rect.right() - ROW_PADDING;
    let Some(count) = count else {
        return right;
    };
    let galley = painter.layout_no_wrap(
        count.to_string(),
        FontId::proportional(style::SMALL_SIZE),
        palette.text_secondary,
    );
    let width = chip_width(&galley);
    let chip = Rect::from_min_size(
        pos2(right - width, rect.center().y - CHIP_HEIGHT / 2.0),
        vec2(width, CHIP_HEIGHT),
    );
    paint_chip(painter, chip, galley, (palette.surface_subtle, palette.text_secondary), None);
    chip.left() - style::SPACE_S
}

/// A card whose last refresh failed: its name, its key and why.
struct Failed<'a> {
    name: &'a str,
    key: &'a str,
    message: &'a str,
}

/// "Not updated", ending at `right`: the reason on hover, a retry on click. Returns where it
/// starts and the retry.
fn refresh_failed(
    ui: &Ui,
    painter: &Painter,
    rect: Rect,
    right: f32,
    failed: &Failed<'_>,
) -> (f32, Option<ListAction>) {
    let palette = theme::palette(ui.ctx());
    let galley = painter.layout_no_wrap(
        format!("{}  Not updated", icons::WARNING),
        FontId::proportional(style::SMALL_SIZE),
        palette.warning,
    );
    let target = Rect::from_min_size(
        pos2(right - galley.size().x, rect.center().y - galley.size().y / 2.0),
        galley.size(),
    );
    let response = ui.interact(
        target.expand(4.0),
        ui.scope_id().with(("refresh_failed", failed.key)),
        Sense::click(),
    );
    response.widget_info(|| {
        let label = format!("{} not updated, retry", failed.name);
        egui::WidgetInfo::labeled(egui::Role::Button, true, label)
    });
    painter.galley(target.min, galley, palette.warning);
    cv_widgets::focus::ring(ui, &response, target.expand(4.0));
    let retry = response
        .on_hover_text(format!("{}\nClick to retry.", failed.message))
        .clicked()
        .then_some(ListAction::Retry);
    (target.left(), retry)
}
