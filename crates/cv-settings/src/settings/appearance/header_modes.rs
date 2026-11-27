//! Settings › Appearance › Pull request header: how the header above a pull request's tabs behaves
//! on each tab ([`HeaderMode`]; the page side is `pull_request::header_reveal`). A card per tab
//! holds a miniature of the page beside the three choices, and chips above the cards set every tab
//! at once. The miniature shows a still frame of the tab's mode; the pointer on a choice, or the
//! visible keyboard focus, plays that choice's story ([`miniature`]), and only then does the page
//! repaint on its own.

use cv_base::theme::{self, Palette, motion, style};
use cv_widgets::{badges, focus, row};
use egui::{Align, Layout, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use review_store::{HeaderMode, HeaderModes};

use super::miniature::{self, Phase};
use crate::settings::rows::{HEADER_HELP, HEADER_TITLE};

/// The column of the three choices on a card.
const CHOICES_WIDTH: f32 = 196.0;
/// The narrowest miniature beside the choices; narrower, the choices go below it.
const MINIATURE_MIN: f32 = 180.0;
/// A card's narrowest width with the miniature beside the choices; two of them fit side by side
/// or the cards stack in one column.
const CARD_MIN: f32 = 2.0 * style::SPACE_L + CHOICES_WIDTH + style::SPACE_L + MINIATURE_MIN;
/// A choice's pictogram.
const PICTOGRAM: egui::Vec2 = vec2(30.0, 22.0);
/// The heading leaves this much room for its text beside the chips, or puts them below it.
const HEADING_TEXT_MIN: f32 = 360.0;

struct Choice {
    mode: HeaderMode,
    label: &'static str,
    hint: &'static str,
}

const CHOICES: [Choice; 3] = [
    Choice {
        mode: HeaderMode::AlwaysShown,
        label: label_of(HeaderMode::AlwaysShown),
        hint: "Stays while you scroll",
    },
    Choice {
        mode: HeaderMode::HideOnScroll,
        label: label_of(HeaderMode::HideOnScroll),
        hint: "Away going down, back at the top",
    },
    Choice {
        mode: HeaderMode::StartMinimized,
        label: label_of(HeaderMode::StartMinimized),
        hint: "Away from the start, peeks on demand",
    },
];

/// A card's choice the user watches, and since when.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Watch {
    mode: HeaderMode,
    since: f64,
}

/// The section: its heading with the chips, then the cards; true when a mode changed.
pub fn show(ui: &mut Ui, modes: &mut HeaderModes) -> bool {
    let mut changed = heading(ui, modes);
    ui.add_space(style::SPACE_L);
    let width = ui.available_width();
    let columns = if width >= 2.0 * CARD_MIN + style::SPACE_L { 2 } else { 1 };
    let card_width = (width - style::SPACE_L * (columns - 1) as f32) / columns as f32;
    let tabs: [(&str, &mut HeaderMode); 4] = [
        ("Overview", &mut modes.overview),
        ("Files", &mut modes.files),
        ("Commits", &mut modes.commits),
        ("Pipelines", &mut modes.pipelines),
    ];
    let mut tabs = tabs.into_iter().enumerate().peekable();
    while tabs.peek().is_some() {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = style::SPACE_L;
            for (index, (name, mode)) in tabs.by_ref().take(columns) {
                ui.allocate_ui_with_layout(
                    vec2(card_width, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| changed |= card(ui, index, name, mode),
                );
            }
        });
        ui.add_space(style::SPACE_L);
    }
    changed
}

/// The title and help line, and the chips that set every tab; true when they changed a mode.
fn heading(ui: &mut Ui, modes: &mut HeaderModes) -> bool {
    let palette = theme::palette(ui.ctx());
    let text = |ui: &mut Ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = style::SPACE_XS;
            ui.label(
                egui::RichText::new(HEADER_TITLE)
                    .font(theme::fonts::semibold_font(style::SECTION_SIZE)),
            );
            let help = egui::RichText::new(HEADER_HELP).color(palette.text_secondary);
            ui.add(egui::Label::new(help).wrap());
        });
    };
    let chips_width = chips_width(ui);
    if ui.available_width() - chips_width >= HEADING_TEXT_MIN {
        row::with_trailing_block(ui, |ui| set_all(ui, modes), text).0
    } else {
        text(ui);
        ui.add_space(style::SPACE_M);
        ui.horizontal_wrapped(|ui| set_all_left_to_right(ui, modes)).inner
    }
}

