//! The mini chat's two faces, drawn in a floating layer over the Files tab. The card: a title line
//! that drags it with "Shrink to a pill", "Back to the full conversation" and "Close" (and the
//! agent and model chip); the question and its answer, scrolling; a
//! notice when the reference is outside the diff; the reference bar; the agent's activity while it
//! works; the one-line follow-up. The pill: "Assistant · N of M" with the reference arrows and
//! "Open the answer". Each returns what the user did as an [`Event`]; [`super`] carries it out.

use std::time::Duration;

use cv_base::theme::{self, icons, motion, style};
use cv_widgets::states::{self, Action as StateAction};
use cv_widgets::{buttons, elide, row};
use egui::{
    Align, Align2, CursorIcon, FontId, Id, Key, Layout, Rect, Response, RichText, Sense, TextEdit,
    Ui, UiBuilder, Vec2, vec2,
};

use super::super::chip;
use super::super::citations::CitedLines;
use super::answer::{self, Pieces};

/// The card's name in the accessibility tree.
pub const CARD_LABEL: &str = "Assistant mini chat";
pub const SHRINK: &str = "Shrink to a pill";
pub(crate) const FULL: &str = "Back to the full conversation";
pub(crate) const CLOSE: &str = "Close";
pub const OPEN: &str = "Open the answer";
pub const PREVIOUS: &str = "Previous reference (Alt+,)";
pub const NEXT: &str = "Next reference (Alt+.)";
pub const OPEN_AT_HEAD: &str = "Open it at the head commit";
pub const SKIP_NEXT: &str = "Skip to the next reference";
pub(crate) const SKIP_PREVIOUS: &str = "Skip to the previous reference";
pub const FOLLOW_UP: &str = "Ask a follow-up";
pub const ANSWERING: &str = "Answering…";

/// The side of a square icon button slot.
const SLOT: f32 = 32.0;
/// The card's padding on every side.
const PADDING: f32 = 16.0;
/// One cycle of the working dot's pulse.
const PULSE_SECONDS: f64 = 1.2;
/// How often the working dot repaints, about 30 Hz: our own choice, smooth for a slow pulse.
const PULSE_FRAME: Duration = Duration::from_millis(33);

/// What the user did in the mini chat this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Event {
    /// To the next (`true`) or previous reference.
    Step(bool),
    Shrink,
    Open,
    Full,
    Close,
    Send,
    Stop,
    OpenAtHead,
    /// The title line (or the pill) moved by this much.
    Drag(Vec2),
    Released,
}

/// What the card shows.
pub(crate) struct Card<'a> {
    pub turn: usize,
    pub question: &'a str,
    pub answer: &'a str,
    pub is_linked: bool,
    pub references: &'a [CitedLines],
    pub current: Option<usize>,
    /// The fill's strength on the current reference.
    pub fill: f32,
    /// The current reference's file, when the pull request does not change it.
    pub outside: Option<&'a str>,
    /// The activity line while a question is on its way.
    pub working: Option<String>,
    pub is_busy: bool,
    pub is_stopping: bool,
    /// The agent and model chip's label.
    pub chip: &'a str,
}

/// The reference bar's and the pill's count: "Reference 2 of 4", or how many there are.
pub(crate) fn count_label(current: Option<usize>, count: usize) -> String {
    match (current, count) {
        (_, 0) => "No references".to_owned(),
        (Some(index), _) => format!("Reference {} of {count}", index + 1),
        (None, 1) => "1 reference".to_owned(),
        (None, _) => format!("{count} references"),
    }
}

