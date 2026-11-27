//! The conversation's turns: the question (with its attached lines), the agent's work as one
//! quiet activity line ([`activity`]), the answer as Markdown with its checked citations linked,
//! and a muted footer with the turn's token counts and cache reads. A failed last turn shows its
//! message and Retry.

pub(super) mod activity;

use std::fmt::Write as _;

use cv_base::theme::{self, icons, style};
use cv_markdown::{self as markdown, RenderOptions};
use cv_widgets::glyph;
use cv_widgets::states::{self, Action as StateAction};
use egui::{RichText, Ui};
use review_agent::AgentKind;
use review_store::{ChatQuote, ChatTurn, ChatUsage, TurnEnd, TurnKind};

use self::activity::Phase;
use super::{Action, Assistant};

/// Draws every turn; returns what the user picked on one.
pub(crate) fn show(ui: &mut Ui, assistant: &Assistant) -> Option<Action> {
    let mut action = None;
    let last = assistant.chat.turns.len().saturating_sub(1);
    for (index, turn) in assistant.chat.turns.iter().enumerate() {
        if index > 0 {
            ui.add_space(style::SPACE_M);
        }
        let linked = assistant.links.get(index, &turn.answer);
        let place = Place { index, is_last: index == last };
        if let Some(picked) = turn_view(ui, assistant, turn, linked, place) {
            action = Some(picked);
        }
    }
    action
}

#[derive(Clone, Copy)]
struct Place {
    index: usize,
    is_last: bool,
}

fn turn_view(
    ui: &mut Ui,
    assistant: &Assistant,
    turn: &ChatTurn,
    linked: Option<&str>,
    place: Place,
) -> Option<Action> {
    question(ui, turn, place.index);
    ui.add_space(style::SPACE_S);
    // A question still waiting for the checkout or the agent shows that wait instead.
    let is_working =
        turn.end == TurnEnd::Running && assistant.waiting.is_none() && assistant.block.is_none();
    let phase = if is_working { Phase::Working } else { Phase::Finished };
    let agent = assistant.chat.agent.label();
    let id = activity_id(assistant.chat.agent, place.index, turn);
    if activity::show(ui, id, &turn.steps, phase) {
        ui.add_space(style::SPACE_XS);
    }
    if !turn.answer.is_empty() {
        let text = linked.unwrap_or(&turn.answer);
        let id = ("assistant_answer", agent, place.index);
        markdown::render(ui, id, text, RenderOptions::default());
    }
    match &turn.end {
        TurnEnd::Running => None,
        TurnEnd::Done => {
            usage_footer(ui, turn.usage);
            None
        }
        TurnEnd::Stopped => {
            note(ui, "Stopped.");
            None
        }
        TurnEnd::Failed(message) if place.is_last => {
            let actions = [StateAction::primary(icons::ARROW_CLOCKWISE, "Retry")];
            ui.add_space(style::SPACE_XS);
            states::error_with_actions(ui, message, &actions).map(|_| Action::Retry)
        }
        TurnEnd::Failed(message) => {
            note(ui, &format!("Failed: {message}"));
            None
        }
    }
}

/// The id of turn `index`'s activity line, which keeps its running clock.
pub(super) fn activity_id(agent: AgentKind, index: usize, turn: &ChatTurn) -> egui::Id {
    egui::Id::unique(("assistant_activity", agent.label(), index, turn.question.as_str()))
}

/// The question in a bubble, the attached lines above it.
fn question(ui: &mut Ui, turn: &ChatTurn, index: usize) {
    let palette = theme::palette(ui.ctx());
    bubble(ui, &format!("Question {}", index + 1), |ui| {
        if let Some(quote) = &turn.quote {
            let look = |text: &str| {
                RichText::new(text).size(style::SMALL_SIZE).color(palette.text_secondary)
            };
            glyph::truncated(
                ui,
                look(icons::QUOTES),
                look(&format!("{}, {}", quote.path, lines(quote))),
            );
        }
        let icon = match turn.kind {
            TurnKind::Question => "",
            TurnKind::Update => icons::GIT_BRANCH,
        };
        let text = if icon.is_empty() {
            turn.question.clone()
        } else {
            format!("{icon}  {}", turn.question)
        };
        let label = ui.add(egui::Label::new(RichText::new(text).color(palette.text)).wrap());
        glyph::speak_as(&label, &turn.question);
    });
}

/// "line 12" or "lines 12–18" of `quote`.
pub(super) fn lines(quote: &ChatQuote) -> String {
    if quote.start == quote.end {
        format!("line {}", quote.start)
    } else {
        format!("lines {}–{}", quote.start, quote.end)
    }
}

/// A tinted bubble the width of its column with the same inset on every side; screen readers
/// (and the walkthroughs) find it as a group called `name`.
pub(super) fn bubble<R>(ui: &mut Ui, name: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = theme::palette(ui.ctx());
    let shown = egui::Frame::new()
        .fill(palette.surface_subtle)
        .corner_radius(style::RADIUS_CARD)
        .inner_margin(egui::Margin::same(style::SPACE_M as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
    let rect = shown.response.rect;
    ui.interact(rect, shown.response.id.with("bubble"), egui::Sense::hover())
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Group, true, name));
    shown.inner
}

fn note(ui: &mut Ui, text: &str) {
    let palette = theme::palette(ui.ctx());
    ui.label(RichText::new(text).size(style::SMALL_SIZE).color(palette.text_secondary));
}

/// "1.2k in · 340 out · 12k cached": proves the CLI's prompt caching works.
fn usage_footer(ui: &mut Ui, usage: Option<ChatUsage>) {
    let Some(usage) = usage else {
        return;
    };
    let count = |value: u64| cv_widgets::numbers::compact_count(value);
    let mut text = format!("{} in · {} out", count(usage.input), count(usage.output));
    // Writing to a String cannot fail.
    if usage.cache_read > 0 {
        let _infallible = write!(text, " · {} cached", count(usage.cache_read));
    }
    if usage.cache_write > 0 {
        let _infallible = write!(text, " · {} written to the cache", count(usage.cache_write));
    }
    ui.add_space(style::SPACE_XS);
    note(ui, &text);
}
