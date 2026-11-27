//! Model values for tests, one constructor per type: a test sets the fields its assertions read by
//! struct update (`PullRequestSummary { title: "…".into(), ..samples::summary("acme/app", 7) }`)
//! and leaves the rest here, so a new field of a model type edits this file instead of every test
//! that builds one.
//!
//! No `person`: the crates' person helpers differ in their login rules on purpose, and
//! `UserIdentity` derives `Default`.

use review_core::{
    Connection, DiffStat, MergeStatus, ProviderKind, PullRequestDetail, PullRequestState,
    PullRequestSummary, UserIdentity,
};

/// Open, not a draft, by [`author`], `feature/{number}` into `main`, created 2026-10-16 09:30 UTC,
/// titled "Change {number}"; `repository_name` is the key's last segment; everything else empty or `None`.
pub fn summary(repository_key: &str, number: u64) -> PullRequestSummary {
    PullRequestSummary {
        repository_key: repository_key.to_owned(),
        repository_name: repository_key.rsplit('/').next().unwrap_or(repository_key).to_owned(),
        number,
        title: format!("Change {number}"),
        author: author(),
        state: PullRequestState::Open,
        is_draft: false,
        source_branch: format!("feature/{number}"),
        target_branch: "main".to_owned(),
        created_at: time::macros::datetime!(2026-10-16 09:30 UTC),
        updated_at: None,
        web_url: String::new(),
        labels: Vec::new(),
        reviewers: Vec::new(),
        checks: None,
        diff_stat: DiffStat::default(),
        head_sha: None,
        target_sha: None,
    }
}

/// [`summary`] as a detail: head "a" x 40, target "b" x 40, mergeable, no description, no fetch refs.
pub fn detail(repository_key: &str, number: u64) -> PullRequestDetail {
    PullRequestDetail {
        summary: summary(repository_key, number),
        description: String::new(),
        target_sha: "b".repeat(40),
        head_sha: "a".repeat(40),
        merge_status: MergeStatus::Mergeable,
        git_fetch_refs: Vec::new(),
        closed_at: None,
        auto_complete: None,
    }
}

/// A connection named after its id, without organizations or token.
pub fn connection(id: &str, kind: ProviderKind, base_url: &str) -> Connection {
    Connection {
        id: id.to_owned(),
        name: id.to_owned(),
        kind,
        base_url: base_url.to_owned(),
        organizations: Vec::new(),
        token: None,
    }
}

/// Ada Lovelace (id "1", login "ada"): the author of every sample, and the user a `Script` signs in.
pub fn author() -> UserIdentity {
    UserIdentity {
        id: "1".to_owned(),
        display_name: "Ada Lovelace".to_owned(),
        login: Some("ada".to_owned()),
        avatar_url: None,
    }
}
