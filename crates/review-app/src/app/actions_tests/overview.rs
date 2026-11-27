//! The overview while a write of the user's is on its way: its row shows the write as pending,
//! leaves the open list once the merge went through, and is itself again after a failure. The
//! write's page is never polled here, as when the user left it for the overview: the list alone
//! records the outcome.

use std::time::Instant;

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{ProviderError, PullRequestState, Result};
use tokio::sync::oneshot;

use super::{ActionRunner, Refresh, TIMEOUT, key, scripted_services};
use cv_base::theme;
use cv_lists::pull_request_list::{ListSource, PullRequestListPage};
use cv_runtime::route::Navigator;
use cv_services::Services;
use cv_services::writes::Effect;
use cv_testkit::ScratchDir;

const ROW: &str = "7 Speed up login";
const PENDING: &str = "Pending on #7: Merging…";

/// The overview over the scripted host, and the page's write runner, which nobody polls.
struct Overview {
    /// Made in the first frame, on the harness's context.
    services: Option<Services>,
    list: PullRequestListPage,
    navigator: Navigator,
    actions: ActionRunner,
    dir: ScratchDir,
}

fn overview(name: &str) -> Harness<'static, Overview> {
    let overview = Overview {
        services: None,
        list: PullRequestListPage::new(ListSource::Mine),
        navigator: Navigator::default(),
        actions: ActionRunner::default(),
        dir: ScratchDir::new(&format!("writes-overview-{name}")),
    };
    let mut harness =
        Harness::builder().with_size([1280.0, 720.0]).with_step_dt(1.0 / 60.0).build_ui_state(
            |ui, overview: &mut Overview| {
                let Overview { services, list, navigator, dir, .. } = overview;
                let services = services.get_or_insert_with(|| {
                    cv_base::theme::fonts::install(ui.ctx());
                    scripted_services(dir, ui.ctx().clone())
                });
                list.show(ui, services, navigator);
            },
            overview,
        );
    pump_until(&mut harness, "the listed row", row_is_live);
    harness
}

/// A merge whose answer `release` hands over when the test says so.
fn hold_a_merge(harness: &mut Harness<'_, Overview>) -> oneshot::Sender<Result<()>> {
    let (release, answer) = oneshot::channel::<Result<()>>();
    let Overview { services, actions, .. } = harness.state_mut();
    let services = services.as_ref().expect("made in the first frame");
    let write = (&key(), Effect::State(PullRequestState::Merged));
    let ticket =
        actions.run(services, "Pull request merged.", Refresh::PullRequest, write, async {
            answer.await.unwrap_or(Ok(()))
        });
    assert!(ticket.is_some(), "the merge starts");
    release
}

fn pump_until(
    harness: &mut Harness<'_, Overview>,
    what: &str,
    mut is_done: impl FnMut(&Harness<'_, Overview>) -> bool,
) {
    let started = Instant::now();
    loop {
        harness.step();
        if is_done(harness) {
            return;
        }
        assert!(started.elapsed() < TIMEOUT, "timed out waiting for {what}");
        cv_testkit::let_threads_run(std::time::Duration::from_millis(5));
    }
}

/// The row is listed and takes clicks.
fn row_is_live(harness: &Harness<'_, Overview>) -> bool {
    harness
        .query_by_role_and_label(Role::Link, ROW)
        .is_some_and(|row| !row.accesskit_node().is_disabled())
}

fn is_pending(harness: &Harness<'_, Overview>) -> bool {
    harness.query_by_label(PENDING).is_some()
}

#[test]
fn a_merged_row_shows_pending_then_leaves_the_open_list() {
    let mut harness = overview("merged");
    let release = hold_a_merge(&mut harness);
    pump_until(&mut harness, "the pending marker", is_pending);
    assert!(row_is_live(&harness), "a pending row still opens");
    // The marker sits on the meta line, clear of the title line, centred on its words' capitals
    // rather than on their line box, whose middle the meta line's centre is.
    let row = harness.get_by_role_and_label(Role::Link, ROW).rect();
    let marker = harness.get_by_label(PENDING).rect();
    assert!(
        marker.top() > row.center().y && marker.bottom() < row.bottom(),
        "{marker:?} in {row:?}"
    );
    let font = theme::fonts::semibold_font(theme::style::SMALL_SIZE);
    let painter = egui::Painter::new(
        harness.ctx.clone(),
        egui::LayerId::background(),
        egui::Rect::EVERYTHING,
    );
    let line = painter.layout_no_wrap("H".into(), font.clone(), egui::Color32::WHITE).size().y;
    let words_top = row.center().y + 10.5 - line / 2.0;
    let capitals = words_top + theme::fonts::optical_center(&painter, &font);
    assert!((marker.center().y - capitals).abs() < 0.01, "{marker:?} around {capitals}");
    harness.run_steps(10);
    assert!(is_pending(&harness), "pending for as long as the host holds the answer");

    release.send(Ok(())).expect("the merge waits");
    pump_until(&mut harness, "the row to start leaving", |harness| {
        harness.state().list.leaving_for_test() == 1
    });
    assert!(!is_pending(&harness), "the write landed");
    assert!(!row_is_live(&harness), "a leaving row takes no clicks");
    pump_until(&mut harness, "the row to be gone", |harness| {
        harness.state().list.leaving_for_test() == 0
    });
    assert!(harness.query_by_label(ROW).is_none(), "gone from the open list");
    let actions = &harness.state().actions;
    assert!(actions.is_busy(), "the page never took the result: the list recorded the merge");
}

#[test]
fn a_failed_write_leaves_the_row_as_it_was() {
    let mut harness = overview("failed");
    let release = hold_a_merge(&mut harness);
    pump_until(&mut harness, "the pending marker", is_pending);

    let refused = ProviderError::Unsupported("The platform refused the merge.".to_owned());
    release.send(Err(refused)).expect("the merge waits");
    pump_until(&mut harness, "the row back to normal", |harness| !is_pending(harness));
    assert!(row_is_live(&harness), "listed and clickable as before");
    harness.run_steps(30);
    assert_eq!(harness.state().list.leaving_for_test(), 0, "it never left");
    assert!(row_is_live(&harness));
}
