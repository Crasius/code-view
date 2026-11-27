//! The mini chat: with "Keep the answer beside the code" on (Settings → Assistant, or the chip
//! menu of the card and panel), a citation followed from an answer opens the Files tab, and a
//! small chat floats in a corner of the Files tab's content, never over the file tree or the tab
//! bar. It keeps that answer in view, steps through its references (◀ ▶, Alt+, and Alt+.) with
//! the diff following, and takes a follow-up into the same conversation, from the draft the card
//! and panel write too; a new answer replaces the old one there. While the conversation's latest
//! question runs, it offers Stop in place of Send, on an earlier answer too. It shrinks to a pill,
//! drags to another corner (kept in the settings) and fades while it covers the cited lines,
//! unless the pointer is on it. A reference to a file the pull request does not change shows a
//! notice with its way on, never a dead end.
//! The card's title line carries the agent and model chip (`super::chip`).
//!
//! Its state lives on the [`Assistant`], so it survives tab switches; the Files tab tells it
//! where its content and the cited lines are this frame
//! ([`cv_pr_session::assistant_link::publish_room`], [`cv_pr_session::assistant_link::note_cited`]).
//! Off, a citation only opens the file and nothing here runs.

mod answer;
mod card;
mod place;

use cv_base::theme::{self, motion, style};
use cv_pr_session::assistant_link;
use cv_pr_session::session::PullRequestSession;
use cv_pr_session::web_link;
use cv_services::Services;
use egui::{Context, Id, Key, Modifiers, Order, Rect, emath::TSTransform};
use review_store::TurnEnd;

use self::card::{Card, Event};
use self::place::{Corner, Laid};
use super::chip;
use super::citations::{self, CitedLines};
use super::{Action, Assistant, actions, turns};

#[cfg(any(test, feature = "test-support"))]
pub use self::card::{
    ANSWERING, CARD_LABEL, FOLLOW_UP, NEXT, OPEN, OPEN_AT_HEAD, PREVIOUS, SHRINK, SKIP_NEXT,
};

/// Opacity while the card covers the cited lines and the pointer is elsewhere (the design's).
const COVERING_OPACITY: f32 = 0.35;

/// The references of an answer whose citations were linked (citations the checkout has), in
/// order, each target once.
fn references(linked: &str) -> Vec<CitedLines> {
    let mut found: Vec<CitedLines> = Vec::new();
    for link in citations::linked(linked) {
        if !found.contains(&link.lines) {
            found.push(link.lines);
        }
    }
    found
}

/// The reference after (`forward`) or before `current` among `count`; it stays at the ends. With
/// none picked yet, forward goes to the first.
pub(crate) fn step(current: Option<usize>, count: usize, forward: bool) -> Option<usize> {
    match (current, forward) {
        (None, true) if count > 0 => Some(0),
        (Some(index), true) if index + 1 < count => Some(index + 1),
        (Some(index), false) if index > 0 => Some(index - 1),
        _ => current,
    }
}

/// Shown as the card, as the pill, or not at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Face {
    #[default]
    Closed,
    Card,
    Pill,
}

/// The mini chat's state, on the assistant.
#[derive(Default)]
pub(crate) struct MiniChat {
    face: Face,
    /// The turn whose answer it shows.
    turn: usize,
    /// The reference the user is on.
    current: Option<usize>,
    /// Turns of the conversation when it last looked: a new one is shown.
    seen_turns: usize,
    motions: place::Motions,
    pieces: answer::Pieces,
}

impl MiniChat {
    pub(super) fn close(&mut self) {
        self.face = Face::Closed;
    }

    fn pick(&mut self, current: Option<usize>) {
        if current != self.current {
            self.current = current;
            self.motions.fill();
        }
    }
}

