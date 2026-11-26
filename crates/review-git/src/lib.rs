//! Local diff engine. Each repository gets a bare, blob-less partial clone; trees and commits
//! are fetched once, blobs only when a diff or preview needs them. All heavy lifting is done by
//! the `git` CLI, so a 100k-line pull request costs a few git invocations, not thousands of API calls.

// RS-DONT-03: no FFI here; unsafe belongs in the platform/ modules of review-store and review-app.
#![forbid(unsafe_code)]

mod changed_files;
pub mod checkout;
mod command;
mod commits;
mod file_diff;
mod line_counts;
mod mirror;
mod platform;
mod progress;
mod whitespace;

pub use changed_files::{ChangeKind, ChangedFile};
pub use commits::CommitInfo;
pub use file_diff::{DiffLine, FileDiff, LineKind};
pub use line_counts::{COUNT_WAIT, CountSink, WantedBy, download_chunk_sizes};
pub use mirror::{DiffOptions, FULL_FILE_CONTEXT, FileEntry, GitMirror, local_ref_name};
pub use progress::{GitEvent, GitProgress, ProgressSink};
pub use whitespace::{EdgeChange, WhitespaceSummary, is_whitespace};
