//! Rerun buttons: one action as a button, several as a split button whose caret lists the rest.
//! Without a token the button stays visible but disabled, and an "Add a token" action sits next
//! to it, so the way to enable it is one click away.

pub mod requests;

use cv_base::theme::{self, icons, motion, style};
use cv_widgets::{buttons, elide, row, states};
use egui::{Color32, Id, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use review_core::{ProviderKind, RerunAction, RerunKind};

const CARET_WIDTH: f32 = 32.0;
const ICON_GAP: f32 = 6.0;
pub const NEEDS_TOKEN: &str = "Rerunning needs a personal access token on this connection.";
const ADD_TOKEN: &str = "Add a token to rerun";
/// Between a full-width rerun button and the "Add a token to rerun" row under it.
const TOKEN_ROW_GAP: f32 = style::SPACE_XS;
/// What the "Add a token to rerun" row adds under a full-width rerun button.
pub const TOKEN_ROW_HEIGHT: f32 = TOKEN_ROW_GAP + buttons::SUBTLE_HEIGHT;

/// The platform's words for a rerun.
pub fn label(kind: RerunKind, platform: ProviderKind) -> &'static str {
    match (kind, platform) {
        (RerunKind::StageAll, _) => "Rerun just this stage",
        (RerunKind::StageFailed, _) => "Retry stage",
        (RerunKind::Job, ProviderKind::GitLab) => "Retry job",
        (RerunKind::Job, _) => "Re-run job",
        (RerunKind::RunFailed, ProviderKind::GitLab) => "Retry failed jobs",
        (RerunKind::RunFailed, _) => "Re-run failed jobs",
        (RerunKind::RunAll, _) => "Re-run all jobs",
    }
}

/// What the user picked.
pub enum Picked {
    Rerun(RerunAction),
    AddToken,
}

pub struct RerunButton<'a> {
    pub reruns: &'a [RerunAction],
    pub platform: ProviderKind,
    pub has_token: bool,
    /// Another rerun is being started; buttons wait for it.
    pub is_busy: bool,
    /// Fill the available width (inside stage cards) instead of hugging the label.
    pub is_full_width: bool,
}

