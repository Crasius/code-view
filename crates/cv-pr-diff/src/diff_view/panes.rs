//! The panes of the text diff: one full-width pane (inline, New only, a whole added or deleted
//! file) or the old and the new side, their widths, the draggable boundary between them, and the
//! sideways scrolling of each pane's code when lines do not wrap.

use egui::{Rect, Ui, pos2, vec2};
use review_store::PaneSizes;

use super::frame::{Frame, RowLayout, Screen};
use crate::find::bar::Panes;
use crate::h_scroll;
use cv_code::diff_paint::TEXT_END_PADDING;
use cv_code::diff_rows::Layout;
use cv_code::line_source::Pane;
use cv_widgets::splitter::{Splitter, share_range};

/// The panes of a row at `rect`: the whole row for one pane, else the left one `left_width` wide
/// and the right one; each with its slot (0 left, 1 right).
pub(super) fn pane_rects(
    rect: Rect,
    single_pane: bool,
    left_width: f32,
) -> Vec<(Pane, usize, Rect)> {
    if single_pane {
        return vec![(Pane::Inline, 0, rect)];
    }
    vec![
        (Pane::Left, 0, Rect::from_min_size(rect.min, vec2(left_width, rect.height()))),
        (Pane::Right, 1, Rect::from_min_max(pos2(rect.left() + left_width, rect.top()), rect.max)),
    ]
}

/// The panes of `content` the find widget searches one at a time, the left one `left_width` wide.
pub(super) fn find_panes(layout: Layout, content: Rect, left_width: f32) -> Panes {
    let rects = pane_rects(content, layout.is_single_pane(), left_width);
    match rects.as_slice() {
        [(_, _, left), (_, _, right)] => Panes::Two { left: *left, right: *right },
        _ => Panes::One(content),
    }
}

/// The left pane's width in rows `width` wide: the `split` share of it side by side (each side
/// keeps its minimum), all of it for one pane.
pub(super) fn left_pane_width(layout: Layout, width: f32, split: f32) -> f32 {
    if layout.is_single_pane() {
        return width;
    }
    let range = share_range(width);
    (width * split.clamp(*range.start(), *range.end())).round()
}

/// The boundary between the old and the new side being dragged or let go.
pub(super) struct SplitDrag {
    pub is_dragged: bool,
    /// The user finished moving it (time to save).
    pub committed: bool,
}

/// The draggable boundary between the old and the new side of a side-by-side layout across
/// `rows`, at the `split` share of their width; nothing for one pane.
pub(super) fn split_handle(
    ui: &mut Ui,
    layout: Layout,
    rows: Rect,
    y_range: egui::Rangef,
    split: &mut f32,
) -> SplitDrag {
    if layout.is_single_pane() {
        return SplitDrag { is_dragged: false, committed: false };
    }
    let width = rows.width();
    let x = rows.left() + left_pane_width(layout, width, *split);
    let resize = Splitter::new("diff_split", "Resize diff sides")
        .default_value(PaneSizes::DEFAULT.diff_split)
        .range(share_range(width))
        .per_point(1.0 / width.max(1.0))
        .draws_line(false)
        .show(ui, x, y_range, split);
    SplitDrag { is_dragged: resize.response.dragged(), committed: resize.committed }
}

/// How each pane's code scrolls sideways when lines do not wrap.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Sideways {
    /// How far each pane's code is scrolled (0 left, 1 right).
    pub offsets: [f32; 2],
}

/// How wide each pane's code is and how far it can scroll sideways.
pub(super) struct Reach {
    pub content_widths: [f32; 2],
    pub max_offsets: [f32; 2],
}

impl Sideways {
    /// How wide each pane's code of `widest` columns is with `layout` and how far it can scroll
    /// sideways; the offsets are kept within that.
    pub fn clamp(&mut self, widest: [usize; 2], layout: &RowLayout) -> Reach {
        let char_width = layout.metrics.char_width;
        let content_widths = widest.map(|columns| columns as f32 * char_width + TEXT_END_PADDING);
        let max_offsets =
            [0, 1].map(|slot| (content_widths[slot] - layout.code_area_widths[slot]).max(0.0));
        for (offset, max) in self.offsets.iter_mut().zip(max_offsets) {
            *offset = offset.min(max);
        }
        Reach { content_widths, max_offsets }
    }

    /// Shift+wheel or a sideways wheel scrolls the pane under the pointer. What it scrolls leaves
    /// the frame's scroll delta, as a scroll area's does, so a trackpad swipe sees it taken.
    pub fn wheel(&mut self, ui: &Ui, frame: &Frame<'_>, single_pane: bool, max_offsets: [f32; 2]) {
        let delta = ui.input(|input| input.smooth_scroll_delta.x);
        if delta == 0.0 || !ui.rect_contains_pointer(ui.clip_rect()) {
            return;
        }
        let Some(pointer) = ui.ctx().pointer_hover_pos() else {
            return;
        };
        let slot = usize::from(!single_pane && pointer.x > frame.origin.x + frame.left_width);
        let offset = (self.offsets[slot] - delta).clamp(0.0, max_offsets[slot]);
        let scrolled = self.offsets[slot] - offset;
        self.offsets[slot] = offset;
        ui.input_mut(|input| input.smooth_scroll_delta.x -= scrolled);
    }

