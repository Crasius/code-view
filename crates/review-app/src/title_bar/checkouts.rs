//! The title bar's chips of running checkouts: "Checkout #12 · Fetch #12 from the mirror", one per
//! checkout, right-aligned before the window's own parts. A click opens the pull request with the
//! checkout's progress, so a checkout whose popover was closed stays in reach from any page.

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};

use cv_base::theme::{self, icons, style};
use cv_pr_page::checkout;
use cv_services::Services;
use cv_widgets::{focus, row};

use super::JumpAction;

const CHIP_HEIGHT: f32 = 26.0;
const GAP: f32 = 8.0;

/// The chips between `after` and `before`, right-aligned; returns the rect they cover and the
/// pull request a click opens.
pub(super) fn show(
    ui: &mut Ui,
    bar: Rect,
    after: f32,
    before: f32,
    services: &Services,
) -> (Rect, Option<JumpAction>) {
    let palette = theme::palette(ui.ctx());
    let mut right = before;
    let mut covered = Rect::NOTHING;
    let mut action = None;
    let mut running: Vec<_> = services.checkouts.running().collect();
    running.sort_by_key(|(key, _)| key.number);
    for (key, job) in running {
        let step = job.progress().current().map(|row| row.label.clone()).unwrap_or_default();
        let text = format!("Checkout #{} · {step}", key.number);
        let galley = ui.painter().layout_no_wrap(
            text.clone(),
            egui::FontId::proportional(style::SMALL_SIZE),
            palette.text,
        );
        let width = style::SPACE_S + icons::SIZE_SMALL + 6.0 + galley.size().x + style::SPACE_S;
        if right - width < after {
            break;
        }
        let rect = Rect::from_min_size(
            pos2(right - width, bar.center().y - CHIP_HEIGHT / 2.0),
            vec2(width, CHIP_HEIGHT),
        );
        let id = ui.make_persistent_id((
            "checkout_chip",
            &key.connection_id,
            &key.repository_key,
            key.number,
        ));
        let response = ui.interact(rect, id, Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &text));
        let fill = row::state_fill(ui, &response, palette.surface_subtle);
        ui.painter().rect(
            rect,
            style::RADIUS_CONTROL,
            fill,
            Stroke::new(1.0, palette.border),
            egui::StrokeKind::Inside,
        );
        let slot = Rect::from_center_size(
            pos2(rect.left() + style::SPACE_S + icons::SIZE_SMALL / 2.0, rect.center().y),
            vec2(icons::SIZE_SMALL, icons::SIZE_SMALL),
        );
        if theme::motion::is_reduced(ui.ctx()) {
            ui.painter().text(
                slot.center(),
                Align2::CENTER_CENTER,
                icons::CIRCLE_NOTCH,
                icons::rest_font(&palette, icons::SIZE_SMALL),
                palette.accent,
            );
        } else {
            egui::Spinner::new()
                .size(icons::SIZE_SMALL - 2.0)
                .color(palette.accent)
                .paint_at(ui, slot);
        }
        let text_left = slot.right() + 6.0;
        ui.painter().galley(
            pos2(text_left, rect.center().y - galley.size().y / 2.0),
            galley,
            palette.text,
        );
        focus::ring(ui, &response, rect);
        if response.on_hover_text("Show the checkout").clicked() {
            checkout::reveal(ui.ctx(), key);
            action = Some(JumpAction::Open { key: key.clone(), listed: None });
        }
        covered = covered.union(rect);
        right = rect.left() - GAP;
    }
    (covered, action)
}
