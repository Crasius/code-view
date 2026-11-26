//! Pipelines tab: every pipeline, job and status check of the head commit in one table; a run's
//! row opens it full width (Azure DevOps' run page: stage graph or job list), a job's row its log
//! beside the run's jobs. The breadcrumb names the run and the job. While a run is unfinished,
//! this tab and the Overview follow it with [`follow_running`].

#![forbid(unsafe_code)]
// RS-LINT-02: egui lays out in f32 points while rows, columns and byte offsets are usize or u64.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "egui pixel math converts between f32 points and usize indices"
)]

mod attempts;
pub mod checks;
mod checks_table;
mod graph;
mod job_tree;
mod jobs_list;
mod log;
mod log_page;
mod panes;
mod rerun;
mod run_header;
mod run_view;
pub mod stage_bars;
#[cfg(test)]
mod stage_pan_check;
#[cfg(test)]
mod stage_view_shots;
#[cfg(test)]
mod stage_zoom_check;
mod tree;
mod waiting;

use std::borrow::Cow;

use cv_base::theme::icons;
use cv_pr_session::session::PullRequestSession;
use cv_runtime::intents;
use cv_runtime::load::Live;
use cv_runtime::refresh;
use cv_services::Services;
use cv_shell::focus_mode::{self, Surface};
use cv_widgets::states::{self, Action};
use cv_widgets::time_text;
use egui::Ui;
use review_core::{AttemptSource, PipelineJob, PipelineRun, Timestamp};

use self::attempts::Known;
use self::checks::{Check, CheckKey};
use self::checks_table::{ChecksTable, TableAction, TableInput};
pub use self::graph::stage_card::ellipsized;
use self::job_tree::LogTarget;
use self::log::viewer::LogViewer;
use self::log_page::{
    AttemptNode, JobList, LogPane, Switch, attempt_pending, focused_log, jobs_beside_log, log_bar,
};
use self::rerun::Picked;
use self::rerun::requests::RerunRequests;
use self::run_header::{HeaderAction, HeaderInput, RunHeader, is_free_key, step_keys};
use self::run_view::{RunView, RunViewAction, RunViewInput};
use self::stage_bars::Stages;
use self::tree::{find, first_log, log_owner, owning_job, replace_node, retried_ancestor};

/// What the tab shows: the checks table, one run full width, or a job's log beside the run's jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Page {
    #[default]
    Checks,
    Run,
    Log,
}

/// A log to open once the run's stages are known.
enum PendingLog {
    /// "Logs" on a run: its first failed log, else its first log.
    FirstFailed,
    /// A job picked in the table or the switcher.
    Job(String),
    /// The earlier attempt just picked: its first failed log, else its first log, once known.
    Attempt,
}

/// An earlier attempt on screen in place of the latest: the retried stage or job (by its id in
/// the run's stages) and the attempt's number.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ShownAttempt {
    node_id: String,
    number: u32,
}

#[derive(Default)]
pub struct PipelinesTab {
    page: Page,
    selected_run: Option<String>,
    /// The job the header names: the one whose log is open, or a picked job without a log.
    selected_job: Option<String>,
    table: ChecksTable,
    header: RunHeader,
    view: RunView,
    log: LogViewer,
    pending_log: Option<PendingLog>,
    /// An earlier attempt shown in the log page; `None` shows the latest.
    attempt: Option<ShownAttempt>,
    reruns: RerunRequests,
}

impl PipelinesTab {
    /// Shows the checks table, as it was left (filters, scroll position, keyboard row).
    pub fn show_checks(&mut self) {
        self.page = Page::Checks;
    }

    /// Shows one run full width at its stages, even when its Jobs pivot was shown last.
    pub fn open_run(&mut self, run_id: &str) {
        if self.selected_run.as_deref() != Some(run_id) {
            self.selected_run = Some(run_id.to_owned());
            self.header = RunHeader::default();
        }
        self.show_stages();
        self.view.show_stages();
    }