/// A citation was followed into the Files tab: the mini chat (when it is on) shows the answer it
/// came from, on that reference, appearing if it was not open.
pub(crate) fn follow(assistant: &mut Assistant, target: &CitedLines) {
    let turns = assistant.chat.turns.len();
    let found = assistant.chat.turns.iter().enumerate().rev().find_map(|(turn, shown)| {
        let linked = assistant.links.get(turn, &shown.answer)?;
        let index = references(linked).iter().position(|reference| reference == target)?;
        Some((turn, index))
    });
    let (turn, current) = match found {
        Some((turn, index)) => (turn, Some(index)),
        None => (turns.saturating_sub(1), None),
    };
    let mini = &mut assistant.mini;
    if mini.face == Face::Closed {
        mini.face = Face::Card;
        mini.current = None;
        mini.motions.appear();
    }
    mini.turn = turn;
    mini.seen_turns = turns;
    mini.pick(current);
}

/// Draws the mini chat over the Files tab's content, after the tab drew; returns the lines a step
/// to another reference asks the diff to show.
pub(crate) fn show(
    assistant: &mut Assistant,
    ctx: &Context,
    session: &PullRequestSession,
    services: &mut Services,
) -> Option<CitedLines> {
    let settings = &services.settings.assistant;
    if !settings.enabled || !settings.mini_chat {
        assistant.mini.close();
        return None;
    }
    let corner =
        Corner { left: settings.mini_chat_corner.left, top: settings.mini_chat_corner.top };
    let turns = assistant.chat.turns.len();
    let mini = &mut assistant.mini;
    if turns > mini.seen_turns && mini.face != Face::Closed {
        mini.turn = turns - 1;
        mini.current = None;
    }
    mini.seen_turns = turns;
    if mini.turn >= turns {
        mini.close();
    }
    let room = assistant_link::room(ctx).filter(|_| mini.face != Face::Closed)?;
    assistant.resolver.publish(ctx, session);
    let shown = assistant.chat.turns.get(assistant.mini.turn);
    let linked = shown.and_then(|turn| assistant.links.get(assistant.mini.turn, &turn.answer));
    let references = linked.map(references).unwrap_or_default();
    let mut event = if ctx.input_mut(|input| input.consume_key(Modifiers::ALT, Key::Period)) {
        Some(Event::Step(true))
    } else if ctx.input_mut(|input| input.consume_key(Modifiers::ALT, Key::Comma)) {
        Some(Event::Step(false))
    } else {
        None
    };
    let current = assistant.mini.current.and_then(|index| references.get(index));
    let outside = current.filter(|reference| !assistant.resolver.is_changed(&reference.path));
    let place = Place { room, corner };
    let label = chip::label(assistant, services);
    let outside_path = outside.map(|reference| reference.path.as_str());
    let (drawn, chip) = draw(assistant, ctx, place, &references, outside_path, &label);
    let picked = chip.and_then(|chip| chip::menu(&chip, assistant, services));
    if let Some(picked) = picked {
        actions::act(assistant, picked, session, services);
    }
    event = event.or(drawn);
    let changes = Changes { place, references: &references, session };
    let jump = act(assistant, ctx, event?, &changes, services)?;
    assistant.resolver.is_changed(&jump.path).then_some(jump)
}

/// What carrying out an event reads.
struct Changes<'a> {
    place: Place,
    references: &'a [CitedLines],
    session: &'a PullRequestSession,
}

/// Carries out what the user did in the mini chat.
fn act(
    assistant: &mut Assistant,
    ctx: &Context,
    event: Event,
    changes: &Changes<'_>,
    services: &mut Services,
) -> Option<CitedLines> {
    let now = ctx.input(|input| input.time);
    let Changes { place, references, session } = changes;
    let mini = &mut assistant.mini;
    match event {
        Event::Step(forward) => {
            let next = step(mini.current, references.len(), forward);
            if next == mini.current {
                return None;
            }
            mini.pick(next);
            return references.get(next?).cloned();
        }
        Event::Shrink | Event::Open => {
            mini.motions.morph(now);
            mini.face = if event == Event::Shrink { Face::Pill } else { Face::Card };
        }
        Event::Full => {
            assistant_link::request(ctx, None);
            mini.close();
        }
        Event::Close => mini.close(),
        Event::Send => {
            let question = assistant.composer.clone();
            if actions::ask(assistant, &question, false) {
                assistant.composer.clear();
            }
            assistant.save(services);
        }
        Event::Stop => actions::act(assistant, Action::Stop, session, services),
        Event::OpenAtHead => {
            let current = mini.current.and_then(|index| references.get(index));
            if let Some(url) = current.and_then(|reference| head_url(session, reference)) {
                ctx.open_url(egui::OpenUrl::new_tab(url));
            }
        }
        Event::Drag(delta) => mini.motions.drag(delta),
        Event::Released => {
            let corner = mini.motions.release(place.room, now)?;
            services.settings.assistant.mini_chat_corner.left = corner.left;
            services.settings.assistant.mini_chat_corner.top = corner.top;
            services.save_settings();
        }
    }
    None
}

