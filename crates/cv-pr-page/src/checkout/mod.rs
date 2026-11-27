//! "Check out" in the pull request's header: the pull request's branch in a new worktree beside
//! the user's clone, or in the clone itself, then a terminal or a coding agent opened there.
//!
//! The header button changes with the checkout: "Check out ▾" opens the choices ([`choose`]);
//! while it runs, "Checking out… k/n" opens its progress ([`progress`]); once it is done, a split
//! button opens the last used tool, its menu everything else ([`ready`]). What could go wrong
//! before or while it runs (no clone, a clone with changes, the branch checked out already, a fork,
//! a failure) shows as a card in the same popover ([`cards`]), always with a way on. The work runs
//! in [`cv_services::checkouts`]; the overview's content never changes for it.
//!
//! The page also hands the service each new head of the pull request ([`follow`]), which the
//! checkout then follows silently where it can.

mod cards;
mod choose;
mod face;
mod progress;
mod ready;

use std::path::{Path, PathBuf};

use egui::{PopupCloseBehavior, Response, Ui};
use review_core::{AgentKind, PullRequestDetail, PullRequestState};
use review_git::checkout::{Changes, fork_branch, source_name, worktree_path};
use review_store::checkouts::{
    CheckoutKind, CheckoutPlace, CheckoutRecord, CheckoutSettings, OpenWith, RememberedCheckout,
};

use self::face::{Face, face};
pub(crate) use self::face::{chip, chip_width};
use cv_base::theme::style;
use cv_pr_session::session::PullRequestSession;
use cv_runtime::intents::{self, SettingsSection};
use cv_runtime::route::PullRequestKey;
use cv_services::Services;
use cv_services::agents::Detection;
use cv_services::checkouts::{self, Adopted, FolderPurpose, Found, JobPhase, Place, Plan, Query};
use cv_widgets::buttons;

/// The popover's width: room for a path and the step list.
const POPOVER_WIDTH: f32 = 400.0;
const CARET_WIDTH: f32 = 28.0;
/// Below this much room left of the button the header's button shows only its icon.
const COMPACT_BELOW: f32 = 360.0;
const REVEAL_ID: &str = "checkout_reveal";
const REMOVAL_ID: &str = "checkout_offer_removal";

/// What the popover shows besides its usual content.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Panel {
    #[default]
    Usual,
    /// A fork's checkout waits for a yes.
    Fork,
    /// The saved log of a done checkout.
    Log,
    /// Removing the worktree asks first.
    Remove,
    /// The last update by hand changed nothing; why.
    Blocked,
}

/// The choices in the popover, set from Settings each time it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Choice {
    place: CheckoutPlace,
    then: OpenWith,
    remember: bool,
}

/// The header's checkout button and its popover, per page.
#[derive(Default)]
pub(crate) struct CheckoutButton {
    choice: Option<Choice>,
    /// "Change folder…": the folder the new worktree goes into, instead of the pattern's.
    folder: Option<PathBuf>,
    /// "Pick existing clone…" picked this folder; the lookup tells whether it is one.
    picked_clone: Option<PathBuf>,
    panel: Panel,
    /// Opens the popover next frame (after a menu item, whose click would close it at once).
    opens: bool,
    /// An update by hand runs; when it changes nothing, the popover says why.
    awaits_update: bool,
    /// The running checkout's "Log" fold is open.
    is_log_open: bool,
}

/// What a click in the button, its popover or its menu asks for.
#[derive(Debug, Clone, PartialEq)]
enum Act {
    Start,
    ConfirmFork,
    Close,
    CancelJob,
    Dismiss,
    Retry,
    AskFolder(FolderPurpose, &'static str),
    UseWorktree,
    OpenFolder(PathBuf),
    Adopt { path: PathBuf, then_update: bool },
    Open(OpenWith),
    UpdateNow,
    Remove(Changes),
    Keep,
    ShowLog,
    AskRemove,
    CopyPath(PathBuf),
    Settings(SettingsSection),
}

/// What the page knows that the button needs.
struct View<'a> {
    key: &'a PullRequestKey,
    detail: &'a PullRequestDetail,
    /// Agents whose CLI is installed.
    agents: [(AgentKind, bool); 2],
    settings: &'a CheckoutSettings,
}

impl<'a> View<'a> {
    fn new(
        key: &'a PullRequestKey,
        detail: &'a PullRequestDetail,
        agents: [(AgentKind, bool); 2],
        services: &'a Services,
    ) -> Self {
        Self { key, detail, agents, settings: &services.settings.checkouts }
    }
}