impl RerunButton<'_> {
    /// Draws nothing when there is nothing to rerun.
    pub fn show(self, ui: &mut Ui) -> Option<Picked> {
        let (first, rest) = self.reruns.split_first()?;
        let mut picked = None;
        let is_enabled = self.has_token && !self.is_busy;
        let primary_label = label(first.kind, self.platform);
        let text_width =
            elide::text_width(ui, primary_label, &egui::FontId::proportional(style::BODY_SIZE));
        let caret = if rest.is_empty() { 0.0 } else { CARET_WIDTH };
        let content_width = icons::SIZE + ICON_GAP + text_width;
        let width = if self.is_full_width {
            ui.available_width()
        } else {
            content_width + style::SPACE_M * 2.0 + caret
        };
        let (rect, _) = ui.allocate_exact_size(vec2(width, style::CONTROL_HEIGHT), Sense::hover());
        let main_rect = Rect::from_min_max(rect.min, pos2(rect.right() - caret, rect.bottom()));
        let id = ui.scope_id().with(("rerun", &first.target, first.kind as u8));
        let main = ui.interact(
            main_rect,
            id.with("main"),
            if is_enabled { Sense::click() } else { Sense::hover() },
        );
        main.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, is_enabled, primary_label)
        });
        paint_part(ui, &main, main_rect, is_enabled);
        let palette = theme::palette(ui.ctx());
        // While a rerun starts, its spinner takes the icon's place: same size, same label.
        let text_color =
            if is_enabled || self.is_busy { palette.text } else { palette.text_disabled };
        self.paint_face(ui, main_rect, content_width, primary_label, text_color);
        cv_widgets::focus::ring(ui, &main, main_rect);
        let main = hint(main, self.has_token, self.is_busy);
        if main.clicked() {
            picked = Some(Picked::Rerun(first.clone()));
        }
        if !rest.is_empty() {
            let caret_rect = Rect::from_min_max(pos2(main_rect.right(), rect.top()), rect.max);
            let more = Part { rect: caret_rect, id: id.with("more"), is_enabled, text_color };
            if let Some(action) = self.more(ui, more) {
                picked = Some(Picked::Rerun(action));
            }
        }
        if !self.has_token && self.add_token(ui) {
            picked = Some(Picked::AddToken);
        }
        picked
    }

    /// The rerun icon, or a spinner while a rerun starts, and `text`, centered in `rect`.
    fn paint_face(&self, ui: &Ui, rect: Rect, content_width: f32, text: &str, color: Color32) {
        let palette = theme::palette(ui.ctx());
        let icon_center =
            pos2(rect.center().x - content_width / 2.0 + icons::SIZE / 2.0, rect.center().y);
        let icon_slot = Rect::from_center_size(icon_center, vec2(icons::SIZE, icons::SIZE));
        if self.is_busy {
            states::spinner(ui, icon_slot, color);
        } else {
            let font = icons::rest_font(&palette, icons::SIZE);
            ui.painter().text(
                icon_center,
                egui::Align2::CENTER_CENTER,
                icons::ARROWS_CLOCKWISE,
                font,
                color,
            );
        }
        ui.painter().text(
            pos2(icon_slot.right() + ICON_GAP, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(style::BODY_SIZE),
            color,
        );
    }

    /// The caret half of a split button and the menu of every rerun it opens; returns the rerun
    /// picked there.
    fn more(&self, ui: &Ui, part: Part) -> Option<RerunAction> {
        let Part { rect, id, is_enabled, text_color } = part;
        let more = ui.interact(rect, id, if is_enabled { Sense::click() } else { Sense::hover() });
        more.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, is_enabled, "More rerun options")
        });
        paint_part(ui, &more, rect, is_enabled);
        let palette = theme::palette(ui.ctx());
        ui.painter().vline(
            rect.left(),
            rect.y_range().shrink(6.0),
            Stroke::new(1.0, palette.border_strong),
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icons::CARET_DOWN,
            egui::FontId::proportional(14.0),
            text_color,
        );
        cv_widgets::focus::ring(ui, &more, rect);
        let more = hint(more, self.has_token, self.is_busy).on_hover_text("More rerun options");
        let mut picked = None;
        egui::Popup::menu(&more).show(|ui| {
            ui.set_min_width(200.0);
            for action in self.reruns {
                let text = label(action.kind, self.platform);
                if buttons::menu_item(ui, icons::ARROWS_CLOCKWISE, text, None, None).clicked() {
                    picked = Some(action.clone());
                }
            }
        });
        picked
    }

    /// "Add a token to rerun", beside the button or centered under a full-width one; true when
    /// clicked.
    fn add_token(&self, ui: &mut Ui) -> bool {
        let add = if self.is_full_width {
            // Centered under the full-width button at a fixed gap: the stage card reserves
            // exactly [`TOKEN_ROW_HEIGHT`] for it.
            ui.add_space(TOKEN_ROW_GAP - ui.spacing().item_spacing.y);
            let row = vec2(ui.available_width(), buttons::SUBTLE_HEIGHT);
            ui.allocate_ui_with_layout(row, egui::Layout::top_down(egui::Align::Center), |ui| {
                buttons::subtle(ui, icons::KEY, ADD_TOKEN)
            })
            .inner
        } else {
            buttons::subtle(ui, icons::KEY, ADD_TOKEN)
        };
        add.on_hover_text("Open the connection setup to add a personal access token").clicked()
    }
}

/// One half of a split button.
#[derive(Clone, Copy)]
struct Part {
    rect: Rect,
    id: Id,
    is_enabled: bool,
    text_color: Color32,
}

fn hint(response: Response, has_token: bool, is_busy: bool) -> Response {
    if !has_token {
        response.on_hover_text(NEEDS_TOKEN)
    } else if is_busy {
        response.on_hover_text("A rerun is being started…")
    } else {
        response
    }
}

/// Secondary-button look for one half of a split button.
fn paint_part(ui: &Ui, response: &Response, rect: Rect, is_enabled: bool) {
    let palette = theme::palette(ui.ctx());
    let hover = if is_enabled { row::hover_amount(ui, response) } else { 0.0 };
    let fill = if is_enabled && response.is_pointer_button_down_on() {
        palette.pressed
    } else {
        motion::mix(palette.surface_subtle, palette.hover, hover)
    };
    let stroke = Stroke::new(1.0, if is_enabled { palette.border_strong } else { palette.border });
    ui.painter().rect(rect, style::RADIUS_CONTROL, fill, stroke, StrokeKind::Inside);
}
