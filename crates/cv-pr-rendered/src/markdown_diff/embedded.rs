//! The rendered diff embedded in another scroll area (a card of the folder summary): no scroll
//! area, find bar, overview strip or keys of its own. It is as tall as its rows (measured or
//! estimated), lays out and paints the ones in the holding area's view, and hands the holding area
//! the scroll a thread asked for, or the one that keeps the block at the top of the view in place
//! while blocks above it are measured.

use std::sync::Arc;

use egui::{IdSalt, Rect, Ui, vec2};
use review_store::RenderedWidth;

use super::bars::{paint_header, show_banner};
use super::lanes::{Areas, Lanes, reading_column};
use super::model::Model;
use super::rows::begin_frame;
use super::{Frame, MarkdownDiff, Place, RenderedEvent, RenderedInputs};
use cv_base::theme;
use cv_code::diff_paint;
use cv_pr_threads::ThreadsUi;
use cv_widgets::surfaces;

/// Where a rendered diff embedded in another scroll area (a card of the folder summary) goes this
/// frame. It has no scroll area of its own: it is as tall as its rows, and paints the ones in view.
#[derive(Clone, Copy)]
pub struct Embedding {
    /// Tells the diffs embedded in one scroll area apart.
    pub id: IdSalt,
    /// The diff's top left, in the coordinates of `view`.
    pub origin: egui::Pos2,
    pub width: f32,
    /// The visible part of the scroll area holding the diff.
    pub view: Rect,
    /// The old side's share side by side.
    pub split: f32,
    pub width_choice: RenderedWidth,
}

/// What an embedded diff keeps between frames.
#[derive(Default)]
pub(super) struct EmbeddedPlace {
    /// The height of the header and banner above the rows, last laid out.
    chrome: f32,
    /// Where the rows want the view, as y below the diff's top for the view's top.
    scroll: Option<f32>,
}

/// Lays the rows out and measures the ones near the view, without painting, so a card knows its height before the scroll area holding it places anything.
pub fn prepare(
    diff: &mut MarkdownDiff,
    ui: &mut Ui,
    inputs: &RenderedInputs<'_>,
    embedding: &Embedding,
) {
    ui.push_id(embedding.id, |ui| {
        lay_out(diff, ui, inputs, embedding);
    });
}

/// The rows in view, with the side by side header and the banner over them; returns what the
/// user did. There is no find bar, overview strip or key handling: the scroll area
/// holding the diff has its own.
pub fn show(
    diff: &mut MarkdownDiff,
    ui: &mut Ui,
    inputs: &RenderedInputs<'_>,
    embedding: &Embedding,
    threads_ui: &mut ThreadsUi,
) -> Option<RenderedEvent> {
    ui.push_id(embedding.id, |ui| {
        let (model, areas, lanes) = lay_out(diff, ui, inputs, embedding);
        let palette = theme::palette(ui.ctx());
        surfaces::mark_code(ui, areas.full.intersect(embedding.view));
        let find = std::mem::take(&mut diff.find);
        let frame = Frame {
            origin: areas.content.min,
            width: areas.content.width(),
            lanes,
            options: inputs.render_options(),
            id: ui.scope_id().with("markdown_diff"),
            can_comment: inputs.can_comment,
            mode: diff.mode,
            model: &model,
            is_resizing: false,
            threads: inputs.threads,
            show_threads: inputs.show_threads,
            find: &find,
        };
        let mut event = diff.paint_rows(ui, &frame, diff.viewport.0, threads_ui);
        if let Some(header) = areas.header {
            paint_header(ui, header, &lanes, &model, &palette);
            let x = areas.content.left() + areas.left_width;
            let divider = Rect::from_x_y_ranges(x..=x, header.y_range());
            diff_paint::paint_divider(ui.painter(), x, divider, &palette);
        }
        if let Some(banner) = areas.banner
            && let Some(banner_event) = show_banner(ui, banner, &palette)
        {
            event = Some(banner_event);
        }
        diff.find = find;
        event
    })
    .inner
}

/// The model, rows and geometry of an embedded frame, the rows near the view measured. The
/// view is the holding scroll area's: only a thread asked for moves it ([`Place::Thread`]).
fn lay_out(
    diff: &mut MarkdownDiff,
    ui: &mut Ui,
    inputs: &RenderedInputs<'_>,
    embedding: &Embedding,
) -> (Arc<Model>, Areas, Lanes) {
    let (model, mode, has_banner) = begin_frame(diff, ui.ctx(), inputs);
    diff.pending.take_if(|place| !matches!(place, Place::Thread(_)));
    let full = Rect::from_min_size(embedding.origin, vec2(embedding.width, height(diff).max(1.0)));
    let areas = Areas::new(full, mode, has_banner, embedding.split, 0.0);
    let content = areas.content;
    diff.embedded.chrome = content.top() - full.top();
    let column = reading_column(embedding.width_choice, mode, inputs.scale);
    let lanes = Lanes::new(mode, content.width(), areas.left_width, column);
    diff.viewport.0 = embedding.view.translate(-content.min.to_vec2());
    diff.scroll_to = None;
    let id = ui.scope_id().with("markdown_diff");
    diff.layout(ui, &model, &lanes, inputs.render_options(), embedding.view.height(), id);
    if let Some(top) = diff.scroll_to.take() {
        diff.embedded.scroll = Some(diff.embedded.chrome + top);
    }
    (model, areas, lanes)
}

/// The height of the header, the banner and the rows (measured or estimated).
pub fn height(diff: &MarkdownDiff) -> f32 {
    diff.embedded.chrome + diff.heights.total()
}

/// Where the rows asked the view to go since the last call, as y below the diff's top for the
/// view's top: a thread asked for, or the block at the top of the view kept in place while blocks
/// above it were measured.
pub fn take_scroll_request(diff: &mut MarkdownDiff) -> Option<f32> {
    diff.embedded.scroll.take()
}
