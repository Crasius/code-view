//! Visual building blocks of the setup flow: stepper, choice rows, callouts, access rows.

use cv_base::theme::{self, Palette, icons};
use cv_widgets::{buttons, focus, glyph, row};
use egui::{Align, Color32, Frame, Layout, Margin, Response, RichText, Sense, Stroke, Ui, vec2};
use review_core::setup::{Access, FeatureCheck};

const HEADLINE_SIZE: f32 = 22.0;
const STEP_DOT: f32 = 22.0;
const CALLOUT_PADDING: Margin = Margin::symmetric(14, 12);
const SHEET_PADDING: Margin = Margin::symmetric(32, 28);

pub(crate) fn headline(ui: &mut Ui, title: &str, detail: &str) {
    let palette = theme::palette(ui.ctx());
    ui.label(RichText::new(title).font(theme::fonts::semibold_font(HEADLINE_SIZE)));
    if !detail.is_empty() {
        ui.add_space(theme::style::SPACE_XS);
        ui.label(RichText::new(detail).color(palette.text_secondary));
    }
    ui.add_space(theme::style::SPACE_XL);
}

pub(crate) fn section(ui: &mut Ui, title: &str) {
    ui.add_space(theme::style::SPACE_S);
    ui.label(RichText::new(title).font(theme::fonts::semibold_font(theme::style::SECTION_SIZE)));
    ui.add_space(theme::style::SPACE_XS);
}

pub(crate) fn small(ui: &mut Ui, text: &str) -> Response {
    let palette = theme::palette(ui.ctx());
    ui.label(RichText::new(text).size(theme::style::SMALL_SIZE).color(palette.text_secondary))
}

/// A note in small print after an info icon.
pub(crate) fn small_note(ui: &mut Ui, note: &str) {
    let label = small(ui, &format!("{}  {note}", icons::INFO));
    glyph::speak_as(&label, note);
}

/// Numbered steps joined by a line; finished steps show a check and can be clicked to go back.
pub(crate) fn stepper(ui: &mut Ui, labels: &[&str], current: usize) -> Option<usize> {
    let palette = theme::palette(ui.ctx());
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::style::SPACE_S;
        for (index, label) in labels.iter().enumerate() {
            if index > 0 {
                let (rect, _) = ui.allocate_exact_size(vec2(28.0, STEP_DOT), Sense::hover());
                let color = if index <= current { palette.accent } else { palette.border };
                ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.5, color));
            }
            let is_done = index < current;
            let is_current = index == current;
            let font = if is_current {
                theme::fonts::semibold_font(theme::style::BODY_SIZE)
            } else {
                egui::FontId::proportional(theme::style::BODY_SIZE)
            };
            let text_color = if index > current { palette.text_secondary } else { palette.text };
            let galley = ui.painter().layout_no_wrap((*label).to_owned(), font, text_color);
            let size = vec2(STEP_DOT + 8.0 + galley.size().x, STEP_DOT.max(galley.size().y));
            let sense = if is_done { Sense::click() } else { Sense::hover() };
            let (rect, response) = ui.allocate_exact_size(size, sense);
            let center = egui::pos2(rect.left() + STEP_DOT / 2.0, rect.center().y);
            let (fill, stroke, glyph, glyph_color) = if is_done {
                (palette.accent, Stroke::NONE, icons::CHECK.to_owned(), palette.on_accent)
            } else if is_current {
                (palette.accent, Stroke::NONE, (index + 1).to_string(), palette.on_accent)
            } else {
                (
                    palette.surface,
                    Stroke::new(1.0, palette.border),
                    (index + 1).to_string(),
                    palette.text_secondary,
                )
            };
            ui.painter().circle(center, STEP_DOT / 2.0, fill, stroke);
            ui.painter().text(
                center,
                egui::Align2::CENTER_CENTER,
                glyph,
                egui::FontId::proportional(theme::style::SMALL_SIZE),
                glyph_color,
            );
            let text_pos =
                egui::pos2(rect.left() + STEP_DOT + 8.0, rect.center().y - galley.size().y / 2.0);
            if is_done && response.hovered() {
                ui.painter().hline(
                    text_pos.x..=text_pos.x + galley.size().x,
                    text_pos.y + galley.size().y,
                    Stroke::new(1.0, palette.text_secondary),
                );
            }
            ui.painter().galley(text_pos, galley, text_color);
            let state = if is_done {
                "done"
            } else if is_current {
                "current step"
            } else {
                "not yet"
            };
            let description = format!("Step {} of {}: {label}, {state}", index + 1, labels.len());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, is_done, &description)
            });
            focus::ring(ui, &response, rect.expand(2.0));
            if response.clicked() {
                clicked = Some(index);
            }
        }
    });
    clicked
}