impl CheckoutButton {
    /// The popover is open: the header stays on screen.
    pub fn is_engaged(&self, ctx: &egui::Context, key: &PullRequestKey) -> bool {
        egui::Popup::is_id_open(ctx, popover_id(key)) || self.opens
    }

    /// The button in the header's command bar (laid out right to left), its popover and menu.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        session: &PullRequestSession,
        detail: &PullRequestDetail,
        services: &mut Services,
    ) {
        let key = &session.key;
        let agents = agents(ui.ctx(), services);
        let compact = ui.available_width() < COMPACT_BELOW;
        let popover = popover_id(key);
        let was_open = egui::Popup::is_id_open(ui.ctx(), popover);
        let (anchor, mut act, toggled) = match services.checkouts.record(key) {
            Some(record) if services.checkouts.job(key).is_none() => {
                let status = services.checkouts.status(key);
                let view = View::new(key, detail, agents, services);
                let (anchor, act) = ready::button(ui, &view, record, status, compact);
                (anchor, act, false)
            }
            _ => {
                let response = face_of(ui, key, services, compact);
                (response.rect, None, response.clicked())
            }
        };
        if take_request(ui.ctx(), REMOVAL_ID, key) {
            self.reopen_with(Panel::Remove);
        }
        let is_requested =
            take_request(ui.ctx(), REVEAL_ID, key) || std::mem::take(&mut self.opens);
        let command = if is_requested {
            Some(egui::SetOpenCommand::Bool(true))
        } else if toggled {
            Some(egui::SetOpenCommand::Toggle)
        } else {
            None
        };
        if command.is_some() && !was_open {
            let has_record = services.checkouts.record(key).is_some();
            self.opening(key, detail, has_record, session, services);
        }
        let has_mirror = session.mirror.ready().is_some();
        let shown = egui::Popup::new(popover, ui.ctx().clone(), anchor, ui.layer_id())
            .open_memory(command)
            .align(egui::RectAlign::BOTTOM_END)
            .gap(style::SPACE_XS)
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .width(POPOVER_WIDTH)
            .accessible_name("Check out")
            .show(|ui| {
                ui.set_width(POPOVER_WIDTH);
                let reads = ServicesView {
                    checkouts: &services.checkouts,
                    is_asking_folder: services.checkouts.is_asking_folder(),
                    has_mirror,
                };
                let record = services.checkouts.record(key);
                self.popover(ui, &View::new(key, detail, agents, services), record, &reads)
            });
        match shown {
            Some(inner) => act = act.or(inner.inner),
            None if !self.opens => self.panel = Panel::Usual,
            None => {}
        }
        if self.awaits_update {
            self.watch_update(key, services);
        }
        if let Some(act) = act {
            self.apply(ui.ctx(), act, session, services);
        }
        self.take_picked(ui.ctx(), session, services);
        if services.checkouts.job(key).is_some_and(checkouts::Job::is_running) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// The popover opens: the choices start from Settings, and the clone is looked for anew.
    fn opening(
        &mut self,
        key: &PullRequestKey,
        detail: &PullRequestDetail,
        has_record: bool,
        session: &PullRequestSession,
        services: &mut Services,
    ) {
        if has_record || services.checkouts.job(key).is_some() {
            return;
        }
        let settings_key = checkouts::settings_key(key);
        let (place, then) = services.settings.checkouts.preselected(&settings_key);
        let remember = services.settings.checkouts.remembered.contains_key(&settings_key);
        self.choice = Some(Choice { place, then, remember });
        self.panel = Panel::Usual;
        look_up(key, detail, session, services);
    }

    /// The popover's content for the checkout's state.
    fn popover(
        &mut self,
        ui: &mut Ui,
        view: &View<'_>,
        record: Option<&CheckoutRecord>,
        services: &ServicesView<'_>,
    ) -> Option<Act> {
        ui.spacing_mut().item_spacing.y = style::SPACE_S;
        if let Some(job) = services.checkouts.job(view.key) {
            return match job.phase() {
                JobPhase::Running => progress::running(ui, job, &mut self.is_log_open),
                JobPhase::Failed(failure) => cards::failed(ui, view, job, failure),
            };
        }
        if let Some(record) = record {
            return match self.panel {
                Panel::Log => ready::log(ui, record),
                Panel::Remove => cards::remove(
                    ui,
                    record,
                    services.checkouts.removal(view.key),
                    services.checkouts.offer(view.key).and_then(|offer| offer.kept.as_deref()),
                ),
                Panel::Blocked => {
                    cards::blocked(ui, view, record, services.checkouts.status(view.key))
                }
                Panel::Usual | Panel::Fork => None,
            };
        }
        let found = services.checkouts.found(view.key);
        let choice = self.choice.get_or_insert(Choice {
            place: CheckoutPlace::Worktree,
            then: OpenWith::Nothing,
            remember: false,
        });
        let picked = self.picked_clone.as_deref();
        match found.and_then(|found| found.ready()) {
            Some(Found::NoClone) => {
                cards::no_clone(ui, view, picked, services.is_asking_folder, services.has_mirror)
            }
            Some(Found::Clone(survey)) if self.panel == Panel::Fork => {
                cards::fork(ui, view, survey)
            }
            Some(Found::Clone(survey)) => {
                let folder = self.folder.as_deref();
                let place = choose::Place { survey, folder, has_mirror: services.has_mirror };
                choose::show(ui, view, choice, &place)
            }
            None => choose::looking(ui, view, found.and_then(|found| found.error())),
        }
    }

    fn apply(
        &mut self,
        ctx: &egui::Context,
        act: Act,
        session: &PullRequestSession,
        services: &mut Services,
    ) {
        let key = &session.key;
        let popover = popover_id(key);
        match act {
            Act::Start => self.start(session, services, false),
            Act::ConfirmFork => self.start(session, services, true),
            Act::Close => {
                self.panel = Panel::Usual;
                egui::Popup::close_id(ctx, popover);
            }
            Act::CancelJob => services.checkouts.cancel(key),
            Act::Dismiss => {
                services.checkouts.dismiss(key);
                if let Some(detail) = session.detail.ready() {
                    look_up(key, detail, session, services);
                }
            }
            Act::Retry => {
                if let Some(plan) = services.checkouts.job(key).map(|job| job.plan.clone()) {
                    services.checkouts.dismiss(key);
                    services.checkouts.start(plan, &services.background);
                }
            }
            Act::AskFolder(purpose, title) => {
                services.checkouts.ask_folder(purpose, title, &services.background);
            }
            Act::UseWorktree => {
                services.checkouts.dismiss(key);
                if let Some(choice) = &mut self.choice {
                    choice.place = CheckoutPlace::Worktree;
                }
            }
            Act::OpenFolder(folder) => services.toasts.open_folder(ctx, &folder),
            Act::Adopt { path, then_update } => {
                self.adopt(ctx, session, services, path, then_update);
            }
            Act::Open(with) => checkouts::open_with(services, key, with),
            Act::UpdateNow => {
                if let Some(mirror) = session.mirror.ready() {
                    services.checkouts.update_now(
                        key,
                        &mirror.git,
                        &mirror.head,
                        &services.background,
                    );
                    self.awaits_update = true;
                }
            }
            Act::Remove(changes) => {
                services.checkouts.remove(key, changes, &services.background);
            }
            Act::Keep => {
                services.checkouts.keep(key);
                self.panel = Panel::Usual;
                egui::Popup::close_id(ctx, popover);
            }
            Act::ShowLog => self.reopen_with(Panel::Log),
            Act::AskRemove => self.reopen_with(Panel::Remove),
            Act::CopyPath(path) => ctx.copy_text(path.display().to_string()),
            Act::Settings(section) => intents::request_settings_section(ctx, section),
        }
    }

    fn reopen_with(&mut self, panel: Panel) {
        self.panel = panel;
        self.opens = true;
    }

    /// Builds the plan from the choices and the lookup, and starts it.
    fn start(
        &mut self,
        session: &PullRequestSession,
        services: &mut Services,
        is_fork_confirmed: bool,
    ) {
        let key = &session.key;
        let (Some(choice), Some(mirror), Some(detail)) =
            (self.choice, session.mirror.ready(), session.detail.ready())
        else {
            return;
        };
        let Some(Some(Found::Clone(survey))) =
            services.checkouts.found(key).map(|found| found.ready())
        else {
            return;
        };
        if survey.is_fork && !is_fork_confirmed {
            self.panel = Panel::Fork;
            return;
        }
        let settings = &services.settings.checkouts;
        let source = &detail.summary.source_branch;
        let place = match choice.place {
            CheckoutPlace::Worktree => Place::Worktree(worktree_folder(
                settings,
                &survey.clone,
                key.number,
                source,
                self.folder.as_deref(),
            )),
            CheckoutPlace::Clone => Place::Clone,
        };
        let plan = Plan {
            key: key.clone(),
            mirror: mirror.git.clone(),
            clone: survey.clone.clone(),
            place,
            head: mirror.head.clone(),
            branch: survey.branch.clone(),
            upstream: survey.upstream(&detail.summary.source_branch),
            then: choice.then,
            then_name: then_name(choice.then, settings),
        };
        if choice.remember {
            let remembered = RememberedCheckout { place: choice.place, then: choice.then };
            services.settings.checkouts.remembered.insert(checkouts::settings_key(key), remembered);
            services.save_settings();
        }
        self.panel = Panel::Usual;
        // The popover stays open on the progress; closing it leaves the checkout running.
        services.checkouts.start(plan, &services.background);
    }

    /// "Clone into a folder…" and the other folder dialogs answered: act on the folder.
    fn take_picked(
        &mut self,
        ctx: &egui::Context,
        session: &PullRequestSession,
        services: &mut Services,
    ) {
        let key = &session.key;
        if let Some(folder) =
            services.checkouts.take_picked(&FolderPurpose::WorktreeFolder(key.clone()))
        {
            self.folder = Some(folder);
        }
        if let Some(folder) = services.checkouts.take_picked(&FolderPurpose::PickClone(key.clone()))
        {
            services
                .settings
                .checkouts
                .pinned_clones
                .insert(checkouts::settings_key(key), folder.clone());
            services.save_settings();
            self.picked_clone = Some(folder);
            if let Some(detail) = session.detail.ready() {
                look_up(key, detail, session, services);
            }
        }
        if let Some(folder) = services.checkouts.take_picked(&FolderPurpose::CloneInto(key.clone()))
        {
            self.clone_into(ctx, session, services, &folder);
        }
    }

    /// A new clone of the repository in `folder`, from the mirror.
    fn clone_into(
        &mut self,
        ctx: &egui::Context,
        session: &PullRequestSession,
        services: &mut Services,
        folder: &std::path::Path,
    ) {
        let (Some(mirror), Some(detail), Some(origin)) =
            (session.mirror.ready(), session.detail.ready(), session.clone_url())
        else {
            return;
        };
        let name = session.key.repository_key.rsplit('/').next().unwrap_or("repository");
        let then = self.choice.map_or(OpenWith::Nothing, |choice| choice.then);
        let settings = &services.settings.checkouts;
        let plan = Plan {
            key: session.key.clone(),
            mirror: mirror.git.clone(),
            clone: folder.join(name),
            place: Place::NewClone { origin },
            head: mirror.head.clone(),
            branch: source_name(&detail.summary.source_branch).to_owned(),
            upstream: None,
            then,
            then_name: then_name(then, settings),
        };
        services.checkouts.start(plan, &services.background);
        egui::Popup::open_id(ctx, popover_id(&session.key));
    }

    /// "Open it" and "Update it": the working tree that has the branch becomes the checkout.
    fn adopt(
        &mut self,
        ctx: &egui::Context,
        session: &PullRequestSession,
        services: &mut Services,
        path: PathBuf,
        then_update: bool,
    ) {
        let key = &session.key;
        let Some(Some(Found::Clone(survey))) =
            services.checkouts.found(key).map(|found| found.ready())
        else {
            return;
        };
        // The head the branch had is unknown here: the update compares against the new head.
        let adopted = Adopted {
            clone: survey.clone.clone(),
            path,
            branch: survey.branch.clone(),
            head: String::new(),
        };
        services.checkouts.dismiss(key);
        if let Err(error) = services.checkouts.adopt(key, adopted) {
            let folder = cv_runtime::toasts::ToastAction::open_folder(services.paths.config_dir());
            services.toasts.error(format!("Could not save the checkout: {error}"), folder);
            return;
        }
        egui::Popup::close_id(ctx, popover_id(key));
        if then_update {
            if let Some(mirror) = session.mirror.ready() {
                services.checkouts.update_now(key, &mirror.git, &mirror.head, &services.background);
                self.awaits_update = true;
            }
        } else {
            let then = self.choice.map_or(OpenWith::Nothing, |choice| choice.then);
            checkouts::open_with(services, key, then);
        }
    }

    /// An update by hand finished: if it changed nothing, the popover says why.
    fn watch_update(&mut self, key: &PullRequestKey, services: &Services) {
        let Some(status) = services.checkouts.status(key) else {
            return;
        };
        if status.is_updating {
            return;
        }
        self.awaits_update = false;
        if status.blocked.is_some() {
            self.reopen_with(Panel::Blocked);
        }
    }
}

