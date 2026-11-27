//! The 2 px edge on a row's inner left that says how current the row is, so a long list shows at a
//! glance where a refresh has not reached yet. A fresh row has none; otherwise, from the strongest:
//! - failed (warning): the row's checks and votes, or the card's listing, could not be refreshed;
//!   the edge's hit area retries;
//! - updating (a short light travels down a faint track): the row's checks and votes are on their
//!   way ([`super::super::statuses`]);
//! - held (dotted, as waiting): the row's checks and votes are asked for, but the host's rate-limit
//!   pause holds the request back ([`super::super::pauses`]). The light is for work under way; a
//!   pause lasts up to an hour, and the light would draw every frame of it;
//! - waiting (dotted, quiet): the card's listing is refreshing, so the row shows the last known state;
//! - changed (accent): the host changed the row since the user last saw it
//!   ([`super::super::changes`]).
//!
//! Color and pattern alone say nothing to a screen reader, so the edge's hit area carries the state
//! in words, as its tooltip and accessible name.

use cv_base::theme::{self, motion, style};
use egui::{Color32, Id, Rect, Sense, Shape, Ui, pos2};
use review_core::ProviderKind;

use super::super::pauses::Pauses;
use super::super::queries::ConnectionResult;
use super::super::statuses::{RowStatuses, StatusRow};

/// The edge's width, and how far its hit area reaches into the row.
const WIDTH: f32 = 2.0;
const HIT_WIDTH: f32 = 10.0;
/// The travelling light: its length and the seconds of one pass.
const LIGHT: f32 = 24.0;
const PASS: f64 = 1.4;
/// The waiting edge's dots: their gap and their inset from the row's top and bottom.
const DOT_GAP: f32 = 4.0;
const DOT_INSET: f32 = 6.0;

/// How current one row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge<'a> {
    Fresh,
    Changed,
    Waiting,
    /// Held by the rate-limit pause of the row's platform.
    Held(ProviderKind),
    Updating,
    Failed(&'a str),
}

/// The edge of `row` in `result`'s card, while `pauses` hold requests back.
pub fn of<'a>(
    result: &'a ConnectionResult,
    statuses: &'a RowStatuses,
    pauses: &Pauses,
    row: &StatusRow,
) -> Edge<'a> {
    if let Some(message) = statuses.failure(row).or_else(|| result.load.refresh_error()) {
        Edge::Failed(message)
    } else if statuses.is_held(row, pauses) {
        Edge::Held(row.kind)
    } else if statuses.is_running(row) {
        Edge::Updating
    } else if result.load.is_refreshing() {
        Edge::Waiting
    } else if result.changes.is_changed(&row.repository_key, row.number) {
        Edge::Changed
    } else {
        Edge::Fresh
    }
}

/// Where the edge goes: the row, its id, whether it is the card's last (its corner is round), its
/// card's fade and its number in words.
pub struct Place<'a> {
    pub rect: Rect,
    pub id: Id,
    pub is_last: bool,
    pub opacity: f32,
    pub number: &'a str,
}

/// Paints `edge` on the row's inner left; true when its failure's Retry was clicked.
pub fn paint(ui: &Ui, place: &Place<'_>, edge: Edge<'_>) -> bool {
    if edge == Edge::Fresh {
        return false;
    }
    let palette = theme::palette(ui.ctx());
    let rect = place.rect;
    // Inside the card's 1 px border, and above the round corner of the card's last row.
    let bottom =
        if place.is_last { rect.bottom() - f32::from(style::RADIUS_CARD) } else { rect.bottom() };
    let strip =
        Rect::from_x_y_ranges(rect.left() + 1.0..=rect.left() + 1.0 + WIDTH, rect.top()..=bottom);
    let mut painter = ui.painter_at(strip);
    painter.multiply_opacity(place.opacity);
    let number = place.number;
    let (words, sense) = match edge {
        Edge::Fresh => return false,
        Edge::Changed => {
            painter.rect_filled(strip, 0.0, palette.accent);
            (
                format!("{number} changed since you last looked: new commits, votes or state"),
                Sense::hover(),
            )
        }
        Edge::Waiting => {
            paint_dots(&painter, strip, palette.border_strong);
            (
                format!("{number} waits for the list's refresh and shows the last known state"),
                Sense::hover(),
            )
        }
        Edge::Held(kind) => {
            paint_dots(&painter, strip, palette.border_strong);
            (
                format!(
                    "{number} waits for {} to accept requests again; its checks and approvals \
                     update on their own",
                    kind.label()
                ),
                Sense::hover(),
            )
        }
        Edge::Updating => {
            painter.rect_filled(strip, 0.0, palette.accent.gamma_multiply(0.18));
            paint_light(ui, &painter, strip, palette.accent.gamma_multiply(0.75));
            (format!("Updating the checks and approvals of {number}"), Sense::hover())
        }
        Edge::Failed(message) => {
            painter.rect_filled(strip, 0.0, palette.warning);
            (format!("{number} was not updated: {message}\nClick to try again"), Sense::click())
        }
    };
    let hit = Rect::from_x_y_ranges(rect.left()..=rect.left() + HIT_WIDTH, rect.top()..=bottom);
    let response = ui.interact(hit, place.id.with("edge"), sense);
    let role = if sense == Sense::click() { egui::Role::Button } else { egui::Role::Label };
    response.widget_info(|| egui::WidgetInfo::labeled(role, true, &words));
    let response = if sense == Sense::click() {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };
    response.on_hover_text(words).clicked()
}

/// The waiting edge: round dots down the strip, clear of the row's top and bottom.
fn paint_dots(painter: &egui::Painter, strip: Rect, color: Color32) {
    let radius = WIDTH / 2.0;
    let mut y = strip.top() + DOT_INSET + radius;
    while y <= strip.bottom() - DOT_INSET - radius {
        painter.circle_filled(pos2(strip.center().x, y), radius, color);
        y += WIDTH + DOT_GAP;
    }
}

/// The updating edge's light, one pass every [`PASS`] seconds from above the row to below it;
/// under reduced motion it rests in the middle.
fn paint_light(ui: &Ui, painter: &egui::Painter, strip: Rect, color: Color32) {
    let travel = strip.height() + LIGHT;
    let top = if motion::is_reduced(ui.ctx()) {
        strip.center().y - LIGHT / 2.0
    } else {
        ui.ctx().request_repaint();
        let phase = (ui.input(|input| input.time) % PASS / PASS) as f32;
        strip.top() - LIGHT + travel * motion::ease_in_out_cubic(phase)
    };
    let light = Rect::from_x_y_ranges(strip.x_range(), top..=top + LIGHT);
    painter.add(Shape::rect_filled(light.intersect(strip), 0.0, color));
}
