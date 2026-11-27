//! Copying and saving a downloaded log, each off the UI thread: "Copy log" with its short
//! "Copied" confirmation, the selected lines for Ctrl+C, and "Download log". A copy or download
//! that failed offers Retry, which runs it once more for the same log; a download that could not
//! be written also offers the folder it was meant for.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cv_base::platform::{self, shortcut};
use cv_base::theme::icons;
use cv_runtime::background::Background;
use cv_runtime::load::Load;
use cv_runtime::toasts::{self, ToastAction, Toasts, WaysOut};
use cv_widgets::buttons;
use cv_widgets::text_selection::Selection;
use egui::{Align, Layout, Ui, vec2};
use review_core::ProviderError;

use super::super::download::LoadedLog;
use super::raw_log;

/// How long "Copied" replaces "Copy log".
const COPIED_SECONDS: f64 = 1.5;
/// The copy button's icon and label: idle, while the text is built, after it reached the clipboard.
const COPY_IDLE: (&str, &str) = (icons::COPY, "Copy log");
const COPY_BUSY: (&str, &str) = (icons::COPY, "Copying…");
const COPY_DONE: (&str, &str) = (icons::CHECK, "Copied");
/// Every face of the copy button, for the one width its slot keeps.
const COPY_STATES: [(&str, &str); 3] = [COPY_IDLE, COPY_BUSY, COPY_DONE];
const COPY_HINT: &str = concat!(
    "Copy the whole log as plain text, without colors (",
    shortcut!(command shift "C"),
    ")"
);

/// Where a failure toast's Retry asks for the failed copy or download once more.
const RETRY_ID: &str = "log_exports.retry";

/// A copy or download of a log that failed, for Retry.
enum Export {
    Copy,
    Selection(Selection),
    Download { title: String },
}

/// How "Download log" ended.
enum Saved {
    /// The dialog was cancelled.
    Cancelled,
    To(PathBuf),
    /// The file could not be written into `folder`.
    Failed {
        message: String,
        folder: Option<PathBuf>,
    },
}

/// The copies and the download of the log on screen; each is built on request, never per frame.
#[derive(Default)]
pub(crate) struct LogExports {
    /// The plain text being built for "Copy log".
    copy: Load<String>,
    /// When the copied text reached the clipboard, for the short "Copied" confirmation.
    copied_at: Option<f64>,
    download: Load<Saved>,
    /// The title of the log being downloaded, for Retry.
    download_title: Option<String>,
    /// The selected text being read for Ctrl+C, and the selection.
    copy_selection: Load<String>,
    selection: Option<Selection>,
    /// The export that failed last and the log file it was for, until Retry runs it.
    failed: Option<(Export, PathBuf)>,
}

impl LogExports {
    /// Forgets the copies of the log shown before; a download in progress still reports where
    /// it saved the file.
    pub(crate) fn forget_copies(&mut self) {
        self.copy = Load::Idle;
        self.copied_at = None;
        self.copy_selection = Load::Idle;
    }

    /// Runs the export a failure toast's Retry asked for, then hands what finished to the
    /// clipboard, or says where the download went or what failed.
    pub(crate) fn finish(
        &mut self,
        ui: &Ui,
        toasts: &mut Toasts,
        log: &Arc<LoadedLog>,
        background: &Background,
    ) {
        self.retry_requested(ui, log, background);
        match self.copy.take_finished() {
            Some(Ok(text)) => {
                ui.ctx().copy_text(text);
                self.copied_at = Some(ui.input(|input| input.time));
            }
            Some(Err(message)) => self.failed(toasts, log, Export::Copy, message, None),
            None => {}
        }
        match self.copy_selection.take_finished() {
            Some(Ok(text)) => ui.ctx().copy_text(text),
            Some(Err(message)) => {
                if let Some(selection) = self.selection {
                    self.failed(toasts, log, Export::Selection(selection), message, None);
                }
            }
            None => {}
        }
        if let Some(result) = self.download.take_finished() {
            let saved = result.unwrap_or_else(|message| Saved::Failed { message, folder: None });
            match saved {
                Saved::To(path) => toasts.success(format!("Saved the log to {}", path.display())),
                Saved::Failed { message, folder } => {
                    let title = self.download_title.take().unwrap_or_default();
                    self.failed(toasts, log, Export::Download { title }, message, folder);
                }
                Saved::Cancelled => {}
            }
        }
    }

    /// Toasts why `export` of `log` failed, with Retry and the folder a download was meant for.
    fn failed(
        &mut self,
        toasts: &mut Toasts,
        log: &LoadedLog,
        export: Export,
        message: String,
        folder: Option<PathBuf>,
    ) {
        self.failed = Some((export, log.file.path().to_owned()));
        let retry = ToastAction::retry(egui::Id::unique(RETRY_ID));
        toasts.error(message, WaysOut::new(retry).and_some(folder.map(ToastAction::open_folder)));
    }

