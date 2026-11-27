//! Where the mini chat sits and how it gets there: one of the four corners of the Files tab's
//! content, [`INSET`] in from its edges. Dragged, it follows the pointer inside that content;
//! released, it glides to the nearest corner. Appearing it grows out of its corner, card and
//! pill change size into each other, and the fill fades onto a newly picked reference
//! ([`Motions`]). Each motion is timed from its start ([`progress`]), eases out, asks for frames
//! only while it runs and lands at once under reduced motion.

use cv_base::theme::motion;
use egui::{Context, Pos2, Rect, Vec2, pos2, vec2};

/// Room between the mini chat and the content's edges.
pub(crate) const INSET: f32 = 16.0;
/// The open card, as the design gives it; a smaller Files tab gets a smaller card.
pub(crate) const CARD: Vec2 = vec2(400.0, 520.0);
/// The pill the card shrinks to.
pub(crate) const PILL: Vec2 = vec2(300.0, 44.0);
/// The card's corner radius; the pill is fully rounded.
pub(crate) const CARD_RADIUS: f32 = 10.0;
/// Appearing, card ↔ pill and the snap to a corner (the design's timing).
pub(crate) const MOVE_SECONDS: f32 = 0.26;
/// The fill moving between references and the fade while it covers the cited lines.
pub(crate) const FADE_SECONDS: f32 = 0.2;
/// The card starts this small when it appears, growing out of its corner.
pub(crate) const APPEAR_SCALE: f32 = 0.92;

/// A corner of the Files tab's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Corner {
    pub left: bool,
    pub top: bool,
}

impl Corner {
    /// The corner of `room` nearest to `center`.
    pub(crate) fn nearest(room: Rect, center: Pos2) -> Self {
        Self { left: center.x < room.center().x, top: center.y < room.center().y }
    }

    /// The point of `room` this corner names.
    pub(crate) fn point(self, room: Rect) -> Pos2 {
        pos2(
            if self.left { room.left() } else { room.right() },
            if self.top { room.top() } else { room.bottom() },
        )
    }

    /// A rect of `size` in this corner of `room`, [`INSET`] in from its edges.
    pub(crate) fn place(self, room: Rect, size: Vec2) -> Rect {
        let x = if self.left { room.left() + INSET } else { room.right() - INSET - size.x };
        let y = if self.top { room.top() + INSET } else { room.bottom() - INSET - size.y };
        Rect::from_min_size(pos2(x, y), size)
    }
}

/// `size`, no larger than `room` leaves inside its insets.
pub(crate) fn fit(size: Vec2, room: Rect) -> Vec2 {
    let most = (room.size() - Vec2::splat(2.0 * INSET)).max(Vec2::ZERO);
    size.min(most)
}

/// A rect of `size` with its top left at `min`, moved inside `room` where it would leave it.
pub(crate) fn keep_inside(min: Pos2, size: Vec2, room: Rect) -> Rect {
    let most = (room.max - size).max(room.min);
    Rect::from_min_size(min.clamp(room.min, most), size)
}

/// A motion's clock: still, due to start with the next frame drawn, or running since a time.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum Clock {
    #[default]
    Still,
    Due,
    Since(f64),
}

impl Clock {
    /// The motion's progress this frame; a due one starts now, a finished one stops.
    fn run(&mut self, ctx: &Context, seconds: f32) -> f32 {
        let started = match *self {
            Self::Still => return 1.0,
            Self::Due => ctx.input(|input| input.time),
            Self::Since(started) => started,
        };
        let t = progress(ctx, started, seconds);
        *self = if t >= 1.0 { Self::Still } else { Self::Since(started) };
        t
    }
}

/// The mini chat this frame: its frame, its content at full size held at the frame's corner (a
/// growing frame reveals it instead of reflowing it), and how far it appeared and the fill faded.
pub(crate) struct Laid {
    pub frame: Rect,
    pub content: Rect,
    pub radius: f32,
    pub appear: f32,
    pub fill: f32,
}

/// The mini chat's motions and where it was drawn last.
#[derive(Default)]
pub(crate) struct Motions {
    frame: Option<Rect>,
    appear: Clock,
    fill: Clock,
    /// Card ↔ pill: the size it changes from and when.
    morph: Option<(Vec2, f64)>,
    /// The top left while dragged.
    dragged: Option<Pos2>,
    /// Released at this top left at this time, it glides to its corner.
    snap: Option<(Pos2, f64)>,
}

impl Motions {
    /// It opens: it appears with the next frame drawn, wherever a drag left it.
    pub(crate) fn appear(&mut self) {
        self.appear = Clock::Due;
        self.dragged = None;
        self.snap = None;
        self.morph = None;
    }

    /// Another reference was picked: its fill fades in.
    pub fn fill(&mut self) {
        self.fill = Clock::Due;
    }

