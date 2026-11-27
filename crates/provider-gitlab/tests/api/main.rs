//! The GitLab provider against a mock GitLab host (wiremock), one module per subject in one test
//! binary; `support` builds the provider on the app's HTTP client.

mod auto_merge;
mod comment_edits;
mod images;
mod linked;
mod live_updates;
mod merge_policy;
mod overview;
mod pages;
mod pipelines;
mod rate_limits;
mod row_statuses;
mod setup;
mod support;
mod writes;
