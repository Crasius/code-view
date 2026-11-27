//! The rows of a card that are not pull requests: a failed answer, an empty card, a wait for a
//! rate limit, skeleton rows, and the Favorites card's notes.

use cv_base::theme::{self, icons, style};
use cv_runtime::intents;
use cv_widgets::states::{self, Action};
use cv_widgets::{buttons, glyph, skeleton};
use egui::{Rect, Ui, pos2, vec2};
use review_core::ProviderKind;

use super::super::favorites::HiddenFavorites;
use super::super::queries::ConnectionResult;
use super::header::repositories_text;
use super::indicators::{self, REVIEWER_SIZE};
use super::{
    AVATAR_SIZE, Columns, EmptyOffer, LINE_OFFSET, ListAction, META_MARK_GAP, ROW_PADDING, size,
};

/// A failed first answer with Retry and the connection's setup. Where the request named
/// repositories (the Favorites card mixes several), the message says which.
pub(crate) fn paint_message(
    ui: &mut Ui,
    rect: Rect,
    result: &ConnectionResult,
) -> Option<ListAction> {
    let message = result.load.error()?;
    let message = match result.repositories_label() {
        Some(repositories) => format!("{repositories} on {}: {message}", result.connection_name),
        None => message.to_owned(),
    };
    let mut picked = None;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(rect.shrink2(vec2(ROW_PADDING, style::SPACE_M))),
        |ui| {
            let actions = [
                Action::primary(icons::ARROW_CLOCKWISE, "Retry"),
                Action::secondary(icons::GEAR, "Check connection"),
            ];
            picked = states::error_with_actions(ui, &message, &actions);
        },
    );
    if picked? == 0 {
        Some(ListAction::Retry)
    } else {
        intents::request_fix(ui.ctx(), &result.connection_id);
        None
    }
}

/// The note of an empty card and its way forward: a card whose rows the repository filter hid
/// always offers to clear the filters, any other one the page's `offer`.
pub(crate) fn paint_empty(
    ui: &mut Ui,
    rect: Rect,
    is_filtered: bool,
    offer: EmptyOffer,
) -> Option<ListAction> {
    let palette = theme::palette(ui.ctx());
    let (text, offer) = if is_filtered {
        ("No pull requests in the chosen repository.", EmptyOffer::ClearFilters)
    } else {
        ("No pull requests in this view.", offer)
    };
    let mut action = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(ROW_PADDING, 0.0))), |ui| {
        ui.horizontal_centered(|ui| {
            ui.label(egui::RichText::new(text).color(palette.text_secondary));
            let button = match offer {
                EmptyOffer::Nothing => None,
                EmptyOffer::ClearFilters => {
                    Some((icons::X, "Clear filters", ListAction::ClearFilters))
                }
                EmptyOffer::ShowAll => {
                    Some((icons::FUNNEL, "Show all states", ListAction::ShowAll))
                }
            };
            if let Some((icon, label, picked)) = button
                && cv_widgets::buttons::subtle(ui, icon, label).clicked()
            {
                action = Some(picked);
            }
        });
    });
    action
}

/// Starred repositories the role hides: how many, why, and "Add token", which opens the
/// connection's setup at its token step.
pub(crate) fn paint_hidden_favorites(ui: &mut Ui, rect: Rect, hidden: &HiddenFavorites) {
    let words = format!(
        "{} hidden: {} has no token to know who you are.",
        repositories_text(hidden.repositories),
        hidden.connection_name
    );
    let text = format!("{}  {words}", icons::KEY);
    let tooltip = format!("Open the setup of {} at its token", hidden.connection_name);
    let wants_token = note_with_button(ui, rect, &text, &words, |ui| {
        buttons::subtle(ui, icons::KEY, "Add token").on_hover_text(tooltip).clicked()
    });
    if wants_token {
        intents::request_fix(ui.ctx(), &hidden.connection_id);
    }
}

