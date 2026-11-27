//! Short confirmations, notes and errors in the bottom-right corner. An error always carries its
//! ways out ([`WaysOut`], `ways_out`) and stays until one is clicked or it is dismissed; the
//! same error again replaces the one shown instead of stacking.
//!
//! The clock is egui's input time, like the refresh schedule's: a toast's age counts from the
//! first frame shown after it came, so one pushed while the window is hidden waits until it is
//! shown, and tests drive expiry with simulated frames.

mod ways_out;

use std::path::Path;
use std::time::Duration;

use cv_base::theme::{self, Palette, icons, motion, style};
use cv_widgets::{buttons, glyph};
use egui::{Align2, Frame, Margin, RichText, Ui};

use self::ways_out::Run;
pub use self::ways_out::{ProviderFailure, ToastAction, WaysOut, take_retry};

const SHOW_FOR: Duration = Duration::from_secs(5);
/// Toasts with buttons stay longer: there is something to decide. Errors with buttons stay until
/// dismissed: what failed has not happened, and the way out must not fade away.
const SHOW_WITH_ACTIONS_FOR: Duration = Duration::from_secs(10);
const MAX_VISIBLE: usize = 4;
/// Fade-in after appearing and fade-out before expiring, in seconds.
const FADE_IN: f32 = 0.18;
const FADE_OUT: f32 = 0.3;
const TOAST_WIDTH: f32 = 420.0;
const ICON_SIZE: f32 = 18.0;
/// The left inset is `style::SPACE_ROW_X`; `Margin` takes `i8`.
const CARD_MARGIN: Margin = Margin { left: 14, right: 6, top: 6, bottom: 6 };

#[derive(Clone, Copy, PartialEq, Eq)]
enum Severity {
    Success,
    Error,
    Info,
}

struct Toast {
    text: String,
    severity: Severity,
    /// The input time the toast's age counts from: its first shown frame, moved
    /// forward while the pointer holds it. `None` until first shown.
    shown_at: Option<f64>,
    actions: Vec<ToastAction>,
}

impl Toast {
    /// How long the toast shows; `None` until dismissed.
    fn lifetime(&self) -> Option<Duration> {
        match (self.severity, self.actions.is_empty()) {
            (_, true) => Some(SHOW_FOR),
            (Severity::Error, false) => None,
            (Severity::Success | Severity::Info, false) => Some(SHOW_WITH_ACTIONS_FOR),
        }
    }

    fn is_expired(&self, now: f64) -> bool {
        self.lifetime().is_some_and(|lifetime| self.age(now) >= lifetime)
    }

    /// Seconds until the toast expires; infinite for one that waits to be dismissed.
    fn seconds_left(&self, now: f64) -> f32 {
        self.lifetime()
            .map_or(f32::INFINITY, |lifetime| lifetime.as_secs_f32() - self.age(now).as_secs_f32())
    }

    fn age(&self, now: f64) -> Duration {
        self.shown_at
            .map_or(Duration::ZERO, |shown_at| Duration::from_secs_f64((now - shown_at).max(0.0)))
    }
}

/// Short confirmations and errors in the bottom-right corner. They fade in and out and can be
/// dismissed early.
#[derive(Default)]
pub struct Toasts {
    toasts: Vec<Toast>,
}

impl Toasts {
    pub fn success(&mut self, text: impl Into<String>) {
        self.push(text.into(), Severity::Success, Vec::new());
    }

    /// A failure with its ways out (Retry, "Fix token", "Open on GitHub", …); it stays until one
    /// is clicked or it is dismissed, and replaces the same failure shown before.
    pub fn error(&mut self, text: impl Into<String>, ways_out: impl Into<WaysOut>) {
        let text = text.into();
        self.toasts.retain(|toast| toast.severity != Severity::Error || toast.text != text);
        self.push(text, Severity::Error, ways_out.into().into_actions());
    }

