//! The log viewer over a downloaded log with Azure DevOps timestamps and ANSI colors (or another
//! log a test writes), for the copy, selection and scroll tests, and the wait for what a pass put
//! on the clipboard. No network.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cv_base::theme;
use cv_runtime::background::Background;
use cv_runtime::toasts::Toasts;
use cv_testkit::ScratchDir;
use egui::OutputCommand;
use egui_kittest::Harness;
use review_store::{LogFile, ThemePreference};

use super::super::download::LoadedLog;
use super::LogViewer;
use crate::job_tree::LogTarget;

const LOG: &str = "2026-10-18T09:00:00.1000000Z ##[section]Starting: Build\r\n\
2026-10-18T09:00:01.2000000Z \u{1b}[36m$ cargo test --workspace\u{1b}[0m\r\n\
2026-10-18T09:00:05.3000000Z    Compiling review-app v0.1.0\r\n\
2026-10-18T09:00:09.4000000Z test result: \u{1b}[32mok\u{1b}[0m. 162 passed; 0 failed\r\n\
2026-10-18T09:00:09.5000000Z ##[warning]2 tests ignored\r\n\
2026-10-18T09:00:10.6000000Z ##[error]\u{1b}[31merror[E0308]\u{1b}[0m: mismatched types\r\n\
2026-10-18T09:00:10.7000000Z ##[section]Finishing: Build\r\n";

/// [`LOG`] as "Copy log" copies it.
pub(super) const PLAIN: &str = "##[section]Starting: Build\n\
$ cargo test --workspace\n   Compiling review-app v0.1.0\n\
test result: ok. 162 passed; 0 failed\n\
##[warning]2 tests ignored\n\
##[error]error[E0308]: mismatched types\n\
##[section]Finishing: Build\n";

pub(super) struct State {
    pub viewer: LogViewer,
    target: LogTarget,
    /// The log shown; a test swaps it for another as a new download would.
    pub log: Arc<LoadedLog>,
    background: Background,
    toasts: Toasts,
    /// The log files written so far, which numbers the next one.
    logs: usize,
    /// Last, so `viewer` and `log`, which hold its files, drop first.
    scratch: ScratchDir,
}

impl State {
    /// Shows `text` in place of the log, as a new download would, its first error on line
    /// `first_error`.
    pub(super) fn swap_log(&mut self, text: &str, first_error: Option<usize>) {
        self.log = write_log(&self.scratch, self.logs, text, first_error);
        self.logs += 1;
    }
}

/// Numbers each harness's scratch folder: tests run in parallel, and `ScratchDir` empties a
/// folder of the same name.
static SCRATCH: AtomicUsize = AtomicUsize::new(0);

/// `text` as a downloaded log in the file `{number}.log` of `scratch`.
fn write_log(
    scratch: &ScratchDir,
    number: usize,
    text: &str,
    first_error: Option<usize>,
) -> Arc<LoadedLog> {
    let path = scratch.path().join(format!("{number}.log"));
    std::fs::write(&path, text).expect("write the log");
    Arc::new(LoadedLog { file: LogFile::open(&path).expect("index the log"), first_error })
}

/// The toolbar and lines of [`LOG`], in the light theme.
pub(super) fn harness() -> Harness<'static, State> {
    harness_of(LOG, Some(5))
}

/// The toolbar and lines of `text` as a downloaded log, its first error on line `first_error`,
/// in the light theme.
pub(super) fn harness_of(text: &str, first_error: Option<usize>) -> Harness<'static, State> {
    let scratch =
        ScratchDir::new(&format!("log-viewer-{}", SCRATCH.fetch_add(1, Ordering::Relaxed)));
    let log = write_log(&scratch, 0, text, first_error);
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
        logs: 1,
        scratch,
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
                let palette = theme::activate(ui.ctx(), ThemePreference::Light);
                ui.painter().rect_filled(ui.max_rect(), 0, palette.ground);
                let State { viewer, target, log, background, toasts, .. } = state;
                viewer.show_log(ui, target, log, background, toasts, 13.0);
            },
            state,
        )
}

/// Where [`keep_copies`] puts the text a pass copied.
const COPIED: &str = "test_harness::copied";

/// Keeps the text each pass puts on the clipboard: the harness hands over the output of its last
/// frame only, and one step runs a frame per queued event (`key_press_modifiers` queues four), so
/// a fast copy would land in a frame nobody looks at.
fn keep_copies(ctx: &egui::Context) {
    ctx.on_end_pass(
        "test_harness",
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

/// Steps until a pass copied text, and takes it. The text is built on the background runtime.
pub(super) fn wait_for_copy(harness: &mut Harness<'_, State>) -> String {
    cv_testkit::poll_for("the log to reach the clipboard", || {
        harness.step();
        harness.ctx.data_mut(|data| data.remove_temp::<String>(egui::Id::unique(COPIED)))
    })
}
