//! The find widget of a diff, as an editor has it: a small panel floating at the top right of the
//! pane that has the focus, over the code, and what it holds: the text searched for, the options,
//! the search running and the hits of the last one, and the current hit.
//!
//! It searches only the focused pane ([`Panes`], [`FindBar::follow_focus`]): side by side the
//! one last clicked or holding the selection (before either, the one under the pointer when the
//! widget opens, else the new side); a click into the other pane moves the widget and the search
//! there and keeps the text. One pane searches the whole diff.
//!
//! One row: the field with the Match case toggle inside its right end, the count, previous, next
//! and close, all Phosphor icons. Where the pane is narrow the count goes first, then the field
//! shrinks; the widget never leaves its pane, so it never covers the overview strip. Ctrl+F (⌘F)
//! opens it while the diff has the keyboard or the pointer, with the selected text in the field;
//! Esc or its close button closes it and hands the keyboard back to the diff. Enter and F3 go to
//! the next hit, Shift with either to the previous one, wrapping around; Alt+C switches Match
//! case while the widget has the keyboard. Typing searches once it pauses for `TYPING_PAUSE`;
//! the option searches at once.
//!
//! The widget paints after the rows, so it is on top of them and takes its own clicks; what its
//! buttons and Enter ask for is acted on at the start of the next frame (see [`FindBar::keys`]).

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use egui::{
    Align, Align2, Id, Key, Layout, Margin, Modifiers, Pos2, Rect, Response, RichText, Sense,
    Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2,
};

use super::{Found, Hit, Hits, MatchOptions, Query};
use cv_base::platform::{DIFF_FIND_KEY, shortcut};
use cv_base::theme::{self, icons, style};
use cv_code::find_paint::HitSide;
use cv_widgets::find_bar::{self, FieldLook, Step};
use cv_widgets::{buttons, focus};

/// How long typing has to pause before the text is searched (our own choice).
const TYPING_PAUSE: Duration = Duration::from_millis(160);
/// The widget's distance from the pane's top and right edge.
const INSET: f32 = style::SPACE_S;
const PADDING: f32 = style::SPACE_XS;
/// The field is a control's height; the buttons beside it are smaller.
const HEIGHT: f32 = style::CONTROL_HEIGHT + 2.0 * PADDING;
const FIELD_WIDTH: f32 = 260.0;
/// Below this the field keeps its toggles and a few characters.
const FIELD_MIN_WIDTH: f32 = 150.0;
/// The Match case toggle inside the field.
const TOGGLE: f32 = 22.0;
/// Room kept for the count, so "12 of 340" does not move the buttons.
const COUNT_WIDTH: f32 = 76.0;
const NEXT_HINT: &str = concat!("Next match (", shortcut!(Enter), ", F3)");
const PREVIOUS_HINT: &str =
    concat!("Previous match (", shortcut!(shift Enter), ", ", shortcut!(shift "F3"), ")");
const CLOSE_HINT: &str = "Close find (Esc)";
const CASE_HINT: &str = concat!("Match case (", shortcut!(alt "C"), ")");

/// The panes of a view, each searched on its own.
#[derive(Debug, Clone, Copy)]
pub enum Panes {
    /// One pane showing the whole diff.
    One(Rect),
    /// The old version on the left, the new one on the right.
    Two { left: Rect, right: Rect },
}

impl Panes {
    /// The rect of the pane showing `side`.
    fn rect(&self, side: HitSide) -> Rect {
        match (*self, side) {
            (Self::One(rect), _) => rect,
            (Self::Two { left, .. }, HitSide::Old) => left,
            (Self::Two { right, .. }, HitSide::New | HitSide::Both) => right,
        }
    }

    /// The side of the pane at `position`.
    fn side_at(&self, position: Pos2) -> Option<HitSide> {
        match *self {
            Self::One(rect) => rect.contains(position).then_some(HitSide::Both),
            Self::Two { left, right } => (left.contains(position).then_some(HitSide::Old))
                .or_else(|| right.contains(position).then_some(HitSide::New)),
        }
    }
}

/// Whether the widget shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Shown {
    #[default]
    Closed,
    Open,
    /// Esc closed it this frame, for [`FindBar::show`] to report.
    ClosedByKey,
}