/// The page of `reference`'s file at the head commit on the platform, at its line.
fn head_url(session: &PullRequestSession, reference: &CitedLines) -> Option<String> {
    let detail = session.detail.ready()?;
    let pull_request = &detail.summary.web_url;
    let url = web_link::line_url(
        session.kind,
        pull_request,
        &reference.path,
        &detail.head_sha,
        reference.first,
    );
    Some(url.unwrap_or_else(|| pull_request.clone()))
}

/// Lays out the card or the pill in its layer; returns what the user did there, and the card's
/// chip (the pill has none). `outside` is the current reference's file when the pull request does
/// not change it, `chip` the chip's label.
fn draw(
    assistant: &mut Assistant,
    ctx: &Context,
    place: Place,
    references: &[CitedLines],
    outside: Option<&str>,
    chip: &str,
) -> (Option<Event>, Option<egui::Response>) {
    let mini = &mut assistant.mini;
    let is_pill = mini.face == Face::Pill;
    let laid = mini.motions.lay_out(ctx, place.room, place.corner, is_pill);
    let id = Id::unique("assistant_mini_chat");
    let is_dragged = mini.motions.is_dragged();
    let opacity = covering_opacity(ctx, id, &laid, outside.is_some(), is_dragged) * laid.appear;
    // Appearing, it grows out of its corner.
    let scale = place::APPEAR_SCALE + (1.0 - place::APPEAR_SCALE) * laid.appear;
    let pivot = place.corner.point(place.room).to_vec2();
    let layer = egui::LayerId::new(Order::Middle, id);
    ctx.set_transform_layer(layer, TSTransform::new(pivot * (1.0 - scale), scale));
    #[cfg(any(test, feature = "test-support"))]
    cv_base::frame_probe::note(ctx, DRAWN_NOTE, Drawn { rect: laid.frame, opacity, is_pill });

    let turn = &assistant.chat.turns[mini.turn];
    // Busy is the conversation's state, not the shown answer's: on an earlier answer while a later
    // question runs, the follow-up waits too and Stop stops the running one.
    let last = assistant.chat.turns.len().saturating_sub(1);
    let running = (assistant.chat.turns.get(last))
        .filter(|turn| turn.end == TurnEnd::Running)
        .map(|turn| (last, turn));
    let is_busy = running.is_some();
    let working = running.map(|(index, running)| {
        assistant.waiting.map_or_else(
            || {
                let id = turns::activity_id(assistant.chat.agent, index, running);
                turns::activity::working_summary(ctx, id, &running.steps)
            },
            str::to_owned,
        )
    });
    let linked = assistant.links.get(mini.turn, &turn.answer);
    let view = Card {
        turn: mini.turn,
        question: &turn.question,
        answer: linked.unwrap_or(&turn.answer),
        is_linked: linked.is_some(),
        references,
        current: mini.current,
        fill: laid.fill,
        outside,
        working,
        is_busy,
        is_stopping: assistant.is_stopping,
        chip,
    };
    let palette = theme::palette(ctx);
    let Laid { frame, content, radius, .. } = laid;
    let shown = egui::Area::new(id)
        .order(Order::Middle)
        .fixed_pos(frame.min)
        .constrain(false)
        .show(ctx, |ui| {
            ui.set_clip_rect(frame.expand(2.0 * style::SPACE_L).intersect(place.room));
            ui.multiply_opacity(opacity);
            ui.painter().add(style::popup_shadow(&palette).as_shape(frame, radius));
            let stroke = egui::Stroke::new(1.0, palette.border);
            ui.painter().rect(frame, radius, palette.surface, stroke, egui::StrokeKind::Inside);
            ui.expand_to_include_rect(frame);
            ui.set_clip_rect(frame.intersect(place.room));
            let shown = if is_pill {
                (card::pill(ui, content, mini.current, references.len()), None)
            } else {
                let (event, chip) =
                    card::card(ui, content, &view, &mut mini.pieces, &mut assistant.composer);
                (event, Some(chip))
            };
            ui.interact(frame, id.with("group"), egui::Sense::hover()).widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Group, true, card::CARD_LABEL)
            });
            shown
        });
    shown.inner
}