    /// Shows a run full width, or a job's log beside its run's jobs.
    fn open_check(&mut self, key: CheckKey) {
        self.open_run(&key.run_id);
        if let Some(job_id) = key.job_id {
            self.selected_job = Some(job_id.clone());
            self.pending_log = Some(PendingLog::Job(job_id));
        }
    }

    /// Back from a log, or from one still waiting for its stages, to the run's stages.
    fn show_stages(&mut self) {
        self.page = Page::Run;
        (self.selected_job, self.attempt, self.pending_log) = (None, None, None);
    }

    /// The breadcrumb levels below the pull request, each with its link's tooltip: "Pipelines",
    /// the run, the open job.
    pub fn trail(&self, session: &PullRequestSession) -> Vec<(String, &'static str)> {
        let Some(run) = self.selected(session) else {
            return Vec::new();
        };
        let pipelines = ("Pipelines".to_owned(), "Back to the checks");
        let run = (run.name.clone(), "Back to the run's stages");
        match self.page {
            Page::Checks => Vec::new(),
            Page::Run => vec![pipelines, run],
            Page::Log => {
                let job = self.log.target().map(|target| match &self.attempt {
                    Some(attempt) => (format!("{} · Attempt {}", target.title, attempt.number), ""),
                    None => (target.title.clone(), ""),
                });
                [pipelines, run].into_iter().chain(job).collect()
            }
        }
    }

    /// A breadcrumb level was clicked: 0 is the pull request, then the levels of [`Self::trail`].
    pub fn go_to_level(&mut self, level: usize) {
        match level {
            0 | 1 => self.page = Page::Checks,
            _ => self.show_stages(),
        }
    }