/// The find widget of one diff view.
#[derive(Default)]
pub struct FindBar {
    shown: Shown,
    text: String,
    options: MatchOptions,
    /// The focused pane side by side (see [`Self::follow_focus`]).
    side: Option<HitSide>,
    /// When the text last changed (input time); it is searched once typing pauses.
    typed_at: Option<f64>,
    /// The search running: its query and source, and where its hits arrive.
    running: Option<(Query, u64, Receiver<Found>)>,
    found: Option<Found>,
    current: Option<usize>,
    /// The field takes the keyboard, its text selected, when the widget shows next.
    wants_focus: bool,
    /// The keyboard was in the widget last frame.
    keyboard_in_bar: bool,
    /// A step the widget's buttons or Enter asked for, taken by the next [`Self::keys`].
    queued: Option<Step>,
    /// Where the widget was painted last: presses there are its own, not the pane's.
    painted: Option<Rect>,
}

impl FindBar {
    /// Ctrl+F, while the view `owns_keys` (it has the keyboard or the pointer) or the widget has
    /// the keyboard: opens the widget with `selected` text, if one line; Esc in it closes it. While
    /// it is open F3 and Shift+F3 step, and with the keyboard in it Alt+C switches Match case. The
    /// step to take: these keys' or the one the widget asked for last frame.
    pub fn keys(
        &mut self,
        ui: &Ui,
        owns_keys: bool,
        selected: impl FnOnce() -> Option<String>,
    ) -> Option<Step> {
        // Esc takes the keyboard from any widget as the frame starts, and the diff's own keys
        // would take it next: whether it was in the widget is known from the frame before.
        let escape = ui.input(|input| input.key_pressed(Key::Escape));
        if self.is_open() && escape && std::mem::take(&mut self.keyboard_in_bar) {
            ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
            self.close();
            self.shown = Shown::ClosedByKey;
        }
        let in_bar = has_keyboard(ui.ctx()) || self.keyboard_in_bar;
        let shift = ui.input(|input| input.modifiers.shift);
        // Ctrl+Shift+F belongs to the frame statistics.
        let find = (owns_keys || in_bar)
            && !shift
            && ui.input_mut(|input| input.consume_shortcut(&DIFF_FIND_KEY));
        if find {
            self.open(selected());
        }
        if self.is_open()
            && in_bar
            && ui.input_mut(|input| input.consume_key(Modifiers::ALT, Key::C))
        {
            self.options.match_case = !self.options.match_case;
        }
        let goes_on = (owns_keys || in_bar) && self.is_open() && self.found.is_some();
        let next = goes_on && ui.input(|input| input.key_pressed(Key::F3));
        self.queued.take().or_else(|| next.then(|| Step::with_shift(shift)))
    }

    /// Opens the widget (again) with the field taking the keyboard; a selected line becomes the
    /// text.
    pub(crate) fn open(&mut self, selected: Option<String>) {
        if let Some(text) = selected.filter(|text| !text.is_empty() && !text.contains('\n'))
            && text != self.text
        {
            self.text = text;
            self.typed_at = None;
        }
        self.shown = Shown::Open;
        self.wants_focus = true;
    }

    fn close(&mut self) {
        self.shown = Shown::Closed;
        self.running = None;
        self.found = None;
        self.current = None;
        self.painted = None;
    }

    /// Follows the focus among `panes`: side by side a press into a pane (not on the widget) or
    /// the pane of the `selected` text focuses it; before either, the pane under the pointer when
    /// the widget opens, else the new side. The side searched.
    pub fn follow_focus(&mut self, ui: &Ui, panes: &Panes, selected: Option<HitSide>) -> HitSide {
        if matches!(panes, Panes::One(_)) {
            return HitSide::Both;
        }
        let press = ui.input(|input| {
            input.pointer.primary_pressed().then(|| input.pointer.interact_pos()).flatten()
        });
        let on_widget = |position: &Pos2| self.painted.is_some_and(|rect| rect.contains(*position));
        let pressed = press.filter(|position| !on_widget(position));
        let hovered = || ui.ctx().pointer_hover_pos().and_then(|position| panes.side_at(position));
        let chosen = selected.or_else(|| pressed.and_then(|position| panes.side_at(position)));
        if let Some(side) = chosen.filter(|side| *side != HitSide::Both) {
            self.side = Some(side);
        } else if self.side.is_none() && self.is_open() {
            self.side = Some(hovered().unwrap_or(HitSide::New));
        }
        self.side.or_else(hovered).unwrap_or(HitSide::New)
    }

