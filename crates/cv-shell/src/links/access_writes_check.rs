//! A copied link's read ([`AccessCheck`]) against the user's own writes: a detail read before a
//! merge must not undo the merge in the page cache. The test lives here, not with the writes in
//! `cv-services`, because `AccessCheck` belongs to this crate and `cv-services` sits below it.

use cv_runtime::cache::page_cache::pull_request_part;
use cv_runtime::route::PullRequestKey;
use cv_services::Services;
use cv_services::writes::{Effect, record};
use cv_testkit::{ScratchDir, poll_for, samples};
use review_core::{PullRequestDetail, PullRequestState};

use super::link_access::AccessCheck;

fn key(number: u64) -> PullRequestKey {
    PullRequestKey { connection_id: "gh".into(), repository_key: "acme/app".into(), number }
}

/// A link opened while a merge ran reads the detail; it arrives after the merge landed, still open.
#[test]
fn a_detail_read_before_a_merge_never_undoes_it_in_the_page_cache() {
    let dir = ScratchDir::new("writes-link-read");
    let mut services =
        Services::new(egui::Context::default(), dir.app_paths(), cv_providers::create_provider);
    let part = pull_request_part(&key(7).storage_key(), "detail");
    services.cache.put(&part, samples::detail("acme/app", 7), 0.0);
    let mut read =
        AccessCheck::answering_for_test(&services, key(7), -1.0, samples::detail("acme/app", 7));
    assert!(record(&mut services, &key(7), Effect::State(PullRequestState::Merged), None));

    poll_for("the access check's answer", || read.poll(&mut services));
    let cached: PullRequestDetail = services.cache.get(&part).expect("remembered");
    assert_eq!(cached.summary.state, PullRequestState::Merged, "the older read lost");
}