/// "Set all tabs to" and its chips, added right to left.
fn set_all(ui: &mut Ui, modes: &mut HeaderModes) -> bool {
    let mut changed = false;
    for choice in CHOICES.iter().rev() {
        changed |= set_all_chip(ui, choice, modes);
    }
    set_all_label(ui);
    changed
}

/// "Set all tabs to" and its chips, added left to right.
fn set_all_left_to_right(ui: &mut Ui, modes: &mut HeaderModes) -> bool {
    set_all_label(ui);
    let mut changed = false;
    for choice in &CHOICES {
        changed |= set_all_chip(ui, choice, modes);
    }
    changed
}

fn set_all_label(ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    ui.label(
        egui::RichText::new("Set all tabs to")
            .size(style::SMALL_SIZE)
            .color(palette.text_secondary),
    );
}

/// The chip of `choice`; a click sets every tab to its mode.
fn set_all_chip(ui: &mut Ui, choice: &Choice, modes: &mut HeaderModes) -> bool {
    let all = HeaderModes::all(choice.mode);
    let is_clicked = chip(ui, choice.label).clicked();
    let changed = is_clicked && *modes != all;
    if changed {
        *modes = all;
    }
    changed
}

/// The heading's chips with their label and gaps, as wide as they lay out.
fn chips_width(ui: &Ui) -> f32 {
    let label = text_width(ui, "Set all tabs to");
    let chips: f32 = CHOICES.iter().map(|choice| chip_size(ui, choice.label).x).sum();
    label + chips + 3.0 * ui.spacing().item_spacing.x
}

fn text_width(ui: &Ui, text: &str) -> f32 {
    let font = egui::FontId::proportional(style::SMALL_SIZE);
    ui.painter().layout_no_wrap(text.to_owned(), font, theme::palette(ui.ctx()).text).size().x
}

fn chip_size(ui: &Ui, label: &str) -> egui::Vec2 {
    vec2(text_width(ui, label) + 2.0 * style::SPACE_M, style::CONTROL_HEIGHT)
}

/// A rounded outlined button with a short label, named "Set all tabs to …" for screen readers.
fn chip(ui: &mut Ui, label: &str) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(chip_size(ui, label), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Set all tabs to {label}"))
    });
    let radius = (style::CONTROL_HEIGHT / 2.0) as u8;
    ui.painter().rect_filled(rect, radius, row::state_fill(ui, &response, palette.surface));
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, palette.border_strong),
        StrokeKind::Inside,
    );
    let font = egui::FontId::proportional(style::SMALL_SIZE);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, font, palette.text);
    focus::ring(ui, &response, rect);
    response
}

/// The card of tab `name` (`index` in the tab bar): its name and mode, the miniature and the
/// choices; true when the mode changed.
fn card(ui: &mut Ui, index: usize, name: &str, mode: &mut HeaderMode) -> bool {
    let palette = theme::palette(ui.ctx());
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(style::RADIUS_POPUP)
        .inner_margin(egui::Margin::same(style::SPACE_L as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = style::SPACE_S;
                ui.label(
                    egui::RichText::new(name).font(theme::fonts::semibold_font(style::BODY_SIZE)),
                );
                let (fill, ink) = badge_colors(*mode, &palette);
                badges::pill(ui, label_of(*mode), fill, ink);
            });
            ui.add_space(style::SPACE_L);
            card_body(ui, index, name, mode)
        })
        .inner
}