    /// How far down from its pane's top the open widget covers the code: a hit there is not in
    /// view.
    pub fn cover(&self) -> f32 {
        if self.painted.is_some() { INSET + HEIGHT } else { 0.0 }
    }

    /// What the widget searches for in the version `side`.
    pub(crate) fn query(&self, side: HitSide) -> Query {
        Query { text: self.text.clone(), options: self.options, side }
    }

    /// Starts the search for the widget's text in `side` over the view's `source` text when it is
    /// not the one found or running, once typing paused: `start` runs it (see [`super::spawn`]).
    /// A closed widget or an empty field searches nothing.
    pub fn refresh(
        &mut self,
        ctx: &egui::Context,
        side: HitSide,
        source: u64,
        start: impl FnOnce(Query) -> Receiver<Found>,
    ) {
        let query = self.query(side);
        if !self.is_open() || query.text.is_empty() {
            self.running = None;
            self.found = None;
            self.current = None;
            return;
        }
        let is_running = self
            .running
            .as_ref()
            .is_some_and(|(running, from, _)| *running == query && *from == source);
        let is_found =
            self.found.as_ref().is_some_and(|found| found.query == query && found.source == source);
        if is_running || is_found {
            return;
        }
        if let Some(typed) = self.typed_at {
            let waited = ctx.input(|input| input.time) - typed;
            let left = TYPING_PAUSE.as_secs_f64() - waited;
            if left > 0.0 {
                ctx.request_repaint_after(Duration::from_secs_f64(left));
                return;
            }
        }
        self.typed_at = None;
        let receiver = start(query.clone());
        self.running = Some((query, source, receiver));
    }

    /// Takes in a finished search; true when its hits arrived this frame (no hit is current yet).
    pub fn poll(&mut self) -> bool {
        let Some((_, _, receiver)) = &self.running else {
            return false;
        };
        match receiver.try_recv() {
            Ok(found) => {
                self.running = None;
                self.found = Some(found);
                self.current = None;
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.running = None;
                false
            }
        }
    }

    fn is_open(&self) -> bool {
        self.shown == Shown::Open
    }

    /// The hits of the last search, while the widget is open.
    pub fn hits(&self) -> Option<&Hits> {
        self.found.as_ref().map(|found| &found.hits)
    }

    pub fn current(&self) -> Option<usize> {
        self.current
    }

    pub fn set_current(&mut self, current: Option<usize>) {
        self.current = current;
    }

    /// Moves to the next or previous hit, wrapping around; the hit to go to.
    pub fn step(&mut self, step: Step) -> Option<&Hit> {
        let hits = &self.found.as_ref()?.hits.list;
        self.current = find_bar::step_index(self.current, hits.len(), step);
        hits.get(self.current?)
    }

