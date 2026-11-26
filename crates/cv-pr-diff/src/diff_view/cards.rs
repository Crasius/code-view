//! Comment cards between diff rows, shared by the text diff and the rendered Markdown diff: a card
//! is laid out where it is painted, its height is kept by key, and a card whose height changes
//! because a comment was handed on (posted, answered by the platform, failed) eases to its new
//! height (see [`settle`]) instead of moving the rows below it in one jump.

use std::collections::HashMap;

use egui::{Rect, Ui, UiBuilder};

use cv_base::theme;
use cv_widgets::settle::{self, Tween};

/// What a card's row counts with until the card was laid out once.
pub const DEFAULT_CARD_HEIGHT: f32 = 140.0;
/// Room above and below a card inside its row.
pub const CARD_GAP: f32 = 8.0;
pub const CARD_MAX_WIDTH: f32 = 880.0;

/// Measured card heights by key, and the cards easing to a new one after a hand-off.
#[derive(Default)]
pub struct CardHeights {
    measured: HashMap<String, f32>,
    tweens: HashMap<String, Tween>,
    /// The clock and settle duration of this pass, for the eases.
    now: f64,
    duration: f32,
}

impl CardHeights {
    /// The clock and settle duration of this pass.
    pub fn begin(&mut self, ctx: &egui::Context) {
        self.begin_at(ctx.input(|input| input.time), settle::duration(ctx));
    }

    fn begin_at(&mut self, now: f64, duration: f32) {
        self.now = now;
        self.duration = duration;
    }

    /// The height the card's row has this pass: easing, or as measured.
    pub fn shown(&self, key: &str) -> Option<f32> {
        match self.tweens.get(key) {
            Some(tween) => Some(tween.shown(self.now, self.duration)),
            None => self.measured.get(key).copied(),
        }
    }

    pub fn is_easing(&self, key: &str) -> bool {
        self.tweens.contains_key(key)
    }

    pub fn any_easing(&self) -> bool {
        !self.tweens.is_empty()
    }

    /// Forgets the eases that landed.
    pub fn settle(&mut self) {
        let (now, duration) = (self.now, self.duration);
        self.tweens.retain(|_, tween| tween.is_moving(now, duration));
    }

    /// Keeps the measured height of the card under `key`. With `animate` (a comment of the card
    /// was just handed on) a change eases from the height the row has now; else the row takes it
    /// at once. True when the row height changes: a first measurement or a new height.
    pub fn store(&mut self, key: &str, height: f32, animate: bool) -> bool {
        let previous = self.measured.insert(key.to_owned(), height);
        let Some(previous) = previous.filter(|previous| (previous - height).abs() > 0.5) else {
            return previous.is_none();
        };
        let (now, duration) = (self.now, self.duration);
        let mut tween = self.tweens.get(key).copied().unwrap_or_else(|| Tween::at(previous));
        tween.retarget(height, animate, now, duration);
        if tween.is_moving(now, duration) {
            self.tweens.insert(key.to_owned(), tween);
        } else {
            self.tweens.remove(key);
        }
        true
    }

    /// The card under `from` goes on as the card under `to`, with its height and any ease: a
    /// posted comment's card becomes the loaded thread's at the same height.
    pub fn hand_over(&mut self, from: &str, to: &str) {
        if let Some(height) = self.measured.remove(from) {
            self.measured.insert(to.to_owned(), height);
        }
        if let Some(tween) = self.tweens.remove(from) {
            self.tweens.insert(to.to_owned(), tween);
        }
    }

    /// The rows were laid out anew (another layout): every card is measured again.
    pub fn clear(&mut self) {
        self.measured.clear();
        self.tweens.clear();
    }

    /// A card laid out on `row` at another height than its row now has (it just appeared, its
    /// text changed, or an ease just started that this pass did not clip: `was_clipped`) has this
    /// pass redone with the right height, so the rows below never show at a stale one.
    pub fn redo_unless_shown(&self, ui: &Ui, row: Rect, key: &str, was_clipped: bool) {
        let starts_easing = !was_clipped && self.is_easing(key);
        let is_off = self.shown(key).is_some_and(|shown| (row.height() - shown).abs() > 0.5);
        if (is_off || starts_easing) && can_discard(ui.ctx()) {
            ui.ctx().request_discard("a comment card was measured");
        }
    }
}

/// egui runs another pass of this frame if this one asks to be discarded.
pub fn can_discard(ctx: &egui::Context) -> bool {
    ctx.current_pass_index() + 1 < ctx.options(|options| options.max_passes.get())
}

/// Lays out a card in `card` (open-ended height) on a band across `row`, and reports the row height
/// it needs. `clip`: the row is easing to that height, so the card shows only inside the row.
pub fn measure_card<R>(
    ui: &mut Ui,
    row: Rect,
    card: Rect,
    clip: bool,
    content: impl FnOnce(&mut Ui) -> R,
) -> (f32, R) {
    let palette = theme::palette(ui.ctx());
    ui.painter().rect_filled(row, 0, palette.ground);
    let inner = ui.scope_builder(UiBuilder::new().max_rect(card), |ui| {
        if clip {
            ui.set_clip_rect(ui.clip_rect().intersect(row));
        }
        content(ui)
    });
    let height = inner.response.rect.height() + CARD_GAP * 2.0;
    (height, inner.inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DURATION: f32 = 0.2;

    fn heights() -> CardHeights {
        let mut heights = CardHeights::default();
        heights.begin_at(10.0, DURATION);
        heights
    }

    #[test]
    fn a_first_height_and_a_new_one_change_the_row_and_the_same_one_does_not() {
        let mut heights = heights();
        assert_eq!(heights.shown("a"), None, "unmeasured");
        assert!(heights.store("a", 120.0, false), "the first measurement");
        assert!(!heights.store("a", 120.3, false), "within half a point is the same height");
        assert!(heights.store("a", 180.0, false), "a new height");
        assert_eq!(heights.shown("a"), Some(180.0), "taken at once without a hand-off");
        assert!(!heights.any_easing());
    }

    #[test]
    fn a_handed_on_card_eases_to_its_new_height_and_lands() {
        let mut heights = heights();
        heights.store("a", 100.0, false);
        assert!(heights.store("a", 200.0, true));
        assert!(heights.is_easing("a") && heights.any_easing());
        assert_eq!(heights.shown("a"), Some(100.0), "the ease starts at the old height");
        heights.begin_at(10.0 + f64::from(DURATION) / 2.0, DURATION);
        let halfway = heights.shown("a").expect("easing");
        assert!(halfway > 100.0 && halfway < 200.0, "halfway at {halfway}");
        heights.begin_at(10.0 + f64::from(DURATION) * 2.0, DURATION);
        heights.settle();
        assert!(!heights.is_easing("a"), "landed");
        assert_eq!(heights.shown("a"), Some(200.0));
    }

    #[test]
    fn hand_over_moves_the_height_and_the_ease_to_the_new_key() {
        let mut heights = heights();
        heights.store("draft", 100.0, false);
        heights.store("draft", 150.0, true);
        heights.hand_over("draft", "thread-1");
        assert_eq!(heights.shown("draft"), None);
        assert!(heights.is_easing("thread-1"), "the ease goes on under the thread");
        assert!(!heights.store("thread-1", 150.0, true), "the loaded card has that height");
        heights.clear();
        assert_eq!(heights.shown("thread-1"), None);
        assert!(!heights.any_easing());
    }
}
