//! The floating live-update card under the pull request's tabs. It overlays the content instead
//! of pushing it down, so appearing never moves what the user reads, and slides in from 8 points
//! above while it fades (180 ms; instant under reduced motion).
//!
//! Two looks: "pending" while the user writes something (accent edge, Reload as the one primary
//! action, Later), and "updated" after an update was applied quietly (success check, optional
//! Show changes, dismiss). Both are polite status regions for screen readers.

use egui::{Align2, Id, Pos2, Rect, RichText, Sense, Stroke, pos2, vec2};

use cv_base::theme::{self, icons, style};
use cv_widgets::buttons;

/// How far the card travels while it appears.
const SLIDE: f32 = 8.0;
const ACCENT_EDGE: f32 = 3.0;
pub const MAX_WIDTH: f32 = 760.0;
const MARGIN_RIGHT: i8 = 8;
/// The row's `SPACE_S` gaps: five between its six items (icon, title, detail, the space before
/// the actions, two buttons) and the width of that space.
const ROW_GAPS: f32 = 6.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Look {
    /// News waits until the user reloads; their draft stays as it is.
    Pending,
    /// News was applied; `can_show_changes` offers the diff of what was pushed.
    Updated { can_show_changes: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content {
    pub look: Look,
    pub title: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerAction {
    Reload,
    Later,
    ShowChanges,
    Dismiss,
}

pub struct Shown {
    pub action: Option<BannerAction>,
    pub is_hovered: bool,
}

/// Paints the card with its top centre at `top_center`, `shown` (0..1, eased) of the way in.
pub fn show(
    ctx: &egui::Context,
    id: Id,
    top_center: Pos2,
    max_width: f32,
    content: &Content,
    shown: f32,
) -> Shown {
    let palette = theme::palette(ctx);
    let mut action = None;
    let lift = (1.0 - shown) * SLIDE;
    let area = egui::Area::new(id)
        .order(egui::Order::Middle)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(top_center - vec2(0.0, lift))
        .interactable(shown > 0.5)
        .show(ctx, |ui| {
            ui.multiply_opacity(shown);
            ui.set_max_width(max_width);
            let is_pending = content.look == Look::Pending;
            let left = if is_pending { 16 } else { 12 };
            let frame = egui::Frame::new()
                .fill(palette.surface)
                .stroke(Stroke::new(
                    1.0,
                    if is_pending { palette.border_strong } else { palette.border },
                ))
                .corner_radius(style::RADIUS_POPUP)
                .shadow(style::popup_shadow(&palette))
                .inner_margin(egui::Margin { left, right: MARGIN_RIGHT, top: 6, bottom: 6 })
                .show(ui, |ui| {
                    action = card_row(ui, content, max_width - f32::from(left));
                });
            let rect = frame.response.rect;
            if is_pending {
                paint_accent_edge(ui, rect, palette.accent);
            }
            // A polite live region: read out after what the screen reader is saying, never
            // interrupting someone who is typing.
            let label = format!("{}: {}", content.title, content.detail);
            ui.interact(rect, id.with("announce"), Sense::hover())
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Status, true, &label));
            rect
        });
    let is_hovered = ctx.pointer_hover_pos().is_some_and(|pointer| area.inner.contains(pointer));
    Shown { action, is_hovered }
}

/// The icon, title, detail and actions in one row, `room` wide inside the card's left margin.
fn card_row(ui: &mut egui::Ui, content: &Content, room: f32) -> Option<BannerAction> {
    let palette = theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = style::SPACE_S;
        let (icon, color) = match content.look {
            Look::Pending => (icons::GIT_COMMIT, palette.accent),
            Look::Updated { .. } => (icons::CHECK_CIRCLE, palette.success),
        };
        glyph(ui, icon, color);
        let title = ui.label(
            RichText::new(&content.title)
                .font(theme::fonts::semibold_font(style::BODY_SIZE))
                .color(palette.text),
        );
        if !content.detail.is_empty() {
            // The detail gives way (with an ellipsis) so the actions always fit.
            let actions = match content.look {
                Look::Pending => {
                    buttons::icon_button_width(ui, "Later")
                        + buttons::icon_button_width(ui, "Reload")
                }
                Look::Updated { .. } => {
                    buttons::icon_button_width(ui, "Show changes") + buttons::icon_width()
                }
            };
            let used = f32::from(MARGIN_RIGHT)
                + icons::SIZE
                + title.rect.width()
                + actions
                + style::SPACE_S * ROW_GAPS;
            let detail_room = (room - used).max(80.0);
            ui.scope(|ui| {
                ui.set_max_width(detail_room);
                let detail =
                    egui::Label::new(RichText::new(&content.detail).color(palette.text_secondary))
                        .truncate();
                ui.add(detail).on_hover_text(&content.detail);
            });
        }
        ui.add_space(style::SPACE_S);
        actions(ui, &content.look)
    })
    .inner
}