    /// Shows `folder` in the system's file manager; a failure says so with the path to copy.
    /// Test builds record the folder instead (`opened_folder`).
    pub fn open_folder(&mut self, ctx: &egui::Context, folder: &Path) {
        if let Err(error) = open_folder(ctx, folder) {
            let path = folder.display().to_string();
            self.error(
                format!("Could not open the folder {path}: {error}"),
                ToastAction::copy("Copy path", path),
            );
        }
    }

    /// A neutral note with buttons, e.g. "Back" after the app went somewhere on its own.
    pub fn info(&mut self, text: impl Into<String>, actions: Vec<ToastAction>) {
        self.push(text.into(), Severity::Info, actions);
    }

    fn push(&mut self, text: String, severity: Severity, actions: Vec<ToastAction>) {
        self.toasts.push(Toast { text, severity, shown_at: None, actions });
    }

    /// The texts of every toast waiting to be shown or dismissed (test builds only).
    #[cfg(any(test, feature = "test-support"))]
    pub fn texts(&self) -> Vec<&str> {
        self.toasts.iter().map(|toast| toast.text.as_str()).collect()
    }

    /// The texts of the error toasts waiting to be shown or dismissed (test builds only).
    #[cfg(any(test, feature = "test-support"))]
    pub fn error_texts(&self) -> Vec<&str> {
        self.toasts
            .iter()
            .filter(|toast| toast.severity == Severity::Error)
            .map(|toast| toast.text.as_str())
            .collect()
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|input| input.time);
        for toast in &mut self.toasts {
            toast.shown_at.get_or_insert(now);
        }
        self.toasts.retain(|toast| !toast.is_expired(now));
        if self.toasts.is_empty() {
            return;
        }
        let palette = theme::palette(ctx);
        let first_visible = self.toasts.len().saturating_sub(MAX_VISIBLE);
        let fades = Fades {
            fade_in: motion::duration(ctx, FADE_IN),
            fade_out: motion::duration(ctx, FADE_OUT),
        };
        let mut clicked = None;
        let area = egui::Area::new(egui::Id::unique("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, [-style::SPACE_L, -style::SPACE_L])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(TOAST_WIDTH);
                for (index, toast) in self.toasts.iter().enumerate().skip(first_visible) {
                    let opacity = fades.opacity(toast, now);
                    let click = ui
                        .scope(|ui| {
                            ui.multiply_opacity(opacity);
                            card(ui, toast, &palette)
                        })
                        .inner;
                    if let Some(click) = click {
                        clicked = Some((index, click));
                    }
                    ui.add_space(style::SPACE_S);
                }
            });
        // Toasts with buttons wait while the pointer is on its way to one; they start over after.
        if area.response.contains_pointer() {
            let held_at = now - f64::from(fades.fade_in);
            let waits =
                |toast: &&mut Toast| !toast.actions.is_empty() && toast.lifetime().is_some();
            for toast in self.toasts.iter_mut().filter(waits) {
                toast.shown_at = Some(held_at);
            }
        }
        match clicked {
            Some((index, Click::Action(position))) => {
                let mut toast = self.toasts.remove(index);
                match toast.actions.swap_remove(position).run {
                    Run::Context(run) => run(ctx),
                    Run::OpenFolder(folder) => self.open_folder(ctx, &folder),
                }
                ctx.request_repaint();
            }
            Some((index, Click::Dismiss)) => {
                self.toasts.remove(index);
            }
            None => {}
        }
        self.schedule_repaint(ctx, now, fades);
    }

    /// Every frame while a toast fades, else once when the next one starts fading out.
    fn schedule_repaint(&self, ctx: &egui::Context, now: f64, fades: Fades) {
        let is_fading = self.toasts.iter().any(|toast| {
            toast.age(now).as_secs_f32() < fades.fade_in || toast.seconds_left(now) < fades.fade_out
        });
        if is_fading {
            ctx.request_repaint();
            return;
        }
        let fade_out_starts = Duration::from_secs_f32(fades.fade_out);
        // Toasts that wait to be dismissed need no timer.
        let next_change = self
            .toasts
            .iter()
            .filter_map(|toast| {
                let lifetime = toast.lifetime()?;
                Some(lifetime.saturating_sub(toast.age(now)).saturating_sub(fade_out_starts))
            })
            .min();
        if let Some(next_change) = next_change {
            ctx.request_repaint_after(next_change);
        }
    }
}