    /// The widget over the focused pane of `panes` (searching `side`), after the rows painted.
    /// True when it closed: the view takes the keyboard back.
    pub fn show(&mut self, ui: &mut Ui, panes: &Panes, side: HitSide) -> bool {
        if !self.is_open() {
            return std::mem::take(&mut self.shown) == Shown::ClosedByKey;
        }
        let pane = panes.rect(side);
        let fit = Fit::of(pane.width() - 2.0 * INSET);
        let rect = Rect::from_min_size(
            pos2((pane.right() - INSET - fit.width).max(pane.left()), pane.top() + INSET),
            vec2(fit.width, HEIGHT),
        );
        let clip = pane.intersect(ui.clip_rect());
        let palette = theme::palette(ui.ctx());
        let painter = ui.painter().with_clip_rect(clip);
        painter.add(style::card_shadow(&palette).as_shape(rect, style::RADIUS_CARD));
        painter.rect(
            rect,
            style::RADIUS_CARD,
            palette.surface,
            Stroke::new(1.0, palette.border),
            StrokeKind::Inside,
        );
        // Presses on the panel stay its own, not the code's below it.
        ui.interact(rect, ui.scope_id().with("find_widget"), Sense::click());
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink(PADDING))
                .layout(Layout::left_to_right(Align::Center))
                .id_salt("find_widget"),
        );
        child.set_clip_rect(clip);
        let closed = self.show_controls(&mut child, &fit);
        if closed {
            self.close();
            return true;
        }
        self.painted = Some(rect);
        self.keyboard_in_bar = ui.memory(egui::Memory::focused).is_some_and(|id| {
            ui.ctx().read_response(id).is_some_and(|response| rect.intersects(response.rect))
        });
        #[cfg(any(test, feature = "test-support"))]
        note_bar(ui.ctx(), BarNote { rect, pane, side, count: self.count_text() });
        false
    }

    /// The field with Match case, the count, the steps and close; true when close was clicked.
    fn show_controls(&mut self, ui: &mut Ui, fit: &Fit) -> bool {
        let margin = Margin {
            left: style::SPACE_S as i8,
            right: (TOGGLE + 2.0 * style::SPACE_XS) as i8,
            top: 5,
            bottom: 5,
        };
        let look =
            FieldLook { id: field_id(), hint: "Find", width: fit.field, margin: Some(margin) };
        let field = find_bar::field(ui, &mut self.text, &look);
        if field.response.changed() {
            self.typed_at = Some(ui.input(|input| input.time));
        }
        if std::mem::take(&mut self.wants_focus) {
            find_bar::focus_all(ui, field_id(), &self.text);
        }
        if let Some(step) = field.submitted {
            self.queue(ui, step);
        }
        let field_rect = field.response.rect;
        let count = self.count_text();
        if !fit.has_count && !count.is_empty() {
            // The count where it has no room of its own.
            field.response.on_hover_text(&count);
        }
        let toggle = Rect::from_center_size(
            pos2(field_rect.right() - style::SPACE_XS - TOGGLE / 2.0, field_rect.center().y),
            vec2(TOGGLE, TOGGLE),
        );
        let case = &mut self.options.match_case;
        if case_toggle(ui, toggle, *case).on_hover_text(CASE_HINT).clicked() {
            *case = !*case;
        }
        if fit.has_count {
            ui.add_space(style::SPACE_XS);
            count_label(ui, &count);
        }
        let previous = buttons::icon(ui, icons::ARROW_UP, PREVIOUS_HINT).clicked();
        let next = buttons::icon(ui, icons::ARROW_DOWN, NEXT_HINT).clicked();
        if previous || next {
            self.queue(ui, if next { Step::Next } else { Step::Previous });
        }
        buttons::icon(ui, icons::X, CLOSE_HINT).clicked()
    }

    /// A step for the next frame, which comes at once.
    fn queue(&mut self, ui: &Ui, step: Step) {
        self.queued = Some(step);
        ui.ctx().request_repaint();
    }

    /// "3 of 12", "12 matches" before one is current, or that nothing was found; empty before the
    /// first search ends.
    fn count_text(&self) -> String {
        let Some(found) = &self.found else {
            return String::new();
        };
        let count = found.hits.list.len();
        match (count, self.current) {
            (0, _) => "No results".to_owned(),
            (1, None) => "1 match".to_owned(),
            (_, None) => format!("{count} matches"),
            (_, Some(current)) => format!("{} of {count}", current + 1),
        }
    }
}

/// How the widget fits a pane: its width, the field's, and whether the count shows besides the
/// field, the steps and close.
struct Fit {
    width: f32,
    field: f32,
    has_count: bool,
}

impl Fit {
    /// The widget in `room`: the count goes first, then the field shrinks.
    fn of(room: f32) -> Self {
        let button = buttons::icon_width();
        let spacing = style::SPACE_XS;
        let width = |field: f32, has_count: bool| {
            2.0 * PADDING
                + field
                + if has_count { spacing + COUNT_WIDTH } else { 0.0 }
                + 3.0 * (button + spacing)
        };
        let mut fit = Self { width: 0.0, field: 0.0, has_count: true };
        if width(FIELD_MIN_WIDTH, true) > room {
            fit.has_count = false;
        }
        let rest = width(0.0, fit.has_count);
        fit.field = (room - rest).clamp(0.0, FIELD_WIDTH);
        fit.width = (rest + fit.field).min(room.max(0.0));
        fit
    }
}

