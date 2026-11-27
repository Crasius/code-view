//! The pull request's header slides away while the tab's content scrolls down. Scrolling up
//! scrolls the content first: the header comes back by scroll only once the content under the
//! pointer is at its top and the scroll goes on, by what the content left over. Bringing the
//! pointer past the tab bar, into the strip above it, glides it back. The tab bar stays and becomes
//! the top edge; the height the header frees goes to the content.
//!
//! How the header behaves is set per tab ([`HeaderMode`], Settings › Appearance). Hide on scroll
//! is all of the above. Always shown keeps the header whole on its tab: nothing there moves it.
//! Start minimized slides it away on entering the tab and lets it back only on demand: the peek
//! strip, the keyboard focus, an action or the merge button in use, as below, but never a scroll
//! up (nor Home), however far past the top it goes. Once it peeked, the next scroll down over the
//! content tucks it away again, as on a Hide on scroll tab.
//!
//! The header follows the scroll input over the content, not any tab's scroll area. On the way
//! down it takes its part of the frame's scroll delta before the content runs and leaves the rest
//! to the content, so the lines move exactly with the wheel or the fingers. On the way up it takes
//! what is left of the delta after the content ran: egui's scroll areas consume the delta while
//! they can scroll and pass it on at their edge to an enclosing scroll area. The content runs in a
//! scope that counts as one ([`HeaderReveal::show_content`]), so a trackpad at the top hands the
//! rest on instead of rubber-banding. Once the scrolling stops, a header caught halfway glides to
//! fully hidden or shown, in the direction of the last scroll that moved it.
//!
//! The pointer on the tab bar never brings the header back: it is aiming at a tab, and a header
//! gliding in would push the tab away from under it, whatever way the pointer came. Only the strip
//! above the tab bar's top edge peeks ([`peek_strip`]): the header's part still shown, and at
//! least [`PEEK_STRIP`] points of what lies above a hidden header (the app's top bar, or a banner
//! over the page). The pointer has to rest there a moment ([`PEEK_DWELL_SECONDS`]), so a flick at
//! the tabs that overshoots their top edge and turns back moves nothing, nor does a pointer
//! crossing on its way to the top bar. The header then stays until a scroll down over the content
//! hides it again; the pointer leaving the strip alone never does, so it does not flicker.
//!
//! Entering a tab (another tab, or the pull request opened or shown again) shows the header whole
//! at once, or on a Start minimized tab slides it away (eased; at once under reduced motion). A
//! header never laid out yet has no height to slide by: it slides away in the frame after its
//! first layout. The header also comes back on its own when the keyboard focus moves into it, one
//! of its actions runs (a refresh excepted: it changes nothing on screen) or the merge button is in
//! use (eased).
//! While a popup is open nothing moves. A header that is hidden is still laid out (clipped above
//! the tab bar), so Tab reaches its controls and brings it back.

use egui::{Id, Key, Modifiers, Pos2, Rangef, Rect, Ui, UiBuilder, UiKind, UiStackInfo};

use review_store::HeaderMode;

use cv_base::theme::motion;
use cv_runtime::route::PullRequestTab as Tab;

/// After this long without scroll input the scrolling counts as stopped, even without the end of
/// the gesture (wheels without phases).
const SETTLE_SECONDS: f64 = 0.15;

/// How long the pointer rests above the tab bar before the header glides back: our own choice,
/// long enough for an overshoot past the tabs to turn back or a pointer to pass on to the top bar,
/// short enough to read as an answer to the move.
const PEEK_DWELL_SECONDS: f64 = 0.2;

/// The least height of the strip above the tab bar that peeks, in points: our own choice. A hidden
/// header leaves no strip of its own, so this much of what lies above it (the bottom of the app's
/// top bar) stands in: enough to hit without aiming, little enough to leave the top bar's controls
/// mostly to themselves.
const PEEK_STRIP: f32 = 10.0;

/// What the page knows this frame that decides whether the header may move.
pub struct Frame {
    pub tab: Tab,
    /// How the header behaves on [`Frame::tab`], from the settings.
    pub mode: HeaderMode,
    /// The header is in use (the merge button armed or its options open): it stays.
    pub is_in_use: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Down,
    Up,
}