    /// Starts the failed export once more when its toast's Retry was clicked, if the same log is
    /// still on screen.
    fn retry_requested(&mut self, ui: &Ui, log: &Arc<LoadedLog>, background: &Background) {
        if !toasts::take_retry(ui.ctx(), egui::Id::unique(RETRY_ID)) {
            return;
        }
        let Some((export, path)) = self.failed.take() else {
            return;
        };
        if path != log.file.path() {
            return;
        }
        match export {
            Export::Copy => self.start_copy(log, background),
            Export::Selection(selection) => self.start_copy_selection(selection, log, background),
            Export::Download { title } => self.start_download(&title, log, background),
        }
    }

    pub(crate) fn is_copying(&self) -> bool {
        self.copy.is_loading()
    }

    /// Builds the log's plain text, without its colors, for the clipboard.
    pub(crate) fn start_copy(&mut self, log: &LoadedLog, background: &Background) {
        if self.copy.is_loading() {
            return;
        }
        let path = log.file.path().to_owned();
        self.copied_at = None;
        self.copy.start(background, async move {
            tokio::task::spawn_blocking(move || raw_log::plain_text(&path))
                .await
                .map_err(|error| ProviderError::Local(format!("copying stopped: {error}")))?
                .map_err(|error| {
                    ProviderError::Local(format!("Could not read the log to copy it: {error}"))
                })
        });
    }

    /// Reads the selected lines from the file, so a selection of 100k lines does not stall the
    /// frame.
    pub(crate) fn start_copy_selection(
        &mut self,
        selection: Selection,
        log: &Arc<LoadedLog>,
        background: &Background,
    ) {
        if self.copy_selection.is_loading() {
            return;
        }
        self.selection = Some(selection);
        let log = log.clone();
        self.copy_selection.start(background, async move {
            tokio::task::spawn_blocking(move || raw_log::selected_text(&log.file, &selection))
                .await
                .map_err(|error| ProviderError::Local(format!("copying stopped: {error}")))?
                .map_err(|error| {
                    ProviderError::Local(format!(
                        "Could not read the log to copy the selection: {error}"
                    ))
                })
        });
    }

    /// Asks where to save the log of `title` and copies the file there, as the platform sent it.
    pub(crate) fn start_download(&mut self, title: &str, log: &LoadedLog, background: &Background) {
        if self.download.is_loading() {
            return;
        }
        let source = log.file.path().to_owned();
        self.download_title = Some(title.to_owned());
        let file_name = raw_log::file_name(title);
        // Taken on the UI thread, where the clicked window is the active one.
        let owner = platform::dialog_owner();
        self.download.start(background, async move {
            tokio::task::spawn_blocking(move || {
                let request = platform::SaveRequest {
                    title: "Download log",
                    file_name: &file_name,
                    kind: "Log files",
                    extension: "log",
                };
                let Some(destination) = platform::ask_save_path(owner, &request) else {
                    return Ok(Saved::Cancelled);
                };
                Ok(match raw_log::save(&source, &destination) {
                    Ok(()) => Saved::To(destination),
                    Err(error) => Saved::Failed {
                        message: format!(
                            "Could not save the log to {}: {error}",
                            destination.display()
                        ),
                        folder: destination.parent().map(Path::to_path_buf),
                    },
                })
            })
            .await
            .map_err(|error| ProviderError::Local(format!("saving stopped: {error}")))?
        });
    }

    /// The copy button's icon and label now: "Copy log", "Copying…", or "Copied" for a moment.
    pub(crate) fn copy_face(&self, ui: &Ui) -> (&'static str, &'static str) {
        let now = ui.input(|input| input.time);
        let shown_for =
            self.copied_at.map(|at| now - at).filter(|elapsed| *elapsed < COPIED_SECONDS);
        if let Some(elapsed) = shown_for {
            ui.ctx().request_repaint_after(Duration::from_secs_f64(COPIED_SECONDS - elapsed));
        }
        match (self.copy.is_loading(), shown_for.is_some()) {
            (true, _) => COPY_BUSY,
            (false, true) => COPY_DONE,
            (false, false) => COPY_IDLE,
        }
    }

    /// "Copy log", turning into "Copied" for a moment; the slot keeps one width in every state,
    /// so nothing in the toolbar moves.
    pub(crate) fn copy_button(&self, ui: &mut Ui) -> bool {
        let (icon, text) = self.copy_face(ui);
        let width = COPY_STATES
            .iter()
            .map(|(_, label)| buttons::subtle_width(ui, label))
            .fold(0.0, f32::max);
        let slot = vec2(width, ui.spacing().interact_size.y);
        ui.allocate_ui_with_layout(slot, Layout::left_to_right(Align::Center), |ui| {
            ui.add_enabled_ui(!self.copy.is_loading(), |ui| {
                buttons::subtle(ui, icon, text).on_hover_text(COPY_HINT)
            })
            .inner
        })
        .inner
        .clicked()
    }
}
