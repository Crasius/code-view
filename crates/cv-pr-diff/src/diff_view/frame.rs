//! What one frame of the text diff lays its rows out with: what it shows ([`DiffInputs`]), the
//! code metrics and wrap columns ([`RowLayout`]), where the rows go on screen ([`Screen`],
//! [`Frame`]) and the part in view; for rows shown inside another view's scrolling (a card of the
//! folder summary), where they go in that view ([`Embedded`]) and what showing them gave (see
//! [`super::DiffView::show_embedded`]).

use std::sync::Arc;

use egui::{Rect, Ui, vec2};

use super::offsets::HeightKey;
use super::panes::{self, Reach, left_pane_width};
use super::{DiffEvent, DiffView};
use review_core::ReviewThread;
use review_git::FileDiff;

use crate::find::FindInputs;
use cv_base::theme::motion;
use cv_code::contents::FileContents;
use cv_code::diff_paint::{Metrics, TEXT_END_PADDING};
use cv_code::diff_rows::Layout;
use cv_pr_threads::ThreadsUi;
use cv_syntax::highlight::LineSpans;
use cv_widgets::surfaces;

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent settings of the file view (wrap, may comment, comments outside hunks, show comments, scroll past the end), no state machine"
)]
pub struct DiffInputs<'a> {
    pub diff: &'a Arc<FileDiff>,
    pub contents: Option<&'a FileContents>,
    pub threads: &'a [&'a ReviewThread],
    pub layout: Layout,
    pub font_size: f32,
    pub wrap: bool,
    pub can_comment: bool,
    /// The platform takes comments on unchanged lines outside the hunks (GitHub does not).
    pub can_comment_outside_hunks: bool,
    pub path: &'a str,
    pub old_path: Option<&'a str>,
    /// Syntax colors by diff line, worked out from the diff's own text: all the colors a folder
    /// summary card has, and the file view's until the file's own colors reach a line.
    pub line_spans: Option<&'a LineSpans>,
    /// "Show comments in the diff": the threads' cards show under their lines. Off, a count in
    /// the gutter stands for them and shows them there on a click.
    pub show_threads: bool,
    /// Whether the selection's occurrences are marked (the app's setting).
    pub find: FindInputs,
    /// "Scroll past the end": the rows hold room below the last one (see
    /// [`past_end`](crate::diff_overview::past_end)).
    pub past_end: bool,
}

impl DiffInputs<'_> {
    /// Both file versions are loaded, so collapsed lines can be shown.
    pub(crate) fn can_expand(&self) -> bool {
        self.contents.is_some_and(|contents| contents.old.is_some() && contents.new.is_some())
    }

    /// The file's versions are in or failed to load: no more lines will come with them.
    pub(crate) fn are_contents_settled(&self) -> bool {
        self.contents.is_some_and(|contents| contents.error.is_some() || self.can_expand())
    }
}

/// What the rows of a frame are laid out with.
pub(super) struct RowLayout {
    pub metrics: Metrics,
    /// Columns per visual row of each pane's code when lines wrap.
    pub wrap_columns: Option<[usize; 2]>,
    pub code_area_widths: [f32; 2],
}

impl RowLayout {
    /// The layout of rows `width` wide with a left pane `left_width` wide, for code with
    /// line numbers up to `max_line_number`.
    pub(crate) fn new(
        ui: &Ui,
        inputs: &DiffInputs<'_>,
        width: f32,
        left_width: f32,
        max_line_number: u32,
    ) -> Self {
        let metrics = Metrics::new(ui, inputs.font_size, max_line_number);
        let two_numbers = inputs.layout.has_two_numbers();
        let pane_widths = if inputs.layout.is_single_pane() {
            [width; 2]
        } else {
            [left_width, width - left_width]
        };
        let code_area_widths = pane_widths.map(|width| width - metrics.text_inset(two_numbers));
        let wrap_columns = inputs.wrap.then(|| {
            code_area_widths.map(|width| {
                let code_width = (width - TEXT_END_PADDING).max(metrics.char_width * 8.0);
                ((code_width / metrics.char_width).floor() as usize).max(8)
            })
        });
        Self { metrics, wrap_columns, code_area_widths }
    }

    /// What the rows' heights depend on besides the rows.
    pub(crate) fn height_key(&self) -> HeightKey {
        HeightKey {
            line_height: self.metrics.line_height,
            gap_height: self.metrics.gap_height,
            wrap_columns: self.wrap_columns,
        }
    }
}

/// Where the file view's rows go on screen this frame: `content` with its left pane
/// `left_width` wide, laid out with `layout`, and the overview `strip` beside it; `reach` says how
/// far each pane scrolls sideways.
pub(super) struct Screen<'a> {
    pub content: Rect,
    pub strip: Rect,
    pub left_width: f32,
    pub layout: &'a RowLayout,
    pub reach: Reach,
}

/// The visible part of the rows, in content coordinates (empty before the first frame).
#[derive(Clone, Copy)]
pub(super) struct Viewport(pub Rect);

impl Default for Viewport {
    fn default() -> Self {
        Self(Rect::ZERO)
    }
}

/// Per-frame geometry shared by all rows.
pub(super) struct Frame<'a> {
    pub origin: egui::Pos2,
    pub width: f32,
    /// Width of the left pane (the whole row when inline).
    pub left_width: f32,
    pub metrics: &'a Metrics,
    /// Wrap width of each pane's code area when wrapping.
    pub wrap_width: Option<[f32; 2]>,
    /// See [`cv_code::code_job::JobStyle::syntax_mix`].
    pub syntax_mix: f32,
}

