//! A canvas the user moves by grabbing it and zooms, as in Figma or Azure DevOps' pipeline view.
//!
//! * A primary drag anywhere pans once it passes egui's click distance, also when it starts on a
//!   card or a control inside one: below the distance it is still a click, past it egui drops the
//!   click, so nothing fires. The content then follows the pointer 1:1 from where it was pressed.
//! * The middle button, and the primary button while Space is held, pan at once.
//! * Releasing a pan flings it for a short coast (none under reduced motion); a new press, the
//!   wheel or keyboard focus moving onto a card stops the coast.
//!   Repaints are requested only while it moves.
//! * The wheel scrolls as in a document (Shift for sideways), within the content's own bounds.
//! * Ctrl+wheel and a trackpad pinch zoom around the pointer, which keeps the point under it in
//!   place, between [`MIN_ZOOM`] and [`MAX_ZOOM`]. Ctrl+Plus/Minus step through [`ZOOM_STEPS`] and
//!   Ctrl+0 returns to 100 %, while the pointer is over the canvas or keyboard focus is in it; the
//!   control in the bottom-right corner does the same and fits the whole graph. Steps glide.
//! * Panning may leave the content's bounds, but always keeps [`KEEP_VISIBLE`] points of the
//!   content on screen, so the graph can't be lost at any zoom.
//!
//! The content is drawn on its own layer in canvas coordinates, and egui maps that layer to the
//! screen with the pan and zoom, for painting and hit-testing alike; the accessibility tree gets
//! the same transform. Text on it is laid out at its size on screen (see `sharp_text`), so it is
//! as crisp zoomed as at 100 %. The canvas is a drag-only widget under everything drawn on it,
//! so clicks reach the cards and drags reach the canvas; while it is dragged egui hovers nothing
//! else, so cards show no hover or tooltips mid-pan. Zoom and position last for the session.

use cv_base::theme::{self, motion};
use cv_widgets::surfaces;
use egui::emath::TSTransform;
use egui::{
    Context, CursorIcon, Id, Key, LayerId, Modifiers, PointerButton, Pos2, Rect, Response, Sense,
    Ui, UiBuilder, Vec2, vec2,
};

use super::{sharp_text, zoom_controls};

/// Content that always stays on screen, per axis (or all of it, when it is smaller).
const KEEP_VISIBLE: f32 = 120.0;
/// Time constant of the fling's exponential decay; it coasts `velocity × FLING_DECAY` points.
const FLING_DECAY: f32 = 0.16;
/// Time constant of the glide to a focused control or to a zoom step.
const GLIDE: f32 = 0.07;
/// Below this many points left to travel, movement ends in one step.
const SETTLE: f32 = 0.5;
/// Below this ratio left to zoom, a zoom glide ends in one step.
const ZOOM_SETTLE: f32 = 0.002;
/// How far outside the view a focused card control may sit and still be followed (the
/// overscan the graph builds beyond the view, and some).
const FOCUS_REACH: f32 = 160.0;
/// Room left around a control that keyboard focus brought into view.
const FOCUS_MARGIN: f32 = 16.0;
const INDICATOR_THICKNESS: f32 = 4.0;
const INDICATOR_INSET: f32 = 3.0;
const INDICATOR_MIN_LENGTH: f32 = 32.0;

pub(crate) const MIN_ZOOM: f32 = 0.25;
pub(crate) const MAX_ZOOM: f32 = 2.0;
/// The levels the zoom buttons and Ctrl+Plus/Minus step through, as in a browser.
pub(crate) const ZOOM_STEPS: [f32; 12] =
    [0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

#[derive(Clone, Copy)]
struct State {
    /// The point of the zoomed content shown at the viewport's top-left corner, in screen points.
    offset: Vec2,
    /// Screen points per canvas point.
    zoom: f32,
    /// Of `offset`, in points per second, while flinging.
    velocity: Vec2,
    /// Where `offset` glides to, to bring a focused control into view.
    target: Option<Vec2>,
    /// A zoom step or fit in progress.
    glide: Option<ZoomGlide>,
    grab: Option<Grab>,
    /// The focused widget last frame, to follow keyboard focus onto the canvas.
    focused: Option<Id>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            offset: Vec2::ZERO,
            zoom: 1.0,
            velocity: Vec2::ZERO,
            target: None,
            glide: None,
            grab: None,
            focused: None,
        }
    }
}