/// The widget has the keyboard: its field is focused.
pub fn has_keyboard(ctx: &egui::Context) -> bool {
    ctx.memory(|memory| memory.has_focus(field_id()))
}

/// The count in its fixed slot, so a longer count does not move the buttons.
fn count_label(ui: &mut Ui, text: &str) {
    let palette = theme::palette(ui.ctx());
    let label = RichText::new(text).size(style::SMALL_SIZE).color(palette.text_secondary);
    ui.allocate_ui_with_layout(
        vec2(COUNT_WIDTH, ui.available_height()),
        Layout::left_to_right(Align::Center),
        |ui| ui.add(egui::Label::new(label).truncate()),
    );
}

/// Match case as a small icon toggle at `rect` inside the field: while on, tinted and outlined in
/// the accent (not by color alone), reported as a selected or unselected button.
fn case_toggle(ui: &mut Ui, rect: Rect, is_on: bool) -> Response {
    let response = ui.interact(rect, ui.scope_id().with("match_case"), Sense::click());
    let palette = theme::palette(ui.ctx());
    if is_on {
        ui.painter().rect(
            rect,
            style::RADIUS_CONTROL,
            palette.selection,
            Stroke::new(1.0, palette.accent),
            StrokeKind::Inside,
        );
    } else if response.hovered() {
        ui.painter().rect_filled(rect, style::RADIUS_CONTROL, palette.hover);
    }
    let color = if is_on { palette.accent_text } else { palette.text_secondary };
    let font = icons::state_font(&palette, icons::SIZE_SMALL, is_on);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icons::TEXT_AA, font, color);
    focus::ring(ui, &response, rect);
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, is_on, "Match case"));
    response
}

/// The id of the widget's field; one diff view shows at a time.
fn field_id() -> Id {
    Id::unique("diff_find_field")
}

/// What the widget showed last, for tests.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, PartialEq)]
pub struct BarNote {
    /// The widget.
    pub rect: Rect,
    /// The pane it floats over and searches.
    pub pane: Rect,
    /// The version searched.
    pub side: HitSide,
    pub count: String,
}

#[cfg(any(test, feature = "test-support"))]
fn note_bar(ctx: &egui::Context, note: BarNote) {
    let note = (ctx.cumulative_pass_nr(), note);
    ctx.data_mut(|data| data.insert_temp(Id::unique(BAR_NOTE), note));
}

/// What the last frame showed of the widget (see [`BarNote`]); `None` while it is closed.
#[cfg(any(test, feature = "test-support"))]
pub fn last_bar(ctx: &egui::Context) -> Option<BarNote> {
    let (pass, note): (u64, BarNote) = ctx.data(|data| data.get_temp(Id::unique(BAR_NOTE)))?;
    (pass + 1 == ctx.cumulative_pass_nr()).then_some(note)
}

/// Where the widget is painted this pass, for the layout checks (`app::clip_check`): an opaque
/// panel over the code.
#[cfg(any(test, feature = "test-support"))]
pub fn painted_now(ctx: &egui::Context) -> Option<Rect> {
    let (pass, note): (u64, BarNote) = ctx.data(|data| data.get_temp(Id::unique(BAR_NOTE)))?;
    (pass == ctx.cumulative_pass_nr()).then_some(note.rect)
}

#[cfg(any(test, feature = "test-support"))]
const BAR_NOTE: &str = "diff_find_bar";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_narrow_pane_drops_the_count_then_shrinks_the_field() {
        let wide = Fit::of(800.0);
        assert!(wide.has_count);
        assert_eq!(wide.field, FIELD_WIDTH);
        let mid = Fit::of(wide.width - FIELD_WIDTH + FIELD_MIN_WIDTH - 1.0);
        assert!(!mid.has_count, "the count goes first");
        assert!(mid.field >= FIELD_MIN_WIDTH, "the field keeps its least width then");
        for room in [800.0, 420.0, 300.0, 200.0, 120.0, 40.0] {
            let fit = Fit::of(room);
            assert!(fit.width <= room, "{room}: {} fits", fit.width);
        }
    }
}
