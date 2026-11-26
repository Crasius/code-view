//! Reusable presentational widgets. No services, no I/O: data in, response out.
#![forbid(unsafe_code)]
// RS-LINT-02: egui lays out in f32 points while rows, columns and byte offsets are usize, so pixel
// math converts between them (`mono_text.rs`, `skeleton.rs`, `change_breakdown.rs`, `numbers.rs`).
// `progress_log/redact.rs` parses external text and opts back in with the RS-DONT-04 #![warn(...)].
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "egui pixel math converts between f32 points and usize indices"
)]

pub mod atlas_watch;
pub mod avatar;
pub mod badges;
pub mod buttons;
pub mod change_breakdown;
pub mod checkbox;
pub mod copy_chip;
pub mod crumb_fit;
pub mod elide;
pub mod find_bar;
pub mod focus;
pub mod glyph;
pub mod merge_strategy_icon;
pub mod mono_text;
pub mod numbers;
pub mod progress_log;
pub mod provider_icon;
pub mod refresh_status;
pub mod row;
pub mod scroll;
pub mod settle;
pub mod skeleton;
pub mod splitter;
pub mod star;
pub mod states;
pub mod surfaces;
pub mod tabs;
pub mod text_selection;
pub mod time_text;