/// The miniature beside the choices (below them on a narrow card). The choices are laid out first,
/// so the miniature already plays the one the pointer is on in the same frame.
fn card_body(ui: &mut Ui, index: usize, name: &str, mode: &mut HeaderMode) -> bool {
    let body = ui.available_rect_before_wrap();
    let is_beside = body.width() >= CHOICES_WIDTH + style::SPACE_L + MINIATURE_MIN;
    let (miniature_rect, choices_rect) = if is_beside {
        let split = body.right() - CHOICES_WIDTH;
        let left = Rect::from_min_max(body.min, pos2(split - style::SPACE_L, body.max.y));
        (left, Rect::from_min_max(pos2(split, body.top()), body.max))
    } else {
        (body, body)
    };
    let mut choices_ui =
        ui.new_child(UiBuilder::new().max_rect(choices_rect).layout(Layout::top_down(Align::Min)));
    if !is_beside {
        // Below the miniature and its caption, which come next.
        choices_ui.add_space(miniature::HEIGHT_WITH_CAPTION + style::SPACE_L);
    }
    let (changed, watched) = choices(&mut choices_ui, name, mode);
    let phase = watch(ui, index, watched);
    let shown = watched.unwrap_or(*mode);
    let mut miniature_ui = ui
        .new_child(UiBuilder::new().max_rect(miniature_rect).layout(Layout::top_down(Align::Min)));
    miniature::show(&mut miniature_ui, index, name, shown, phase);
    let bottom = choices_ui.min_rect().bottom().max(miniature_ui.min_rect().bottom());
    ui.advance_cursor_after_rect(Rect::from_min_max(body.min, pos2(body.right(), bottom)));
    changed
}

/// What the card's miniature plays: the choice `watched`, from when the user started watching it.
/// Repaints are asked for only while it plays.
fn watch(ui: &Ui, index: usize, watched: Option<HeaderMode>) -> Phase {
    let ctx = ui.ctx();
    let id = egui::Id::unique("settings.header_mode_watch").with(index);
    let now = ctx.input(|input| input.time);
    let Some(mode) = watched else {
        ctx.data_mut(|data| data.remove::<Watch>(id));
        return Phase::Still;
    };
    let watch = ctx.data_mut(|data| {
        let watch = data.get_temp_mut_or_insert_with(id, || Watch { mode, since: now });
        if watch.mode != mode {
            *watch = Watch { mode, since: now };
        }
        *watch
    });
    if motion::is_reduced(ctx) {
        return Phase::Frozen;
    }
    ctx.request_repaint();
    Phase::Playing(now - watch.since)
}

/// The three choices of tab `name` as a radio group; returns whether the mode changed, and the
/// choice the pointer is on or the visible keyboard focus.
fn choices(ui: &mut Ui, name: &str, mode: &mut HeaderMode) -> (bool, Option<HeaderMode>) {
    ui.spacing_mut().item_spacing.y = style::SPACE_S;
    let mut changed = false;
    let mut watched = None;
    for option in &CHOICES {
        let is_selected = *mode == option.mode;
        let response = choice(ui, name, option, is_selected);
        let has_visible_focus = response.has_focus() && focus::is_visible(ui.ctx());
        if response.hovered() || has_visible_focus {
            watched = Some(option.mode);
        }
        if response.clicked() && !is_selected {
            *mode = option.mode;
            changed = true;
        }
    }
    (changed, watched)
}