/// What a choice row shows at its right end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    /// A radio mark: the row is one of several choices.
    Radio,
    /// A chevron: the row leads on to the next step.
    Chevron,
}

/// A full-width list row: icon, title, detail, and its `mark`.
pub(crate) fn choice_row(
    ui: &mut Ui,
    icon: &str,
    title: &str,
    detail: &str,
    is_selected: bool,
    mark: Mark,
) -> Response {
    let palette = theme::palette(ui.ctx());
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(vec2(width, theme::style::ROW_TWO_LINE), Sense::click());
    let fill = if is_selected {
        palette.selection
    } else if response.hovered() {
        palette.hover
    } else {
        palette.surface
    };
    let stroke = if is_selected {
        Stroke::new(1.5, palette.accent)
    } else {
        Stroke::new(1.0, palette.border)
    };
    ui.painter().rect(rect, 4, fill, stroke, egui::StrokeKind::Inside);
    let painter = ui.painter();
    // A platform's row shows its mark in its brand color.
    let icon_center = egui::pos2(rect.left() + 28.0, rect.center().y);
    cv_widgets::provider_icon::paint_glyph(
        painter,
        &palette,
        icon_center,
        icon,
        egui::FontId::proportional(22.0),
        palette.accent,
        true,
    );
    painter.text(
        egui::pos2(rect.left() + 52.0, rect.center().y - 9.0),
        egui::Align2::LEFT_CENTER,
        title,
        theme::fonts::semibold_font(theme::style::BODY_SIZE),
        palette.text,
    );
    painter.text(
        egui::pos2(rect.left() + 52.0, rect.center().y + 10.0),
        egui::Align2::LEFT_CENTER,
        detail,
        egui::FontId::proportional(theme::style::SMALL_SIZE),
        palette.text_secondary,
    );
    let mark_center = egui::pos2(rect.right() - 24.0, rect.center().y);
    if mark == Mark::Chevron {
        painter.text(
            mark_center,
            egui::Align2::CENTER_CENTER,
            icons::CARET_RIGHT,
            egui::FontId::proportional(16.0),
            palette.text_secondary,
        );
    } else {
        painter.circle_stroke(
            mark_center,
            8.0,
            Stroke::new(1.5, if is_selected { palette.accent } else { palette.text_secondary }),
        );
        if is_selected {
            painter.circle_filled(mark_center, 4.5, palette.accent);
        }
    }
    let label = format!("{title}. {detail}");
    response.widget_info(|| match mark {
        Mark::Radio => {
            egui::WidgetInfo::selected(egui::Role::RadioButton, true, is_selected, &label)
        }
        Mark::Chevron => egui::WidgetInfo::labeled(egui::Role::Button, true, &label),
    });
    focus::ring(ui, &response, rect);
    response
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Info,
    Success,
    Warning,
    Error,
}

impl Tone {
    fn colors(self, palette: &Palette) -> (Color32, Color32, &'static str) {
        match self {
            Tone::Info => (palette.selection, palette.accent_text, icons::INFO),
            Tone::Success => (palette.success_surface, palette.success, icons::CHECK_CIRCLE),
            Tone::Warning => (palette.warning_surface, palette.warning, icons::WARNING),
            Tone::Error => (palette.error_surface, palette.error, icons::X_CIRCLE),
        }
    }
}