/// The card laid out in `content` (its full size in its place), shown within `frame`; what the
/// user did, and the chip, whose menu [`super`] opens.
pub(crate) fn card(
    ui: &mut Ui,
    content: Rect,
    view: &Card<'_>,
    pieces: &mut Pieces,
    composer: &mut String,
) -> (Option<Event>, Response) {
    let inner = content.shrink(PADDING);
    let mut event = None;
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), SLOT));
    event = event.or(drag_handle(ui, header, Id::unique("assistant_mini_card_drag")));
    let mut title = ui
        .new_child(UiBuilder::new().max_rect(header).layout(Layout::left_to_right(Align::Center)));
    // Not selectable: a press on the title drags the card.
    let name = RichText::new("Assistant").font(theme::fonts::semibold_font(style::SECTION_SIZE));
    title.add(egui::Label::new(name).selectable(false));
    let chip = title
        .with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = style::SPACE_XS;
            for (icon, label, picked) in [
                (icons::X, CLOSE, Event::Close),
                (icons::ARROWS_OUT_SIMPLE, FULL, Event::Full),
                (icons::MINUS, SHRINK, Event::Shrink),
            ] {
                if icon_slot(ui, icon, label, true) {
                    event = Some(picked);
                }
            }
            chip::in_title(ui, view.chip)
        })
        .inner;

    let below = Rect::from_min_max(egui::pos2(inner.left(), header.bottom()), inner.max);
    let mut footer =
        ui.new_child(UiBuilder::new().max_rect(below).layout(Layout::bottom_up(Align::Min)));
    event = event.or(footer_parts(&mut footer, view, composer));
    let body = Rect::from_min_max(
        egui::pos2(inner.left(), header.bottom() + style::SPACE_M),
        egui::pos2(inner.right(), footer.min_rect().top() - style::SPACE_M),
    );
    let mut body_ui = ui.new_child(UiBuilder::new().max_rect(body));
    egui::ScrollArea::vertical()
        .id_salt(("assistant_mini_body", view.turn))
        .auto_shrink([false, false])
        .max_height(body.height().max(0.0))
        .show(&mut body_ui, |ui| {
            let palette = theme::palette(ui.ctx());
            let question = RichText::new(view.question).size(style::SMALL_SIZE);
            ui.add(egui::Label::new(question.color(palette.text_secondary)).wrap());
            ui.add_space(style::SPACE_S);
            let current = view.current.and_then(|index| view.references.get(index));
            let fill = current.map(|reference| (reference, view.fill));
            answer::show(ui, pieces, view.answer, view.is_linked, view.turn, fill);
        });
    (event, chip)
}

/// From the bottom up: the follow-up, the activity, the reference bar and the notice above it.
fn footer_parts(ui: &mut Ui, view: &Card<'_>, composer: &mut String) -> Option<Event> {
    let mut event = follow_up(ui, view, composer);
    if let Some(working) = &view.working {
        ui.add_space(style::SPACE_S);
        working_line(ui, working);
    }
    if !view.references.is_empty() {
        ui.add_space(style::SPACE_S);
        let bar = ui.allocate_ui_with_layout(
            vec2(ui.available_width(), SLOT),
            Layout::left_to_right(Align::Center),
            |ui| reference_bar(ui, view.current, view.references.len()),
        );
        event = event.or(bar.inner);
    }
    if let Some(path) = view.outside {
        ui.add_space(style::SPACE_S);
        let shown = ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 0.0),
            Layout::top_down(Align::Min),
            |ui| outside_notice(ui, path, view.current, view.references.len()),
        );
        event = event.or(shown.inner);
    }
    event
}

/// ◀ "Reference N of M" ▶, centered; an arrow is disabled at its end.
fn reference_bar(ui: &mut Ui, current: Option<usize>, count: usize) -> Option<Event> {
    let label = count_label(current, count);
    let font = FontId::proportional(style::BODY_SIZE);
    let text_width = elide::text_width(ui, &label, &font);
    let spacing = style::SPACE_S;
    let total = 2.0 * SLOT + text_width + 2.0 * spacing;
    ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
    ui.spacing_mut().item_spacing.x = spacing;
    arrows(ui, current, count, |ui| {
        ui.label(label);
    })
}

