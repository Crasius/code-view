//! Settings → Links → Browser extension: registers Code View as the extension's native messaging
//! host for the current user ([`cv_shell::browser::registration`]), shows whether it is registered
//! for this copy of Code View, and leads to the unpacked extension for "Load unpacked".
//!
//! Reading and writing the registration touches the registry and the disk, so each runs on the
//! background runtime; until the first answer the status slot shows a skeleton of its size. A
//! failure shows under the status with the buttons that retry it.

#[cfg(test)]
mod walkthrough;

use std::path::PathBuf;
use std::sync::Arc;

use cv_base::theme::{self, icons, style};
use cv_runtime::background::{Task, TaskPoll};
use cv_services::Services;
use cv_shell::browser::registration::{Registry, Status};
use cv_shell::browser::{self, extension_files};
use cv_widgets::{buttons, row, skeleton};
use egui::{Align, RichText, Sense, Ui, vec2};

/// The row's label in the Links section.
pub(crate) const LABEL: &str = "Browser extension";
pub(crate) const DESCRIPTION: &str = "Chrome and Edge open pull request links of your connections here instead of in a tab. \
     Registering lets the Code View extension start this copy of Code View, for your user only.";
const INSTRUCTION: &str = "Then in chrome://extensions or edge://extensions, turn on Developer mode, choose Load unpacked and pick the extension folder. After an update, choose Reload on the Code View extension there.";
const STATUS_WIDTH: f32 = 220.0;
/// Why a step that never answered failed.
const STOPPED: &str = "The registration step stopped unexpectedly. Try again.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Work {
    Read,
    Register,
    Unregister,
    OpenFolder,
}

/// What a background step found: the status after it, what failed, the folder to show.
struct Done {
    status: Option<Status>,
    failure: Option<String>,
    folder: Option<PathBuf>,
}

impl Done {
    /// A step that failed before it could read the status.
    fn failed(message: impl Into<String>) -> Self {
        Self { status: None, failure: Some(message.into()), folder: None }
    }
}

#[derive(Default)]
pub(crate) struct BrowserExtension {
    registry: Option<Result<Arc<Registry>, String>>,
    status: Option<Status>,
    failure: Option<String>,
    running: Option<(Work, Task<Done>)>,
}

impl BrowserExtension {
    /// A row that reads and writes `registry` (tests: a scratch folder and keys in memory).
    #[cfg(test)]
    pub fn with_registry(registry: Registry) -> Self {
        Self { registry: Some(Ok(Arc::new(registry))), ..Self::default() }
    }

    pub(crate) fn control(&mut self, ui: &mut Ui, services: &mut Services) {
        self.finish(ui.ctx(), services);
        let registry = self
            .registry
            .get_or_insert_with(|| {
                registry_for(services).map(Arc::new).map_err(|error| error.to_string())
            })
            .clone();
        let registry = match registry {
            Ok(registry) => registry,
            Err(message) => {
                self.failure =
                    Some(format!("Code View cannot reach the browsers' registration: {message}"));
                failure_line(ui, self.failure.as_deref());
                return;
            }
        };
        if self.status.is_none() && self.failure.is_none() && self.running.is_none() {
            self.start(Work::Read, &registry, services);
        }
        // Each row in a horizontal of its own: a right-to-left layout straight in the section's
        // column takes all the height left below it and centers its row there.
        let asked = ui
            .horizontal(|ui| {
                row::with_trailing(
                    ui,
                    Align::Center,
                    |ui| self.buttons(ui),
                    |ui| self.status_line(ui),
                )
            })
            .inner
            .0;
        if let Some(work) = asked {
            self.start(work, &registry, services);
        }
        failure_line(ui, self.failure.as_deref());
        ui.add_space(style::SPACE_S);
        let palette = theme::palette(ui.ctx());
        let open = ui
            .horizontal(|ui| {
                row::with_trailing(
                    ui,
                    Align::Center,
                    |ui| {
                        let is_busy = self.is_running(Work::OpenFolder);
                        let open = buttons::subtle(ui, icons::FOLDER_OPEN, "Open extension folder")
                            .clicked();
                        open && !is_busy
                    },
                    |ui| {
                        let text = RichText::new(INSTRUCTION)
                            .size(style::SMALL_SIZE)
                            .color(palette.text_secondary);
                        ui.add(egui::Label::new(text).wrap());
                    },
                )
            })
            .inner
            .0;
        if open {
            self.start(Work::OpenFolder, &registry, services);
        }
    }

    fn is_running(&self, work: Work) -> bool {
        self.running.as_ref().is_some_and(|(running, _)| *running == work)
    }

    /// The buttons for the status, right to left; the step a click asks for.
    fn buttons(&self, ui: &mut Ui) -> Option<Work> {
        let is_idle = self.running.is_none();
        let register = |ui: &mut Ui, icon, text| {
            buttons::primary_busy(ui, icon, text, self.is_running(Work::Register)).clicked()
        };
        let unregister = |ui: &mut Ui| {
            buttons::secondary_busy(
                ui,
                icons::X_CIRCLE,
                "Unregister",
                self.is_running(Work::Unregister),
            )
            .clicked()
        };
        let asked = match self.status {
            Some(Status::NotRegistered) => {
                register(ui, icons::PLUGS, "Register").then_some(Work::Register)
            }
            Some(Status::Outdated) => {
                let update =
                    register(ui, icons::ARROW_CLOCKWISE, "Update").then_some(Work::Register);
                update.or_else(|| unregister(ui).then_some(Work::Unregister))
            }
            Some(Status::Registered) => unregister(ui).then_some(Work::Unregister),
            None if self.failure.is_some() => {
                let again =
                    buttons::secondary_busy(ui, icons::ARROW_CLOCKWISE, "Check again", false);
                again.clicked().then_some(Work::Read)
            }
            None => None,
        };
        asked.filter(|_| is_idle)
    }