/// The Favorites card before anything is starred: what a star does, and the way to the
/// repositories, where the stars are. True when that way was taken.
pub(crate) fn paint_no_favorites(ui: &mut Ui, rect: Rect) -> bool {
    let text = "Star a repository to see its pull requests here, from every connection.";
    note_with_button(ui, rect, text, text, |ui| {
        buttons::subtle(ui, icons::BOOKS, "Browse repositories").clicked()
    })
}

/// A note whose `button` keeps its width at the row's right; the sentence `text` is cut to the
/// rest, and `spoken` (its words without glyphs) is its accessible name and full text on hover.
/// Returns what `button` returned.
fn note_with_button(
    ui: &mut Ui,
    rect: Rect,
    text: &str,
    spoken: &str,
    button: impl FnOnce(&mut Ui) -> bool,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let mut picked = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(ROW_PADDING, 0.0))), |ui| {
        ui.horizontal_centered(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                picked = button(ui);
                ui.add_space(style::SPACE_S);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let label = ui.add(
                        egui::Label::new(egui::RichText::new(text).color(palette.text_secondary))
                            .truncate()
                            .show_tooltip_when_elided(false),
                    );
                    glyph::speak_as(&label, spoken);
                    label.on_hover_text(spoken);
                });
            });
        });
    });
    picked
}

pub(crate) fn paint_waiting(ui: &mut Ui, rect: Rect, kind: ProviderKind) {
    let palette = theme::palette(ui.ctx());
    let words = format!(
        "Waiting for {} to accept requests again. The list loads on its own.",
        kind.label()
    );
    let text = format!("{}  {words}", icons::CLOCK);
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(ROW_PADDING, 0.0))), |ui| {
        ui.horizontal_centered(|ui| {
            let label = ui.label(egui::RichText::new(text).color(palette.text_secondary));
            glyph::speak_as(&label, &words);
        });
    });
}

/// The row's shape while its first answer loads: same columns, same heights.
pub(crate) fn paint_skeleton(ui: &Ui, rect: Rect, seed: usize) {
    let columns = Columns::for_row(rect);
    let center = rect.center().y;
    skeleton::paint_circle(
        ui,
        pos2(rect.left() + ROW_PADDING + AVATAR_SIZE / 2.0, center),
        AVATAR_SIZE / 2.0,
    );
    let text_width = columns.text.1 - columns.text.0;
    let title = (text_width * (0.45 + (seed * 37 % 30) as f32 / 100.0)).max(80.0);
    let meta = (text_width * (0.3 + (seed * 53 % 20) as f32 / 100.0)).max(60.0);
    skeleton::paint_bar(
        ui,
        Rect::from_min_size(pos2(columns.text.0, center - LINE_OFFSET - 6.5), vec2(title, 13.0)),
    );
    // The meta line's platform mark slot, then its text.
    skeleton::paint_circle(
        ui,
        pos2(columns.text.0 + icons::SIZE_SMALL / 2.0, center + LINE_OFFSET),
        5.0,
    );
    let meta_left = columns.text.0 + icons::SIZE_SMALL + META_MARK_GAP;
    skeleton::paint_bar(
        ui,
        Rect::from_min_size(pos2(meta_left, center + LINE_OFFSET - 5.0), vec2(meta, 10.0)),
    );
    if let Some((left, right)) = columns.repository {
        let width = right - left;
        skeleton::paint_bar(
            ui,
            Rect::from_min_size(pos2(left, center - LINE_OFFSET - 5.0), vec2(width * 0.5, 10.0)),
        );
        skeleton::paint_bar(
            ui,
            Rect::from_min_size(pos2(left, center + LINE_OFFSET - 5.0), vec2(width * 0.75, 10.0)),
        );
    }
    if let Some((left, right)) = columns.size {
        size::paint_placeholder(ui, Rect::from_x_y_ranges(left..=right, rect.y_range()));
    }
    if let Some((left, right)) = columns.status {
        skeleton::paint_bar(
            ui,
            Rect::from_center_size(pos2(f32::midpoint(left, right), center), vec2(64.0, 20.0)),
        );
    }
    for index in 0..2 {
        let x = indicators::slot_x(columns.reviewers.1, index);
        skeleton::paint_circle(ui, pos2(x, center), REVIEWER_SIZE / 2.0);
    }
}
