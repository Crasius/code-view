//! The provider against a mocked Azure DevOps REST API, one module per subject. One test binary,
//! so the helpers in [`support`] are shared without a `dead_code` warning in a binary that uses
//! only some of them.

mod support;

mod attempts;
mod auto_complete;
mod comment_edits;
mod images;
mod linked;
mod live_updates;
mod merge_policy;
mod organizations;
mod overview;
mod rate_limits;
mod statuses;
mod writes;