impl State {
    /// The zoom this state is at or gliding to.
    fn goal(&self) -> f32 {
        self.glide.map_or(self.zoom, |glide| glide.zoom)
    }

    /// Zooms to `zoom` at once, keeping the content under `anchor` (relative to the viewport) there.
    fn zoom_around(&mut self, zoom: f32, anchor: Vec2) {
        let point = (anchor + self.offset) / self.zoom;
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.offset = point * self.zoom - anchor;
    }

    /// Glides to `zoom`, keeping the content under `anchor` (relative to the viewport) there.
    fn glide_around(&mut self, zoom: f32, anchor: Vec2) {
        let point = (anchor + self.offset) / self.zoom;
        self.glide_to(zoom, point, anchor);
    }

    /// Glides to `zoom` while the canvas point `point` moves to `screen` (relative to the viewport).
    fn glide_to(&mut self, zoom: f32, point: Vec2, screen: Vec2) {
        self.glide = Some(ZoomGlide { zoom: zoom.clamp(MIN_ZOOM, MAX_ZOOM), point, screen });
        self.target = None;
        self.velocity = Vec2::ZERO;
    }

    /// Applies a zoom control or shortcut; `anchor` is where a step zooms around.
    fn apply(&mut self, command: Command, anchor: Vec2, content: Vec2, view: Vec2) {
        match command {
            Command::Step(steps) => self.glide_around(next_step(self.goal(), steps), anchor),
            Command::Reset => self.glide_around(1.0, anchor),
            Command::Fit => self.glide_to(fit_zoom(content, view), content / 2.0, view / 2.0),
        }
    }
}

#[derive(Clone, Copy)]
struct Grab {
    press: Pos2,
    offset: Vec2,
    /// Past the click distance (or a button that pans at once).
    is_panning: bool,
}

#[derive(Clone, Copy)]
struct ZoomGlide {
    zoom: f32,
    /// The canvas point that travels to `screen` while the zoom changes.
    point: Vec2,
    screen: Vec2,
}

/// What a zoom control or shortcut asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// Whole steps of [`ZOOM_STEPS`], negative to zoom out.
    Step(i32),
    /// Back to 100 %.
    Reset,
    /// The whole graph in view (never above 100 %), centered.
    Fit,
}

/// Offsets one axis pair may take.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bounds {
    min: Vec2,
    max: Vec2,
}

impl Bounds {
    /// Where a drag may take the view: anywhere that keeps some content on screen.
    fn free(content: Vec2, view: Vec2) -> Self {
        let keep = Vec2::splat(KEEP_VISIBLE).min(content);
        Self { min: (keep - view).min(Vec2::ZERO), max: (content - keep).max(Vec2::ZERO) }
    }

    /// The content's own extent, as a scroll area would allow.
    fn natural(content: Vec2, view: Vec2) -> Self {
        Self { min: Vec2::ZERO, max: (content - view).max(Vec2::ZERO) }
    }

    /// Grown to hold `offset`, so the wheel never jumps a panned view back.
    fn including(self, offset: Vec2) -> Self {
        Self { min: self.min.min(offset), max: self.max.max(offset) }
    }

    fn clamp(self, offset: Vec2) -> Vec2 {
        offset.max(self.min).min(self.max)
    }
}

/// One frame of the canvas, between [`begin`] and [`Pan::finish`].
pub(crate) struct Pan {
    /// Draws on the canvas, in canvas coordinates: one point is one point of content at 100 %.
    pub ui: Ui,
    /// The canvas on screen.
    rect: Rect,
    id: Id,
    content: Vec2,
    offset: Vec2,
    zoom: f32,
    /// The zoom changed this frame or is gliding.
    is_zooming: bool,
}

