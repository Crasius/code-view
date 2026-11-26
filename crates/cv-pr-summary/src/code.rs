//! A loaded card's rows: the file view's diff view in its compact layout, embedded in the card's
//! body, with the threads on the card's lines. What a card's body did ([`CodeOutput`]) is the same
//! for a rendered card.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Rect, Ui, UiBuilder};
use review_core::ReviewThread;
use review_git::{ChangeKind, ChangedFile, FileDiff};

use super::SummaryInputs;
use super::bodies::CardBody;
use cv_code::diff_rows::Layout;
use cv_editor::markdown_editor;
use cv_pr_diff::diff_view::{self, DiffEvent, DiffInputs, Embedded};
use cv_pr_diff::find::FindInputs;
use cv_pr_rendered::markdown_diff::RenderedEvent;
use cv_pr_threads::ThreadsUi;
use cv_syntax::highlight::LineSpans;

/// The pull request's threads by the path they are on, gathered once per frame.
pub type ThreadsByPath<'a> = HashMap<&'a str, Vec<&'a ReviewThread>>;

/// Which card's rows to paint, and how this frame.
pub struct CardCode<'a> {
    pub file: &'a ChangedFile,
    pub inputs: &'a SummaryInputs<'a>,
    /// The threads on the file.
    pub threads: &'a [&'a ReviewThread],
    pub layout: Layout,
    /// The old side's share of a side-by-side card.
    pub split: f32,
}

/// What a loaded card's rows did this frame.
pub struct CodeOutput {
    pub event: Option<DiffEvent>,
    /// What the user did in a rendered card (see [`super::rendered`]).
    pub rendered: Option<RenderedEvent>,
    /// The share the user moved the boundary between the sides to, and whether they let go.
    pub split: Option<f32>,
    pub split_committed: bool,
    /// Code rows painted.
    #[cfg(test)]
    pub rows: usize,
    /// The rows' height, comment cards included.
    pub height: f32,
    /// Where the rows want the view scrolled: y below the body's top for the view's top.
    pub scroll: Option<f32>,
    pub has_selection: bool,
}

pub fn threads_by_path<'a>(threads: &[&'a ReviewThread]) -> ThreadsByPath<'a> {
    let mut by_path: ThreadsByPath<'a> = HashMap::new();
    for thread in threads {
        if let Some(anchor) = &thread.anchor {
            by_path.entry(anchor.path.as_str()).or_default().push(*thread);
        }
    }
    by_path
}

pub fn threads_of<'a>(
    threads: &'a ThreadsByPath<'a>,
    file: &ChangedFile,
) -> &'a [&'a ReviewThread] {
    threads.get(file.path.as_str()).map_or(&[], Vec::as_slice)
}

/// A card's layout: compact, side by side when the user picked that, except for an added or
/// deleted file, which shows whole in one pane as in the file view.
pub fn card_layout(file: &ChangedFile, context: u32, is_split: bool) -> Layout {
    let is_whole = matches!(file.kind, ChangeKind::Added | ChangeKind::Deleted);
    if is_split && !is_whole { Layout::CompactSplit(context) } else { Layout::Compact(context) }
}

/// A card's rows as the diff view takes them: the compact `layout`, colored by diff line.
pub fn card_inputs<'a>(
    diff: &'a Arc<FileDiff>,
    spans: Option<&'a LineSpans>,
    file: &'a ChangedFile,
    inputs: &SummaryInputs<'_>,
    threads: &'a [&'a ReviewThread],
    layout: Layout,
) -> DiffInputs<'a> {
    DiffInputs {
        diff,
        contents: None,
        threads,
        layout,
        font_size: inputs.font_size,
        wrap: false,
        can_comment: inputs.can_comment,
        can_comment_outside_hunks: inputs.can_comment_outside_hunks,
        path: &file.path,
        old_path: file.old_path.as_deref(),
        line_spans: spans,
        show_threads: inputs.show_threads,
        find: FindInputs::default(),
    }
}

/// Paints a loaded card's rows in `body`, clipped to `clip`, with the file view's diff view. The
/// keys that act on a selection go to the card holding it, else to the one under the pointer.
pub fn show(
    ui: &mut Ui,
    body: Rect,
    clip: Rect,
    card: &mut CardBody,
    code: &CardCode<'_>,
    threads_ui: &mut ThreadsUi,
) -> CodeOutput {
    let CardBody { diff, view, spans, .. } = card;
    let file = code.file;
    if view.composer().is_some() || !code.threads.is_empty() {
        // Suggestions and pasted code in the comment editors refer to this file.
        markdown_editor::FileContext { path: file.path.clone(), diff: diff.clone() }
            .publish(ui.ctx());
    }
    let diff_inputs =
        card_inputs(diff, spans.as_deref(), file, code.inputs, code.threads, code.layout);
    // The whole visible part of the summary: where the view's top is matters, not the card's.
    let visible = ui.clip_rect();
    let embedded = Embedded {
        origin: body.min,
        width: body.width(),
        view: visible,
        split: code.split,
        keep_below: diff_view::keep_below(ui.ctx(), visible),
    };
    let has_keyboard = view.has_selection() || ui.rect_contains_pointer(clip);
    let output = ui
        .scope_builder(UiBuilder::new().max_rect(body), |ui| {
            ui.set_clip_rect(clip);
            view.show_embedded(ui, &diff_inputs, threads_ui, &embedded, has_keyboard)
        })
        .inner;
    CodeOutput {
        event: output.event,
        rendered: None,
        split: output.split,
        split_committed: output.split_committed,
        #[cfg(test)]
        rows: output.rows,
        height: view.height(),
        scroll: view.take_scroll_request(),
        has_selection: view.has_selection(),
    }
}