/// A glide to fully hidden (down) or shown (up).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Glide {
    from: f32,
    towards: Direction,
    started: f64,
}

#[derive(Debug, Default)]
pub struct HeaderReveal {
    /// Points of the header slid out of view; 0 shows it whole.
    hidden: f32,
    /// The header's full height when it was last laid out.
    height: f32,
    glide: Option<Glide>,
    last_direction: Option<Direction>,
    /// When the scroll last went over the content.
    last_scroll: f64,
    /// The tab shown last frame; another one is entered per its mode. `None` until the page shows.
    tab: Option<Tab>,
    /// The mode of the tab shown.
    mode: HeaderMode,
    /// A Start minimized tab was entered before the header had a height: it slides away once it
    /// has one.
    is_tuck_pending: bool,
    /// The widget with the keyboard focus last frame.
    focused: Option<Id>,
    /// The header's whole area last frame, the hidden part included.
    rect: Option<Rect>,
    /// The tab's content below the tab bar last frame: scrolling over it moves the header.
    content: Option<Rect>,
    /// The strip above the tab bar last frame where the resting pointer brings the header back.
    strip: Option<Rect>,
    /// Since when the pointer rests in the strip.
    near_since: Option<f64>,
    /// The header follows the scroll this frame: its tab lets it move and no popup is open.
    is_following: bool,
    /// The header's part slid out of view as drawn this frame.
    drawn_hidden: f32,
}

impl HeaderReveal {
    /// The page is shown again: the next frame enters its tab anew, as when it opened.
    pub fn reset(&mut self) {
        self.show_whole();
        self.tab = None;
    }

    /// Shows the header whole at once.
    fn show_whole(&mut self) {
        self.hidden = 0.0;
        self.glide = None;
        self.last_direction = None;
        self.near_since = None;
        self.is_tuck_pending = false;
    }

