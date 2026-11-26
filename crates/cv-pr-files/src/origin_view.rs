//! "Show original version": the file as it was when an outdated comment was written, with the
//! comment at its original line. The diff comes from the local mirror (a commit no ref reaches any
//! more is fetched by id, once, on demand) and shows in the regular diff view; a banner leads back.
//! Everything here is dropped when the user goes back, so the buffers live only while shown.

use std::sync::Arc;

use egui::{Label, RichText, Sense, Ui, vec2};
use review_core::{ProviderError, ProviderKind, ReviewThread, ThreadOrigin};
use review_git::{ChangedFile, DiffOptions, FileDiff, GitMirror};
use review_store::DiffLayout;

use cv_base::theme::{self, icons, style};
use cv_code::contents::ContentsLoader;
use cv_pr_diff::diff_skeleton;
use cv_pr_diff::diff_view::{DiffEvent, DiffInputs, DiffView};
use cv_pr_diff::find::FindInputs;
use cv_pr_diff::notice::{self, Action};
use cv_pr_threads::{ThreadEvent, ThreadsUi};
use cv_runtime::load::Load;
use cv_services::Services;
use cv_shell::focus_mode;
use cv_widgets::{buttons, glyph, provider_icon, skeleton, time_text};

const BANNER_HEIGHT: f32 = 36.0;
pub const BACK_LABEL: &str = "Back to latest";

/// What the mirror gave for the origin.
enum Version {
    /// The file between the origin's two commits, and when its head was written.
    Diff { file: ChangedFile, diff: Arc<FileDiff>, authored_at: i64 },
    /// The commits are gone from the remote (a force push and garbage collection): why.
    Missing(String),
}

/// What the banner says, for focus mode's strip to say it in its place.
pub struct Face {
    /// "Showing the version from 1a2b3c4 · 3 days ago", or why another view shows.
    pub text: String,
    /// The file the comment was written on.
    pub path: String,
    /// Why the version is missing while the comment's own lines show: Retry fetches it again.
    pub missing: Option<String>,
}

/// What the banner and the view asked for.
pub enum OriginEvent {
    Back,
    Thread(ThreadEvent),
}

/// What the origin view takes from the Files tab and the settings each frame.
pub struct OriginInputs<'a> {
    /// The pull request's mirror and target commit; `None` while the mirror is being prepared.
    pub mirror: Option<(Arc<GitMirror>, String)>,
    pub layout: DiffLayout,
    pub options: DiffOptions,
    pub font_size: f32,
    pub wrap: bool,
    pub split: &'a mut f32,
    pub platform: ProviderKind,
    /// The comment on the platform's page.
    pub web_url: Option<String>,
}

/// The version an outdated thread was written on, while it is shown.
pub struct OriginView {
    /// The thread as it sits in that version: at its original lines, not outdated there.
    thread: ReviewThread,
    origin: ThreadOrigin,
    version: Load<Version>,
    started: f64,
    contents: ContentsLoader,
    view: DiffView,
    /// The comment's own hunk as a diff, for when the commits are gone.
    hunk: Option<Arc<FileDiff>>,
}

enum BannerAction {
    Back,
    Retry,
}

/// What the area below the banner shows this frame.
enum Body {
    Diff(Shown),
    Unavailable { title: &'static str, detail: String },
    Loading,
}

/// A diff below the banner: the version's, or the comment's own hunk without its contents.
struct Shown {
    diff: Arc<FileDiff>,
    path: String,
    old_path: Option<String>,
    whole_file: Option<review_core::DiffSide>,
    has_contents: bool,
}

impl OriginView {
    /// `None` for a thread without an origin.
    pub fn new(thread: &ReviewThread) -> Option<Self> {
        let origin = thread.origin.clone()?;
        let mut shown = thread.clone();
        shown.anchor = Some(origin.anchor.clone());
        shown.is_outdated = false;
        let hunk = origin
            .diff_hunk
            .as_deref()
            .map(|hunk| Arc::new(FileDiff::parse_unified(format!("{hunk}\n").as_bytes())))
            .filter(|diff| !diff.is_empty());
        let mut view = DiffView::default();
        view.reveal_thread(&thread.id);
        Some(Self {
            thread: shown,
            origin,
            version: Load::Idle,
            started: 0.0,
            contents: ContentsLoader::default(),
            view,
            hunk,
        })
    }