impl Pan {
    pub(crate) fn zoom(&self) -> f32 {
        self.zoom
    }

    /// The part of the canvas on screen, in canvas coordinates.
    pub fn viewport(&self) -> Rect {
        Rect::from_min_size((self.offset / self.zoom).to_pos2(), self.rect.size() / self.zoom)
    }

    /// Paints the position indicators over what was drawn on the canvas, lays its text out for
    /// the zoom and shows the zoom control.
    pub(crate) fn finish(self, ui: &Ui) {
        let Self { ui: canvas, rect, id, content, offset, zoom, is_zooming } = self;
        let layer = canvas.layer_id();
        let to_screen = TSTransform::new(rect.min.to_vec2() - offset, zoom);
        // On the canvas' own layer, above the cards, so mapped back from screen points.
        let to_canvas = to_screen.inverse();
        let palette = theme::palette(ui.ctx());
        let painter = canvas.painter();
        let color = palette.border_strong.gamma_multiply(0.8);
        let size = rect.size();
        let scaled = content * zoom;
        let natural = Bounds::natural(scaled, size);
        for axis in 0..2 {
            if scaled[axis] <= size[axis] + SETTLE {
                continue;
            }
            let along = size[axis] - INDICATOR_INSET * 2.0 - INDICATOR_THICKNESS;
            let length =
                (size[axis] / scaled[axis] * along).clamp(INDICATOR_MIN_LENGTH.min(along), along);
            let progress = (offset[axis] / natural.max[axis]).clamp(0.0, 1.0);
            let start = INDICATOR_INSET + (along - length) * progress;
            let across = size[1 - axis] - INDICATOR_INSET - INDICATOR_THICKNESS;
            let (min, extent) = if axis == 0 {
                (vec2(start, across), vec2(length, INDICATOR_THICKNESS))
            } else {
                (vec2(across, start), vec2(INDICATOR_THICKNESS, length))
            };
            let bar = to_canvas * Rect::from_min_size(rect.min + min, extent);
            painter.rect_filled(bar, bar.width().min(bar.height()) / 2.0, color);
        }
        let ctx = ui.ctx();
        sharp_text::sharpen(ctx, layer, id, zoom, sharp_text::text_zoom(zoom, is_zooming));
        place_accessible_bounds(ctx, &canvas, to_screen);

        let shown = rect.intersect(ui.clip_rect());
        if let Some(command) = zoom_controls::show(ctx, id, shown, zoom) {
            let mut state: State = ctx.data(|data| data.get_temp(id)).unwrap_or_default();
            state.apply(command, size / 2.0, content, size);
            ctx.data_mut(|data| data.insert_temp(id, state));
            ctx.request_repaint();
        }
    }
}

/// This frame's input as the canvas reads it.
#[derive(Clone, Copy)]
struct Frame {
    /// The canvas on screen.
    rect: Rect,
    pointer: Option<Pos2>,
    /// Space is held and no text field takes it: the primary button pans at once.
    is_space_down: bool,
    is_reduced: bool,
    is_hovered: bool,
}

