//! Window chrome: the navigation rail and the breadcrumbs (their trail and the routes are
//! `cv_runtime`'s), the app-wide input behaviors ([`input`], contextual [`zoom`], pull request
//! [`links`]), the single instance and the browser extension's side ([`browser`]). The title bar
//! lives in the bin.

// RS-LINT-02: egui lays out in f32 points while rows, columns and byte offsets are usize, so pixel
// math converts between them (frame_stats, zoom, autoscroll, smooth_wheel).
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "egui pixel math converts between f32 points and usize indices"
)]

pub mod attention;
pub mod browser;
pub mod focus_mode;
pub mod frame_pacing;
pub mod frame_stats;
pub mod input;
pub mod links;
pub mod nav_rail;
pub mod rate_limit_banner;
pub mod single_instance;
pub mod top_bar;
pub mod window_placement;
pub mod zoom;