    pub fn thread_id(&self) -> &str {
        &self.thread.id
    }

    /// Loads the version again (Retry).
    pub fn retry(&mut self) {
        self.version = Load::Idle;
    }

    /// What the banner says right now.
    pub fn face(&self) -> Face {
        let short = short_sha(&self.origin.head_sha);
        let missing = match self.version.ready() {
            Some(Version::Missing(reason)) if self.hunk.is_some() => Some(reason.clone()),
            _ => None,
        };
        let text = match self.version.ready() {
            Some(Version::Diff { authored_at, .. }) => {
                format!(
                    "Showing the version from {short} · {}",
                    time_text::relative(time_text::from_unix(*authored_at))
                )
            }
            Some(Version::Missing(_)) if missing.is_some() => format!(
                "The version from {short} is not available: showing the lines the comment was written on"
            ),
            Some(Version::Missing(_)) => format!("The version from {short} is not available"),
            None => format!("Loading the version from {short}…"),
        };
        Face { text, path: self.origin.anchor.path.clone(), missing }
    }

    /// Follows the thread when a reload brought new replies or another status.
    pub fn update_thread(&mut self, thread: &ReviewThread) {
        if thread.comments != self.thread.comments || thread.status != self.thread.status {
            self.thread.comments.clone_from(&thread.comments);
            self.thread.status = thread.status;
        }
    }