    /// Draws the header with `add_header`, slid up by the hidden part and clipped at its top
    /// edge, and takes the height still shown. `add_header` returns true when one of the header's
    /// actions ran. Call before the content runs: the header takes its part of the scroll here.
    pub fn show(&mut self, ui: &mut Ui, frame: &Frame, add_header: impl FnOnce(&mut Ui) -> bool) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|input| input.time);
        let seconds = f64::from(motion::duration(&ctx, motion::MEDIUM));
        self.follow(ui, frame, now, seconds);
        let hidden = self.hidden_at(now, seconds);
        if self.glide.is_some() {
            ctx.request_repaint();
        }

        let top = ui.cursor().top();
        let mut area = ui.max_rect();
        area.min.y = top - hidden;
        let mut header = ui.new_child(UiBuilder::new().id_salt("header").max_rect(area));
        header.shrink_clip_rect(Rect::everything_below(top));
        let has_acted = add_header(&mut header);
        let rect = header.min_rect();
        self.rect = Some(rect);
        self.height = rect.height();
        self.hidden = self.hidden.min(self.height);
        let shown = (self.height - hidden).clamp(0.0, self.height);
        self.drawn_hidden = self.height - shown;
        self.strip = Some(peek_strip(area.x_range(), top + shown, shown));
        ui.allocate_space(egui::vec2(ui.available_width(), shown));
        if has_acted {
            self.reveal(now, seconds);
        }
    }

    /// Remembers where the content starts, right below the tab bar at the cursor of `ui`.
    pub fn mark_content(&mut self, ui: &Ui) {
        let below = ui.cursor().top();
        self.content = Some(ui.max_rect().intersect(Rect::everything_below(below)));
    }

    /// Runs the tab's content with `add_content` in a scope that counts as a scroll area, so that
    /// its scroll areas hand on what they cannot scroll; then the header takes what is left of a
    /// scroll up. `has_band` is false while the band is left out (focus mode).
    pub fn show_content<R>(
        &mut self,
        ui: &mut Ui,
        has_band: bool,
        add_content: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        let is_following = has_band && self.is_following;
        // Only a Hide on scroll tab's header takes what the content leaves of a scroll up.
        let takes_scroll_up = is_following && self.mode == HeaderMode::HideOnScroll;
        // The scope is there either way, so the content's ids stay when the setting changes.
        let builder = if takes_scroll_up {
            UiBuilder::new().ui_stack_info(UiStackInfo::new(UiKind::ScrollArea))
        } else {
            UiBuilder::new()
        };
        let shown = ui.scope_builder(builder, add_content).inner;
        if takes_scroll_up && self.is_pointer_over_content(ui) {
            let ctx = ui.ctx();
            let (now, delta) = ctx.input(|input| (input.time, input.smooth_scroll_delta.y));
            let left = self.after_content(delta, now);
            ctx.input_mut(|input| input.smooth_scroll_delta.y = left);
            if left < delta {
                ctx.request_repaint();
            }
        }
        shown
    }

    /// Points of the header slid out of view as drawn this frame; 0 while it shows whole.
    pub fn drawn_hidden(&self) -> f32 {
        self.drawn_hidden
    }

    /// The header follows this frame's input: the scroll down over the content, the pointer in
    /// the strip above the tab bar, the keyboard paging of the Files tab, and whatever brings it back.
    fn follow(&mut self, ui: &Ui, frame: &Frame, now: f64, seconds: f64) {
        let ctx = ui.ctx();
        self.is_following = false;
        let has_entered = self.tab.replace(frame.tab) != Some(frame.tab);
        self.mode = frame.mode;
        if frame.mode == HeaderMode::AlwaysShown {
            self.show_whole();
            return;
        }
        if has_entered {
            self.enter(now, seconds);
        } else if self.is_tuck_pending {
            self.tuck(now, seconds);
        }
        if self.has_focus_moved_in(ctx) || frame.is_in_use {
            self.reveal(now, seconds);
        }
        if egui::Popup::is_any_open(ctx) {
            self.near_since = None;
            return;
        }
        self.is_following = true;
        if self.is_pointer_over_content(ui) {
            let delta = ctx.input(|input| input.smooth_scroll_delta.y);
            let left = self.before_content(delta, now);
            ctx.input_mut(|input| input.smooth_scroll_delta.y = left);
        }
        // The strip reaches above the page's clip rect: the pointer is checked against it as is.
        let pointer = ctx.input(|input| input.pointer.hover_pos());
        let is_near = pointer.zip(self.strip).is_some_and(|(at, strip)| is_in_strip(strip, at));
        if self.peek(is_near, now, seconds) {
            ctx.request_repaint_after_secs(PEEK_DWELL_SECONDS as f32);
        }
        if frame.tab == Tab::Files
            && let Some(direction) = paging_key(ui)
            && (direction == Direction::Down || frame.mode == HeaderMode::HideOnScroll)
        {
            self.glide_towards(direction, now, seconds);
        }
        let is_scrolling = ctx.input(egui::InputState::is_scrolling);
        if !is_scrolling && now - self.last_scroll >= SETTLE_SECONDS {
            self.settle(now, seconds);
        } else if self.is_partly_hidden() {
            ctx.request_repaint_after_secs(SETTLE_SECONDS as f32);
        }
    }

    /// Entering the tab: the header shows whole, or on a Start minimized tab slides away.
    fn enter(&mut self, now: f64, seconds: f64) {
        self.show_whole();
        if self.mode == HeaderMode::StartMinimized {
            self.tuck(now, seconds);
        }
    }

    /// Slides the header away, once it has a height to slide by.
    fn tuck(&mut self, now: f64, seconds: f64) {
        self.is_tuck_pending = self.height <= 0.0;
        if !self.is_tuck_pending {
            self.glide_towards(Direction::Down, now, seconds);
        }
    }

    fn is_pointer_over_content(&self, ui: &Ui) -> bool {
        self.content.is_some_and(|content| ui.rect_contains_pointer(content))
    }

    /// The keyboard focus moved to a control of the header this frame.
    fn has_focus_moved_in(&mut self, ctx: &egui::Context) -> bool {
        let focused = ctx.memory(egui::Memory::focused);
        let has_moved = focused.is_some() && focused != self.focused;
        self.focused = focused;
        has_moved
            && focused.and_then(|id| ctx.read_response(id)).is_some_and(|response| {
                self.rect.is_some_and(|rect| rect.intersects(response.rect))
            })
    }

    /// Before the content runs: a scroll down by `delta` (egui's sign: negative moves the content
    /// up) hides the header first; returns the part the content still scrolls by. A scroll up goes
    /// to the content whole.
    fn before_content(&mut self, delta: f32, now: f64) -> f32 {
        if delta != 0.0 {
            self.last_scroll = now;
        }
        if delta < 0.0 { self.scroll(delta, now) } else { delta }
    }

    /// After the content ran: what is left of a scroll up by `delta` (the content is at its top)
    /// brings the header back on a Hide on scroll tab; returns what the header left.
    fn after_content(&mut self, delta: f32, now: f64) -> f32 {
        let is_taken =
            self.mode == HeaderMode::HideOnScroll && delta > 0.0 && self.hidden_in_the_end() > 0.0;
        if is_taken { self.scroll(delta, now) } else { delta }
    }

    /// Moves the header by the scroll `delta` (positive moves the content down, as scrolling up
    /// does) and returns the part left for the content.
    fn scroll(&mut self, delta: f32, now: f64) -> f32 {
        self.glide = None;
        self.last_scroll = now;
        self.last_direction = Some(if delta < 0.0 { Direction::Down } else { Direction::Up });
        let hidden = (self.hidden - delta).clamp(0.0, self.height);
        let taken = hidden - self.hidden;
        self.hidden = hidden;
        delta + taken
    }

    /// The pointer is `is_near` the header, in the strip above the tab bar: once it rested there
    /// for the dwell, the header glides back. Returns true while the dwell still runs on a hidden
    /// header.
    fn peek(&mut self, is_near: bool, now: f64, seconds: f64) -> bool {
        if !is_near {
            self.near_since = None;
            return false;
        }
        let since = *self.near_since.get_or_insert(now);
        if now - since < PEEK_DWELL_SECONDS {
            return self.hidden_in_the_end() > 0.0;
        }
        self.reveal(now, seconds);
        false
    }

    /// The scrolling stopped: a header caught halfway glides on the way it was going.
    fn settle(&mut self, now: f64, seconds: f64) {
        if self.glide.is_none() && self.is_partly_hidden() {
            let direction = self.last_direction.unwrap_or(Direction::Up);
            self.glide_towards(direction, now, seconds);
        }
    }

    /// Glides back to shown, unless it is shown or already on the way.
    fn reveal(&mut self, now: f64, seconds: f64) {
        self.glide_towards(Direction::Up, now, seconds);
    }

    fn glide_towards(&mut self, direction: Direction, now: f64, seconds: f64) {
        let is_there = match self.glide {
            Some(glide) => glide.towards == direction,
            None => match direction {
                Direction::Down => self.hidden >= self.height,
                Direction::Up => self.hidden <= 0.0,
            },
        };
        if is_there {
            return;
        }
        let from = self.hidden_at(now, seconds);
        self.last_direction = Some(direction);
        self.glide = Some(Glide { from, towards: direction, started: now });
    }

    /// The hidden part at `now`, the glide stepped on; the glide ends once it arrived.
    fn hidden_at(&mut self, now: f64, seconds: f64) -> f32 {
        let Some(glide) = self.glide else {
            return self.hidden;
        };
        let to = match glide.towards {
            Direction::Down => self.height,
            Direction::Up => 0.0,
        };
        let progress = if seconds > 0.0 { ((now - glide.started) / seconds) as f32 } else { 1.0 };
        if progress >= 1.0 {
            self.hidden = to;
            self.glide = None;
            return self.hidden;
        }
        self.hidden = glide.from + (to - glide.from) * motion::ease_out_cubic(progress);
        self.hidden
    }

    /// The hidden part once a running glide arrived: a glide up counts as shown.
    fn hidden_in_the_end(&self) -> f32 {
        match self.glide {
            Some(Glide { towards: Direction::Up, .. }) => 0.0,
            Some(Glide { towards: Direction::Down, .. }) => self.height,
            None => self.hidden,
        }
    }

    fn is_partly_hidden(&self) -> bool {
        self.hidden > 0.0 && self.hidden < self.height
    }
}

