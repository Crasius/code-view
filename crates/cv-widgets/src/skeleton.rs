//! Skeleton placeholders shaped like the content they stand in for. A soft highlight sweeps
//! across them left to right while visible; the repaint it requests stops as soon as no skeleton
//! is drawn, so idle stays idle. Under reduced motion they hold still, and so they do while the
//! window is in the background ([`platform::is_window_active`]): most loads wait for the window to
//! come back, so a sweep there would repaint at the full frame rate for as long as the window sits
//! behind others, and a load that does finish asks for its own frame.
//!
//! A placeholder only appears once its load has taken a moment ([`reveal`]): content that is there
//! within the grace period replaces the old content directly, without a skeleton flashing between.

use std::time::Duration;

use cv_base::platform;
use cv_base::theme::{self, motion, style};
use egui::{Color32, Context, Id, Rect, Response, Sense, Ui, vec2};

const SHIMMER_PERIOD_SECONDS: f64 = 1.6;
/// Shorter than a frame at any refresh rate up to 1000 Hz.
const SHIMMER_REPAINT: Duration = Duration::from_millis(1);
/// Width of the highlight band, in points.
const SHIMMER_BAND: f32 = 420.0;
/// How long a load may take before its placeholder shows.
pub const GRACE_SECONDS: f64 = 0.13;
/// How long the placeholder then takes to fade in.
const FADE_IN_SECONDS: f64 = 0.15;

/// The current time on egui's clock, to pass to [`reveal`] later as when a load started.
pub fn now(ctx: &Context) -> f64 {
    ctx.input(|input| input.time)
}

/// How much of a placeholder to show for a load that started at `started` ([`now`]): nothing in
/// the grace period, then fading in. Schedules the repaints that change it.
pub fn reveal(ctx: &Context, started: f64) -> f32 {
    let waited = now(ctx) - started;
    let shown = reveal_after(waited, motion::is_reduced(ctx));
    if waited < GRACE_SECONDS {
        ctx.request_repaint_after(Duration::from_secs_f64(GRACE_SECONDS - waited));
    } else if shown < 1.0 {
        ctx.request_repaint();
    }
    shown
}

/// [`reveal`] after waiting `waited` seconds; under reduced motion the placeholder appears whole.
fn reveal_after(waited: f64, is_reduced_motion: bool) -> f32 {
    if waited < GRACE_SECONDS {
        return 0.0;
    }
    if is_reduced_motion {
        return 1.0;
    }
    ((waited - GRACE_SECONDS) / FADE_IN_SECONDS).min(1.0) as f32
}

/// Tells assistive technology that `response`'s area is loading `what`.
pub fn announce(response: &Response, what: &str) {
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, true, what));
}

fn still_id() -> Id {
    Id::unique("skeleton_still")
}

/// Draws the skeletons `add` paints without the sweep and its repaints: placeholders of a load
/// that failed and will not arrive until a retry, or that waits out a rate-limit pause. A `still`
/// inside another leaves the outer one still after it.
pub fn still<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let was_still = ui.ctx().data_mut(|data| {
        let was_still = data.get_temp::<bool>(still_id()).is_some();
        data.insert_temp(still_id(), true);
        was_still
    });
    let result = add(ui);
    if !was_still {
        ui.ctx().data_mut(|data| data.remove::<bool>(still_id()));
    }
    result
}

/// Color of a skeleton block at `rect` for this frame.
fn shade(ui: &Ui, rect: Rect) -> Color32 {
    let palette = theme::palette(ui.ctx());
    let (base, highlight) = if palette.is_dark {
        (palette.surface_subtle, palette.pressed)
    } else {
        (palette.pressed, palette.hover)
    };
    let is_still = ui.ctx().data(|data| data.get_temp::<bool>(still_id()).unwrap_or(false));
    let is_active = ui.input(|input| platform::is_window_active(input.viewport()));
    if is_still || !is_active || motion::is_reduced(ui.ctx()) {
        return base;
    }
    // The sweep is time-based, so it repaints on every display refresh (240 Hz on a 240 Hz
    // monitor) rather than on a fixed 60 Hz tick: a frame ends at vsync, after this short delay.
    // Not `request_repaint()`: a zero delay means "never settles" to egui_kittest, whose
    // harnesses would then step through a whole load (a rate-limit pause takes minutes).
    ui.ctx().request_repaint_after(SHIMMER_REPAINT);
    let screen = ui.ctx().content_rect();
    let time = ui.input(|input| input.time);
    let phase = ((time % SHIMMER_PERIOD_SECONDS) / SHIMMER_PERIOD_SECONDS) as f32;
    let sweep = screen.left() - SHIMMER_BAND + phase * (screen.width() + SHIMMER_BAND * 2.0);
    let distance = (rect.center().x - sweep).abs() / SHIMMER_BAND;
    let glow = (1.0 - distance).clamp(0.0, 1.0);
    let glow = glow * glow * (3.0 - 2.0 * glow);
    motion::mix(base, highlight, glow * 0.8)
}

