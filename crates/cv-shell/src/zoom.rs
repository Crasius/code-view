//! Contextual zoom, as in an editor inside a browser. Ctrl+wheel over a code surface (see
//! `cv_widgets::surfaces`: the file view's diff, the folder view's cards, a log, a rendered file),
//! or with keyboard focus inside one, changes only the code font size; over a pan canvas (the
//! stage graph) the canvas zooms itself; anywhere else it zooms the whole app. Ctrl+Plus,
//! Ctrl+Minus and Ctrl+0 follow keyboard focus the same way (with no widget focused, the code
//! under the pointer has it), and go to a pan canvas under the pointer unless focus is in code.
//! Both levels live in the settings, so they survive restarts. The code views keep the line under
//! the pointer in place when the code size changes.
//!
//! egui already turns Ctrl+wheel into a zoom gesture instead of a scroll, so content never
//! scrolls along; this module only reads the notches and decides what they zoom. A small overlay
//! names the new level and offers a reset.
//!
//! Changing the code size never re-highlights anything: highlighting stores token kinds per line,
//! and only the visible rows are laid out again at the new size.

use egui::{
    Align2, Context, Event, Frame, Key, Margin, Modifiers, MouseWheelUnit, Rect, RichText, Ui,
};
use review_store::AppSettings;

use cv_base::platform::shortcut;
use cv_base::theme::{self, Palette, icons, motion, style};
use cv_widgets::{buttons, surfaces};

/// Browser zoom steps.
const UI_SCALES: [f32; 11] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];
pub const UI_SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.5..=2.0;
pub const CODE_SIZE_RANGE: std::ops::RangeInclusive<f32> = 8.0..=32.0;
/// Trackpad pixels (Ctrl+pinch) per zoom step.
const POINTS_PER_STEP: f32 = 50.0;
/// The overlay stays this long after the last change, then fades out.
const OVERLAY_SECONDS: f64 = 1.6;
const OVERLAY_FADE: f32 = 0.2;
/// The widest texts `label` produces, which size its slot.
const WIDEST_LABELS: [&str; 2] = ["Zoom 200 %", "Code 32.5 px"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    App,
    Code,
}

#[derive(Default)]
pub struct Zoom {
    /// Fractional trackpad steps not applied yet.
    pending: f32,
    overlay: Option<Overlay>,
}

#[derive(Clone, Copy)]
struct Overlay {
    target: Target,
    changed_at: f64,
}

impl Zoom {
    /// Applies this frame's Ctrl+wheel and Ctrl+Plus/Minus/0 to `settings`; true when they changed.
    /// Call at the start of the frame, before the views run.
    pub fn handle_input(&mut self, ctx: &Context, settings: &mut AppSettings) -> bool {
        let (wheel_steps, wheel_target) = self.wheel(ctx);
        let mut changed = false;
        if wheel_steps != 0 {
            changed |= step(settings, wheel_target, wheel_steps);
            self.show_for(ctx, wheel_target);
        }
        if is_canvas_keys(ctx) {
            // A pan canvas takes Ctrl+Plus/Minus/0 for its own zoom.
            return changed;
        }
        let keyboard_target = focus_target(ctx);
        let (zoom_in, zoom_out, reset) = ctx.input_mut(|input| {
            let zoom_in = input.consume_key(Modifiers::COMMAND, Key::Plus)
                || input.consume_key(Modifiers::COMMAND, Key::Equals);
            let zoom_out = input.consume_key(Modifiers::COMMAND, Key::Minus);
            let reset = input.consume_key(Modifiers::COMMAND, Key::Num0);
            (zoom_in, zoom_out, reset)
        });
        let key_steps = i32::from(zoom_in) - i32::from(zoom_out);
        if key_steps != 0 {
            changed |= step(settings, keyboard_target, key_steps);
            self.show_for(ctx, keyboard_target);
        }
        if reset {
            changed |= reset_level(settings, keyboard_target);
            self.show_for(ctx, keyboard_target);
        }
        changed
    }

