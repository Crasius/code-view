//! What the folder summary loads and when: the cards that want their diff (on screen and one
//! screen around it, nearest first), the diffs on their way from the mirror and the ones that
//! failed, the syntax colors worked out for loaded cards, and how fast the view moves, since
//! nothing new starts while it flies by.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use egui::Rect;
use review_git::FileDiff;

use super::bodies::{BodyCache, Cancel, CardBody};
use super::layout::CardIndex;
use cv_pr_diff::patch_colors;
use cv_runtime::load::Load;
use cv_syntax::highlight::LineSpans;

/// Loads running at once: enough to fill a screen quickly, few enough to leave git and the
/// coloring thread to the file the user opens next.
pub(crate) const MAX_RUNNING: usize = 4;
/// Cards looked at for loading per frame, nearest first; fewer than the body cache keeps.
pub(crate) const MAX_WANTED: usize = 96;
/// Faster than this (points per second, about eight screens) the view is flying by (a fling, a
/// dragged scroll bar, a jump): nothing new starts loading. A steady wheel glide stays below it.
const FLING_SPEED: f32 = 8_000.0;
/// How long the view must have slowed down before loads start again.
const SETTLE_SECONDS: f64 = 0.12;

/// A card's diff on its way from the mirror.
struct Running {
    path: String,
    /// Index into the files.
    file: usize,
    load: Load<Arc<FileDiff>>,
}

type SpansMessage = (u64, String, LineSpans);

pub(crate) struct Loads {
    /// Slots on screen and one screen around it, nearest first, as of the last frame.
    wanted: Vec<usize>,
    running: Vec<Running>,
    /// Why a card's diff did not load, by path, until the user retries.
    failed: HashMap<String, String>,
    spans_sender: Sender<SpansMessage>,
    spans_receiver: Receiver<SpansMessage>,
    /// Bumped with every new comparison, so colors of an older one are ignored.
    generation: u64,
    /// (time, scroll offset) of the last frame the view showed in.
    last_motion: Option<(f64, f32)>,
    settles_at: f64,
}

impl Default for Loads {
    fn default() -> Self {
        let (spans_sender, spans_receiver) = mpsc::channel();
        Self {
            wanted: Vec::new(),
            running: Vec::new(),
            failed: HashMap::new(),
            spans_sender,
            spans_receiver,
            generation: 0,
            last_motion: None,
            settles_at: 0.0,
        }
    }
}

impl Loads {
    /// A new comparison: the running loads, the failures and the colors still on their way belong
    /// to the old one.
    pub(crate) fn start_over(&mut self) {
        self.generation += 1;
        self.running.clear();
        self.failed.clear();
    }

    pub(crate) fn wanted(&self) -> &[usize] {
        &self.wanted
    }

    /// The cards were rebuilt: the slots planned for are gone.
    pub(crate) fn clear_wanted(&mut self) {
        self.wanted.clear();
    }

    /// The expanded cards on screen and one screen around it, those on screen first, then by
    /// distance; a few dozen at most however small the cards are.
    pub(crate) fn plan(
        &mut self,
        index: &CardIndex,
        viewport: Rect,
        is_expanded: impl Fn(usize) -> bool,
    ) {
        self.wanted.clear();
        let height = viewport.height().max(1.0);
        let (top, bottom) = (viewport.top(), viewport.bottom());
        let center = f32::midpoint(top, bottom);
        let mut candidates: Vec<(bool, f32, usize)> = index
            .overlapping(top - height, bottom + height)
            .filter(|&card| is_expanded(card))
            .map(|card| {
                let (card_top, card_bottom) = (index.top(card), index.bottom(card));
                let is_visible = card_bottom > top && card_top < bottom;
                let distance = if card_bottom < center {
                    center - card_bottom
                } else {
                    (card_top - center).max(0.0)
                };
                (!is_visible, distance, card)
            })
            .collect();
        candidates.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        self.wanted.extend(candidates.into_iter().take(MAX_WANTED).map(|(_, _, card)| card));
    }

    /// The diffs that arrived since the last frame, as (path, file index, diff); the failures are
    /// kept for their cards.
    pub(crate) fn finish(&mut self) -> Vec<(String, usize, Arc<FileDiff>)> {
        let mut finished = Vec::new();
        let failed = &mut self.failed;
        self.running.retain_mut(|running| {
            running.load.poll();
            match std::mem::take(&mut running.load) {
                Load::Loading(task) => {
                    running.load = Load::Loading(task);
                    return true;
                }
                Load::Ready(diff) => finished.push((running.path.clone(), running.file, diff)),
                Load::Failed(message) => {
                    failed.insert(running.path.clone(), message);
                }
                Load::Idle => {}
            }
            false
        });
        finished
    }

    /// Another load may start: fewer than [`MAX_RUNNING`] are on their way.
    pub(crate) fn has_room(&self) -> bool {
        self.running.len() < MAX_RUNNING
    }

    pub(crate) fn push(&mut self, path: String, file: usize, load: Load<Arc<FileDiff>>) {
        self.running.push(Running { path, file, load });
    }

    /// Loads on their way.
    #[cfg(test)]
    pub fn running(&self) -> usize {
        self.running.len()
    }

    /// `path`'s diff is on its way or failed: nothing to start for it.
    pub(crate) fn is_tried(&self, path: &str) -> bool {
        self.failed.contains_key(path) || self.running.iter().any(|running| running.path == path)
    }

    pub(crate) fn failure(&self, path: &str) -> Option<&str> {
        self.failed.get(path).map(String::as_str)
    }

    /// The card's Retry: its diff loads again once it is wanted.
    pub(crate) fn retry(&mut self, path: &str) {
        self.failed.remove(path);
    }

    /// Takes in the colors worked out for the current comparison.
    pub(crate) fn receive_spans(&self, bodies: &mut BodyCache) {
        while let Ok((generation, path, spans)) = self.spans_receiver.try_recv() {
            if generation == self.generation {
                bodies.set_spans(&path, spans);
            }
        }
    }

    /// Queues the syntax colors of a loaded card that has none yet.
    pub(crate) fn color(&self, ctx: &egui::Context, path: &str, body: &mut CardBody) {
        if body.spans.is_some() || body.coloring.is_some() || body.placeholder.is_some() {
            return;
        }
        let cancel = Cancel::default();
        body.coloring = Some(cancel.clone());
        let (sender, generation, ctx, path) =
            (self.spans_sender.clone(), self.generation, ctx.clone(), path.to_owned());
        // No grammar for the file: plain text, and the card stops waiting for colors.
        patch_colors::color_whole(path.clone(), body.diff.clone(), cancel.0, move |spans| {
            if sender.send((generation, path, spans)).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// The view slowed down long enough ago for loads to start.
    pub(crate) fn is_settled(&self, now: f64) -> bool {
        now >= self.settles_at
    }

    /// Notes how fast the view moves; while it flies by, loads wait until it settles.
    pub(crate) fn track_motion(&mut self, ctx: &egui::Context, now: f64, offset: f32) {
        if let Some((time, previous)) = self.last_motion {
            let elapsed = (now - time).max(1.0 / 1000.0) as f32;
            let speed = (offset - previous).abs() / elapsed;
            if speed > FLING_SPEED {
                self.settles_at = now + SETTLE_SECONDS;
            }
        }
        self.last_motion = Some((now, offset));
        if now < self.settles_at {
            ctx.request_repaint_after(Duration::from_secs_f64(self.settles_at - now));
        }
    }

    /// The view did not show (another folder, a file open): the next frame's offset is no motion.
    pub(crate) fn forget_motion(&mut self) {
        self.last_motion = None;
    }
}