    /// The banner, then the diff of the origin's file with the thread at its original line.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        inputs: &mut OriginInputs<'_>,
        services: &mut Services,
        threads_ui: &mut ThreadsUi,
    ) -> Option<OriginEvent> {
        self.start_loading(ui, inputs, services);
        // In focus mode the strip says what the banner says, with the same ways out.
        let banner = if focus_mode::is_shown(ui.ctx()) { None } else { self.banner(ui, inputs) };
        let mut event = match banner {
            Some(BannerAction::Back) => Some(OriginEvent::Back),
            Some(BannerAction::Retry) => {
                self.retry();
                ui.ctx().request_repaint();
                None
            }
            None => None,
        };
        if let Some(picked) = self.show_body(ui, inputs, services, threads_ui) {
            event = Some(picked);
        }
        // Esc goes back unless the diff used it this frame (a selection) or a text field has focus.
        if !ui.ctx().text_edit_focused()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            event = Some(OriginEvent::Back);
        }
        event
    }

    /// Starts loading the version once the mirror is there, then the file's contents.
    fn start_loading(&mut self, ui: &Ui, inputs: &OriginInputs<'_>, services: &mut Services) {
        self.version.poll();
        self.contents.poll();
        if let Some((git, target)) = &inputs.mirror
            && self.version.is_idle()
        {
            let (git, target, origin, options) =
                (git.clone(), target.clone(), self.origin.clone(), inputs.options);
            self.started = skeleton::now(ui.ctx());
            self.version.start(&services.background, async move {
                load(&git, &target, &origin, options).await
            });
        }
        if let (Some(Version::Diff { file, .. }), Some((git, _))) =
            (self.version.ready(), &inputs.mirror)
        {
            self.contents.ensure(file, git, services, false);
        }
    }

    /// What shows below the banner: the version, the comment's own lines while it is gone, why
    /// nothing can show, or the skeleton while it loads.
    fn body(&self) -> Body {
        match &self.version {
            Load::Ready(Version::Diff { file, diff, .. }) => Body::Diff(Shown {
                diff: diff.clone(),
                path: file.path.clone(),
                old_path: file.old_path.clone(),
                whole_file: super::whole_file_side(file),
                has_contents: true,
            }),
            Load::Ready(Version::Missing(reason)) => match &self.hunk {
                Some(hunk) => Body::Diff(Shown {
                    diff: hunk.clone(),
                    path: self.origin.anchor.path.clone(),
                    old_path: None,
                    whole_file: None,
                    has_contents: false,
                }),
                None => Body::Unavailable {
                    title: "This version is no longer available",
                    detail: reason.clone(),
                },
            },
            Load::Failed(message) => {
                Body::Unavailable { title: "Couldn't load this version", detail: message.clone() }
            }
            Load::Idle | Load::Loading(_) => Body::Loading,
        }
    }

    fn show_body(
        &mut self,
        ui: &mut Ui,
        inputs: &mut OriginInputs<'_>,
        services: &mut Services,
        threads_ui: &mut ThreadsUi,
    ) -> Option<OriginEvent> {
        match self.body() {
            Body::Diff(shown) => self.show_diff(ui, &shown, inputs, services, threads_ui),
            Body::Unavailable { title, detail } => self.unavailable(ui, title, &detail, inputs),
            Body::Loading => {
                let layout = super::file_layout(None, inputs.layout);
                diff_skeleton::show(
                    ui,
                    layout,
                    inputs.font_size,
                    skeleton::reveal(ui.ctx(), self.started),
                );
                None
            }
        }
    }

    /// The version (or the comment's own lines) in the regular diff view, with the thread.
    fn show_diff(
        &mut self,
        ui: &mut Ui,
        shown: &Shown,
        inputs: &mut OriginInputs<'_>,
        services: &mut Services,
        threads_ui: &mut ThreadsUi,
    ) -> Option<OriginEvent> {
        let threads = [&self.thread];
        let diff_inputs = DiffInputs {
            diff: &shown.diff,
            contents: if shown.has_contents { self.contents.current() } else { None },
            threads: &threads,
            layout: super::file_layout(shown.whole_file, inputs.layout),
            font_size: inputs.font_size,
            wrap: inputs.wrap,
            can_comment: false,
            can_comment_outside_hunks: false,
            path: &shown.path,
            old_path: shown.old_path.as_deref(),
            line_spans: None,
            // The version a thread was written on shows that thread.
            show_threads: true,
            find: FindInputs::of(&services.settings),
        };
        // Its own id: the latest version's scroll position stays where the user left it.
        let event = ui
            .push_id("origin_diff", |ui| {
                self.view.show(ui, &diff_inputs, &mut *inputs.split, threads_ui)
            })
            .inner;
        match event {
            Some(DiffEvent::Thread(thread_event)) => {
                return Some(OriginEvent::Thread(thread_event));
            }
            Some(DiffEvent::RetryContents) => self.contents.retry(),
            Some(DiffEvent::SplitResized) => services.save_settings(),
            Some(
                DiffEvent::ColorAnyway | DiffEvent::SwitchLayout(_) | DiffEvent::Create { .. },
            )
            | None => {}
        }
        None
    }

    /// "Showing the version from 1a2b3c4 · 3 days ago" with the file, Open on the platform and
    /// Back to latest. While the commits are gone it says what is shown instead.
    fn banner(&self, ui: &mut Ui, inputs: &OriginInputs<'_>) -> Option<BannerAction> {
        let palette = theme::palette(ui.ctx());
        let Face { text, missing, .. } = self.face();
        let mut event = None;
        egui::Frame::new()
            .fill(palette.warning_surface)
            .inner_margin(egui::Margin::symmetric(12, 0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), BANNER_HEIGHT),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if buttons::secondary_icon(ui, icons::ARROW_LEFT, BACK_LABEL)
                            .on_hover_text("Back to the latest version (Esc)")
                            .clicked()
                        {
                            event = Some(BannerAction::Back);
                        }
                        if let Some(reason) = &missing
                            && buttons::subtle(ui, icons::ARROW_CLOCKWISE, "Retry")
                                .on_hover_text(reason)
                                .clicked()
                        {
                            event = Some(BannerAction::Retry);
                        }
                        if let Some(url) = &inputs.web_url
                            && buttons::subtle(
                                ui,
                                provider_icon::glyph(inputs.platform),
                                &format!("Open on {}", inputs.platform.label()),
                            )
                            .clicked()
                        {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            glyph::decorative(
                                ui,
                                RichText::new(icons::CLOCK_COUNTER_CLOCKWISE)
                                    .font(icons::state_font(&palette, icons::SIZE_SMALL, false))
                                    .color(palette.text),
                            );
                            let response = ui.add(
                                Label::new(
                                    RichText::new(&text).color(palette.text).size(style::BODY_SIZE),
                                )
                                .truncate()
                                .sense(Sense::hover()),
                            );
                            response.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Label, true, &text)
                            });
                            response.on_hover_text(format!(
                                "{} at {}",
                                self.origin.anchor.path, self.origin.head_sha
                            ));
                            ui.add(
                                Label::new(
                                    RichText::new(&self.origin.anchor.path)
                                        .color(palette.text_secondary)
                                        .size(style::SMALL_SIZE + 1.0),
                                )
                                .truncate(),
                            );
                        });
                    },
                );
            });
        event
    }

    /// The version could not be shown: why, Retry, the comment on the platform and the way back.
    fn unavailable(
        &mut self,
        ui: &mut Ui,
        title: &str,
        detail: &str,
        inputs: &OriginInputs<'_>,
    ) -> Option<OriginEvent> {
        let palette = theme::palette(ui.ctx());
        let open_label = format!("Open on {}", inputs.platform.label());
        let mut actions =
            vec![Action { icon: icons::ARROW_CLOCKWISE, label: "Retry", is_primary: true }];
        if inputs.web_url.is_some() {
            actions.push(Action {
                icon: provider_icon::glyph(inputs.platform),
                label: &open_label,
                is_primary: false,
            });
        }
        actions.push(Action { icon: icons::ARROW_LEFT, label: BACK_LABEL, is_primary: false });
        let back = actions.len() - 1;
        match notice::show(ui, icons::WARNING, palette.warning, title, detail, &actions)? {
            0 => {
                self.version = Load::Idle;
                None
            }
            picked if picked == back => Some(OriginEvent::Back),
            _ => {
                if let Some(url) = &inputs.web_url {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
                None
            }
        }
    }
}