/// What the popover reads from the services.
struct ServicesView<'a> {
    checkouts: &'a checkouts::Checkouts,
    is_asking_folder: bool,
    has_mirror: bool,
}

/// The button before the checkout is done: "Check out", "Checking out… k/n", "Checkout failed".
fn face_of(ui: &mut Ui, key: &PullRequestKey, services: &Services, compact: bool) -> Response {
    let is_open = egui::Popup::is_id_open(ui.ctx(), popover_id(key));
    match services.checkouts.job(key) {
        Some(job) if job.is_running() => {
            let progress = job.progress();
            let state = Face::Running { done: progress.done_count(), total: progress.steps.len() };
            face(ui, state, is_open, compact)
        }
        Some(_) => face(ui, Face::Failed, is_open, compact),
        None => face(ui, Face::Idle, is_open, compact),
    }
}

/// Which agents' CLIs are installed. Looked for only once a popover or menu is open, so a page
/// whose checkout nobody touches starts no CLI; until then an agent counts as there.
fn agents(ctx: &egui::Context, services: &mut Services) -> [(AgentKind, bool); 2] {
    let is_asked = egui::Popup::is_any_open(ctx);
    [AgentKind::Claude, AgentKind::Codex].map(|kind| {
        let is_installed = if is_asked {
            let detection = services.agents.detection(kind, &services.background);
            matches!(detection, Detection::Installed(_))
        } else {
            services.agents.installed(kind).unwrap_or(true)
        };
        (kind, is_installed)
    })
}