    /// The level overlay below the top bar's right end; true when Reset changed the settings.
    pub fn show_overlay(&mut self, ctx: &Context, settings: &mut AppSettings) -> bool {
        let Some(overlay) = self.overlay else {
            return false;
        };
        let now = ctx.input(|input| input.time);
        let age = now - overlay.changed_at;
        let fade = f64::from(motion::duration(ctx, OVERLAY_FADE));
        let remaining = OVERLAY_SECONDS + fade - age;
        let palette = theme::palette(ctx);
        let mut is_reset = false;
        let area = egui::Area::new(egui::Id::unique("zoom_overlay"))
            .anchor(Align2::RIGHT_TOP, [-16.0, 52.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let opacity = if remaining >= fade || fade <= 0.0 {
                    1.0
                } else {
                    (remaining / fade).clamp(0.0, 1.0) as f32
                };
                ui.multiply_opacity(opacity);
                Frame::new()
                    .fill(palette.surface)
                    .stroke(egui::Stroke::new(1.0, palette.border))
                    .corner_radius(style::RADIUS_POPUP)
                    .inner_margin(Margin { left: 14, right: 6, top: 6, bottom: 6 })
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        is_reset = level_row(ui, overlay.target, settings, &palette);
                    });
            });
        if is_reset {
            reset_level(settings, overlay.target);
            self.show_for(ctx, overlay.target);
            return true;
        }
        if area.response.contains_pointer() {
            // Kept open while the pointer rests on it, so Reset stays reachable.
            self.overlay = Some(Overlay { changed_at: now, ..overlay });
            ctx.request_repaint_after_secs(OVERLAY_SECONDS as f32);
        } else if remaining <= 0.0 {
            self.overlay = None;
        } else if remaining <= fade {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after_secs((remaining - fade) as f32);
        }
        false
    }

    /// Whole notches of Ctrl+wheel this frame and what they zoom.
    fn wheel(&mut self, ctx: &Context) -> (i32, Target) {
        let delta: f32 = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    Event::MouseWheel { unit, delta, modifiers, .. }
                        if modifiers.command || modifiers.ctrl =>
                    {
                        Some(match unit {
                            MouseWheelUnit::Point => delta.y / POINTS_PER_STEP,
                            MouseWheelUnit::Line | MouseWheelUnit::Page => delta.y,
                        })
                    }
                    _ => None,
                })
                .sum()
        });
        let pointer = ctx.pointer_hover_pos();
        // Over a pan canvas the canvas zooms itself.
        if delta == 0.0 || pointer.is_some_and(|pos| surfaces::pans_at(ctx, pos)) {
            return (0, Target::App);
        }
        self.pending += delta;
        let steps = self.pending.trunc();
        self.pending -= steps;
        let over_code = pointer.is_some_and(|pos| surfaces::is_code_at(ctx, pos));
        let target = if over_code { Target::Code } else { focus_target(ctx) };
        (steps as i32, target)
    }

    fn show_for(&mut self, ctx: &Context, target: Target) {
        let now = ctx.input(|input| input.time);
        self.overlay = Some(Overlay { target, changed_at: now });
        ctx.request_repaint();
    }
}

/// The level and its Reset button; true when Reset was clicked.
fn level_row(ui: &mut Ui, target: Target, settings: &AppSettings, palette: &Palette) -> bool {
    ui.horizontal(|ui| {
        // A fixed slot sized for the widest level, so the overlay keeps its width while the level
        // steps.
        let font = egui::TextStyle::Body.resolve(ui.style());
        let slot = WIDEST_LABELS
            .iter()
            .map(|text| {
                ui.painter().layout_no_wrap((*text).to_owned(), font.clone(), palette.text).size().x
            })
            .fold(0.0, f32::max);
        let layout = egui::Layout::left_to_right(egui::Align::Center);
        ui.allocate_ui_with_layout(egui::vec2(slot, ui.spacing().interact_size.y), layout, |ui| {
            ui.set_min_width(slot);
            ui.label(RichText::new(label(target, settings)).color(palette.text));
        });
        ui.add_space(style::SPACE_S);
        let is_default = is_default(settings, target);
        let reset = ui.add_enabled_ui(!is_default, |ui| {
            buttons::secondary_icon(ui, icons::ARROW_COUNTER_CLOCKWISE, "Reset")
                .on_hover_text(shortcut!(command "0"))
        });
        reset.inner.clicked()
    })
    .inner
}

/// Code while keyboard focus sits inside a code surface, or while no widget has it and the pointer
/// rests on code (the keys that act on code go to the code under the pointer then, as a diff's
/// selection keys do); the app otherwise.
fn focus_target(ctx: &Context) -> Target {
    if ctx.memory(|memory| memory.focused().is_none()) {
        let over_code = ctx.pointer_hover_pos().is_some_and(|pos| surfaces::is_code_at(ctx, pos));
        return if over_code { Target::Code } else { Target::App };
    }
    match focused_rect(ctx) {
        Some(rect) if surfaces::is_code_around(ctx, rect) => Target::Code,
        _ => Target::App,
    }
}