/// The fade-in and fade-out durations of this frame, in seconds (zero with reduced motion).
#[derive(Clone, Copy)]
struct Fades {
    fade_in: f32,
    fade_out: f32,
}

impl Fades {
    fn opacity(self, toast: &Toast, now: f64) -> f32 {
        fade(toast.age(now).as_secs_f32(), self.fade_in)
            .min(fade(toast.seconds_left(now), self.fade_out))
    }
}

/// What was clicked in a toast.
#[derive(Clone, Copy)]
enum Click {
    Dismiss,
    /// The action at this position.
    Action(usize),
}

/// One toast: its icon and message with the dismiss button, and its actions under the message.
fn card(ui: &mut Ui, toast: &Toast, palette: &Palette) -> Option<Click> {
    let (icon, color) = match toast.severity {
        Severity::Success => (icons::CHECK_CIRCLE, palette.success),
        Severity::Error => (icons::WARNING, palette.error),
        Severity::Info => (icons::CLIPBOARD_TEXT, palette.accent_text),
    };
    let mut click = None;
    Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(style::RADIUS_POPUP)
        .inner_margin(CARD_MARGIN)
        .shadow(ui.visuals().popup_shadow)
        .show(ui, |ui| {
            let icon_width = ui
                .horizontal(|ui| {
                    let icon =
                        glyph::decorative(ui, RichText::new(icon).color(color).size(ICON_SIZE));
                    message(ui, &toast.text, palette);
                    if buttons::icon(ui, icons::X, "Dismiss").clicked() {
                        click = Some(Click::Dismiss);
                    }
                    icon.rect.width() + ui.spacing().item_spacing.x
                })
                .inner;
            if toast.actions.is_empty() {
                return;
            }
            // The buttons line up under the message.
            ui.horizontal(|ui| {
                ui.add_space(icon_width - ui.spacing().item_spacing.x);
                for (position, action) in toast.actions.iter().enumerate() {
                    if buttons::subtle(ui, action.icon, &action.label).clicked() {
                        click = Some(Click::Action(position));
                    }
                }
            });
        });
    click
}

/// Long messages wrap inside the toast; the dismiss button keeps its room.
fn message(ui: &mut Ui, text: &str, palette: &Palette) {
    let button_room = buttons::SUBTLE_HEIGHT + ui.spacing().item_spacing.x;
    ui.scope(|ui| {
        ui.set_max_width((ui.available_width() - button_room).max(0.0));
        ui.add(egui::Label::new(RichText::new(text).color(palette.text)).wrap());
    });
}

/// Eased 0..1 over `seconds` of `elapsed`; instant when `seconds` is zero (reduced motion).
fn fade(elapsed: f32, seconds: f32) -> f32 {
    if seconds <= 0.0 { 1.0 } else { motion::ease_out_cubic(elapsed / seconds) }
}

const OPENED_FOLDER: &str = "toasts.opened_folder";

/// The system's file manager on `folder`. Test builds open nothing: the folder is kept for
/// `opened_folder`.
fn open_folder(ctx: &egui::Context, folder: &Path) -> std::io::Result<()> {
    if cfg!(any(test, feature = "test-support")) {
        ctx.data_mut(|data| {
            data.insert_temp(egui::Id::unique(OPENED_FOLDER), folder.to_path_buf())
        });
        return Ok(());
    }
    cv_base::platform::open_folder(folder)
}

/// The folder an "Open folder" button opened last (test builds only).
#[cfg(any(test, feature = "test-support"))]
pub fn opened_folder(ctx: &egui::Context) -> Option<std::path::PathBuf> {
    ctx.data(|data| data.get_temp(egui::Id::unique(OPENED_FOLDER)))
}

#[cfg(test)]
mod tests;
