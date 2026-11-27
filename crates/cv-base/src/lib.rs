//! Code View's base layer, below every other `cv-*` crate: [`platform`] (everything the app does
//! differently per operating system, and the counting global allocator in [`platform::heap`]),
//! [`theme`] (the visual identity), [`frame`] (the frame budgets and percentiles the frame stats
//! overlay and the frame-budget benchmarks share) and [`mailbox`] (one-shot hand-overs between
//! widgets). `frame_probe` (with `test-support`) lets tests read what shipped paint code painted.
// RS-LINT-02: egui lays out in f32 points while rows, columns and byte offsets are usize, so pixel
// math converts between them (`frame.rs`, `theme/`, `platform/windows/`). Only the Win32 code
// reinterprets signed and unsigned words (`windows/caption.rs`, `windows/placement.rs`).
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "egui pixel math converts between f32 points and usize indices"
)]
#![cfg_attr(
    windows,
    expect(
        clippy::cast_possible_wrap,
        reason = "Win32 words carry signed values in unsigned types"
    )
)]

pub mod frame;
#[cfg(any(test, feature = "test-support"))]
pub mod frame_probe;
pub mod mailbox;
pub mod platform;
pub mod theme;

/// The crate was built with the `test-support` feature: test probes and fakes are compiled in.
/// The binary refuses a release build with it (`main.rs`).
pub const TEST_SUPPORT: bool = cfg!(feature = "test-support");
