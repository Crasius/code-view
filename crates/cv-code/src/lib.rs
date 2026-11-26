//! The code of a text diff, shared by the Files tab's file view, the folder summary's cards and the
//! thread cards' code excerpts: the rows of a diff ([`diff_rows`], with [`gaps`] and the summary's
//! [`compaction`]), the full old and new text with its colors ([`contents`]), the code line each
//! row shows ([`line_source`]) and how it is painted ([`code_job`], [`diff_paint`], the find marks
//! of [`find_paint`]).

#![forbid(unsafe_code)]
// RS-LINT-02: egui lays out in f32 points while rows, columns and line indices are usize and u32,
// so the row and pixel math converts between them (code_job, diff_paint, diff_rows, find_paint).
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "egui pixel math converts between f32 points and usize indices"
)]

pub mod code_job;
pub mod compaction;
pub mod contents;
pub mod diff_paint;
pub mod diff_rows;
pub mod find_paint;
pub mod gaps;
pub mod line_source;
