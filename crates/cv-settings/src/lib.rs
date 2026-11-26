//! Settings and the connection setup. [`settings`] is the Settings page: its sections, their rows,
//! the previews beside them and the keyboard shortcut table. [`onboarding`] is the guided wizard
//! that adds, edits and fixes a connection, the access banner for token problems and the CLI
//! sign-ins; Settings opens it and hosts it while it runs. Views elsewhere reach both through the
//! mailboxes in `cv_runtime::intents`, without naming this crate.

#![forbid(unsafe_code)]
// RS-LINT-02: egui lays out in f32 points while line counts, byte sizes and margins are usize, u64
// or i8.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "egui pixel math converts between f32 points and usize indices"
)]

pub mod onboarding;
pub mod settings;