    /// The status as an icon and a sentence; a skeleton of its size while it is read.
    fn status_line(&self, ui: &mut Ui) {
        let palette = theme::palette(ui.ctx());
        let (glyph, color, text) = match self.status {
            Some(Status::Registered) => {
                (icons::CHECK_CIRCLE, palette.success, "Registered for Chrome and Edge")
            }
            Some(Status::NotRegistered) => {
                (icons::MINUS_CIRCLE, palette.text_secondary, "Not registered")
            }
            Some(Status::Outdated) => {
                (icons::WARNING_CIRCLE, palette.warning, "Registered for another copy of Code View")
            }
            None if self.failure.is_some() => {
                (icons::WARNING_CIRCLE, palette.warning, "Registration unknown")
            }
            None => {
                let (rect, response) = ui.allocate_exact_size(
                    vec2(STATUS_WIDTH, style::BODY_SIZE + 4.0),
                    Sense::hover(),
                );
                skeleton::paint_bar(ui, rect);
                skeleton::announce(&response, "Checking the registration");
                return;
            }
        };
        let (slot, _) = ui.allocate_exact_size(vec2(icons::SIZE, icons::SIZE), Sense::hover());
        ui.painter().text(
            slot.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            icons::rest_font(&palette, icons::SIZE),
            color,
        );
        ui.add(egui::Label::new(RichText::new(text).color(palette.text)).truncate());
    }

    fn start(&mut self, work: Work, registry: &Arc<Registry>, services: &Services) {
        let registry = registry.clone();
        let folder = browser::extension_dir(&services.paths);
        let task = services.background.spawn(async move {
            let step = tokio::task::spawn_blocking(move || run(work, &registry, folder));
            step.await.unwrap_or_else(|_| Done::failed(STOPPED))
        });
        self.running = Some((work, task));
    }

    /// Takes a finished step's result: the status, its failure, the folder to show.
    fn finish(&mut self, ctx: &egui::Context, services: &mut Services) {
        let Some((_, task)) = self.running.as_mut() else {
            return;
        };
        let done = match task.poll() {
            TaskPoll::Pending => return,
            TaskPoll::Done(done) => done,
            TaskPoll::Lost => Done::failed(STOPPED),
        };
        self.running = None;
        self.status = done.status.or(self.status);
        self.failure = done.failure;
        if let Some(folder) = done.folder {
            services.toasts.open_folder(ctx, &folder);
        }
    }
}

/// The registration this build reads and writes: the user's on this system, or in test builds
/// (this crate's tests, and the tests of crates that turn on its `test-support` feature) keys in
/// memory and the test's own folders, so no test reaches the real registry.
#[cfg_attr(
    any(test, feature = "test-support"),
    expect(clippy::unnecessary_wraps, reason = "the registry of a real build can fail")
)]
fn registry_for(services: &Services) -> std::io::Result<Registry> {
    #[cfg(any(test, feature = "test-support"))]
    {
        use cv_shell::browser::registration::MemoryKeys;
        let manifest = browser::host_manifest(&services.paths);
        Ok(Registry::Keys { manifest, keys: Arc::new(MemoryKeys::default()) })
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        Registry::system(&services.paths)
    }
}

/// One step on a blocking thread of the background runtime.
fn run(work: Work, registry: &Registry, folder: PathBuf) -> Done {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            return Done::failed(format!("Code View cannot find its own executable: {error}"));
        }
    };
    let outcome = match work {
        Work::Read => Ok(()),
        Work::Register => extension_files::unpack(&folder)
            .map_err(|error| {
                format!("Could not unpack the extension into {}: {error}", folder.display())
            })
            .and_then(|()| {
                registry.register(&exe).map_err(|error| format!("Registering failed: {error}"))
            }),
        Work::Unregister => {
            registry.unregister().map_err(|error| format!("Unregistering failed: {error}"))
        }
        Work::OpenFolder => extension_files::unpack(&folder).map_err(|error| {
            format!("Could not unpack the extension into {}: {error}", folder.display())
        }),
    };
    let status =
        registry.status(&exe).map_err(|error| format!("Could not read the registration: {error}"));
    let failure = outcome.err().or_else(|| status.as_ref().err().cloned());
    let shows_folder = work == Work::OpenFolder && failure.is_none();
    Done { status: status.ok(), failure, folder: shows_folder.then_some(folder) }
}

/// What failed, with the path to copy when it names one; the buttons above retry.
fn failure_line(ui: &mut Ui, failure: Option<&str>) {
    let Some(failure) = failure else {
        return;
    };
    let palette = theme::palette(ui.ctx());
    ui.add_space(style::SPACE_XS);
    ui.add(
        egui::Label::new(RichText::new(failure).size(style::SMALL_SIZE).color(palette.warning))
            .wrap(),
    );
}