/// The strip across `x` above the tab bar's top edge at `tabs_top` where the resting pointer
/// brings the header back: the header's `shown` part, and at least [`PEEK_STRIP`] points.
fn peek_strip(x: Rangef, tabs_top: f32, shown: f32) -> Rect {
    Rect::from_x_y_ranges(x, tabs_top - shown.max(PEEK_STRIP)..=tabs_top)
}

/// The pointer `at` is in the `strip`, strictly above its bottom: the tab bar's top row is the
/// tab bar's.
fn is_in_strip(strip: Rect, at: Pos2) -> bool {
    strip.contains(at) && at.y < strip.bottom()
}

/// The Files tab's keys that scroll the summary or the rendered diff (their modules take them
/// later in the frame): the keys down hide the header, Home shows it. The keys up only scroll the
/// content, as the wheel up does. Read without taking them, under the same conditions: no
/// modifier, and no field or button has the keyboard.
fn paging_key(ui: &Ui) -> Option<Direction> {
    let ctx = ui.ctx();
    if ctx.text_edit_focused() || ctx.memory(|memory| memory.focused().is_some()) {
        return None;
    }
    ui.input(|input| {
        let keys = [
            (Key::ArrowDown, Direction::Down),
            (Key::PageDown, Direction::Down),
            (Key::End, Direction::Down),
            (Key::Home, Direction::Up),
        ];
        let is_plain = input.modifiers == Modifiers::NONE;
        keys.into_iter().find(|(key, _)| is_plain && input.key_pressed(*key)).map(|(_, way)| way)
    })
}