/// How far the diff's syntax colors show (see [`code_job::JobStyle::syntax_mix`]). Colors there
/// in the first frame the rows show (the usual case: the diff's load waits for them) show at
/// once; colors that arrive later fade in briefly instead of flashing on, and show at once
/// under reduced motion.
///
/// [`code_job::JobStyle::syntax_mix`]: cv_code::code_job::JobStyle::syntax_mix
pub(super) fn syntax_mix(ctx: &egui::Context, inputs: &DiffInputs<'_>) -> f32 {
    let has_colors = inputs.line_spans.is_some_and(|spans| spans.line_count() > 0)
        || inputs.contents.is_some_and(|contents| {
            [&contents.old, &contents.new]
                .into_iter()
                .flatten()
                .any(|side| side.spans.line_count() > 0)
        });
    let id = egui::Id::unique(("syntax_colors_fade", Arc::as_ptr(inputs.diff) as usize));
    motion::toggle(ctx, id, has_colors, motion::FAST)
}

/// Where rows shown inside another view's scrolling go, in that view's coordinates: on screen to
/// paint, or its content's to lay out ahead of it.
#[derive(Debug, Clone, Copy)]
pub struct Embedded {
    /// The top left of the first row.
    pub origin: egui::Pos2,
    pub width: f32,
    /// The visible part of the scrolling view.
    pub view: Rect,
    /// The old side's share of a side-by-side layout, as in the file view.
    pub split: f32,
    /// How far below the top of `view` the line lies that a new code size keeps in place (see
    /// [`keep_below`]); only the rows holding it move the view for it.
    pub keep_below: f32,
}

impl Embedded {
    /// Draw the embedded rows and their controls within the parent's vertical viewport.
    pub(super) fn show(
        &self,
        view: &mut DiffView,
        ui: &mut Ui,
        inputs: &DiffInputs<'_>,
        threads_ui: &mut ThreadsUi,
        has_keyboard: bool,
    ) -> EmbeddedOutput {
        view.place.is_embedded = true;
        view.rebuild_rows(ui, inputs, threads_ui);
        view.place.viewport.0 = self.local_view();
        let left_width = left_pane_width(inputs.layout, self.width, self.split);
        let layout = view.layout_rows(ui, inputs, self.width, left_width, self.sight(), false);
        let rect = Rect::from_min_size(self.origin, vec2(self.width, view.height()));
        // Every card's rows have their own ids: they share one parent in the summary.
        ui.push_id(("embedded_diff", inputs.path), |ui| {
            view.update_range_target(inputs);
            if has_keyboard {
                view.selection_keys(ui, inputs, rect, left_width);
                view.update_range_target(inputs);
            }
            let shown = rect.intersect(self.view);
            // The code zoom and its keyboard focus work here as in the file view.
            surfaces::mark_code(ui, shown);
            let background = view.selecting.area(ui, shown, inputs.path);
            let frame = Frame {
                origin: self.origin,
                width: self.width,
                left_width,
                metrics: &layout.metrics,
                wrap_width: None,
                syntax_mix: syntax_mix(ui.ctx(), inputs),
            };
            let reach = view.sideways.clamp(view.built.widest, &layout);
            let screen =
                Screen { content: shown, strip: Rect::NOTHING, left_width, layout: &layout, reach };
            if shown.is_positive() {
                surfaces::mark_horizontal_scroll(ui, shown);
                view.sideways.wheel(
                    ui,
                    &frame,
                    inputs.layout.is_single_pane(),
                    screen.reach.max_offsets,
                );
            }
            view.track_pointer(ui, &background, &frame, inputs);
            let local = view.place.viewport.0;
            let event = view.paint_rows(ui, &frame, local, inputs, threads_ui);
            if let Some(offset) = view.selecting.edge_scroll(
                ui,
                self.view,
                view.place.viewport.0,
                view.height(),
                view.place.is_embedded,
            ) {
                view.place.scroll_to = Some(offset);
            }
            view.context_menu(&background, inputs);
            // The boundary between the sides moves as in the file view, over the rows on screen.
            let mut split = self.split;
            let committed = shown.is_positive() && {
                view.sideways.show_scrollbars(ui, &screen, inputs.layout);
                let drag =
                    panes::split_handle(ui, inputs.layout, rect, shown.y_range(), &mut split);
                view.is_resizing = drag.is_dragged;
                drag.committed
            };
            #[expect(clippy::float_cmp, reason = "the splitter writes a new share only when moved")]
            let is_moved = split != self.split;
            EmbeddedOutput { event, split: is_moved.then_some(split), split_committed: committed }
        })
        .inner
    }

    /// The visible part in row coordinates (0 is the top of the first row).
    pub(super) fn local_view(&self) -> Rect {
        self.view.translate(-self.origin.to_vec2())
    }

    pub(super) fn sight(&self) -> Sight {
        Sight { height: self.view.height(), keep_below: self.keep_below }
    }
}

pub struct EmbeddedOutput {
    pub event: Option<DiffEvent>,
    /// The share the user moved the boundary between the sides to this frame.
    pub split: Option<f32>,
    /// The user finished moving it.
    pub split_committed: bool,
}

/// The visible part of the rows, as laying them out takes it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Sight {
    pub height: f32,
    /// See [`Embedded::keep_below`].
    pub keep_below: f32,
}

/// How far below the top of `screen`, the visible part of a code view on screen, the line lies
/// that a new code size keeps in place: the one under the pointer while it is over code there,
/// as a browser zooms around the pointer, else the one at the top.
pub fn keep_below(ctx: &egui::Context, screen: Rect) -> f32 {
    ctx.pointer_hover_pos()
        .filter(|pos| screen.contains(*pos) && surfaces::is_code_at(ctx, *pos))
        .map_or(0.0, |pos| pos.y - screen.top())
}
