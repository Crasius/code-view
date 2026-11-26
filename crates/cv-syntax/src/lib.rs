//! Code View's syntax coloring, shared by the diff, the thread cards and rendered Markdown: the
//! [`grammar`] for a file or a fenced block and its compiled sets, a file's text split into lines
//! ([`blob_text`]), whole-file highlighting into compact per-line spans ([`highlight`]) on kept
//! reader threads ([`pool`]), the thread that colors one file at a time ([`color_worker`]) and the
//! colors of code shown outside the diff ([`code_colors`]).
// RS-LINT-02: line starts and span offsets are stored as u32 to keep a large file's index small
// (`blob_text.rs`, `highlight.rs`).
#![expect(clippy::cast_possible_truncation, reason = "line and span offsets are stored as u32")]
#![forbid(unsafe_code)]

pub mod blob_text;
pub mod code_colors;
pub mod color_worker;
pub mod grammar;
pub mod highlight;
pub mod pool;
