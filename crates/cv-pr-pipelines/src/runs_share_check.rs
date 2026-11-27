//! The tab holds the session's runs for a frame by sharing them: a frame of the checks table and
//! of a run's page leaves the runs' `Arc` as it found it. No network, no tokens.

use std::sync::Arc;

use cv_base::theme;
use cv_pr_session::session::PullRequestSession;
use cv_runtime::cache::page_cache::pull_request_part;
use cv_runtime::route::PullRequestKey;
use cv_services::Services;
use cv_testkit::provider::{Script, Scripted};
use cv_testkit::{ScratchDir, samples};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{PipelineRun, ProviderKind, RunStatus};
use review_store::{ConnectionStore, ThemePreference};

use super::PipelinesTab;

const CONNECTION: &str = "pipelines-share";

/// Answers nothing the test reads: the runs come from the page cache, and the frames never poll
/// the session.
struct Unscripted;

impl Script for Unscripted {}

/// The tab and what it draws from. The services drop last: the session's loads run on their
/// runtime.
struct Scene {
    tab: PipelinesTab,
    session: PullRequestSession,
    services: Services,
}

fn runs() -> Vec<PipelineRun> {
    vec![PipelineRun {
        id: "status:lint".to_owned(),
        name: "lint".to_owned(),
        status: RunStatus::Succeeded,
        is_head: true,
        ..PipelineRun::default()
    }]
}

#[test]
fn a_frame_of_the_tab_leaves_the_shared_runs_as_it_found_them() {
    let scratch = ScratchDir::new(CONNECTION);
    let paths = scratch.app_paths();
    let mut store = ConnectionStore::open(&paths).expect("empty store");
    store
        .upsert(samples::connection(CONNECTION, ProviderKind::GitHub, "https://api.github.com"))
        .expect("store the connection");
    drop(store);
    let mut harness = Harness::builder()
        .with_size([1200.0, 700.0])
        .with_context_setup(theme::fonts::install)
        .build_ui_state(
            |ui, scene: &mut Option<Scene>| {
                if let Some(Scene { tab, session, services }) = scene {
                    theme::activate(ui.ctx(), ThemePreference::Light);
                    tab.show(ui, session, services);
                }
            },
            None,
        );
    let mut services =
        Services::new(harness.ctx.clone(), paths, |_, _| Arc::new(Scripted(Unscripted)));
    let key = PullRequestKey {
        connection_id: CONNECTION.to_owned(),
        repository_key: "erika/fabrikam-logistics".to_owned(),
        number: 7,
    };
    let now = services.now();
    services.cache.put(&pull_request_part(&key.storage_key(), "runs"), runs(), now);
    let session = PullRequestSession::open(key, &mut services).expect("the session opens");
    *harness.state_mut() = Some(Scene { tab: PipelinesTab::default(), session, services });
    let shared = |harness: &Harness<'_, Option<Scene>>| {
        let scene = harness.state().as_ref().expect("the scene is set up");
        Arc::clone(scene.session.runs.ready().expect("the runs from the page cache"))
    };
    let before = shared(&harness);
    let count = Arc::strong_count(&before);

    harness.step();
    assert!(harness.query_by_label("lint, Succeeded").is_some(), "the checks table shows the run");
    assert_eq!(Arc::strong_count(&before), count, "the checks table let go of the runs it held");

    harness.state_mut().as_mut().expect("the scene is set up").tab.open_run("status:lint");
    harness.step();
    assert!(harness.query_by_label("No job details").is_some(), "the run's page shows");
    assert_eq!(Arc::strong_count(&before), count, "the run's page let go of the runs it held");
    assert!(Arc::ptr_eq(&before, &shared(&harness)), "the session still holds the same runs");
}
