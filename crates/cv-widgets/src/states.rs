//! Empty, loading and error states. A state that tells the user to go somewhere or do something
//! carries that action as a button right there, so no message is a dead end.

use cv_base::theme::{self, icons, motion, style};
use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Ui, vec2};
use review_core::ProviderKind;

use super::{buttons, glyph};

const DETAIL_WIDTH: f32 = 480.0;
const BADGE_SIZE: f32 = 64.0;

/// A button offered by an empty or error state. The view acts on the returned index.
#[derive(Clone, Copy)]
pub struct Action<'a> {
    pub icon: &'a str,
    pub label: &'a str,
    pub is_primary: bool,
}

impl<'a> Action<'a> {
    pub fn primary(icon: &'a str, label: &'a str) -> Self {
        Self { icon, label, is_primary: true }
    }

    pub fn secondary(icon: &'a str, label: &'a str) -> Self {
        Self { icon, label, is_primary: false }
    }
}

/// "Open on GitHub", "Open in Azure DevOps": the platform named, never just "the platform".
pub fn open_on_platform(kind: ProviderKind) -> String {
    format!("Open {}", on_platform(kind))
}

/// "on GitHub", "in Azure DevOps".
pub fn on_platform(kind: ProviderKind) -> String {
    match kind {
        ProviderKind::AzureDevOps => format!("in {}", kind.label()),
        ProviderKind::GitHub | ProviderKind::GitLab => format!("on {}", kind.label()),
    }
}

/// Centered message with an icon for empty and error states.
pub fn message(ui: &mut Ui, icon: &str, title: &str, detail: &str) {
    message_with_actions(ui, icon, title, detail, &[]);
}

/// Centered message with buttons below it; returns the index of the clicked action.
pub fn message_with_actions(
    ui: &mut Ui,
    icon: &str,
    title: &str,
    detail: &str,
    actions: &[Action<'_>],
) -> Option<usize> {
    let icon_color = theme::palette(ui.ctx()).text_secondary;
    centered_message(ui, icon, icon_color, title, detail, actions)
}

/// [`message_with_actions`] with the icon in `icon_color` (a warning, a failure).
pub fn notice(
    ui: &mut Ui,
    icon: &str,
    icon_color: Color32,
    title: &str,
    detail: &str,
    actions: &[Action<'_>],
) -> Option<usize> {
    centered_message(ui, icon, icon_color, title, detail, actions)
}

fn centered_message(
    ui: &mut Ui,
    icon: &str,
    icon_color: Color32,
    title: &str,
    detail: &str,
    actions: &[Action<'_>],
) -> Option<usize> {
    let palette = theme::palette(ui.ctx());
    let mut clicked = None;
    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        let (badge, _) = ui.allocate_exact_size(vec2(BADGE_SIZE, BADGE_SIZE), egui::Sense::hover());
        ui.painter().circle_filled(badge.center(), BADGE_SIZE / 2.0, palette.surface_subtle);
        ui.painter().text(
            badge.center(),
            Align2::CENTER_CENTER,
            icon,
            icons::rest_font(&palette, 28.0),
            icon_color,
        );
        ui.add_space(style::SPACE_M);
        ui.label(RichText::new(title).font(theme::fonts::semibold_font(style::SECTION_SIZE)));
        if !detail.is_empty() {
            ui.add_space(style::SPACE_XS);
            let width = DETAIL_WIDTH.min(ui.available_width());
            ui.allocate_ui(vec2(width, 0.0), |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(detail).color(palette.text_secondary))
                });
            });
        }
        if !actions.is_empty() {
            ui.add_space(style::SPACE_L);
            clicked = centered_actions(ui, actions);
        }
    });
    clicked
}

pub fn loading(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        let color = theme::palette(ui.ctx()).text_secondary;
        let (rect, response) =
            ui.allocate_exact_size(vec2(icons::SIZE, icons::SIZE), Sense::hover());
        response.widget_info(|| egui::WidgetInfo::new(egui::Role::ProgressIndicator));
        spinner(ui, rect, color);
        ui.label(RichText::new(text).color(color));
    });
}

/// The busy mark in `rect`: egui's spinner, or a still notch under reduced motion.
pub fn spinner(ui: &Ui, rect: Rect, color: Color32) {
    if motion::is_reduced(ui.ctx()) {
        let font = FontId::proportional(rect.height());
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, icons::CIRCLE_NOTCH, font, color);
    } else {
        egui::Spinner::new().color(color).paint_at(ui, rect);
    }
}

/// Inline error bar.
pub fn error(ui: &mut Ui, text: &str) {
    error_with_actions(ui, text, &[]);
}

