//! The rendered cards of the folder summary: with Preview on, a Markdown file's card shows its
//! rendered diff (see [`markdown_diff`](cv_pr_rendered::markdown_diff)) instead of its changed
//! lines, embedded in the card: as tall as its blocks, only the ones in view painted, and the
//! summary's own scroll area moving it.
//!
//! The two versions load from the mirror for the cards that want them, a few at a time, as the
//! diffs do; the load also parses and lines them up, and a card shows its placeholder until then.
//! A bounded least-recently-shown cache keeps the loaded cards; one dropped keeps its measured
//! height in the summary, so the cards do not move when it loads again.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{IdSalt, Rect, Ui, UiBuilder, pos2};
use review_core::ReviewThread;
use review_git::ChangedFile;
use review_store::{AppSettings, DiffLayout};

use super::code::{self, CodeOutput, ThreadsByPath};
use super::layout::{self, Geometry};
use super::{FolderSummary, SummaryInputs};
use cv_pr_rendered::markdown_diff::embedded::{self, Embedding};
use cv_pr_rendered::markdown_diff::{MarkdownDiff, MarkdownModel, RenderedInputs};
use cv_pr_rendered::preview;
use cv_pr_rendered::preview::Loaded;
use cv_pr_threads::ThreadsUi;
use cv_runtime::load::Load;

/// Loaded cards kept at most: the rendered cards on screen and one screen around it.
const MAX_CARDS: usize = 24;
/// Loads running at once.
pub const MAX_RUNNING: usize = 4;

/// A Markdown file's card: its versions while they load, then the rendered diff of them.
pub struct RenderedCard {
    content: Load<Arc<Loaded>>,
    pub view: MarkdownDiff,
    last_shown: u64,
}

/// What a rendered card needs from the frame besides the summary's inputs.
pub struct RenderedFrame<'a> {
    pub file: &'a ChangedFile,
    pub inputs: &'a SummaryInputs<'a>,
    /// The threads on the file.
    pub threads: &'a [&'a ReviewThread],
    pub is_split: bool,
    /// The old side's share side by side.
    pub split: f32,
    /// The folder's compact context, as its line cards keep, whatever the file view keeps.
    pub context: u32,
}

#[derive(Default)]
pub struct RenderedCards {
    /// Preview was on in the last frame.
    is_on: bool,
    cards: HashMap<String, RenderedCard>,
    /// Why a card's versions did not load, by path, until the user retries.
    failed: HashMap<String, String>,
}

impl RenderedCards {
    /// Preview is `is_on` this frame; true when it was switched since the last one.
    pub fn follow(&mut self, is_on: bool) -> bool {
        std::mem::replace(&mut self.is_on, is_on) != is_on
    }

    /// A new comparison: every card shows other versions.
    pub fn clear(&mut self) {
        self.cards.clear();
        self.failed.clear();
    }

    /// Takes in the versions that arrived; returns whether any did, so the cards are laid out
    /// again. A failure is kept for its card.
    pub fn poll(&mut self) -> bool {
        let mut arrived = false;
        let failed = &mut self.failed;
        self.cards.retain(|path, card| {
            if !card.content.poll() {
                return true;
            }
            if let Load::Failed(message) = &card.content {
                failed.insert(path.clone(), message.clone());
                return false;
            }
            arrived = true;
            true
        });
        arrived
    }

    /// Loads on their way.
    pub fn running(&self) -> usize {
        self.cards.values().filter(|card| card.content.is_loading()).count()
    }

    /// `path`'s versions are neither loaded, on their way nor failed.
    pub fn needs_load(&self, path: &str) -> bool {
        !self.cards.contains_key(path) && !self.failed.contains_key(path)
    }

    /// Takes the load of `path`'s versions; then drops the least recently shown cards beyond the
    /// limit, never one shown in `frame` or wanted (`keep`).
    pub fn start(
        &mut self,
        path: String,
        content: Load<Arc<Loaded>>,
        frame: u64,
        keep: &dyn Fn(&str) -> bool,
    ) {
        let card = RenderedCard { content, view: MarkdownDiff::default(), last_shown: frame };
        self.cards.insert(path, card);
        while self.cards.len() > MAX_CARDS {
            let oldest = self
                .cards
                .iter()
                .filter(|(path, card)| card.last_shown < frame && !keep(path))
                .min_by_key(|(_, card)| card.last_shown)
                .map(|(path, _)| path.clone());
            let Some(oldest) = oldest else {
                break;
            };
            self.cards.remove(&oldest);
        }
    }

    /// `path`'s card once its versions are there, lined up, marked as shown in `frame`.
    pub fn show(&mut self, path: &str, frame: u64) -> Option<&mut RenderedCard> {
        let card = self.cards.get_mut(path).filter(|card| card.model().is_some())?;
        card.last_shown = frame;
        Some(card)
    }

    pub fn failure(&self, path: &str) -> Option<&str> {
        self.failed.get(path).map(String::as_str)
    }

    /// The card's Retry: its versions load again once it is wanted.
    pub fn retry(&mut self, path: &str) {
        self.failed.remove(path);
    }