    fn selected<'a>(&self, session: &'a PullRequestSession) -> Option<&'a PipelineRun> {
        let id = self.selected_run.as_deref()?;
        session.runs.ready()?.iter().find(|run| run.id == id)
    }

    pub fn show(&mut self, ui: &mut Ui, session: &mut PullRequestSession, services: &mut Services) {
        self.log.release_unless(self.page == Page::Log, services);
        self.reruns.poll(session, services);
        follow_running(ui.ctx(), session, services);
        let runs = match (session.runs.ready(), session.runs.error()) {
            (Some(runs), _) => runs.clone(),
            (None, Some(message)) => {
                let message = message.to_owned();
                // Anonymous requests run into rate limits first; a token is the lasting fix.
                let mut actions = vec![Action::primary(icons::ARROW_CLOCKWISE, "Retry")];
                if !session.has_token {
                    actions.push(Action::secondary(icons::KEY, "Add a token"));
                }
                let picked = egui::Frame::new()
                    .inner_margin(egui::Margin::same(24))
                    .show(ui, |ui| states::error_with_actions(ui, &message, &actions))
                    .inner;
                match picked {
                    Some(0) => session.reload_runs(services),
                    Some(_) => intents::request_fix(ui.ctx(), &session.key.connection_id),
                    None => {}
                }
                return;
            }
            (None, None) => {
                waiting::placeholder(ui, session, services, waiting::checks_skeleton);
                return;
            }
        };
        if runs.is_empty() {
            let picked = states::message_with_actions(
                ui,
                icons::ROCKET_LAUNCH,
                "No pipelines",
                "No pipeline runs or status checks were reported for the latest commit.",
                &[Action::secondary(icons::ARROW_CLOCKWISE, "Refresh")],
            );
            if picked.is_some() {
                session.reload_runs(services);
            }
            return;
        }
        let selected = self
            .selected_run
            .as_ref()
            .and_then(|id| runs.iter().find(|run| &run.id == id))
            .cloned();
        if let (Page::Run | Page::Log, Some(run)) = (self.page, selected) {
            self.show_run(ui, &run, &runs, session, services);
        } else {
            // The run is gone (a new commit brought other runs): back to the table.
            self.page = Page::Checks;
            self.checks_page(ui, &runs, session, services);
        }
    }

    fn checks_page(
        &mut self,
        ui: &mut Ui,
        runs: &[PipelineRun],
        session: &mut PullRequestSession,
        services: &mut Services,
    ) {
        // Jobs are rows too: the table waits for the head runs' jobs, so rows never shift in later.
        // An older run's jobs load when it is opened and are rows from then on.
        session.want_head_stages(services);
        if !session.are_head_stages_settled() {
            waiting::placeholder(ui, session, services, waiting::checks_skeleton);
            return;
        }
        let checks = all_checks(runs, session);
        let bars: Vec<Stages<'_>> = checks.iter().map(|check| bars_of(check, session)).collect();
        let title = session.detail.ready().map_or_else(
            || "Checks".to_owned(),
            |detail| format!("Checks for {}", short_sha(&detail.head_sha)),
        );
        let input = TableInput {
            runs,
            checks: &checks,
            bars: &bars,
            platform: session.kind,
            has_token: session.has_token,
            is_busy: self.reruns.is_busy(),
            title: &title,
            filter: &mut services.settings.pipelines.status_filter,
        };
        let action = self.table.show(ui, input);
        drop(bars);
        drop(checks);
        match action {
            Some(TableAction::Logs(key)) if key.job_id.is_none() => {
                self.open_run(&key.run_id);
                self.pending_log = Some(PendingLog::FirstFailed);
            }
            Some(TableAction::Open(key) | TableAction::Logs(key)) => self.open_check(key),
            Some(TableAction::OpenUrl(url)) => ui.ctx().open_url(egui::OpenUrl::new_tab(url)),
            Some(TableAction::Rerun(run_id, action)) => {
                self.reruns.start(vec![(run_id, action)], session, services);
            }
            Some(TableAction::RerunFailed) => {
                self.reruns.start(checks::failed_reruns(runs), session, services);
            }
            Some(TableAction::AddToken) => {
                intents::request_fix(ui.ctx(), &session.key.connection_id);
            }
            Some(TableAction::FilterChanged) => services.save_settings(),
            None => {}
        }
        if let Some(run) = self.reruns.failed_run(runs) {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 0)).show(ui, |ui| {
                self.reruns.show_failure(ui, run, run.web_url.as_deref(), session, services);
            });
        }
    }

    fn show_run(
        &mut self,
        ui: &mut Ui,
        run: &PipelineRun,
        runs: &[PipelineRun],
        session: &mut PullRequestSession,
        services: &mut Services,
    ) {
        session.want_stages(run, services);
        self.sync_shown_log(run, session, services);
        // A job's log can take over the window; the run header then gives way to the log's strip.
        let is_focused = self.page == Page::Log && focus_mode::offer(ui.ctx(), Surface::Log);
        let open_label = states::open_on_platform(session.kind);
        match self.run_header(ui, run, runs, session, is_focused, &open_label) {
            Some(HeaderAction::Back) => {
                self.page = Page::Checks;
                return;
            }
            Some(HeaderAction::Select(key)) => {
                self.open_check(key);
                return;
            }
            Some(HeaderAction::Rerun(picked)) => {
                self.on_rerun_pick(ui, picked, run, session, services);
            }
            Some(HeaderAction::OpenUrl(url)) => ui.ctx().open_url(egui::OpenUrl::new_tab(url)),
            None => {}
        }
        let web_url = run
            .web_url
            .clone()
            .or_else(|| session.detail.ready().map(|detail| detail.summary.web_url.clone()));
        if self.page == Page::Run
            && !egui::Popup::is_any_open(ui.ctx())
            && is_free_key(ui, egui::Key::Escape)
        {
            self.page = Page::Checks;
            return;
        }
        if is_focused {
            self.reruns.show_focus_failure(ui, run, web_url.as_deref(), session, services);
        } else {
            egui::Frame::new()
                .inner_margin(egui::Margin { left: 24, right: 24, top: 0, bottom: 0 })
                .show(ui, |ui| {
                    self.reruns.show_failure(ui, run, web_url.as_deref(), session, services);
                });
        }
        if !run.has_jobs {
            waiting::status_only(ui, session.kind, web_url.as_deref(), &open_label);
            return;
        }
        let Some(stages) = waiting::loaded_stages(ui, run, session, services) else {
            return;
        };
        let is_resized = if self.page == Page::Log {
            self.log_page(ui, &stages, &run.id, session, services, is_focused)
        } else {
            self.run_page(ui, run, &stages, session, services);
            false
        };
        if is_resized {
            services.save_settings();
        }
    }

    /// Once the run's stages are known: drops an earlier attempt the run no longer lists, opens
    /// the log picked before, follows an item that got a log, and names the open log's job.
    fn sync_shown_log(
        &mut self,
        run: &PipelineRun,
        session: &PullRequestSession,
        services: &Services,
    ) {
        let Some(stages) = session.stages(&run.id).and_then(Live::ready) else {
            return;
        };
        if self.attempt.is_some()
            && matches!(self.attempt_node(stages, &run.id, session), Some(AttemptNode::Gone))
        {
            self.attempt = None;
        }
        let attempt = match self.attempt_node(stages, &run.id, session) {
            Some(AttemptNode::Ready(job)) => Some(*job),
            _ => None,
        };
        let view = self.view_stages(stages, attempt.clone());
        let attempt_root = attempt.as_ref().map(|job| job.id.as_str());
        self.open_pending_log(&view, attempt_root, services);
        self.follow_new_log(&view, services);
        if self.page == Page::Log
            && let Some(target) = self.log.target()
        {
            // The header names the check, which is the latest attempt's job.
            let job = match &self.attempt {
                Some(attempt) => owning_job(stages, &attempt.node_id),
                None => owning_job(&view, &target.job_id),
            };
            self.selected_job = job.map(|job| job.id.clone());
        }
    }

    /// The run header above the stages or the log, unless focus mode hides it; J/K step through
    /// the checks either way.
    fn run_header(
        &mut self,
        ui: &mut Ui,
        run: &PipelineRun,
        runs: &[PipelineRun],
        session: &PullRequestSession,
        is_focused: bool,
        open_label: &str,
    ) -> Option<HeaderAction> {
        let order = all_checks(runs, session);
        let current = CheckKey { run_id: run.id.clone(), job_id: self.selected_job.clone() };
        let check = order
            .iter()
            .find(|check| check.is(&current))
            .copied()
            .unwrap_or(Check { run, job: None });
        // Checks without their own page open the pull request, where the platform lists them.
        let web_url = check
            .web_url()
            .map(str::to_owned)
            .or_else(|| session.detail.ready().map(|detail| detail.summary.web_url.clone()));
        let header_input = HeaderInput {
            check,
            order: &order,
            platform: session.kind,
            has_token: session.has_token,
            is_busy: self.reruns.is_busy(),
            web_url: web_url.as_deref(),
            open_label,
        };
        let picked = if is_focused {
            None
        } else {
            egui::Frame::new()
                .inner_margin(egui::Margin { left: 24, right: 24, top: 12, bottom: 4 })
                .show(ui, |ui| self.header.show(ui, &header_input))
                .inner
        };
        let position = order.iter().position(|other| other.is(&current));
        picked.or_else(|| step_keys(ui, &order, position))
    }

    /// Opens the log picked before the run's stages were known; a job without any log opens with
    /// its state in the log pane.
    /// `stages` is what the page shows: with an earlier attempt in place of its latest once that
    /// is known, `attempt_root` the id of that attempt's node in it.
    fn open_pending_log(
        &mut self,
        stages: &[PipelineJob],
        attempt_root: Option<&str>,
        services: &Services,
    ) {
        let target = match self.pending_log.take() {
            None => return,
            Some(PendingLog::FirstFailed) => first_log(stages),
            Some(PendingLog::Job(job_id)) => job_tree::resolve(stages, &job_id),
            Some(PendingLog::Attempt) => {
                if let Some(root) = attempt_root {
                    job_tree::resolve(stages, root)
                } else {
                    // Still loading (or failed, which the log pane explains with Retry): wait.
                    if self.attempt.is_some() {
                        self.pending_log = Some(PendingLog::Attempt);
                    }
                    return;
                }
            }
        };
        if let Some(target) = target {
            self.open_log(target, services);
        }
    }

    /// An item shown without a log switches to its log as soon as a reload of the stages (the
    /// pipeline refresh, while the window is focused) brings one.
    fn follow_new_log(&mut self, stages: &[PipelineJob], services: &Services) {
        let Some(waiting) =
            self.log.target().filter(|target| self.page == Page::Log && target.log_id.is_none())
        else {
            return;
        };
        if let Some(target) =
            job_tree::resolve(stages, &waiting.job_id).filter(|target| target.log_id.is_some())
        {
            self.open_log(target, services);
        }
    }

    fn run_page(
        &mut self,
        ui: &mut Ui,
        run: &PipelineRun,
        stages: &[PipelineJob],
        session: &mut PullRequestSession,
        services: &mut Services,
    ) {
        let action = {
            let loaded = session.loaded_attempts(&run.id);
            let input = RunViewInput {
                run_id: &run.id,
                stages,
                platform: session.kind,
                has_token: session.has_token,
                is_busy: self.reruns.is_busy(),
                known: Known { loaded: &loaded },
            };
            self.view.show(ui, input)
        };
        match action {
            Some(RunViewAction::Log(target)) => self.open_log(target, services),
            Some(RunViewAction::Rerun(action)) => {
                self.reruns.start(vec![(run.id.clone(), action)], session, services);
            }
            Some(RunViewAction::AddToken) => {
                intents::request_fix(ui.ctx(), &session.key.connection_id);
            }
            Some(RunViewAction::Attempt { node_id, number }) => {
                self.pick_attempt(&node_id, number, stages, &run.id, session, services);
            }
            None => {}
        }
    }

    /// Shows attempt `number` of the node `node_id`: the latest goes back to the node's own log,
    /// an earlier one opens its first failed log (loading it first where the platform lists it
    /// by id only).
    fn pick_attempt(
        &mut self,
        node_id: &str,
        number: u32,
        stages: &[PipelineJob],
        run_id: &str,
        session: &mut PullRequestSession,
        services: &Services,
    ) {
        let Some(node) = find(stages, node_id) else {
            return;
        };
        if number == node.attempt_number() {
            self.attempt = None;
            if let Some(target) = job_tree::resolve(stages, node_id) {
                self.open_log(target, services);
            }
            return;
        }
        let Some(attempt) = node.previous_attempts.iter().find(|attempt| attempt.number == number)
        else {
            return;
        };
        if let Some(target) = attempt.lazy_target() {
            session.want_attempt(run_id, target, services);
        }
        self.attempt = Some(ShownAttempt { node_id: node_id.to_owned(), number });
        self.page = Page::Log;
        self.pending_log = Some(PendingLog::Attempt);
    }

    /// The earlier attempt on screen, as far as known; `None` when the latest is shown.
    fn attempt_node(
        &self,
        stages: &[PipelineJob],
        run_id: &str,
        session: &PullRequestSession,
    ) -> Option<AttemptNode> {
        let shown = self.attempt.as_ref()?;
        let Some(attempt) = find(stages, &shown.node_id).and_then(|node| {
            node.previous_attempts.iter().find(|attempt| attempt.number == shown.number)
        }) else {
            return Some(AttemptNode::Gone);
        };
        Some(match &attempt.source {
            AttemptSource::Loaded(job) => AttemptNode::Ready(job.clone()),
            AttemptSource::Lazy(target) => match session.attempt(run_id, target) {
                Some(live) => match (live.ready(), live.error()) {
                    (Some(job), _) => AttemptNode::Ready(Box::new(job.clone())),
                    (None, Some(message)) => AttemptNode::Failed(message.to_owned()),
                    (None, None) => AttemptNode::Loading,
                },
                None => AttemptNode::Loading,
            },
        })
    }

    /// The run's stages as the log page shows them: the earlier attempt on screen in place of
    /// its latest.
    fn view_stages<'a>(
        &self,
        stages: &'a [PipelineJob],
        attempt: Option<PipelineJob>,
    ) -> Cow<'a, [PipelineJob]> {
        match (&self.attempt, attempt) {
            (Some(shown), Some(job)) => {
                let mut view = stages.to_vec();
                replace_node(&mut view, &shown.node_id, job);
                Cow::Owned(view)
            }
            _ => Cow::Borrowed(stages),
        }
    }

    /// The run's jobs beside the open log, with a way back to the stages; the header above names
    /// the job and offers its reruns. Returns true when the job list was resized and the width
    /// should be saved.
    /// `stages` are the run's stages (latest attempts); an earlier attempt on screen takes its
    /// latest's place in the job list, and the switcher above the log moves between attempts. In
    /// focus mode the log (or the attempt on its way) is all there is: its strip takes the
    /// switcher, and J/K still step through the jobs.
    fn log_page(
        &mut self,
        ui: &mut Ui,
        stages: &[PipelineJob],
        run_id: &str,
        session: &mut PullRequestSession,
        services: &mut Services,
        is_focused: bool,
    ) -> bool {
        let attempt_state = self.attempt_node(stages, run_id, session);
        let attempt_job = match &attempt_state {
            Some(AttemptNode::Ready(job)) => Some((**job).clone()),
            _ => None,
        };
        let view = self.view_stages(stages, attempt_job.clone());
        let view: &[PipelineJob] = &view;
        let selected_job = self.log.target().map(|target| target.job_id.clone());
        let switch = self.switch(stages, selected_job.as_deref());
        let (is_resized, mut picks) = {
            let loaded = session.loaded_attempts(run_id);
            let known = Known { loaded: &loaded };
            let shown = selected_job.as_deref().and_then(|id| find(view, id));
            // Whether the platform serves the log yet depends on the job it belongs to (a GitHub
            // step's log is its job's).
            let owner = self
                .log
                .target()
                .and_then(|target| target.log_id.as_deref())
                .and_then(|log_id| log_owner(view, log_id));
            let pending = attempt_pending(attempt_state.as_ref(), switch);
            let pane = LogPane { pending, shown, owner };
            if is_focused {
                let picks = focused_log(ui, &mut self.log, &pane, switch, known, session, services);
                (false, picks)
            } else {
                // A step's log (Azure DevOps) is named here; a job's own log is named by the header.
                let step = shown.filter(|node| {
                    self.selected_job.as_deref() != Some(node.id.as_str())
                        && attempt_job.as_ref().is_none_or(|job| job.id != node.id)
                });
                let bar = log_bar(ui, step, switch, known, self.selected_job.as_deref());
                if bar.back {
                    self.show_stages();
                }
                let jobs = JobList {
                    stages: view,
                    selected: selected_job.as_deref(),
                    attempt: attempt_job
                        .as_ref()
                        .zip(self.attempt.as_ref())
                        .map(|(job, shown)| (job.id.as_str(), shown.number)),
                    known,
                };
                let (is_resized, mut picks) =
                    jobs_beside_log(ui, &mut self.log, &jobs, &pane, session, services);
                picks.attempt = picks.attempt.or(bar.attempt);
                (is_resized, picks)
            }
        };
        if let (Some(index), Some((node, number))) = (picks.pane, switch) {
            // Retry loads the attempt again; the other way out is the latest attempt.
            let latest = node.attempt_number();
            picks.attempt = Some((node.id.clone(), if index == 0 { number } else { latest }));
        }
        if let Some((node_id, number)) = picks.attempt {
            self.pick_attempt(&node_id, number, stages, run_id, session, services);
        }
        if let Some(target) = picks.job {
            // A row outside the earlier attempt leaves it for the latest.
            let is_in_attempt = attempt_job
                .as_ref()
                .is_some_and(|job| find(std::slice::from_ref(job), &target.job_id).is_some());
            if !is_in_attempt {
                self.attempt = None;
            }
            self.open_log(target, services);
        }
        is_resized
    }

    /// The node whose attempts the switcher lists: the one shown in an earlier attempt, else the
    /// open item's nearest retried stage or job.
    fn switch<'a>(&self, stages: &'a [PipelineJob], selected_job: Option<&str>) -> Switch<'a> {
        match &self.attempt {
            Some(shown) => find(stages, &shown.node_id).map(|node| (node, shown.number)),
            None => selected_job
                .and_then(|id| retried_ancestor(stages, id))
                .map(|node| (node, node.attempt_number())),
        }
    }

    fn open_log(&mut self, target: LogTarget, services: &Services) {
        self.page = Page::Log;
        if self.log.target() != Some(&target) {
            self.log.open(target, services);
        }
    }

    fn on_rerun_pick(
        &mut self,
        ui: &Ui,
        picked: Picked,
        run: &PipelineRun,
        session: &PullRequestSession,
        services: &mut Services,
    ) {
        match picked {
            Picked::Rerun(action) => {
                self.reruns.start(vec![(run.id.clone(), action)], session, services);
            }
            Picked::AddToken => intents::request_fix(ui.ctx(), &session.key.connection_id),
        }
    }
}