/// Takes the rest of `ui` as the canvas for `content` points of content (at 100 %) and handles
/// this frame's pan, zoom, wheel and coast.
pub(crate) fn begin(ui: &mut Ui, id: Id, content: Vec2) -> Pan {
    let rect = ui.available_rect_before_wrap();
    ui.advance_cursor_after_rect(rect);
    let ctx = ui.ctx().clone();
    let mut state: State = ctx.data(|data| data.get_temp(id)).unwrap_or_default();
    let layer = LayerId::new(ui.layer_id().order, id.with("layer"));
    // The canvas widget sits on that layer, under the cards; egui decided this frame's
    // interaction from last frame's rects, so its response can be read before it is placed.
    let response = ctx.read_response(id);
    surfaces::mark_pan(ui, rect);
    let size = rect.size();
    let (pointer, dt, is_space_down) = ui.input(|input| {
        (input.pointer.latest_pos(), input.stable_dt.min(0.1), input.key_down(Key::Space))
    });
    let frame = Frame {
        rect,
        pointer,
        is_space_down: is_space_down && !ctx.egui_wants_keyboard_input(),
        is_reduced: motion::is_reduced(&ctx),
        is_hovered: ui.rect_contains_pointer(rect),
    };
    let zoom_before = state.zoom;

    if state.grab.is_none() {
        zoom_input(ui, &mut state, &frame, content);
    }
    zoom_glide(&mut state, dt, frame.is_reduced);
    let scaled = content * state.zoom;
    let free = Bounds::free(scaled, size);
    let natural = Bounds::natural(scaled, size);

    follow_focus(ui, &mut state, layer, rect, free);
    drag(ui, &mut state, &frame, response.as_ref(), free);
    if frame.is_hovered && state.grab.is_none() {
        wheel(ui, &mut state, natural);
    }
    if state.grab.is_none() {
        coast(&mut state, free, dt, frame.is_reduced);
    }
    state.offset = free.clamp(state.offset);
    // Exact: any change of the zoom, however small, needs the repaint below.
    let is_zooming = state.zoom.to_bits() != zoom_before.to_bits() || state.glide.is_some();
    if state.target.is_some() || state.velocity != Vec2::ZERO || is_zooming {
        // A zoom that just stopped needs one more frame to lay its text out at rest.
        ctx.request_repaint();
    }
    set_cursor(&ctx, id, &state, &frame, response.as_ref());

    let (offset, zoom) = (state.offset, state.zoom);
    ctx.data_mut(|data| data.insert_temp(id, state));
    let to_screen = TSTransform::new(rect.min.to_vec2() - offset, zoom);
    let canvas = canvas_ui(ui, id, layer, rect, to_screen);
    Pan { ui: canvas, rect, id, content, offset, zoom, is_zooming }
}

/// Ctrl+wheel or a pinch over the canvas zooms around the pointer; the zoom keys work while the
/// pointer is over the canvas or keyboard focus is in it.
fn zoom_input(ui: &Ui, state: &mut State, frame: &Frame, content: Vec2) {
    let rect = frame.rect;
    let pointer_in = frame.pointer.filter(|pos| rect.contains(*pos));
    if let Some(pos) = pointer_in {
        let factor = ui.input(egui::InputState::zoom_delta);
        // egui reports exactly 1.0 for a frame without zoom input.
        if factor.to_bits() != 1.0_f32.to_bits() {
            state.zoom_around(state.zoom * factor, pos - rect.min);
            state.glide = None;
            state.target = None;
            state.velocity = Vec2::ZERO;
        }
    }
    if pointer_in.is_some() || has_focus_in(ui.ctx(), rect) {
        let anchor = pointer_in.map_or(rect.size() / 2.0, |pos| pos - rect.min);
        if let Some(command) = zoom_keys(ui) {
            state.apply(command, anchor, content, rect.size());
        }
    }
}

/// Keyboard focus that moved onto a card control out of view brings it into view.
fn follow_focus(ui: &Ui, state: &mut State, layer: LayerId, rect: Rect, free: Bounds) {
    let ctx = ui.ctx();
    let focused = ctx.memory(egui::Memory::focused);
    if focused == state.focused {
        return;
    }
    state.focused = focused;
    let is_by_keyboard =
        !ui.input(|input| input.pointer.any_down() || input.pointer.any_released());
    if let Some(widget) =
        focused.filter(|_| is_by_keyboard).and_then(|id| on_canvas(ctx, layer, id, rect))
    {
        state.target = Some(free.clamp(state.offset + into_view(widget, rect)));
        state.velocity = Vec2::ZERO;
    }
}