/// The opacity of the fade over the cited lines: faded while the mini chat covers them and the
/// pointer is elsewhere. A reference outside the diff covers nothing; dragged, it shows whole.
fn covering_opacity(ctx: &Context, id: Id, laid: &Laid, is_outside: bool, is_dragged: bool) -> f32 {
    let is_hovered = ctx.pointer_hover_pos().is_some_and(|pointer| laid.frame.contains(pointer));
    let covers =
        !is_outside && assistant_link::cited(ctx).is_some_and(|cited| cited.intersects(laid.frame));
    let target = if covers && !is_hovered && !is_dragged { COVERING_OPACITY } else { 1.0 };
    motion::glide(ctx, id.with("cover"), target, place::FADE_SECONDS)
}

/// Where the mini chat floats: the Files tab's content this frame, and its corner of it.
#[derive(Clone, Copy)]
struct Place {
    room: Rect,
    corner: Corner,
}

/// How the mini chat was drawn in the last pass, for tests.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Copy)]
pub struct Drawn {
    pub rect: Rect,
    pub opacity: f32,
    pub is_pill: bool,
}

#[cfg(any(test, feature = "test-support"))]
const DRAWN_NOTE: &str = "assistant_mini_drawn";

/// The mini chat as the last pass drew it; `None` when it drew none.
#[cfg(any(test, feature = "test-support"))]
pub fn drawn(ctx: &Context) -> Option<Drawn> {
    cv_base::frame_probe::last::<Drawn>(ctx, DRAWN_NOTE).pop()
}

/// Whether the mini chat asked for a frame in the pass before the last (egui reports a pass's
/// repaint causes in the next one), for tests: only while something of it moves.
#[cfg(any(test, feature = "test-support"))]
pub fn asks_for_frames(ctx: &Context) -> bool {
    let is_mini = |file: &str| file.replace('\\', "/").contains("cv-pr-assistant/src/mini");
    ctx.repaint_causes().iter().any(|cause| is_mini(cause.file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_stays_at_the_ends() {
        assert_eq!(step(None, 4, true), Some(0), "the first, when none is picked");
        assert_eq!(step(None, 4, false), None);
        assert_eq!(step(Some(1), 4, true), Some(2));
        assert_eq!(step(Some(3), 4, true), Some(3), "the last stays");
        assert_eq!(step(Some(0), 4, false), Some(0), "the first stays");
        assert_eq!(step(None, 0, true), None, "nothing to step to");
    }

    #[test]
    fn references_keep_their_order_and_each_target_once() {
        let answer = "[a.rs:1](<codeview-cite:a.rs#1-1>), [b.rs:2-3](<codeview-cite:b.rs#2-3>) \
                      and [`a.rs:1`](<codeview-cite:a.rs#1-1>) again.";
        let found: Vec<(String, u32)> =
            references(answer).into_iter().map(|r| (r.path, r.first)).collect();
        assert_eq!(found, [("a.rs".to_owned(), 1), ("b.rs".to_owned(), 2)]);
    }

    #[test]
    fn the_count_reads_where_the_user_is() {
        assert_eq!(card::count_label(Some(1), 4), "Reference 2 of 4");
        assert_eq!(card::count_label(None, 4), "4 references");
        assert_eq!(card::pill_label(Some(0), 3), "Assistant · 1 of 3");
        assert_eq!(card::pill_label(None, 0), "Assistant");
    }
}