/// The live refresh of the pipelines, for every screen that shows them while it is shown: each
/// [`refresh::pipeline_poll_due`], the runs reload with the stages already loaded of the runs still
/// going (the head runs' where the checks show, a run the user opened). A run that finished since
/// loads its stages once more where they are shown ([`PullRequestSession::want_stages`]), so its
/// final state shows; then nothing more is asked.
pub fn follow_running(ctx: &egui::Context, session: &mut PullRequestSession, services: &Services) {
    let key = format!(
        "pipelines/{}/{}/{}",
        session.key.connection_id, session.key.repository_key, session.key.number
    );
    if refresh::pipeline_poll_due(ctx, &services.refresh, &key, &session.runs, session.has_token) {
        session.refresh_running_runs(services);
        // Stages of finished runs do not change any more; the answer loaded last stays.
        session.reload_running_stages(services);
    }
}

/// Every run and its jobs as far as their stages are known, in the table's order.
fn all_checks<'a>(runs: &'a [PipelineRun], session: &'a PullRequestSession) -> Vec<Check<'a>> {
    let mut checks =
        checks::list(runs, |run| session.stages(&run.id).and_then(Live::ready).map(Vec::as_slice));
    checks::sort(&mut checks);
    checks
}

/// The stage bars of a table row: a run's stages, a job's one bar.
fn bars_of<'a>(check: &Check<'_>, session: &'a PullRequestSession) -> Stages<'a> {
    match check.job {
        None => stages_of(check.run, session),
        Some((_, job)) => Stages::Job { status: job.status, is_manual: check.is_manual() },
    }
}