/// Brings the origin's commits into the mirror (by id, only when missing) and diffs its file
/// there. Commits the remote no longer has make [`Version::Missing`].
async fn load(
    git: &GitMirror,
    target_sha: &str,
    origin: &ThreadOrigin,
    options: DiffOptions,
) -> Result<Version, ProviderError> {
    let head = origin.head_sha.as_str();
    let commits: Vec<&str> = std::iter::once(head).chain(origin.base_sha.as_deref()).collect();
    if let Err(error) = git.ensure_commits_by_id(&commits, &|_| {}).await {
        let reason = error.to_string();
        return Ok(Version::Missing(format!(
            "Its commits could not be fetched ({}). A force push may have replaced them on the remote.",
            reason.trim_end_matches('.')
        )));
    }
    let base = match &origin.base_sha {
        Some(base) => base.clone(),
        // GitHub keeps no base: the merge base with the target is where the diff started.
        None => git.merge_base(target_sha, head).await?,
    };
    let anchor = &origin.anchor;
    let file = git
        .changed_files_fast(&base, head)
        .await?
        .into_iter()
        .find(|file| {
            file.path == anchor.path || anchor.old_path.as_deref() == Some(file.path.as_str())
        })
        .ok_or_else(|| {
            ProviderError::Local(format!(
                "{} has no changes between {} and {}.",
                anchor.path,
                short_sha(&base),
                short_sha(head)
            ))
        })?;
    let diff = git.file_diff(&base, head, &file, options).await?;
    let authored_at = git.commit(head).await?.authored_at;
    Ok(Version::Diff { file, diff: Arc::new(diff), authored_at })
}

fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}