/// A press grabs the canvas; past the click distance (at once for the middle button or with
/// Space) the content follows the pointer, and the release flings it.
fn drag(ui: &Ui, state: &mut State, frame: &Frame, response: Option<&Response>, free: Bounds) {
    let is_dragged = |button| response.is_some_and(|response| response.dragged_by(button));
    let drag_started = response.is_some_and(Response::drag_started);
    if drag_started && (is_dragged(PointerButton::Primary) || is_dragged(PointerButton::Middle)) {
        let press = ui
            .input(|input| input.pointer.press_origin())
            .or(frame.pointer)
            .unwrap_or(frame.rect.center());
        let is_panning = is_dragged(PointerButton::Middle) || frame.is_space_down;
        state.grab = Some(Grab { press, offset: state.offset, is_panning });
        state.velocity = Vec2::ZERO;
        state.target = None;
        state.glide = None;
    }
    let Some(grab) = state.grab.as_mut() else {
        return;
    };
    if response.is_some_and(Response::dragged) {
        if let Some(pos) = frame.pointer {
            let moved = pos - grab.press;
            let click_distance = ui.ctx().options(|options| options.input_options.max_click_dist);
            grab.is_panning |= moved.length() > click_distance;
            if grab.is_panning {
                state.offset = free.clamp(grab.offset - moved);
            }
        }
        return;
    }
    if grab.is_panning && response.is_some_and(Response::drag_stopped) && !frame.is_reduced {
        state.velocity = -ui.input(|input| input.pointer.velocity());
    }
    state.grab = None;
}

/// The wheel scrolls as in a document, within the content's own bounds; what it does not use is
/// left for the scroll areas around the canvas.
fn wheel(ui: &Ui, state: &mut State, natural: Bounds) {
    let delta = ui.input(egui::InputState::smooth_scroll_delta);
    if delta == Vec2::ZERO {
        return;
    }
    let next = natural.including(state.offset).clamp(state.offset - delta);
    let used = state.offset - next;
    ui.input_mut(|input| input.smooth_scroll_delta -= used);
    if used != Vec2::ZERO {
        state.offset = next;
        state.velocity = Vec2::ZERO;
        state.target = None;
        state.glide = None;
    }
}

/// The closed hand while panning, the open one where a press would pan.
fn set_cursor(ctx: &Context, id: Id, state: &State, frame: &Frame, response: Option<&Response>) {
    let is_on_control = is_clickable_under_pointer(ctx, id);
    let is_canvas_hovered = response.is_some_and(Response::hovered);
    match state.grab {
        Some(grab) if grab.is_panning || !is_on_control => {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
        }
        None if frame.is_hovered
            && (frame.is_space_down || (is_canvas_hovered && !is_on_control)) =>
        {
            ctx.set_cursor_icon(CursorIcon::Grab);
        }
        _ => {}
    }
}

/// The child `Ui` on the canvas layer, mapped to the screen by `to_screen`, with the canvas'
/// drag widget placed first so everything drawn later lies above it.
fn canvas_ui(ui: &mut Ui, id: Id, layer: LayerId, rect: Rect, to_screen: TSTransform) -> Ui {
    let ctx = ui.ctx().clone();
    ctx.set_sublayer(ui.layer_id(), layer);
    ctx.set_transform_layer(layer, to_screen);
    let to_canvas = to_screen.inverse();
    let mut canvas = ui.new_child(
        UiBuilder::new().id_salt(id.with("canvas")).layer_id(layer).max_rect(to_canvas * rect),
    );
    canvas.set_clip_rect(to_canvas * rect.intersect(ui.clip_rect()));
    // Drag only, not focusable: under the cards, so their clicks win and drags reach it.
    canvas.interact(to_canvas * rect, id, Sense::DRAG);
    canvas
}

