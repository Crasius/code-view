//! A copied link's read ([`AccessCheck`]) against the user's own writes: a detail read before a
//! merge must not undo the merge in the page cache. The test lives here, not with the writes in
//! `cv-services`, because `AccessCheck` belongs to this crate and `cv-services` sits below it.

use cv_runtime::cache::page_cache::pull_request_part;
use cv_runtime::route::PullRequestKey;
use cv_services::Services;
use cv_services::writes::{Effect, record};
use cv_testkit::{self as test_support, ScratchDir};
use review_core::{
    DiffStat, MergeStatus, PullRequestDetail, PullRequestState, PullRequestSummary, UserIdentity,
};
use review_store::AppPaths;

use super::link_access::AccessCheck;

fn key(number: u64) -> PullRequestKey {
    PullRequestKey { connection_id: "gh".into(), repository_key: "acme/app".into(), number }
}

fn detail(number: u64) -> PullRequestDetail {
    PullRequestDetail {
        summary: PullRequestSummary {
            repository_key: "acme/app".into(),
            repository_name: "app".into(),
            number,
            title: format!("Change {number}"),
            author: UserIdentity {
                id: "2".into(),
                display_name: "ana".into(),
                login: Some("ana".into()),
                avatar_url: None,
            },
            state: PullRequestState::Open,
            is_draft: false,
            source_branch: "feature".into(),
            target_branch: "main".into(),
            created_at: time::macros::datetime!(2026-10-16 09:30 UTC),
            updated_at: None,
            web_url: String::new(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            checks: None,
            diff_stat: DiffStat::default(),
            head_sha: None,
            target_sha: None,
        },
        description: String::new(),
        target_sha: "b".repeat(40),
        head_sha: "a".repeat(40),
        merge_status: MergeStatus::Mergeable,
        git_fetch_refs: Vec::new(),
        closed_at: None,
        auto_complete: None,
    }
}

/// A link opened while a merge ran reads the detail; it arrives after the merge landed, still open.
#[test]
fn a_detail_read_before_a_merge_never_undoes_it_in_the_page_cache() {
    let dir = ScratchDir::new("writes-link-read");
    let paths = AppPaths::new(dir.path().join("config"), dir.path().join("cache"));
    let mut services =
        Services::new(egui::Context::default(), paths, cv_providers::create_provider);
    let part = pull_request_part(&key(7).storage_key(), "detail");
    services.cache.put(&part, detail(7), 0.0);
    let mut read = AccessCheck::answering_for_test(&services, key(7), -1.0, detail(7));
    assert!(record(&mut services, &key(7), Effect::State(PullRequestState::Merged), None));

    let started = std::time::Instant::now();
    while read.poll(&mut services).is_none() {
        assert!(started.elapsed().as_secs() < 30, "the read never answered");
        test_support::let_threads_run(std::time::Duration::from_millis(5));
    }
    let cached: PullRequestDetail = services.cache.get(&part).expect("remembered");
    assert_eq!(cached.summary.state, PullRequestState::Merged, "the older read lost");
}