/// What is known of a run's stages, for its stage bars.
pub fn stages_of<'a>(run: &PipelineRun, session: &'a PullRequestSession) -> Stages<'a> {
    if !run.has_jobs {
        return Stages::Own(run.status);
    }
    match session.stages(&run.id) {
        Some(load) => match (load.ready(), load.error()) {
            (Some(stages), _) => Stages::Ready(stages),
            (None, Some(_)) => Stages::Unavailable(run.status),
            (None, None) => Stages::Loading,
        },
        None if run.is_head => Stages::Loading,
        None => Stages::OnDemand(run.status),
    }
}

fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// Elapsed time; still-running items count up to now.
pub fn duration(started_at: Option<Timestamp>, finished_at: Option<Timestamp>) -> Option<String> {
    let start = started_at?;
    let end = finished_at.unwrap_or_else(time::OffsetDateTime::now_utc);
    Some(time_text::duration(start, end))
}

#[cfg(test)]
mod tests {
    use super::{Page, PendingLog, PipelinesTab};

    #[test]
    fn opening_another_run_drops_the_log_still_waiting_for_the_first() {
        let mut tab = PipelinesTab::default();
        // "Logs" on run A while its stages load, then back to the checks.
        tab.open_run("a");
        tab.pending_log = Some(PendingLog::FirstFailed);
        tab.show_checks();
        tab.open_run("b");
        assert!(tab.pending_log.is_none(), "run B opens at its stages, not at a log");
        assert_eq!(tab.page, Page::Run);
    }
}
// CI probe, never merged.