/// Moves the accessibility bounds of the widgets drawn on the canvas layer to the screen. egui
/// reports them in layer coordinates. A transform on the canvas' own node would also apply to
/// every node nested under a card widget, such as a popup's options, which sit on an
/// untransformed layer of their own; so each bound on the layer is mapped instead.
fn place_accessible_bounds(ctx: &Context, canvas: &Ui, to_screen: TSTransform) {
    // The canvas has a node only while an accessibility client (or a test) reads the tree.
    if ctx.accesskit_node_builder(canvas.unique_id(), |_| ()).is_none() {
        return;
    }
    let layer = canvas.layer_id();
    let widgets: Vec<Id> = ctx.viewport(|viewport| {
        viewport.this_pass.widgets.get_layer(layer).map(|widget| widget.id).collect()
    });
    let (scale, shift) = (f64::from(to_screen.scaling), to_screen.translation);
    let (shift_x, shift_y) = (f64::from(shift.x), f64::from(shift.y));
    for id in widgets {
        ctx.accesskit_node_builder(id, |node| {
            if let Some(bounds) = node.bounds() {
                node.set_bounds(egui::accesskit::Rect {
                    x0: bounds.x0 * scale + shift_x,
                    y0: bounds.y0 * scale + shift_y,
                    x1: bounds.x1 * scale + shift_x,
                    y1: bounds.y1 * scale + shift_y,
                });
            }
        });
    }
}

/// Ctrl+Plus (or Ctrl+=), Ctrl+Minus and Ctrl+0, consumed.
fn zoom_keys(ui: &Ui) -> Option<Command> {
    ui.input_mut(|input| {
        if input.consume_key(Modifiers::COMMAND, Key::Plus)
            || input.consume_key(Modifiers::COMMAND, Key::Equals)
        {
            Some(Command::Step(1))
        } else if input.consume_key(Modifiers::COMMAND, Key::Minus) {
            Some(Command::Step(-1))
        } else if input.consume_key(Modifiers::COMMAND, Key::Num0) {
            Some(Command::Reset)
        } else {
            None
        }
    })
}

/// Keyboard focus is on a widget inside `canvas` (a card control or the zoom control).
fn has_focus_in(ctx: &Context, canvas: Rect) -> bool {
    let focused = ctx.memory(egui::Memory::focused).and_then(|id| ctx.read_response(id));
    focused.is_some_and(|response| {
        let to_screen =
            ctx.layer_transform_to_global(response.layer_id).unwrap_or(TSTransform::IDENTITY);
        canvas.contains_rect(to_screen * response.rect)
    })
}

/// The step `steps` away from `zoom` in [`ZOOM_STEPS`]; a zoom between steps moves to its neighbor.
pub(crate) fn next_step(zoom: f32, steps: i32) -> f32 {
    let mut zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    for _ in 0..steps.unsigned_abs() {
        zoom = if steps > 0 {
            ZOOM_STEPS.iter().copied().find(|step| *step > zoom + 0.001).unwrap_or(zoom)
        } else {
            ZOOM_STEPS.iter().rev().copied().find(|step| *step < zoom - 0.001).unwrap_or(zoom)
        };
    }
    zoom
}

/// The zoom that shows all of `content` in `view`: at most 100 %, at least [`MIN_ZOOM`].
fn fit_zoom(content: Vec2, view: Vec2) -> f32 {
    let fit = (view / content.max(Vec2::splat(1.0))).min_elem();
    fit.min(1.0).clamp(MIN_ZOOM, MAX_ZOOM)
}

/// One frame of a zoom glide: the zoom eases in log space, its point eases to its place.
fn zoom_glide(state: &mut State, dt: f32, is_reduced: bool) {
    let Some(glide) = state.glide else {
        return;
    };
    let ease = if is_reduced { 1.0 } else { 1.0 - (-dt / GLIDE).exp() };
    let screen_now = glide.point * state.zoom - state.offset;
    let mut zoom = (state.zoom.ln() + (glide.zoom.ln() - state.zoom.ln()) * ease).exp();
    let mut screen = screen_now + (glide.screen - screen_now) * ease;
    if (zoom / glide.zoom - 1.0).abs() < ZOOM_SETTLE && (glide.screen - screen).length() < SETTLE {
        zoom = glide.zoom;
        screen = glide.screen;
        state.glide = None;
    }
    state.zoom = zoom;
    state.offset = glide.point * zoom - screen;
}