    /// Card and pill swap: the size changes from the one drawn last.
    pub(crate) fn morph(&mut self, now: f64) {
        self.morph = self.frame.map(|frame| (frame.size(), now));
    }

    pub(crate) fn is_dragged(&self) -> bool {
        self.dragged.is_some()
    }

    /// The pointer moved it by `delta`.
    pub(crate) fn drag(&mut self, delta: Vec2) {
        self.snap = None;
        self.dragged = self.dragged.or(self.frame.map(|frame| frame.min)).map(|min| min + delta);
    }

    /// Let go in `room`: it glides from there to the corner nearest to it, which is returned.
    pub(crate) fn release(&mut self, room: Rect, now: f64) -> Option<Corner> {
        let min = self.dragged.take()?;
        let frame = keep_inside(min, self.frame?.size(), room);
        self.snap = Some((frame.min, now));
        Some(Corner::nearest(room, frame.center()))
    }

    /// Where it is this frame in `corner` of `room`, as a pill or a card.
    pub(crate) fn lay_out(
        &mut self,
        ctx: &Context,
        room: Rect,
        corner: Corner,
        is_pill: bool,
    ) -> Laid {
        let target = fit(if is_pill { PILL } else { CARD }, room);
        let size = match self.morph {
            Some((from, started)) => {
                let mut clock = Clock::Since(started);
                let t = clock.run(ctx, MOVE_SECONDS);
                self.morph = (clock != Clock::Still).then_some((from, started));
                from + (target - from) * t
            }
            None => target,
        };
        let placed = corner.place(room, size);
        let frame = match (self.dragged, self.snap) {
            (Some(min), _) => keep_inside(min, size, room),
            (None, Some((from, started))) => {
                let mut clock = Clock::Since(started);
                let t = clock.run(ctx, MOVE_SECONDS);
                self.snap = (clock != Clock::Still).then_some((from, started));
                Rect::from_min_size(from + (placed.min - from) * t, size)
            }
            (None, None) => placed,
        };
        self.frame = Some(frame);
        let content = Rect::from_min_size(
            pos2(
                if corner.left { frame.left() } else { frame.right() - target.x },
                if corner.top { frame.top() } else { frame.bottom() - target.y },
            ),
            target,
        );
        let radius = if is_pill { size.y / 2.0 } else { CARD_RADIUS.min(size.y / 2.0) };
        let appear = self.appear.run(ctx, MOVE_SECONDS);
        let fill = self.fill.run(ctx, FADE_SECONDS);
        Laid { frame, content, radius, appear, fill }
    }
}

/// Eased progress (0 to 1) of a motion of `seconds` that started at input time `started`; asks
/// for the next frame while it runs. At once under reduced motion.
pub(crate) fn progress(ctx: &Context, started: f64, seconds: f32) -> f32 {
    let total = motion::duration(ctx, seconds);
    let now = ctx.input(|input| input.time);
    if total <= 0.0 {
        return 1.0;
    }
    let linear = ((now - started) as f32 / total).clamp(0.0, 1.0);
    if linear < 1.0 {
        ctx.request_repaint();
    }
    motion::ease_out_cubic(linear)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: Rect = Rect::from_min_max(pos2(300.0, 100.0), pos2(1300.0, 900.0));

    #[test]
    fn a_release_snaps_to_the_nearest_corner() {
        let corner = |x: f32, y: f32| Corner::nearest(ROOM, pos2(x, y));
        assert_eq!(corner(400.0, 850.0), Corner { left: true, top: false });
        assert_eq!(corner(1200.0, 150.0), Corner { left: false, top: true });
        assert_eq!(corner(1200.0, 850.0), Corner { left: false, top: false });
        assert_eq!(corner(799.0, 499.0), Corner { left: true, top: true }, "just off the middle");
    }

    #[test]
    fn a_corner_places_the_card_inside_the_insets() {
        let bottom_right = Corner { left: false, top: false }.place(ROOM, CARD);
        assert_eq!(bottom_right.max, pos2(1300.0 - INSET, 900.0 - INSET));
        let top_left = Corner { left: true, top: true }.place(ROOM, PILL);
        assert_eq!(top_left.min, pos2(300.0 + INSET, 100.0 + INSET));
        assert_eq!(Corner { left: true, top: false }.point(ROOM), pos2(300.0, 900.0));
    }

    #[test]
    fn a_small_room_gets_a_smaller_card_and_a_drag_stays_inside() {
        let small = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 400.0));
        assert_eq!(fit(CARD, small), vec2(300.0 - 2.0 * INSET, 400.0 - 2.0 * INSET));
        assert_eq!(fit(PILL, ROOM), PILL);
        let dragged = keep_inside(pos2(1250.0, 50.0), CARD, ROOM);
        assert_eq!(dragged.min, pos2(1300.0 - CARD.x, 100.0));
    }
}