/// The previous arrow, `between`, the next arrow.
fn arrows(
    ui: &mut Ui,
    current: Option<usize>,
    count: usize,
    between: impl FnOnce(&mut Ui),
) -> Option<Event> {
    let mut event = None;
    let can = |forward| super::step(current, count, forward) != current;
    if icon_slot(ui, icons::ARROW_LEFT, PREVIOUS, can(false)) {
        event = Some(Event::Step(false));
    }
    between(ui);
    if icon_slot(ui, icons::ARROW_RIGHT, NEXT, can(true)) {
        event = Some(Event::Step(true));
    }
    event
}

/// "`<file>` is not changed in this pull request." with its way on: the file at the head commit,
/// and the reference after it (or before it, at the end).
fn outside_notice(ui: &mut Ui, path: &str, current: Option<usize>, count: usize) -> Option<Event> {
    let text = format!("{path} is not changed in this pull request.");
    let forward = super::step(current, count, true) != current;
    let backward = super::step(current, count, false) != current;
    let mut actions = vec![StateAction::primary(icons::ARROW_SQUARE_OUT, OPEN_AT_HEAD)];
    if forward {
        actions.push(StateAction::secondary(icons::ARROW_RIGHT, SKIP_NEXT));
    } else if backward {
        actions.push(StateAction::secondary(icons::ARROW_LEFT, SKIP_PREVIOUS));
    }
    match states::warning_with_actions(ui, &text, &actions)? {
        0 => Some(Event::OpenAtHead),
        _ => Some(Event::Step(forward)),
    }
}

/// The pulsing dot and "Working · 3 s · `<latest step>`"; the dot pulses only while this shows.
fn working_line(ui: &mut Ui, text: &str) {
    let palette = theme::palette(ui.ctx());
    let height = buttons::SUBTLE_HEIGHT;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let dot = egui::pos2(rect.left() + style::SPACE_S, rect.center().y);
    let strength = if motion::is_reduced(ui.ctx()) {
        1.0
    } else {
        let time = ui.input(|input| input.time);
        ui.ctx().request_repaint_after(PULSE_FRAME);
        let phase = (time % PULSE_SECONDS / PULSE_SECONDS) as f32 * std::f32::consts::TAU;
        0.35 + 0.65 * (0.5 + 0.5 * phase.cos())
    };
    ui.painter().circle_filled(dot, 4.0, palette.accent.gamma_multiply(strength));
    let font = FontId::proportional(style::SMALL_SIZE);
    let anchor = egui::pos2(dot.x + style::SPACE_M, rect.center().y);
    let room = (rect.right() - anchor.x).max(0.0);
    let shown = elide::reference(ui, text, &font, room);
    ui.painter().text(anchor, Align2::LEFT_CENTER, shown, font, palette.text_secondary);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
}

/// The one-line follow-up and Send, or Stop while the agent answers.
fn follow_up(ui: &mut Ui, view: &Card<'_>, composer: &mut String) -> Option<Event> {
    let palette = theme::palette(ui.ctx());
    let id = Id::unique("assistant_mini_composer");
    let hint = if view.is_busy { ANSWERING } else { FOLLOW_UP };
    let has_text = !composer.trim().is_empty();
    let mut entered = false;
    let frame = egui::Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.border_strong))
        .corner_radius(style::RADIUS_CARD)
        .inner_margin(egui::Margin::same(style::SPACE_XS as i8));
    let shown = frame.show(ui, |ui| {
        row::with_trailing_block(
            ui,
            |ui| {
                if view.is_busy {
                    let stop = buttons::secondary_busy(ui, icons::SQUARE, "Stop", view.is_stopping);
                    (stop.clicked() && !view.is_stopping).then_some(Event::Stop)
                } else {
                    let send = ui.add_enabled_ui(has_text, |ui| {
                        buttons::primary_busy(ui, icons::PAPER_PLANE_RIGHT, "Send", false)
                    });
                    send.inner.on_hover_text("Send (Enter)").clicked().then_some(Event::Send)
                }
            },
            |ui| {
                let edit = TextEdit::singleline(composer)
                    .id(id)
                    .hint_text(hint)
                    .desired_width(f32::INFINITY)
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(4)));
                let response = ui.add_enabled(!view.is_busy, edit);
                entered = response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            },
        )
        .0
    });
    let sent = (entered && has_text && !view.is_busy).then_some(Event::Send);
    shown.inner.or(sent)
}