pub fn bar(ui: &mut Ui, width: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    paint_bar(ui, rect);
}

pub fn paint_bar(ui: &Ui, rect: Rect) {
    ui.painter().rect_filled(rect, 4, shade(ui, rect));
}

pub fn paint_circle(ui: &Ui, center: egui::Pos2, radius: f32) {
    let rect = Rect::from_center_size(center, vec2(radius * 2.0, radius * 2.0));
    ui.painter().circle_filled(center, radius, shade(ui, rect));
}

pub fn circle(ui: &mut Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    ui.painter().circle_filled(rect.center(), size / 2.0, shade(ui, rect));
}

/// Matches the pull request header: state pill, number, title, and the author line.
pub fn header(ui: &mut Ui) {
    ui.horizontal(|ui| {
        bar(ui, 56.0, 18.0);
        bar(ui, 60.0, 24.0);
        bar(ui, 420.0, 24.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), command_bar);
    });
    ui.add_space(style::SPACE_S);
    ui.horizontal(|ui| {
        circle(ui, 20.0);
        bar(ui, 360.0, 14.0);
    });
    ui.add_space(style::SPACE_XS);
}

/// Matches the header's command bar (menu, merge, votes), laid out right to left. The header
/// also shows it on its own while only the list row's summary is known.
pub fn command_bar(ui: &mut Ui) {
    bar(ui, 28.0, 28.0);
    bar(ui, 92.0, 30.0);
    bar(ui, 110.0, 30.0);
}

/// Matches a pull request list row: avatar, title line, meta line, vote icons.
pub fn list_row(ui: &Ui, rect: Rect, seed: usize) {
    let painter = ui.painter();
    let inner = rect.shrink2(vec2(28.0, 12.0));
    let avatar =
        Rect::from_center_size(egui::pos2(inner.left() + 16.0, inner.center().y), vec2(32.0, 32.0));
    painter.circle_filled(avatar.center(), 16.0, shade(ui, avatar));
    let title_width = 260.0 + (seed * 97 % 220) as f32;
    let meta_width = 340.0 + (seed * 53 % 160) as f32;
    let text_left = inner.left() + 44.0;
    let title = Rect::from_min_size(
        egui::pos2(text_left, inner.center().y - 15.0),
        vec2(title_width, 13.0),
    );
    let meta =
        Rect::from_min_size(egui::pos2(text_left, inner.center().y + 5.0), vec2(meta_width, 10.0));
    painter.rect_filled(title, 4, shade(ui, title));
    painter.rect_filled(meta, 4, shade(ui, meta));
    for index in 0..3 {
        let center = egui::pos2(inner.right() - 12.0 - index as f32 * 26.0, inner.center().y);
        painter.circle_filled(
            center,
            11.0,
            shade(ui, Rect::from_center_size(center, vec2(22.0, 22.0))),
        );
    }
}

/// The geometry of the tree the skeleton stands in for, so its rows sit where the real rows will.
pub struct TreeShape {
    pub row_height: f32,
    /// One nesting level.
    pub indent: f32,
    /// From the row's left edge to the depth-0 column.
    pub inset: f32,
    /// From a level's column to the middle of its icon slot.
    pub icon_center: f32,
    /// From a level's column to the start of the name.
    pub name_left: f32,
}

/// Depths of a plausible nested tree: folders open into files and subfolders.
const TREE_DEPTHS: [u8; 26] =
    [0, 1, 2, 2, 2, 1, 2, 3, 3, 3, 2, 1, 0, 1, 1, 2, 2, 1, 2, 3, 3, 2, 0, 1, 1, 1];
const TREE_ICON: f32 = 12.0;
const TREE_NAME_HEIGHT: f32 = 10.0;

/// Matches file tree rows: an icon and a name of varying width at a varying depth. Fills the
/// available height with whole rows of `shape`'s height, so the count only depends on the window
/// size and the real rows replace these in place.
pub fn tree_rows(ui: &mut Ui, shape: &TreeShape) {
    let (area, response) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    announce(&response, "Loading the changed files");
    let count = (area.height() / shape.row_height).ceil() as usize;
    let painter = ui.painter_at(area);
    for index in 0..count {
        let top = area.top() + index as f32 * shape.row_height;
        let center_y = top + shape.row_height / 2.0;
        let depth = f32::from(TREE_DEPTHS[index % TREE_DEPTHS.len()]);
        let column = area.left() + shape.inset + depth * shape.indent;
        let icon = Rect::from_center_size(
            egui::pos2(column + shape.icon_center, center_y),
            vec2(TREE_ICON, TREE_ICON),
        );
        let name_width =
            (70.0 + (index * 53 % 121) as f32).min(area.right() - column - shape.name_left - 8.0);
        painter.rect_filled(icon, 3, shade(ui, icon));
        if name_width > 0.0 {
            let name = Rect::from_min_size(
                egui::pos2(column + shape.name_left, center_y - TREE_NAME_HEIGHT / 2.0),
                vec2(name_width, TREE_NAME_HEIGHT),
            );
            painter.rect_filled(name, 3, shade(ui, name));
        }
    }
}