/// Later and Reload while pending; Show changes (when offered) and Dismiss once updated.
fn actions(ui: &mut egui::Ui, look: &Look) -> Option<BannerAction> {
    let mut action = None;
    match *look {
        Look::Pending => {
            if buttons::subtle(ui, icons::CLOCK, "Later")
                .on_hover_text("Keep this version until you reload")
                .clicked()
            {
                action = Some(BannerAction::Later);
            }
            if buttons::primary_icon(ui, icons::ARROWS_CLOCKWISE, "Reload")
                .on_hover_text("Show the new version; your draft stays")
                .clicked()
            {
                action = Some(BannerAction::Reload);
            }
        }
        Look::Updated { can_show_changes } => {
            if can_show_changes && buttons::subtle(ui, icons::GIT_DIFF, "Show changes").clicked() {
                action = Some(BannerAction::ShowChanges);
            }
            if buttons::icon(ui, icons::X, "Dismiss").clicked() {
                action = Some(BannerAction::Dismiss);
            }
        }
    }
    action
}

/// A decorative icon: painted, not a label, so screen readers skip it (the card announces itself).
pub(super) fn glyph(ui: &mut egui::Ui, icon: &str, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(icons::SIZE, icons::SIZE), Sense::hover());
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(icons::SIZE),
        color,
    );
}

/// The pending card's left edge in the accent color, inside its rounded corners.
fn paint_accent_edge(ui: &egui::Ui, card: Rect, color: egui::Color32) {
    let edge = Rect::from_min_max(card.min, pos2(card.left() + ACCENT_EDGE, card.bottom()));
    let radius = style::RADIUS_POPUP;
    let corners = egui::CornerRadius { nw: radius, sw: radius, ne: 0, se: 0 };
    ui.painter().rect_filled(edge, corners, color);
}

#[cfg(test)]
mod tests {
    use cv_testkit::Clicks;
    use egui::{Id, Role, pos2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use review_store::ThemePreference;

    use super::{BannerAction, Content, Look, show};
    use cv_base::theme;

    fn pending() -> Content {
        Content {
            look: Look::Pending,
            title: "New changes pushed".to_owned(),
            detail: "2 new commits by Dee Okafor, 1 new comment by Dee Okafor".to_owned(),
        }
    }

    #[test]
    fn pending_card_announces_itself_with_reload_and_later() {
        let mut harness = Harness::builder()
            .with_size([980.0, 200.0])
            .with_context_setup(theme::fonts::install)
            .build_ui_state(
                |ui, action: &mut Option<BannerAction>| {
                    let ctx = ui.ctx().clone();
                    theme::activate(&ctx, ThemePreference::Light);
                    let id = Id::unique("pending_card");
                    let shown = show(&ctx, id, pos2(490.0, 8.0), 940.0, &pending(), 1.0);
                    if let Some(picked) = shown.action {
                        *action = Some(picked);
                    }
                },
                None,
            );
        harness.run();
        let announced = harness.get_by_role(Role::Status);
        assert_eq!(
            announced.accesskit_node().label().as_deref(),
            Some("New changes pushed: 2 new commits by Dee Okafor, 1 new comment by Dee Okafor"),
            "screen readers hear about the push, politely"
        );
        harness.click_at(harness.get_by_role_and_label(Role::Button, "Later").rect());
        harness.run();
        assert_eq!(*harness.state(), Some(BannerAction::Later));
        harness.click_at(harness.get_by_role_and_label(Role::Button, "Reload").rect());
        harness.run();
        assert_eq!(*harness.state(), Some(BannerAction::Reload));
    }
}