/// One choice: its pictogram, label and hint on a tile, the selected one ringed in the accent.
fn choice(ui: &mut Ui, name: &str, option: &Choice, is_selected: bool) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let padding = vec2(style::SPACE_M, 10.0);
    let gap = 10.0;
    let width = ui.available_width();
    let text_width = width - 2.0 * padding.x - PICTOGRAM.x - gap;
    let label = ui.painter().layout(
        option.label.to_owned(),
        theme::fonts::semibold_font(style::BODY_SIZE),
        palette.text,
        text_width,
    );
    let hint = ui.painter().layout(
        option.hint.to_owned(),
        egui::FontId::proportional(style::SMALL_SIZE),
        palette.text_secondary,
        text_width,
    );
    let text_height = label.size().y + 2.0 + hint.size().y;
    let height = text_height.max(PICTOGRAM.y) + 2.0 * padding.y;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::RadioButton,
            true,
            is_selected,
            format!("{name}: {}", option.label),
        )
    });
    let selected =
        motion::toggle(ui.ctx(), response.id.with("selected"), is_selected, motion::FAST);
    let rest = row::state_fill(ui, &response, palette.surface);
    let fill = motion::mix(rest, palette.selection, selected);
    ui.painter().rect_filled(rect, style::RADIUS_CARD, fill);
    let hover = row::hover_amount(ui, &response);
    let border = motion::mix(palette.border, palette.accent, hover * 0.4);
    let border = motion::mix(border, palette.accent, selected);
    let stroke = Stroke::new(1.0 + selected, border);
    ui.painter().rect_stroke(rect, style::RADIUS_CARD, stroke, StrokeKind::Inside);

    let ink = motion::mix(palette.text_disabled, palette.accent, selected);
    let pictogram_at = pos2(rect.left() + padding.x, rect.center().y - PICTOGRAM.y / 2.0);
    pictogram(ui, Rect::from_min_size(pictogram_at, PICTOGRAM), option.mode, ink);
    let text_left = pictogram_at.x + PICTOGRAM.x + gap;
    let text_top = rect.center().y - text_height / 2.0;
    let label_height = label.size().y;
    ui.painter().galley(pos2(text_left, text_top), label, palette.text);
    ui.painter().galley(
        pos2(text_left, text_top + label_height + 2.0),
        hint,
        palette.text_secondary,
    );
    focus::ring(ui, &response, rect.expand(3.0));
    response
}

/// A choice's pictogram for `mode` in `rect` (30 by 22): a frame, the header whole, faded or gone,
/// the tab bar under it, and an arrow for the way it goes.
fn pictogram(ui: &Ui, rect: Rect, mode: HeaderMode, ink: egui::Color32) {
    let painter = ui.painter_at(rect.expand(1.0));
    let at = |x: f32, y: f32| rect.min + vec2(x, y);
    let frame = Rect::from_min_size(at(0.75, 0.75), vec2(28.5, 20.5));
    painter.rect_stroke(frame, 3, Stroke::new(1.5, ink), StrokeKind::Middle);
    let (header, tabs_top) = match mode {
        HeaderMode::AlwaysShown => (1.0, 11.0),
        HeaderMode::HideOnScroll => (0.35, 11.0),
        HeaderMode::StartMinimized => (0.0, 4.0),
    };
    if header > 0.0 {
        let bar = Rect::from_min_size(at(3.0, 4.0), vec2(24.0, 5.0));
        painter.rect_filled(bar, 2, ink.gamma_multiply(header));
    }
    let tabs = Rect::from_min_size(at(3.0, tabs_top), vec2(24.0, 2.0));
    painter.rect_filled(tabs, 1, ink.gamma_multiply(0.45));
    let arrow: &[(f32, f32)] = match mode {
        HeaderMode::AlwaysShown => &[(15.0, 14.0), (15.0, 18.0)],
        HeaderMode::HideOnScroll => &[(12.0, 15.0), (15.0, 18.0), (18.0, 15.0)],
        HeaderMode::StartMinimized => &[(12.0, 13.0), (15.0, 10.0), (18.0, 13.0)],
    };
    let points = arrow.iter().map(|(x, y)| at(*x, *y)).collect();
    painter.add(egui::Shape::line(points, Stroke::new(1.5, ink)));
}

const fn label_of(mode: HeaderMode) -> &'static str {
    match mode {
        HeaderMode::AlwaysShown => "Always shown",
        HeaderMode::HideOnScroll => "Hide on scroll",
        HeaderMode::StartMinimized => "Start minimized",
    }
}

/// The mode badge: neutral while the header stays, in the accent where it moves.
fn badge_colors(mode: HeaderMode, palette: &Palette) -> (egui::Color32, egui::Color32) {
    match mode {
        HeaderMode::AlwaysShown => (palette.hover, palette.text_secondary),
        HeaderMode::HideOnScroll | HeaderMode::StartMinimized => {
            (palette.selection, palette.accent_text)
        }
    }
}