/// One frame of the glide to a focused control, or of the fling after a pan.
fn coast(state: &mut State, free: Bounds, dt: f32, is_reduced: bool) {
    if let Some(target) = state.target {
        let next = if is_reduced {
            target
        } else {
            state.offset + (target - state.offset) * (1.0 - (-dt / GLIDE).exp())
        };
        if (target - next).length() < SETTLE {
            state.offset = target;
            state.target = None;
        } else {
            state.offset = next;
        }
        return;
    }
    if state.velocity == Vec2::ZERO {
        return;
    }
    // Exact integral of the decaying velocity over the frame, so the coast is frame-rate independent.
    let decayed = state.velocity * (-dt / FLING_DECAY).exp();
    let next = state.offset + (state.velocity - decayed) * FLING_DECAY;
    let clamped = free.clamp(next);
    state.velocity = decayed;
    for axis in 0..2 {
        let is_at_edge = next[axis] < free.min[axis] || next[axis] > free.max[axis];
        if is_at_edge {
            state.velocity[axis] = 0.0;
        }
    }
    state.offset = clamped;
    if state.velocity.length() * FLING_DECAY < SETTLE {
        state.velocity = Vec2::ZERO;
    }
}

/// The screen rect of `id` when it is a widget drawn on the canvas `layer` (clipped to the
/// canvas and near its viewport): a card control that keyboard focus moved to.
fn on_canvas(ctx: &Context, layer: LayerId, id: Id, canvas: Rect) -> Option<Rect> {
    let widget = ctx.read_response(id)?;
    if widget.layer_id != layer {
        return None;
    }
    let to_screen = ctx.layer_transform_to_global(layer).unwrap_or(TSTransform::IDENTITY);
    let (rect, interact_rect) = (to_screen * widget.rect, to_screen * widget.interact_rect);
    let is_clipped_to_canvas =
        !interact_rect.is_positive() || canvas.expand(SETTLE).contains_rect(interact_rect);
    let is_near = rect.intersects(canvas.expand(FOCUS_REACH));
    (is_clipped_to_canvas && is_near).then_some(rect)
}

/// The smallest move of the view that shows `widget` (with a margin) inside `canvas`; a widget
/// larger than the view shows its top-left part.
fn into_view(widget: Rect, canvas: Rect) -> Vec2 {
    let wanted = widget.expand(FOCUS_MARGIN);
    let mut delta = Vec2::ZERO;
    for axis in 0..2 {
        if wanted.max[axis] > canvas.max[axis] {
            delta[axis] = wanted.max[axis] - canvas.max[axis];
        }
        if wanted.min[axis] < canvas.min[axis] + delta[axis] {
            delta[axis] = wanted.min[axis] - canvas.min[axis];
        }
    }
    delta
}