/// Looks for the clone of the pull request's repository, once its clone URL is known.
fn look_up(
    key: &PullRequestKey,
    detail: &PullRequestDetail,
    session: &PullRequestSession,
    services: &mut Services,
) {
    let Some(clone_url) = session.clone_url() else {
        return;
    };
    let settings = &services.settings.checkouts;
    let query = Query {
        clone_url,
        source_branch: detail.summary.source_branch.clone(),
        folders: settings.search_folders.clone(),
        pinned: settings.pinned_clones.get(&checkouts::settings_key(key)).cloned(),
    };
    services.checkouts.look_up(key, query, &services.background);
}

/// The pull request's head as the page has it in the mirror: the checkout follows it; once the
/// pull request shows merged or closed, its checkout gets what Settings says. Call once per frame.
pub(crate) fn follow(session: &PullRequestSession, services: &mut Services) {
    if session.detail.ready().is_some_and(|detail| detail.summary.state != PullRequestState::Open) {
        checkouts::finished(services, &session.key);
    }
    if let Some(mirror) = session.mirror.ready() {
        services.checkouts.follow_head(
            &session.key,
            &mirror.git,
            &mirror.head,
            &services.background,
        );
    }
}

/// Where a new worktree of pull request `number` goes: into `folder` when one was picked, else
/// where the pattern says. Named after the pull request's branch, which for a fork differs from
/// the local branch's name.
fn worktree_folder(
    settings: &CheckoutSettings,
    clone: &Path,
    number: u64,
    source_branch: &str,
    folder: Option<&Path>,
) -> PathBuf {
    let source = source_name(source_branch);
    match folder {
        Some(folder) => folder.join(fork_branch(number, source)),
        None => worktree_path(&settings.worktree_pattern, clone, number, source),
    }
}

