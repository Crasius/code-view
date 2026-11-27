//! Retention for the machine-local cache folder ([`crate::AppPaths::cache_dir`], under
//! `%LOCALAPPDATA%` on Windows), so nothing piles up there:
//!
//! - pipeline logs: gone after 7 days; over 1 GB the least recently used go first;
//! - git mirrors: gone after 30 days unused; over 10 GB the least recently used go first,
//!   always whole, never while the open pull request or a git command uses them;
//! - viewed state and thread read marks/drafts: gone after 90 days unused;
//! - embedded images: gone after 14 days unread (every read touches the file); over 256 MB the
//!   least recently read go first;
//! - everything of a removed connection;
//! - everything of a merged or closed pull request, or one whose source branch is gone, at once
//!   (see `sweep`).
//!
//! Deleting renames into `cache\trash` and lets a low-priority thread do the rest (see `trash`).

mod files;
mod leases;
mod mirror_stamp;
mod mirrors;
mod policy;
mod refs;
mod sweep;
mod trash;
mod usage;
mod worker;

#[cfg(test)]
mod finished_tests;
#[cfg(test)]
mod sweep_tests;

use std::time::Duration;

pub use leases::Lease;
pub use mirror_stamp::{MirrorUse, record_use as record_mirror_use};
pub use mirrors::MirrorGuard;
pub(crate) use policy::Limits;
pub use policy::Outcome;
pub use usage::StorageUsage;
pub use worker::{Retention, Schedule, StorageStatus};

pub(crate) use sweep::connection_ids;
#[cfg(test)]
pub(crate) use trash::remove_tree;

const DAY: Duration = Duration::from_hours(24);
const GIB: u64 = 1024 * 1024 * 1024;

pub const LOG_LIMITS: Limits = Limits { max_age: DAY.saturating_mul(7), max_total: Some(GIB) };
pub const MIRROR_LIMITS: Limits =
    Limits { max_age: DAY.saturating_mul(30), max_total: Some(10 * GIB) };
pub const VIEWED_LIMITS: Limits = Limits { max_age: DAY.saturating_mul(90), max_total: None };
pub(crate) const THREAD_LIMITS: Limits =
    Limits { max_age: DAY.saturating_mul(90), max_total: None };
pub(crate) const IMAGE_LIMITS: Limits =
    Limits { max_age: DAY.saturating_mul(14), max_total: Some(GIB / 4) };
