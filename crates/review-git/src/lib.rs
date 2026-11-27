//! Local diff engine. Each repository gets a bare, blob-less partial clone; trees and commits
//! are fetched once, blobs only when a diff or preview needs them. All heavy lifting is done by
//! the `git` CLI, so a 100k-line pull request costs a few git invocations, not thousands of API calls.

// RS-DONT-03: no FFI in this crate.
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
#[cfg(test)]
mod test_repo;
mod whitespace;

pub use changed_files::{ChangeKind, ChangedFile};
pub use commits::CommitInfo;
pub use file_diff::{DiffLine, FileDiff, LineKind};
pub use line_counts::{COUNT_WAIT, WantedBy, download_chunk_sizes};
pub use mirror::{DiffOptions, FULL_FILE_CONTEXT, GitMirror, local_ref_name};
pub use progress::{GitEvent, GitProgress};
pub use whitespace::{EdgeChange, WhitespaceSummary};
