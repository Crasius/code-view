//! The header button's face before the checkout is done, and the chip after the branch line that
//! says a checkout exists (and how far behind it is).

use egui::{Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use review_store::checkouts::CheckoutRecord;

use super::{REMOVAL_ID, is_removable};
use cv_base::theme::{self, icons, motion, style};
use cv_runtime::route::PullRequestKey;
use cv_services::Services;
use cv_services::checkouts;
use cv_widgets::{buttons, focus, row};

const BAR_HEIGHT: f32 = 2.0;
const REMOVE_LINK: &str = "Remove worktree…";

/// The header button: a secondary button with an optional icon, its label, a caret, and a thin
/// progress bar along its bottom while a checkout runs. A `compact` button with an icon shows the
/// label only as its tooltip, so a narrow header keeps room for the title.
pub(super) fn face(
    ui: &mut Ui,
    icon: Option<&str>,
    label: &str,
    share: Option<f32>,
    is_open: bool,
    compact: bool,
) -> Response {
    let palette = theme::palette(ui.ctx());
    let is_icon_only = compact && icon.is_some();
    let galley = ui.painter().layout_no_wrap(
        if is_icon_only { String::new() } else { label.to_owned() },
        egui::FontId::proportional(style::BODY_SIZE),
        palette.text,
    );
    let icon_width = match icon {
        Some(_) if is_icon_only => icons::SIZE,
        Some(_) => icons::SIZE + 6.0,
        None => 0.0,
    };
    let width = style::SPACE_M
        + icon_width
        + galley.size().x
        + style::SPACE_S
        + icons::SIZE_SMALL
        + style::SPACE_M;
    let (rect, response) =
        ui.allocate_exact_size(vec2(width, style::CONTROL_HEIGHT), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let response = if is_icon_only { response.on_hover_text(label) } else { response };
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let fill = row::state_fill(ui, &response, palette.surface);
    ui.painter().rect(
        rect,
        style::RADIUS_CONTROL,
        fill,
        Stroke::new(1.0, palette.border_strong),
        StrokeKind::Inside,
    );
    let mut x = rect.left() + style::SPACE_M;
    if let Some(icon) = icon {
        let color = if icon == icons::WARNING { palette.error } else { palette.text };
        ui.painter().text(
            pos2(x + icons::SIZE / 2.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            icon,
            icons::rest_font(&palette, icons::SIZE),
            color,
        );
        x += icon_width;
    }
    ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, palette.text);
    let caret = if is_open { icons::CARET_UP } else { icons::CARET_DOWN };
    ui.painter().text(
        pos2(rect.right() - style::SPACE_M - icons::SIZE_SMALL / 2.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        caret,
        icons::rest_font(&palette, icons::SIZE_SMALL),
        palette.text_secondary,
    );
    if let Some(share) = share {
        let track = Rect::from_min_max(
            pos2(rect.left() + 1.0, rect.bottom() - BAR_HEIGHT - 1.0),
            pos2(rect.right() - 1.0, rect.bottom() - 1.0),
        );
        ui.painter().rect_filled(track, 1.0, palette.border);
        let shown = motion::glide(ui.ctx(), response.id.with("share"), share, motion::MEDIUM);
        let filled = Rect::from_min_size(track.min, vec2(track.width() * shown, BAR_HEIGHT));
        ui.painter().rect_filled(filled, 1.0, palette.accent);
    }
    focus::ring(ui, &response, rect);
    response
}

/// The chip after the branches: "Checked out", or "Checked out · 2 commits behind" in the warning
/// colors; the path in its tooltip. Nothing without a checkout. Once the pull request is merged or
/// closed (`is_finished`), a worktree the app made offers its removal beside it when it did not go
/// by itself ([`checkouts::finished`]); the tooltip says why.
pub fn chip(ui: &mut Ui, key: &PullRequestKey, services: &Services, is_finished: bool) {
    let Some(record) = services.checkouts.record(key) else {
        return;
    };
    if let Some(reason) = offered_removal(record, key, services, is_finished)
        && buttons::text_link(ui, REMOVE_LINK, reason).clicked()
    {
        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::unique(REMOVAL_ID), key.clone()));
    }
    let palette = theme::palette(ui.ctx());
    let behind = services
        .checkouts
        .status(key)
        .map_or(0, |status| if status.blocked.is_some() { status.behind } else { 0 });
    let (text, fill, color) = if behind > 0 {
        let commits = checkouts::plural(behind, "commit");
        (
            format!("Checked out · {behind} {commits} behind"),
            palette.warning_surface,
            palette.warning,
        )
    } else {
        ("Checked out".to_owned(), palette.surface_subtle, palette.success)
    };
    let response = cv_widgets::badges::pill(ui, &text, fill, color);
    response.on_hover_text(record.path.display().to_string());
}

/// The chip's width (and its removal link's), reserved before the branches take the line's room.
pub fn chip_width(ui: &Ui, key: &PullRequestKey, services: &Services, is_finished: bool) -> f32 {
    let Some(record) = services.checkouts.record(key) else {
        return 0.0;
    };
    let width = |text: &str, size: f32| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(size), egui::Color32::WHITE)
            .size()
            .x
    };
    let link = if offered_removal(record, key, services, is_finished).is_some() {
        width(REMOVE_LINK, style::BODY_SIZE) + style::SPACE_S
    } else {
        0.0
    };
    width("Checked out · 99 commits behind", style::SMALL_SIZE) + style::SPACE_M * 2.0 + link
}

/// The removal link's tooltip, when the page offers the removal.
fn offered_removal<'a>(
    record: &CheckoutRecord,
    key: &PullRequestKey,
    services: &'a Services,
    is_finished: bool,
) -> Option<&'a str> {
    if !is_finished || !is_removable(record) {
        return None;
    }
    let offer = services.checkouts.offer(key)?;
    Some(offer.kept.as_deref().unwrap_or("The pull request is done; its worktree can go"))
}