/// Inline error bar with the ways out of it (retry, open on the platform, fix the token);
/// returns the index of the clicked action.
pub fn error_with_actions(ui: &mut Ui, text: &str, actions: &[Action<'_>]) -> Option<usize> {
    let color = theme::palette(ui.ctx()).error;
    bar(ui, color, text, actions)
}

/// Inline warning bar, like [`error_with_actions`] for things that block but did not fail.
pub fn warning_with_actions(ui: &mut Ui, text: &str, actions: &[Action<'_>]) -> Option<usize> {
    let color = theme::palette(ui.ctx()).warning;
    bar(ui, color, text, actions)
}

/// A "Details" toggle under an error, closed at first: the platform's raw answer for those who
/// need it, with a button to copy it. Never the message itself.
pub fn details(ui: &mut Ui, id_salt: &str, details: &str) {
    let palette = theme::palette(ui.ctx());
    egui::CollapsingHeader::new(
        RichText::new("Details").size(style::SMALL_SIZE).color(palette.text_secondary),
    )
    .id_salt(id_salt)
    .default_open(false)
    .show(ui, |ui| {
        ui.add(
            egui::Label::new(
                RichText::new(details)
                    .monospace()
                    .size(style::SMALL_SIZE)
                    .color(palette.text_secondary),
            )
            .wrap(),
        );
        if buttons::subtle(ui, icons::COPY, "Copy details").clicked() {
            ui.ctx().copy_text(details.to_owned());
        }
    });
}

fn bar(ui: &mut Ui, tone: egui::Color32, text: &str, actions: &[Action<'_>]) -> Option<usize> {
    let palette = theme::palette(ui.ctx());
    let mut clicked = None;
    egui::Frame::new()
        .fill(tone.gamma_multiply(if palette.is_dark { 0.14 } else { 0.07 }))
        .stroke(egui::Stroke::new(1.0, tone.gamma_multiply(0.35)))
        .corner_radius(style::RADIUS_CARD)
        .inner_margin(egui::Margin::symmetric(style::SPACE_M as i8, style::SPACE_S as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                glyph::decorative(ui, RichText::new(icons::WARNING).size(16.0).color(tone));
                ui.vertical(|ui| {
                    ui.label(RichText::new(text).color(palette.text));
                    if !actions.is_empty() {
                        ui.add_space(style::SPACE_XS);
                        ui.horizontal_wrapped(|ui| clicked = action_buttons(ui, actions));
                    }
                });
            });
        });
    clicked
}

fn centered_actions(ui: &mut Ui, actions: &[Action<'_>]) -> Option<usize> {
    let gap = style::SPACE_S;
    let width: f32 =
        actions.iter().map(|action| buttons::icon_button_width(ui, action.label)).sum::<f32>()
            + gap * (actions.len() - 1) as f32;
    ui.allocate_ui_with_layout(
        vec2(width, style::CONTROL_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = gap;
            action_buttons(ui, actions)
        },
    )
    .inner
}

fn action_buttons(ui: &mut Ui, actions: &[Action<'_>]) -> Option<usize> {
    let mut clicked = None;
    for (index, action) in actions.iter().enumerate() {
        let response = if action.is_primary {
            buttons::primary_icon(ui, action.icon, action.label)
        } else {
            buttons::secondary_icon(ui, action.icon, action.label)
        };
        if response.clicked() {
            clicked = Some(index);
        }
    }
    clicked
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    use cv_base::theme::{self, icons, motion};
    use egui::{
        Color32, Context, Event, Modifiers, PointerButton, RawInput, Rect, ViewportId, pos2, vec2,
    };
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use review_store::ThemePreference;

    use super::Action;

    /// How long egui waits before the next frame after a few that paint a spinner, and why it
    /// does not wait.
    fn repaint_delay_with_spinner(is_reduced: bool) -> (Duration, String) {
        let ctx = Context::default();
        theme::fonts::install(&ctx);
        motion::set_reduced(&ctx, is_reduced);
        let mut delay = Duration::MAX;
        for frame in 0..4 {
            let input = RawInput { time: Some(f64::from(frame) / 60.0), ..RawInput::default() };
            let mut output = ctx.run_ui(input, |ui| {
                let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(icons::SIZE, icons::SIZE));
                super::spinner(ui, rect, Color32::GRAY);
            });
            output.textures_delta.clear();
            delay = output.viewport_output[&ViewportId::ROOT].repaint_delay;
        }
        (delay, format!("{:?}", ctx.repaint_causes()))
    }

    #[test]
    fn a_spinner_holds_still_under_reduced_motion() {
        let (turning, _) = repaint_delay_with_spinner(false);
        assert_eq!(turning, Duration::ZERO, "it turns: every frame");
        let (still, causes) = repaint_delay_with_spinner(true);
        assert_eq!(still, Duration::MAX, "it holds still, yet egui repaints for {causes}");
    }

    #[test]
    fn a_notice_shows_its_words_and_answers_with_the_clicked_action() {
        let clicked = Rc::new(Cell::new(None));
        let seen = clicked.clone();
        let mut harness = Harness::builder()
            .with_size([800.0, 400.0])
            .with_context_setup(|ctx| {
                theme::fonts::install(ctx);
                theme::activate(ctx, ThemePreference::Light);
            })
            .build_ui(move |ui| {
                let error = theme::palette(ui.ctx()).error;
                let actions = [
                    Action::primary(icons::ARROW_CLOCKWISE, "Try again"),
                    Action::secondary(icons::COPY, "Copy details"),
                ];
                let detail = "The server did not answer.";
                if let Some(index) =
                    super::notice(ui, icons::WARNING, error, "Diff not loaded", detail, &actions)
                {
                    seen.set(Some(index));
                }
            });
        harness.run();
        harness.get_by_label("Diff not loaded");
        harness.get_by_label("The server did not answer.");
        harness.get_by_label("Try again");
        let at = harness.get_by_label("Copy details").rect().center();
        for pressed in [true, false] {
            harness.event(Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        harness.run();
        assert_eq!(clicked.get(), Some(1));
    }
}