/// A tinted message with its actions right below the text, so no message is a dead end.
pub(crate) fn callout(
    ui: &mut Ui,
    tone: Tone,
    title: &str,
    body: &str,
    actions: impl FnOnce(&mut Ui),
) {
    tinted(ui, tone, title, body, Some(Box::new(actions)));
}

/// A tinted message that needs no action (a confirmation).
pub(crate) fn notice(ui: &mut Ui, tone: Tone, title: &str, body: &str) {
    tinted(ui, tone, title, body, None);
}

type Actions<'a> = Box<dyn FnOnce(&mut Ui) + 'a>;

fn tinted(ui: &mut Ui, tone: Tone, title: &str, body: &str, actions: Option<Actions<'_>>) {
    let palette = theme::palette(ui.ctx());
    let (fill, accent, icon) = tone.colors(&palette);
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, accent.gamma_multiply(0.35)))
        .corner_radius(theme::style::RADIUS_CONTROL)
        .inner_margin(CALLOUT_PADDING)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                glyph::decorative(ui, RichText::new(icon).size(18.0).color(accent));
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(title)
                            .font(theme::fonts::semibold_font(theme::style::BODY_SIZE))
                            .color(palette.text),
                    );
                    if !body.is_empty() {
                        ui.label(RichText::new(body).color(palette.text));
                    }
                    if let Some(actions) = actions {
                        ui.add_space(theme::style::SPACE_XS);
                        ui.horizontal_wrapped(|ui| actions(ui));
                    }
                });
            });
        });
    ui.add_space(theme::style::SPACE_M);
}

/// A button that opens `url` in the browser; the arrow says it leaves the app.
pub(crate) fn open_button(ui: &mut Ui, is_primary: bool, text: &str, url: &str) -> Response {
    let label = format!("{text}  {}", icons::ARROW_SQUARE_OUT);
    let response =
        if is_primary { buttons::primary(ui, &label) } else { buttons::secondary(ui, label) };
    glyph::speak_as(&response, text);
    let response = response.on_hover_text(url);
    if response.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
    response
}

fn access_icon(access: Access, palette: &Palette) -> (&'static str, Color32, &'static str) {
    match access {
        Access::Granted => (icons::CHECK_CIRCLE, palette.success, "Works"),
        Access::Limited => (icons::MINUS_CIRCLE, palette.warning, "Works partly"),
        Access::Missing => (icons::X_CIRCLE, palette.error, "Missing"),
        Access::Unverified => {
            (icons::CIRCLE_DASHED, palette.text_secondary, "Confirm on the token page")
        }
    }
}

/// One feature of the access checklist; missing and partial features carry their fix link.
pub(crate) fn access_row(ui: &mut Ui, check: &FeatureCheck, fix_url: &str, host_label: &str) {
    let palette = theme::palette(ui.ctx());
    let (icon, color, state) = access_icon(check.access, &palette);
    ui.horizontal(|ui| {
        ui.set_min_height(44.0);
        ui.add_space(theme::style::SPACE_XS);
        let icon_response = glyph::decorative(ui, RichText::new(icon).size(20.0).color(color));
        icon_response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, state));
        row::centered_lines(ui, |ui| {
            ui.spacing_mut().item_spacing.y = theme::style::SPACE_LINE;
            ui.label(
                RichText::new(check.feature.label())
                    .font(theme::fonts::semibold_font(theme::style::BODY_SIZE)),
            );
            ui.label(
                RichText::new(&check.detail)
                    .size(theme::style::SMALL_SIZE)
                    .color(palette.text_secondary),
            );
        });
        if matches!(check.access, Access::Missing | Access::Limited) {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                open_button(ui, false, &format!("Fix on {host_label}"), fix_url);
            });
        }
    });
}

/// Thin divider between rows of a list.
pub(crate) fn divider(ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, palette.border));
}

/// A sheet in the surface colour with a hairline border, the frame every step is drawn in.
pub(crate) fn sheet<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = theme::palette(ui.ctx());
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(theme::style::RADIUS_CONTROL)
        .inner_margin(SHEET_PADDING)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}