#[cfg(test)]
mod tests {
    use egui::{Rangef, pos2};
    use review_store::HeaderMode;

    use super::{HeaderReveal, PEEK_DWELL_SECONDS, PEEK_STRIP, is_in_strip, peek_strip};

    const SECONDS: f64 = 0.2;

    fn header(height: f32) -> HeaderReveal {
        HeaderReveal { height, ..HeaderReveal::default() }
    }

    #[test]
    fn scrolling_down_hides_the_header_before_the_content_moves() {
        let mut reveal = header(80.0);
        assert_eq!(reveal.before_content(-30.0, 0.0), 0.0, "the header takes all of it");
        assert_eq!(reveal.hidden, 30.0);
        assert_eq!(reveal.before_content(-70.0, 0.1), -20.0, "the content scrolls by the rest");
        assert_eq!(reveal.hidden, 80.0);
        assert_eq!(reveal.before_content(-40.0, 0.2), -40.0, "a hidden header leaves it all");
        assert_eq!(reveal.after_content(-40.0, 0.2), -40.0, "and takes nothing afterwards");
    }

    #[test]
    fn scrolling_up_goes_to_the_content_first() {
        let mut reveal = header(80.0);
        reveal.hidden = 80.0;
        assert_eq!(reveal.before_content(10.0, 0.0), 10.0, "the content scrolls up first");
        assert_eq!(reveal.hidden, 80.0);
        // The content could scroll: it consumed the delta, nothing is left over.
        assert_eq!(reveal.after_content(0.0, 0.0), 0.0);
        assert_eq!(reveal.hidden, 80.0, "the header stays hidden");
        reveal.settle(1.0, SECONDS);
        assert_eq!(reveal.hidden_at(1.0 + SECONDS, SECONDS), 80.0, "and is not on its way back");
    }

    #[test]
    fn what_the_content_leaves_at_its_top_brings_the_header_back() {
        let mut reveal = header(80.0);
        reveal.hidden = 80.0;
        assert_eq!(reveal.after_content(10.0, 0.0), 0.0, "the header takes the rest");
        assert_eq!(reveal.hidden, 70.0);
        assert_eq!(reveal.after_content(100.0, 0.1), 30.0, "what it does not need is left");
        assert_eq!(reveal.hidden, 0.0);
        assert_eq!(reveal.after_content(5.0, 0.2), 5.0, "a shown header takes nothing");
    }