/// The name of what opens after the checkout, for its last step; `None` for nothing.
fn then_name(then: OpenWith, settings: &CheckoutSettings) -> Option<String> {
    (then != OpenWith::Nothing).then(|| checkouts::tool_name(then, settings))
}

fn popover_id(key: &PullRequestKey) -> egui::Id {
    egui::Id::unique(("checkout_popover", &key.connection_id, &key.repository_key, key.number))
}

/// Opens the checkout's popover on its pull request's page (the title bar's chip).
pub fn reveal(ctx: &egui::Context, key: &PullRequestKey) {
    cv_base::mailbox::post(ctx, egui::Id::unique(REVEAL_ID), key.clone());
}

fn take_request(ctx: &egui::Context, name: &str, key: &PullRequestKey) -> bool {
    cv_base::mailbox::take_if(ctx, egui::Id::unique(name), |asked: &PullRequestKey| asked == key)
        .is_some()
}

/// The kind of checkout removal is offered for.
fn is_removable(record: &CheckoutRecord) -> bool {
    record.kind == CheckoutKind::Worktree
}

/// A settings link with the section as its name.
fn settings_link(ui: &mut Ui, text: &str, section: SettingsSection) -> Option<Act> {
    buttons::text_link(ui, text, "Open Settings").clicked().then_some(Act::Settings(section))
}
