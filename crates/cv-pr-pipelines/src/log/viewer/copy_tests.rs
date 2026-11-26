//! "Copy log" on a log with Azure DevOps timestamps and ANSI colors, by click and by
//! Ctrl+Shift+C (⇧⌘C on macOS): the clipboard gets the plain text, the button confirms with "Copied" without
//! moving the toolbar. No network.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use cv_base::theme;
use cv_runtime::background::Background;
use cv_runtime::toasts::Toasts;
use egui::{Key, Modifiers, OutputCommand};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_store::{LogFile, ThemePreference};

use super::super::download::LoadedLog;
use super::{LogViewer, NEXT_MATCH_HINT};
use crate::job_tree::LogTarget;

pub(super) const LOG: &str = "2026-10-18T09:00:00.1000000Z ##[section]Starting: Build\r\n\
2026-10-18T09:00:01.2000000Z \u{1b}[36m$ cargo test --workspace\u{1b}[0m\r\n\
2026-10-18T09:00:05.3000000Z    Compiling review-app v0.1.0\r\n\
2026-10-18T09:00:09.4000000Z test result: \u{1b}[32mok\u{1b}[0m. 162 passed; 0 failed\r\n\
2026-10-18T09:00:09.5000000Z ##[warning]2 tests ignored\r\n\
2026-10-18T09:00:10.6000000Z ##[error]\u{1b}[31merror[E0308]\u{1b}[0m: mismatched types\r\n\
2026-10-18T09:00:10.7000000Z ##[section]Finishing: Build\r\n";

pub(super) const PLAIN: &str = "##[section]Starting: Build\n\
$ cargo test --workspace\n   Compiling review-app v0.1.0\n\
test result: ok. 162 passed; 0 failed\n\
##[warning]2 tests ignored\n\
##[error]error[E0308]: mismatched types\n\
##[section]Finishing: Build\n";

pub(super) struct State {
    pub viewer: LogViewer,
    target: LogTarget,
    log: Arc<LoadedLog>,
    background: Background,
    toasts: Toasts,
    theme: ThemePreference,
}

/// Numbers each harness's log file: tests run in parallel, and one that rewrites a file another
/// is reading hands it a short log.
static LOG_FILES: AtomicUsize = AtomicUsize::new(0);

pub(super) fn harness(theme: ThemePreference) -> Harness<'static, State> {
    let number = LOG_FILES.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("review-log-copy-{}-{number}.log", std::process::id()));
    std::fs::write(&path, LOG).expect("write the log");
    let log = Arc::new(LoadedLog {
        file: LogFile::open(&path).expect("index the log"),
        first_error: Some(5),
    });
    let state = State {
        viewer: LogViewer::default(),
        target: LogTarget {
            job_id: "job-build".to_owned(),
            log_id: Some("1:12".to_owned()),
            title: "Build and test".to_owned(),
            web_url: None,
        },
        log,
        background: Background::new(egui::Context::default()),
        toasts: Toasts::default(),
        theme,
    };
    Harness::builder()
        .with_size([1100.0, 360.0])
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            // Motion on, whatever the machine says, so every host runs the same frames.
            theme::motion::set_reduced(ctx, false);
            keep_copies(ctx);
        })
        .build_ui_state(
            |ui, state: &mut State| {
                let palette = theme::activate(ui.ctx(), state.theme);
                ui.painter().rect_filled(ui.max_rect(), 0, palette.ground);
                let State { viewer, target, log, background, toasts, .. } = state;
                viewer.show_log(ui, target, log, background, toasts, 13.0);
            },
            state,
        )
}

/// Where [`keep_copies`] puts the text a pass copied.
const COPIED: &str = "copy_tests::copied";

/// Keeps the text each pass puts on the clipboard: the harness hands over the output of its last
/// frame only, and one step runs a frame per queued event (`key_press_modifiers` queues four), so
/// a fast copy would land in a frame nobody looks at.
fn keep_copies(ctx: &egui::Context) {
    ctx.on_end_pass(
        "copy_tests",
        Arc::new(|ui: &mut egui::Ui| {
            let ctx = ui.ctx().clone();
            let copied = ctx.output(|output| {
                output.commands.iter().rev().find_map(|command| match command {
                    OutputCommand::CopyText(text) => Some(text.clone()),
                    _ => None,
                })
            });
            if let Some(text) = copied {
                ctx.data_mut(|data| data.insert_temp(egui::Id::unique(COPIED), text));
            }
        }),
    );
}

/// Steps until a pass copied text, and takes it.
#[expect(
    clippy::disallowed_methods,
    reason = "the text is built on the background runtime; the test thread waits for it between frames"
)]
pub(super) fn wait_for_copy(harness: &mut Harness<'_, State>) -> String {
    let started = Instant::now();
    loop {
        harness.step();
        if let Some(text) =
            harness.ctx.data_mut(|data| data.remove_temp::<String>(egui::Id::unique(COPIED)))
        {
            return text;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "the log never reached the clipboard");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn rect_of(harness: &Harness<'_, State>, label: &str) -> egui::Rect {
    harness.get_by_label(label).rect()
}

#[test]
fn copy_log_puts_plain_text_on_the_clipboard() {
    let mut harness = harness(ThemePreference::Light);
    harness.run();
    let search = rect_of(&harness, NEXT_MATCH_HINT);

    harness.get_by_label("Copy log").click();
    assert_eq!(
        wait_for_copy(&mut harness),
        PLAIN,
        "timestamps and ANSI escapes are stripped, nothing else"
    );
    harness.step();
    assert!(harness.query_by_label("Copied").is_some(), "the button confirms");
    assert_eq!(
        rect_of(&harness, NEXT_MATCH_HINT),
        search,
        "the confirmation does not move the toolbar"
    );
}

#[test]
fn ctrl_shift_c_copies_the_log() {
    let mut harness = harness(ThemePreference::Light);
    harness.run();
    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::C);
    assert_eq!(wait_for_copy(&mut harness), PLAIN);
}