/// Matches diff lines: line-number gutters and code of varying length.
pub fn code_lines(ui: &mut Ui, count: usize, line_height: f32, is_split: bool) {
    for index in 0..count {
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), line_height), Sense::hover());
        let painter = ui.painter();
        let pane_width = if is_split { rect.width() / 2.0 } else { rect.width() };
        let panes = if is_split { 2 } else { 1 };
        for pane in 0..panes {
            let left = rect.left() + pane as f32 * pane_width;
            let indent = [0.0, 4.0, 8.0, 8.0, 12.0, 4.0, 0.0, 8.0][(index + pane) % 8] * 7.0;
            let code_width =
                (60.0 + ((index * 37 + pane * 11) % 380) as f32).min(pane_width - 120.0);
            let height = line_height * 0.45;
            let top = rect.center().y - height / 2.0;
            let gutter = Rect::from_min_size(egui::pos2(left + 10.0, top), vec2(24.0, height));
            let code = Rect::from_min_size(
                egui::pos2(left + 60.0 + indent, top),
                vec2(code_width.max(20.0), height),
            );
            painter.rect_filled(gutter, 2, shade(ui, gutter));
            painter.rect_filled(code, 2, shade(ui, code));
        }
    }
}

/// Matches a comment thread card in the timeline.
pub fn thread_cards(ui: &mut Ui, count: usize) {
    for index in 0..count {
        ui.horizontal(|ui| {
            circle(ui, 28.0);
            ui.vertical(|ui| {
                bar(ui, 180.0, 12.0);
                ui.add_space(style::SPACE_XS);
                bar(ui, 420.0 + (index * 41 % 160) as f32, 12.0);
                bar(ui, 300.0 + (index * 29 % 140) as f32, 12.0);
            });
        });
        ui.add_space(style::SPACE_L);
    }
}

#[cfg(test)]
mod tests {
    use super::{GRACE_SECONDS, bar, reveal_after, still};

    /// The skeletons a frame paints asked for the next frame to move their sweep: egui names what
    /// asked for a frame while it runs that frame, so a second one tells.
    fn sweeps(mut add: impl FnMut(&mut egui::Ui)) -> bool {
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let output = ctx.run_ui(egui::RawInput::default(), &mut add);
            output.drop_without_applying_deltas();
        }
        ctx.repaint_causes().iter().any(|cause| cause.file.ends_with("skeleton.rs"))
    }

    /// A page that waits out a rate-limit pause holds every placeholder still, also those after a
    /// part that holds its own still. Before, the inner one ended the outer one, and the rest of the
    /// page swept on at the full frame rate.
    #[test]
    fn a_still_inside_another_leaves_the_outer_still() {
        assert!(sweeps(|ui| bar(ui, 40.0, 10.0)), "a skeleton sweeps");
        let nested = sweeps(|ui| {
            still(ui, |ui| {
                still(ui, |ui| bar(ui, 40.0, 10.0));
                bar(ui, 40.0, 10.0);
            });
        });
        assert!(!nested, "the outer still holds after the inner one");
        let after = sweeps(|ui| {
            still(ui, |ui| bar(ui, 40.0, 10.0));
            bar(ui, 40.0, 10.0);
        });
        assert!(after, "past the still, skeletons sweep again");
    }

    #[test]
    fn placeholders_wait_out_the_grace_period_then_fade_in() {
        assert_eq!(reveal_after(0.0, false), 0.0);
        assert_eq!(
            reveal_after(GRACE_SECONDS - 0.01, false),
            0.0,
            "a load done within the grace period shows no skeleton"
        );
        let fading = reveal_after(GRACE_SECONDS + 0.05, false);
        assert!(fading > 0.0 && fading < 1.0, "then it fades in: {fading}");
        assert_eq!(reveal_after(GRACE_SECONDS + 1.0, false), 1.0);
        assert_eq!(reveal_after(GRACE_SECONDS - 0.01, true), 0.0, "reduced motion waits as well");
        assert_eq!(reveal_after(GRACE_SECONDS, true), 1.0, "and then shows it without fading");
    }
}