/// A card, row or button under the pointer (other than the canvas) takes clicks there.
fn is_clickable_under_pointer(ctx: &Context, canvas: Id) -> bool {
    let under = ctx.interaction_snapshot(|snapshot| snapshot.contains_pointer.clone());
    under.iter().filter(|id| **id != canvas).any(|id| {
        ctx.read_response(*id)
            .is_some_and(|response| response.enabled() && response.sense.senses_click())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_bounds_keep_some_content_on_screen() {
        let bounds = Bounds::free(vec2(1400.0, 300.0), vec2(900.0, 500.0));
        assert_eq!(bounds.min, vec2(KEEP_VISIBLE - 900.0, KEEP_VISIBLE - 500.0));
        assert_eq!(bounds.max, vec2(1400.0 - KEEP_VISIBLE, 300.0 - KEEP_VISIBLE));
        // Content smaller than what must stay visible stays fully on screen.
        let tiny = Bounds::free(vec2(80.0, 60.0), vec2(900.0, 500.0));
        assert_eq!(tiny.min, vec2(80.0 - 900.0, 60.0 - 500.0));
        assert_eq!(tiny.max, Vec2::ZERO);
    }

    #[test]
    fn the_wheel_stays_in_the_content_but_never_jumps_a_panned_view_back() {
        let natural = Bounds::natural(vec2(1400.0, 300.0), vec2(900.0, 500.0));
        assert_eq!(natural.clamp(vec2(-50.0, 40.0)), vec2(0.0, 0.0));
        assert_eq!(natural.including(vec2(-50.0, 0.0)).clamp(vec2(-50.0, 0.0)), vec2(-50.0, 0.0));
    }

    #[test]
    fn focus_moves_the_view_just_enough() {
        let canvas = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 500.0));
        let right = Rect::from_min_size(egui::pos2(860.0, 100.0), vec2(240.0, 28.0));
        assert_eq!(into_view(right, canvas), vec2(1100.0 + FOCUS_MARGIN - 900.0, 0.0));
        let above = Rect::from_min_size(egui::pos2(100.0, -60.0), vec2(240.0, 28.0));
        assert_eq!(into_view(above, canvas), vec2(0.0, -60.0 - FOCUS_MARGIN));
        let huge = Rect::from_min_size(egui::pos2(100.0, 400.0), vec2(240.0, 900.0));
        assert_eq!(into_view(huge, canvas).y, 400.0 - FOCUS_MARGIN, "too tall: its top shows");
    }

    #[test]
    fn a_fling_decays_stops_at_the_edge_and_settles() {
        let free = Bounds::free(vec2(1400.0, 300.0), vec2(900.0, 500.0));
        let mut state = State { velocity: vec2(2000.0, 0.0), ..State::default() };
        let mut frames = 0;
        while state.velocity != Vec2::ZERO {
            coast(&mut state, free, 1.0 / 60.0, false);
            frames += 1;
            assert!(frames < 200, "the fling settles");
        }
        let distance = state.offset.x;
        assert!(
            (distance - 2000.0 * FLING_DECAY).abs() < 2.0,
            "coasts velocity × decay: {distance}"
        );

        let mut fast = State { velocity: vec2(0.0, -50_000.0), ..State::default() };
        coast(&mut fast, free, 1.0 / 60.0, false);
        assert_eq!(fast.offset.y, free.min.y, "stops at the edge");
        assert_eq!(fast.velocity.y, 0.0);
    }

    #[test]
    fn zoom_steps_walk_browser_levels_and_clamp() {
        assert_eq!(next_step(1.0, 1), 1.1);
        assert_eq!(next_step(1.0, -4), 0.5);
        assert_eq!(next_step(0.8, 1), 0.9, "between steps moves to the neighbor");
        assert_eq!(next_step(2.0, 2), MAX_ZOOM);
        assert_eq!(next_step(0.25, -1), MIN_ZOOM);
    }

    #[test]
    fn zooming_keeps_the_anchor_and_clamps_to_the_range() {
        let mut state = State { offset: vec2(300.0, 40.0), ..State::default() };
        let anchor = vec2(420.0, 180.0);
        let point = (anchor + state.offset) / state.zoom;
        state.zoom_around(1.7, anchor);
        assert!(
            (point * state.zoom - state.offset - anchor).length() < 1e-3,
            "the point under the anchor stays"
        );
        state.zoom_around(40.0, anchor);
        assert_eq!(state.zoom, MAX_ZOOM);
        assert!((point * state.zoom - state.offset - anchor).length() < 1e-3);
        state.zoom_around(0.01, anchor);
        assert_eq!(state.zoom, MIN_ZOOM);
    }

    #[test]
    fn a_zoom_glide_settles_on_its_goal_and_point() {
        let mut state = State::default();
        let (content, view) = (vec2(2000.0, 900.0), vec2(800.0, 500.0));
        state.apply(Command::Fit, Vec2::ZERO, content, view);
        let mut frames = 0;
        while state.glide.is_some() {
            zoom_glide(&mut state, 1.0 / 60.0, false);
            frames += 1;
            assert!(frames < 120, "the glide settles");
        }
        assert_eq!(state.zoom, 0.4);
        assert_eq!(state.offset, content / 2.0 * 0.4 - view / 2.0, "centered");
        assert_eq!(fit_zoom(vec2(300.0, 200.0), view), 1.0, "a small graph fits at 100 %");
        assert_eq!(fit_zoom(vec2(20_000.0, 200.0), view), MIN_ZOOM);
    }
}