    #[test]
    fn a_header_caught_halfway_glides_the_way_the_scroll_went() {
        let mut reveal = header(80.0);
        reveal.before_content(-50.0, 0.0);
        reveal.settle(1.0, SECONDS);
        let midway = reveal.hidden_at(1.1, SECONDS);
        assert!(midway > 50.0 && midway < 80.0, "on its way: {midway}");
        assert_eq!(reveal.hidden_at(1.2, SECONDS), 80.0, "hidden once the glide is done");
        assert_eq!(reveal.glide, None);

        reveal.after_content(5.0, 2.0);
        reveal.settle(3.0, SECONDS);
        assert_eq!(reveal.hidden_at(3.2, SECONDS), 0.0, "a little scroll past the top shows it");
    }

    #[test]
    fn a_scroll_up_inside_the_content_lets_a_half_hidden_header_slide_on_out() {
        let mut reveal = header(80.0);
        reveal.before_content(-50.0, 0.0);
        reveal.before_content(20.0, 0.1);
        reveal.after_content(0.0, 0.1);
        reveal.settle(1.0, SECONDS);
        assert_eq!(reveal.hidden_at(1.2, SECONDS), 80.0);
    }

    #[test]
    fn without_motion_the_glide_lands_at_once() {
        let mut reveal = header(80.0);
        reveal.before_content(-50.0, 0.0);
        reveal.settle(1.0, 0.0);
        assert_eq!(reveal.hidden_at(1.0, 0.0), 80.0);
    }

    #[test]
    fn scrolling_takes_over_from_a_glide() {
        let mut reveal = header(80.0);
        reveal.hidden = 80.0;
        reveal.reveal(0.0, SECONDS);
        let midway = reveal.hidden_at(0.05, SECONDS);
        reveal.before_content(-10.0, 0.05);
        assert_eq!(reveal.glide, None);
        assert_eq!(reveal.hidden, (midway + 10.0).min(80.0));
        reveal.settle(1.0, SECONDS);
        assert_eq!(
            reveal.hidden_at(1.0 + SECONDS, SECONDS),
            80.0,
            "it glides on the way the scroll went"
        );
    }

    #[test]
    fn a_shown_header_does_not_glide() {
        let mut reveal = header(80.0);
        reveal.reveal(0.0, SECONDS);
        reveal.settle(0.0, SECONDS);
        assert_eq!(reveal.glide, None);
    }

    #[test]
    fn the_pointer_resting_at_the_top_edge_brings_the_header_back() {
        let mut reveal = header(80.0);
        reveal.hidden = 80.0;
        assert!(reveal.peek(true, 0.0, SECONDS), "the dwell runs");
        assert_eq!(reveal.glide, None, "a pointer passing by moves nothing");
        assert!(!reveal.peek(false, 0.05, SECONDS), "it left in time");
        reveal.peek(true, 1.0, SECONDS);
        assert!(!reveal.peek(true, 1.0 + 2.0 * PEEK_DWELL_SECONDS, SECONDS));
        assert_eq!(reveal.hidden_at(1.0 + 2.0 * PEEK_DWELL_SECONDS + SECONDS, SECONDS), 0.0);

        // Leaving the edge keeps it; only a scroll down hides it again.
        reveal.peek(false, 2.0, SECONDS);
        reveal.settle(2.0, SECONDS);
        assert_eq!(reveal.hidden_at(3.0, SECONDS), 0.0, "still shown after the pointer left");
        reveal.before_content(-80.0, 3.0);
        assert_eq!(reveal.hidden, 80.0);
    }