    #[cfg(test)]
    pub fn get(&self, path: &str) -> Option<&RenderedCard> {
        self.cards.get(path)
    }
}

impl RenderedCard {
    /// The versions parsed and lined up, once their load built them.
    fn model(&self) -> Option<Arc<MarkdownModel>> {
        self.content.ready().and_then(|loaded| loaded.model.clone())
    }

    /// Lays the card's blocks near the view out without painting them (see
    /// [`embedded::prepare`]); returns their height and where they want the view.
    pub fn prepare(
        &mut self,
        ui: &mut Ui,
        frame: &RenderedFrame<'_>,
        origin: egui::Pos2,
        width: f32,
        view: Rect,
    ) -> (f32, Option<f32>) {
        let Some(model) = self.model() else {
            return (embedded::height(&self.view), None);
        };
        let embedding = embedding(frame, origin, width, view);
        embedded::prepare(&mut self.view, ui, &inputs(&model, frame), &embedding);
        (embedded::height(&self.view), embedded::take_scroll_request(&mut self.view))
    }

    /// Paints the card's blocks in `body`, clipped to `clip`, as a card's code rows are.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        body: Rect,
        clip: Rect,
        frame: &RenderedFrame<'_>,
        threads_ui: &mut ThreadsUi,
    ) -> CodeOutput {
        let mut event = None;
        if let Some(model) = self.model() {
            // The whole visible part of the summary: where the view's top is matters, not the card's.
            let embedding = embedding(frame, body.min, body.width(), ui.clip_rect());
            let inputs = inputs(&model, frame);
            event = ui
                .scope_builder(UiBuilder::new().max_rect(body), |ui| {
                    ui.set_clip_rect(clip);
                    embedded::show(&mut self.view, ui, &inputs, &embedding, threads_ui)
                })
                .inner;
        }
        CodeOutput {
            event: None,
            rendered: event,
            split: None,
            split_committed: false,
            #[cfg(test)]
            rows: 0,
            height: embedded::height(&self.view),
            scroll: embedded::take_scroll_request(&mut self.view),
            has_selection: false,
        }
    }
}

fn embedding(frame: &RenderedFrame<'_>, origin: egui::Pos2, width: f32, view: Rect) -> Embedding {
    Embedding {
        id: IdSalt::new(("summary_rendered", &frame.file.path)),
        origin,
        width,
        view,
        split: frame.split,
        width_choice: frame.inputs.rendered_width,
    }
}

/// The card's rendered diff as the file view would show it, in the summary's layout (New only
/// shows inline here, as the cards' code does).
fn inputs<'a>(model: &'a Arc<MarkdownModel>, frame: &RenderedFrame<'a>) -> RenderedInputs<'a> {
    let inputs = frame.inputs;
    RenderedInputs {
        model,
        platform: inputs.platform,
        layout: if frame.is_split { DiffLayout::SideBySide } else { DiffLayout::Inline },
        context: frame.context,
        // The rendered file follows the code zoom, like the cards' code.
        scale: inputs.font_size / AppSettings::DEFAULT_CODE_FONT_SIZE,
        can_comment: inputs.can_comment,
        threads: frame.threads,
        show_threads: inputs.show_threads,
    }
}

/// Preview was switched: the Markdown cards change between their lines and their rendered
/// diff, so their heights are measured again.
pub fn follow_preview(summary: &mut FolderSummary, inputs: &SummaryInputs<'_>) {
    if !summary.rendered.follow(inputs.preview) {
        return;
    }
    for slot in &summary.slots {
        let path = &inputs.files[slot.file].path;
        if preview::is_markdown(path) {
            summary.heights.remove(path);
        }
    }
    summary.index_dirty = true;
}

/// `FolderSummary::lay_out_bodies` for the loaded rendered cards near the view: their blocks near the
/// view are measured, and their heights go into the index.
pub fn lay_out_rendered(
    summary: &mut FolderSummary,
    ui: &mut Ui,
    inputs: &SummaryInputs<'_>,
    threads: &ThreadsByPath<'_>,
    geometry: &Geometry,
    split: f32,
) -> Vec<(usize, f32)> {
    let mut requests = Vec::new();
    if summary.width <= 0.0 || summary.index.len() != summary.slots.len() {
        return requests;
    }
    let context = summary.context();
    for &card in summary.loads.wanted() {
        let file = &inputs.files[summary.slots[card].file];
        if summary.collapsed.contains(&file.path) || !inputs.renders(file) {
            continue;
        }
        let Some(rendered) = summary.rendered.show(&file.path, summary.frame) else {
            continue;
        };
        let frame = RenderedFrame {
            file,
            inputs,
            threads: code::threads_of(threads, file),
            is_split: summary.split,
            split,
            context,
        };
        let origin = pos2(0.0, summary.index.top(card) + geometry.header);
        let (height, scroll) =
            rendered.prepare(ui, &frame, origin, summary.width, summary.viewport);
        if let Some(y) = scroll {
            requests.push((card, y));
        }
        summary.index_dirty |= layout::note_height(&mut summary.heights, &file.path, height);
    }
    requests
}