/// Ctrl+Plus/Minus/0 belong to a pan canvas while keyboard focus sits inside it, or while the
/// pointer is over it and focus is not in code.
fn is_canvas_keys(ctx: &Context) -> bool {
    match focused_rect(ctx) {
        Some(rect) if surfaces::is_pan_around(ctx, rect) => true,
        Some(rect) if surfaces::is_code_around(ctx, rect) => false,
        _ => ctx.pointer_hover_pos().is_some_and(|pos| surfaces::pans_at(ctx, pos)),
    }
}

/// The focused widget on screen (a widget on a zoomed layer is mapped through its transform).
fn focused_rect(ctx: &Context) -> Option<Rect> {
    let response = ctx.memory(egui::Memory::focused).and_then(|id| ctx.read_response(id))?;
    Some(
        ctx.layer_transform_to_global(response.layer_id)
            .map_or(response.rect, |to_screen| to_screen * response.rect),
    )
}

#[expect(clippy::float_cmp, reason = "a step that clamps to the same value changes nothing")]
fn step(settings: &mut AppSettings, target: Target, steps: i32) -> bool {
    match target {
        Target::App => {
            let scale = next_scale(settings.ui_scale, steps);
            let changed = scale != settings.ui_scale;
            settings.ui_scale = scale;
            changed
        }
        Target::Code => {
            let size = (settings.code_font_size.round() + steps as f32)
                .clamp(*CODE_SIZE_RANGE.start(), *CODE_SIZE_RANGE.end());
            let changed = size != settings.code_font_size;
            settings.code_font_size = size;
            changed
        }
    }
}

/// The browser step `steps` away from `scale` (a scale between steps snaps to its neighbor).
pub fn next_scale(scale: f32, steps: i32) -> f32 {
    let mut scale = scale.clamp(*UI_SCALE_RANGE.start(), *UI_SCALE_RANGE.end());
    for _ in 0..steps.unsigned_abs() {
        scale = if steps > 0 {
            UI_SCALES.iter().copied().find(|candidate| *candidate > scale + 0.001).unwrap_or(scale)
        } else {
            UI_SCALES
                .iter()
                .rev()
                .copied()
                .find(|candidate| *candidate < scale - 0.001)
                .unwrap_or(scale)
        };
    }
    scale
}

fn reset_level(settings: &mut AppSettings, target: Target) -> bool {
    let was_default = is_default(settings, target);
    match target {
        Target::App => settings.ui_scale = AppSettings::DEFAULT_UI_SCALE,
        Target::Code => settings.code_font_size = AppSettings::DEFAULT_CODE_FONT_SIZE,
    }
    !was_default
}

fn is_default(settings: &AppSettings, target: Target) -> bool {
    match target {
        Target::App => settings.ui_scale == AppSettings::DEFAULT_UI_SCALE,
        Target::Code => settings.code_font_size == AppSettings::DEFAULT_CODE_FONT_SIZE,
    }
}

fn label(target: Target, settings: &AppSettings) -> String {
    match target {
        Target::App => format!("Zoom {:.0} %", settings.ui_scale * 100.0),
        Target::Code => format!("Code {} px", trim_number(settings.code_font_size)),
    }
}

fn trim_number(value: f32) -> String {
    if value.fract() == 0.0 { format!("{value:.0}") } else { format!("{value:.1}") }
}

#[cfg(test)]
mod tests {
    use super::{AppSettings, Target, label, next_scale, step};

    #[test]
    fn app_zoom_walks_browser_steps_and_clamps() {
        assert_eq!(next_scale(1.0, 1), 1.1);
        assert_eq!(next_scale(1.0, -2), 0.8);
        assert_eq!(next_scale(1.05, 1), 1.1, "between steps snaps up");
        assert_eq!(next_scale(2.0, 3), 2.0);
        assert_eq!(next_scale(0.5, -1), 0.5);
    }

    #[test]
    fn code_zoom_moves_in_whole_pixels_within_range() {
        let mut settings = AppSettings::default();
        assert!(step(&mut settings, Target::Code, 2));
        assert_eq!(settings.code_font_size, 15.0);
        assert_eq!(label(Target::Code, &settings), "Code 15 px");
        step(&mut settings, Target::Code, -100);
        assert_eq!(settings.code_font_size, 8.0);
        assert!(!step(&mut settings, Target::Code, -1));
        assert_eq!(settings.ui_scale, 1.0, "code zoom leaves the app zoom alone");
        settings.ui_scale = 1.1;
        assert_eq!(label(Target::App, &settings), "Zoom 110 %");
    }
}