    #[test]
    fn entering_a_start_minimized_tab_slides_the_header_away() {
        let mut reveal = HeaderReveal { mode: HeaderMode::StartMinimized, ..header(80.0) };
        reveal.enter(0.0, SECONDS);
        assert_eq!(reveal.hidden_at(0.0, SECONDS), 0.0, "it starts shown and glides");
        assert_eq!(reveal.hidden_at(SECONDS, SECONDS), 80.0);
        reveal.enter(1.0, 0.0);
        assert_eq!(reveal.hidden_at(1.0, 0.0), 80.0, "without motion at once");

        // Before its first layout the header has no height: it slides away once it has one.
        let mut opened = HeaderReveal { mode: HeaderMode::StartMinimized, ..header(0.0) };
        opened.enter(0.0, 0.0);
        assert_eq!(opened.hidden_at(0.0, 0.0), 0.0);
        assert!(opened.is_tuck_pending);
        opened.height = 80.0;
        opened.tuck(0.1, 0.0);
        assert_eq!(opened.hidden_at(0.1, 0.0), 80.0);
    }

    #[test]
    fn entering_another_tab_shows_the_header_whole() {
        let mut reveal = header(80.0);
        reveal.hidden = 80.0;
        reveal.enter(0.0, SECONDS);
        assert_eq!(reveal.hidden_at(0.0, SECONDS), 0.0);
        assert_eq!(reveal.glide, None);
    }

    #[test]
    fn a_minimized_header_comes_back_only_on_demand() {
        let mut reveal = HeaderReveal { mode: HeaderMode::StartMinimized, ..header(80.0) };
        reveal.hidden = 80.0;
        assert_eq!(reveal.after_content(30.0, 0.0), 30.0, "a scroll up past the top is not its");
        assert_eq!(reveal.hidden, 80.0);

        reveal.peek(true, 1.0, SECONDS);
        reveal.peek(true, 1.0 + 2.0 * PEEK_DWELL_SECONDS, SECONDS);
        assert_eq!(reveal.hidden_at(2.0, SECONDS), 0.0, "the peek brings it back");
        assert_eq!(reveal.before_content(-30.0, 3.0), 0.0, "the next scroll down tucks it away");
        reveal.settle(4.0, SECONDS);
        assert_eq!(reveal.hidden_at(4.0 + SECONDS, SECONDS), 80.0);
    }

    #[test]
    fn only_the_strip_above_the_tab_bar_peeks() {
        let x = Rangef::new(0.0, 800.0);
        // The header hidden whole: the tab bar is the top edge at 40, under the app's top bar.
        let strip = peek_strip(x, 40.0, 0.0);
        assert!(!is_in_strip(strip, pos2(100.0, 40.0)), "the tab bar's top row aims at a tab");
        assert!(!is_in_strip(strip, pos2(100.0, 60.0)), "the tab bar itself never peeks");
        assert!(!is_in_strip(strip, pos2(100.0, 300.0)), "nor does the content");
        assert!(is_in_strip(strip, pos2(100.0, 39.0)), "just past the tabs' top edge peeks");
        assert!(is_in_strip(strip, pos2(100.0, 40.0 - PEEK_STRIP)), "up to the strip's top row");
        assert!(!is_in_strip(strip, pos2(100.0, 39.0 - PEEK_STRIP)), "the top bar above it not");
        assert!(!is_in_strip(strip, pos2(900.0, 39.0)), "nor beside the page");

        // A header still shown in part: all of its shown part peeks.
        let strip = peek_strip(x, 100.0, 60.0);
        assert!(is_in_strip(strip, pos2(100.0, 41.0)));
        assert!(!is_in_strip(strip, pos2(100.0, 100.0)));
    }

    #[test]
    fn an_overshoot_past_the_tabs_that_turns_back_moves_nothing() {
        let mut reveal = header(80.0);
        reveal.hidden = 80.0;
        reveal.peek(true, 0.0, SECONDS);
        reveal.peek(true, PEEK_DWELL_SECONDS * 0.8, SECONDS);
        reveal.peek(false, PEEK_DWELL_SECONDS * 0.9, SECONDS);
        reveal.peek(true, PEEK_DWELL_SECONDS * 1.5, SECONDS);
        assert_eq!(reveal.glide, None, "each stay was shorter than the dwell");
        assert_eq!(reveal.hidden_at(10.0, SECONDS), 80.0);
    }
}