/// The pill laid out in `content`: "Assistant · N of M" (which drags it), the arrows and "Open
/// the answer".
pub(crate) fn pill(
    ui: &mut Ui,
    content: Rect,
    current: Option<usize>,
    count: usize,
) -> Option<Event> {
    let inner = content.shrink2(vec2(style::SPACE_M, style::SPACE_XS));
    let mut strip =
        ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::right_to_left(Align::Center)));
    strip.spacing_mut().item_spacing.x = style::SPACE_XS;
    let mut event = None;
    if icon_slot(&mut strip, icons::CARET_UP, OPEN, true) {
        event = Some(Event::Open);
    }
    if count > 0 {
        let mut next = None;
        if icon_slot(
            &mut strip,
            icons::ARROW_RIGHT,
            NEXT,
            super::step(current, count, true) != current,
        ) {
            next = Some(Event::Step(true));
        }
        if icon_slot(
            &mut strip,
            icons::ARROW_LEFT,
            PREVIOUS,
            super::step(current, count, false) != current,
        ) {
            next = Some(Event::Step(false));
        }
        event = event.or(next);
    }
    let label_rect =
        Rect::from_min_max(inner.min, egui::pos2(strip.min_rect().left(), inner.max.y));
    let text = pill_label(current, count);
    let drag = drag_handle(ui, label_rect, Id::unique("assistant_mini_pill_drag"));
    let palette = theme::palette(ui.ctx());
    let font = theme::fonts::semibold_font(style::BODY_SIZE);
    let anchor = egui::pos2(label_rect.left() + style::SPACE_XS, label_rect.center().y);
    ui.painter().text(anchor, Align2::LEFT_CENTER, &text, font, palette.text);
    ui.interact(label_rect, Id::unique("assistant_mini_pill_label"), Sense::hover())
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
    event.or(drag)
}

/// "Assistant · 2 of 4", or with no reference picked yet how many there are.
pub(crate) fn pill_label(current: Option<usize>, count: usize) -> String {
    match (current, count) {
        (_, 0) => "Assistant".to_owned(),
        (Some(index), _) => format!("Assistant · {} of {count}", index + 1),
        (None, _) => format!("Assistant · {}", count_label(None, count)),
    }
}

/// A drag handle over `rect`, laid under the buttons drawn after it: the move cursor, and the
/// drag's movement and end.
fn drag_handle(ui: &Ui, rect: Rect, id: Id) -> Option<Event> {
    let response = ui.interact(rect, id, Sense::drag());
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::Move);
    }
    if response.drag_stopped() {
        return Some(Event::Released);
    }
    // The press frame's movement came before the press: a slow frame takes a move and the press
    // on a button above the handle together, and the card would jump by the way the pointer went.
    let delta = if response.drag_started() { Vec2::ZERO } else { response.drag_delta() };
    (response.dragged() && delta != Vec2::ZERO).then_some(Event::Drag(delta))
}

/// An icon button centered in a square slot of [`SLOT`]; whether it was clicked.
fn icon_slot(ui: &mut Ui, icon: &str, label: &str, is_enabled: bool) -> bool {
    let (slot, _) = ui.allocate_exact_size(Vec2::splat(SLOT), Sense::hover());
    let button = Rect::from_center_size(slot.center(), Vec2::splat(buttons::icon_width()));
    let mut inner = ui.new_child(UiBuilder::new().max_rect(button));
    inner.add_enabled_ui(is_enabled, |ui| buttons::icon(ui, icon, label)).inner.clicked()
}