    /// A scrollbar under each pane's code on `screen`, as wide as its code reaches.
    pub fn show_scrollbars(&mut self, ui: &Ui, screen: &Screen<'_>, layout: Layout) {
        let inset = screen.layout.metrics.text_inset(layout.has_two_numbers());
        let content_widths = screen.reach.content_widths;
        for (_, slot, pane) in
            pane_rects(screen.content, layout.is_single_pane(), screen.left_width)
        {
            let track = Rect::from_min_max(pos2(pane.left() + inset, pane.top()), pane.max);
            let id = ui.scope_id().with(("h_scroll", slot));
            h_scroll::show(ui, track, id, content_widths[slot], &mut self.offsets[slot]);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use egui::{Event, Modifiers, MouseWheelSource, MouseWheelUnit, TouchPhase, pos2, vec2};
    use egui_kittest::Harness;
    use review_git::FileDiff;

    use crate::testing::{Fixture, fixture, show_diff, side};
    use cv_base::platform::PageSwipe;
    use cv_base::theme;
    use cv_code::contents::{Coloring, FileContents};
    use cv_code::diff_rows::Layout;
    use cv_runtime::history::Step;
    use cv_runtime::history::swipe::{Swipe, SwipeFrame};

    /// The diff, and the swipe the app reads after the page.
    struct Page {
        fixture: Fixture,
        swipe: Swipe,
        steps: Vec<Step>,
    }

    /// A trackpad gesture: fingers land, travel `delta` per frame `frames` times, lift.
    fn gesture(harness: &mut Harness<'_, Page>, delta: f32, frames: usize) {
        let wheel = |phase, delta| Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta,
            phase,
            source: MouseWheelSource::Trackpad,
            modifiers: Modifiers::NONE,
        };
        harness.event(wheel(TouchPhase::Start, vec2(0.0, 0.0)));
        harness.step();
        for _ in 0..frames {
            harness.event(wheel(TouchPhase::Move, vec2(delta, 0.0)));
            harness.step();
        }
        harness.event(wheel(TouchPhase::End, vec2(0.0, 0.0)));
        harness.run_steps(5);
    }

    /// Sideways travel over long lines scrolls the code and is no swipe: the panes take what they
    /// scroll, so the history never steps (it stepped forward while the panes left it).
    #[test]
    fn sideways_travel_the_code_scrolls_is_no_swipe() {
        let long = |marker: &str| -> String {
            let line = |index: usize| format!("let {marker}_{index} = \"{}\";", "x".repeat(300));
            (0..40).map(line).collect::<Vec<_>>().join("\n") + "\n"
        };
        let (old, new): (String, String) = (long("old"), long("new"));
        let mut fixture = fixture(Layout::Inline);
        let unified =
            similar::TextDiff::from_lines(&old, &new).unified_diff().context_radius(3).to_string();
        fixture.diff = Arc::new(FileDiff::parse_unified(unified.as_bytes()));
        fixture.contents = FileContents {
            old: Some(side(&old)),
            new: Some(side(&new)),
            coloring: Coloring::Done,
            error: None,
        };
        let page = Page { fixture, swipe: Swipe::default(), steps: Vec::new() };
        let mut harness = Harness::builder()
            .with_size([1180.0, 520.0])
            .with_context_setup(theme::fonts::install)
            .build_ui_state(
                |ui, page: &mut Page| {
                    theme::activate(ui.ctx(), page.fixture.theme);
                    show_diff(ui, &mut page.fixture);
                    let frame = ui.input(SwipeFrame::read);
                    let natural = || Some(PageSwipe { is_natural: true });
                    page.steps.extend(page.swipe.observe(frame, natural));
                },
                page,
            );
        harness.run_steps(10);
        harness.hover_at(pos2(600.0, 300.0));
        harness.step();

        // Fingers to the left: the code scrolls right, far past a swipe's travel.
        gesture(&mut harness, -40.0, 8);
        let offset = harness.state().fixture.view.sideways.offsets[0];
        assert!(offset > 300.0, "the code scrolled sideways: {offset}");
        assert_eq!(harness.state().steps, [], "no step forward");

        // Fingers to the right, past the left edge: the gesture began on the code, so it stays its.
        gesture(&mut harness, 40.0, 12);
        assert_eq!(harness.state().fixture.view.sideways.offsets[0], 0.0, "back at the start");
        assert_eq!(harness.state().steps, [], "no step back");
    }
}
