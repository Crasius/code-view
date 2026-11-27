//! The Files tab while a pull request's changes are prepared, with its live log: the tree
//! skeleton beside the header, the step segments and the log, mid-fetch and after a failure.
//! Checks that nothing moves while lines arrive, that git's progress overwrites its line, that
//! no credential reaches the screen, and that the failure keeps its way out. No network, no
//! tokens.

use cv_testkit::Clicks;
use egui::{Rect, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::ProviderKind;
use review_git::{GitEvent, GitProgress};
use review_store::ThemePreference;

use cv_base::theme;
use cv_pr_session::prepare_progress::{self, FailureAction, SharedProgress, Step};
use cv_pr_tree::side_panel;

const TREE_WIDTH: f32 = 300.0;
const FAILURE: &str = "git failed: fatal: unable to access 'https://x-access-token:ghs_leaked0123456789@github.com/acme/shop.git/': The requested URL returned error: 403";

struct Scene {
    progress: SharedProgress,
    failure: Option<&'static str>,
    theme: ThemePreference,
    clicked: Option<FailureAction>,
}

fn harness(theme: ThemePreference) -> Harness<'static, Scene> {
    let builder = Harness::builder().with_size([1280.0, 760.0]).with_context_setup(|ctx| {
        theme::fonts::install(ctx);
        theme::motion::set_reduced(ctx, false);
    });
    let scene = Scene { progress: SharedProgress::default(), failure: None, theme, clicked: None };
    builder.build_ui_state(
        |ui, scene: &mut Scene| {
            let palette = theme::activate(ui.ctx(), scene.theme);
            ui.painter().rect_filled(ui.max_rect(), 0, palette.ground);
            side_panel::show_skeleton(ui, TREE_WIDTH, scene.failure.is_some());
            let open = (
                cv_widgets::provider_icon::glyph(ProviderKind::GitHub),
                "Open pull request on GitHub",
            );
            let failure = scene
                .failure
                .map(|message| prepare_progress::Failure { message, open: Some(open) });
            if let Some(action) = prepare_progress::show(ui, &scene.progress, failure.as_ref()) {
                scene.clicked = Some(action);
            }
        },
        scene,
    )
}

fn receiving(percent: u8) -> GitEvent {
    let done = 64_380 * u64::from(percent) / 100;
    GitEvent::Progress(GitProgress {
        phase: "Receiving objects".into(),
        percent: Some(percent),
        done: Some(done),
        total: Some(64_380),
        transfer: Some(format!("{:.2} MiB | 9.40 MiB/s", f64::from(percent) * 0.6)),
    })
}

/// The steps up to the middle of the fetch, as the session's background task reports them.
fn up_to_fetch(progress: &SharedProgress) {
    progress.log("Head 9f3c2a1 into target 4be81d0");
    progress.enter(Step::ResolvingRemote);
    progress.log("Remote https://x-access-token:ghs_leaked0123456789@github.com/acme/shop.git");
    progress.enter(Step::OpeningMirror);
    progress
        .log(r"Mirror C:\Users\reviewer\AppData\Local\CodeView\mirrors\github.com_acme_shop.git");
    progress.enter(Step::Fetching);
    progress.report_git(GitEvent::Command(
        "git fetch --progress --no-tags --no-write-fetch-head --filter=blob:none origin +refs/pull/8421/head:refs/review/pull/8421/head +refs/heads/trunk:refs/review/heads/trunk"
            .into(),
    ));
    progress.report_git(GitEvent::Progress(GitProgress {
        phase: "Enumerating objects".into(),
        done: Some(64_380),
        ..GitProgress::default()
    }));
    progress.report_git(GitEvent::Progress(GitProgress {
        phase: "Counting objects".into(),
        percent: Some(100),
        done: Some(18_204),
        total: Some(18_204),
        transfer: None,
    }));
    progress.report_git(GitEvent::Progress(GitProgress {
        phase: "Compressing objects".into(),
        percent: Some(100),
        done: Some(9_118),
        total: Some(9_118),
        transfer: None,
    }));
}

/// The log's header ("Fetching commits and trees, step 4 of 6 · 1.2 s"), not the tree skeleton.
fn header<'h>(harness: &'h Harness<'_, Scene>) -> egui_kittest::Node<'h> {
    harness
        .query_all_by_role(Role::ProgressIndicator)
        .find(|node| node.accesskit_node().label().is_some_and(|label| label.contains(" of 6 ")))
        .expect("the loading header")
}

fn header_rect(harness: &Harness<'_, Scene>) -> Rect {
    header(harness).rect()
}

fn header_label(harness: &Harness<'_, Scene>) -> String {
    header(harness).accesskit_node().label().unwrap_or_default()
}

fn log_rect(harness: &Harness<'_, Scene>) -> Rect {
    harness.get_by_role(Role::Log).rect()
}

#[test]
fn the_loading_log_stays_put_overwrites_progress_and_keeps_retry_on_failure() {
    let mut harness = harness(ThemePreference::Light);
    harness.run_steps(3);
    let (header, log) = (header_rect(&harness), log_rect(&harness));
    let label = header_label(&harness);
    assert!(label.starts_with("Loading pull request details, step 1 of 6"), "{label}");

    up_to_fetch(&harness.state().progress);
    for percent in [8, 21, 37, 64] {
        harness.state().progress.report_git(receiving(percent));
        harness.run_steps(2);
        assert_eq!(header_rect(&harness), header, "the header does not move as steps advance");
        assert_eq!(log_rect(&harness), log, "the log keeps its place and height as lines arrive");
    }
    let label = header_label(&harness);
    assert!(label.starts_with("Fetching commits and trees, step 4 of 6"), "{label}");
    let text = copied_log(&mut harness);
    assert_eq!(
        text.matches("Receiving objects").count(),
        1,
        "progress overwrites its line:\n{text}"
    );
    assert!(text.contains("Receiving objects: 64% (41,203/64,380)"), "{text}");
    assert!(text.contains("Remote https://github.com/acme/shop.git"), "{text}");
    assert!(
        !text.contains("ghs_") && !text.contains("x-access-token"),
        "credentials stay out of the log:\n{text}"
    );

    harness.state().progress.fail(FAILURE);
    harness.state_mut().failure = Some(FAILURE);
    harness.run_steps(3);
    harness.try_run().expect("a failed load settles: no spinner, sweep or elapsed-time repaints");
    assert_eq!(header_rect(&harness), header, "the failure keeps the header in place");
    let label = header_label(&harness);
    assert!(label.starts_with("Fetching commits and trees failed, step 4 of 6"), "{label}");
    assert!(
        harness.query_by_label_contains("ghs_").is_none(),
        "the failure message is redacted on screen"
    );
    harness.click_at(harness.get_by_label("Retry").rect());
    harness.run_steps(2);
    assert!(matches!(harness.state().clicked, Some(FailureAction::Retry)));
    harness.click_at(harness.get_by_label("Open pull request on GitHub").rect());
    harness.run_steps(2);
    assert!(matches!(harness.state().clicked, Some(FailureAction::Open)));
    let text = copied_log(&mut harness);
    assert!(text.contains("Fetching commits and trees failed after"), "{text}");
    assert!(
        text.contains(
            "error: git failed: fatal: unable to access 'https://github.com/acme/shop.git/'"
        ),
        "{text}"
    );
}

/// Clicks "Copy log" and returns what went to the clipboard.
fn copied_log(harness: &mut Harness<'_, Scene>) -> String {
    harness.click_at(harness.get_by_label("Copy log").rect());
    harness.step();
    let copied =
        harness.output().platform_output.commands.iter().find_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        });
    harness.run_steps(1);
    assert!(harness.query_by_label("Copied").is_some(), "the button confirms the copy");
    // "Copied" shows for 1.5 seconds (harness frames are 1/60 s); then "Copy log" is back.
    harness.run_steps(130);
    copied.expect("Copy log puts the log on the clipboard")
}
