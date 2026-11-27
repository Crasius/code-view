//! Local checkouts of pull requests: the records of what is checked out where (`checkouts.json`),
//! the checkout jobs that run ([`job`]), keeping checkouts current and removing worktrees
//! ([`upkeep`]), what happens once a pull request is merged or closed elsewhere ([`finish`]), and
//! finding the user's clones ([`clones`]). Every git and disk access runs on the
//! [`Background`]; the pull request page, the title bar and Settings read the state each frame,
//! and [`poll`] moves finished work into it.
//!
//! What opens after a checkout (a terminal, Claude Code, Codex) starts through the
//! [`AgentDirectory`](crate::agents::AgentDirectory), so test builds record it instead.

mod clones;
mod finish;
mod job;
#[cfg(test)]
mod open_tests;
mod upkeep;

use std::collections::HashMap;
use std::sync::Arc;

use review_git::GitMirror;
use review_git::checkout::Changes;
use review_store::AppPaths;
use review_store::checkouts::{
    CheckoutKind, CheckoutRecord, CheckoutRecords, CheckoutSettings, OpenWith,
};

pub(crate) use self::clones::KnownRepository;
pub use self::clones::{FolderPurpose, Found, Query};
pub(crate) use self::finish::Offer;
pub use self::job::{Failure, Job, JobPhase, JobProgress, Place, Plan, StepState};
pub use self::upkeep::{RemovalState, Status, reason};
use cv_base::platform;
use cv_runtime::background::Background;
use cv_runtime::load::Load;
use cv_runtime::route::PullRequestKey;
use cv_runtime::toasts::ToastAction;
use cv_widgets::progress_log::redact;

use crate::{Services, agent_names};

pub struct Checkouts {
    records: CheckoutRecords,
    jobs: HashMap<PullRequestKey, Job>,
    upkeep: upkeep::Upkeep,
    finish: finish::Finish,
    clones: clones::Clones,
}

impl Checkouts {
    /// The saved records; none (and why) when the file cannot be read. Their log lines are
    /// redacted once here, for a file an earlier build wrote with raw lines; lines added later are
    /// redacted where they are built. A failed save of the redacted file is the message then.
    pub(crate) fn open(paths: &AppPaths) -> (Self, Option<String>) {
        let file = paths.checkouts_file();
        let (records, error) = match CheckoutRecords::load(file.clone()) {
            Ok(mut records) => {
                let saved = records.rewrite_log_lines(redact);
                (records, saved.err().map(|error| save_message(&error)))
            }
            Err(error) => (
                CheckoutRecords::empty(file),
                Some(format!("Could not read the list of local checkouts: {error}")),
            ),
        };
        let checkouts = Self {
            records,
            jobs: HashMap::new(),
            upkeep: upkeep::Upkeep::default(),
            finish: finish::Finish::default(),
            clones: clones::Clones::default(),
        };
        (checkouts, error)
    }

    pub fn record(&self, key: &PullRequestKey) -> Option<&CheckoutRecord> {
        self.records.find(&key.connection_id, &key.repository_key, key.number)
    }

    pub fn records(&self) -> &CheckoutRecords {
        &self.records
    }

    pub fn job(&self, key: &PullRequestKey) -> Option<&Job> {
        self.jobs.get(key)
    }

    /// The checkouts that run now, for the title bar.
    pub fn running(&self) -> impl Iterator<Item = (&PullRequestKey, &Job)> {
        self.jobs.iter().filter(|(_, job)| job.is_running())
    }

    /// Starts the checkout `plan` describes, unless one runs for its pull request already.
    pub fn start(&mut self, plan: Plan, background: &Background) {
        if self.jobs.get(&plan.key).is_some_and(Job::is_running) || self.is_busy(&plan.key) {
            return;
        }
        let key = plan.key.clone();
        self.jobs.insert(key, Job::start(plan, background));
    }

    pub fn cancel(&mut self, key: &PullRequestKey) {
        if let Some(job) = self.jobs.get_mut(key) {
            job.cancel();
        }
    }

    /// Forgets a failed checkout (Cancel on its card).
    pub fn dismiss(&mut self, key: &PullRequestKey) {
        if self.jobs.get(key).is_some_and(|job| !job.is_running()) {
            self.jobs.remove(key);
        }
    }

    pub fn status(&self, key: &PullRequestKey) -> Option<&Status> {
        self.upkeep.status(key)
    }

    /// The pull request's head moved to `head` (the page saw it): the checkout follows it silently
    /// where that is safe. Once per head; never while its checkout or another update runs.
    pub fn follow_head(
        &mut self,
        key: &PullRequestKey,
        mirror: &Arc<GitMirror>,
        head: &str,
        background: &Background,
    ) {
        let Some(record) = self.record(key) else {
            return;
        };
        if record.head == head
            || self.jobs.contains_key(key)
            || self.is_busy(key)
            || !self.upkeep.wants_update(key, head)
        {
            return;
        }
        let record = record.clone();
        self.upkeep.update(&record, key, mirror.clone(), head, background);
    }

    /// "Update checkout" and "Update it": tries again for `head`, whatever was tried before.
    pub fn update_now(
        &mut self,
        key: &PullRequestKey,
        mirror: &Arc<GitMirror>,
        head: &str,
        background: &Background,
    ) {
        let Some(record) = self.record(key).cloned() else {
            return;
        };
        if self.jobs.get(key).is_some_and(Job::is_running) || self.is_busy(key) {
            return;
        }
        self.upkeep.update(&record, key, mirror.clone(), head, background);
    }

    /// Removes the checkout's worktree (worktrees the app made only); `changes` is
    /// [`Changes::Discard`] after the second yes for a worktree with changes.
    pub fn remove(&mut self, key: &PullRequestKey, changes: Changes, background: &Background) {
        let Some(record) = self.record(key).cloned() else {
            return;
        };
        if record.kind != CheckoutKind::Worktree || self.is_busy(key) {
            return;
        }
        self.upkeep.remove(&record, key, changes, background);
    }

    /// An update or a removal runs for the checkout.
    fn is_busy(&self, key: &PullRequestKey) -> bool {
        self.upkeep.is_busy(key) || self.finish.is_busy(key)
    }

    /// The pull request is done and its page offers to remove the worktree.
    pub fn offer(&self, key: &PullRequestKey) -> Option<&Offer> {
        self.finish.offer(key)
    }

    pub fn removal(&self, key: &PullRequestKey) -> Option<&RemovalState> {
        self.upkeep.removal(key)
    }

    /// Keeps the worktree after its removal asked a second time or failed.
    pub fn keep(&mut self, key: &PullRequestKey) {
        self.upkeep.keep(key);
    }

    /// Records a working tree that has the pull request's branch already ("Open it").
    pub fn adopt(&mut self, key: &PullRequestKey, adopted: Adopted) -> std::io::Result<()> {
        self.records.put(CheckoutRecord {
            connection_id: key.connection_id.clone(),
            repository_key: key.repository_key.clone(),
            number: key.number,
            clone: adopted.clone,
            path: adopted.path,
            kind: CheckoutKind::Adopted,
            branch: adopted.branch,
            is_own_branch: false,
            pull_ref: pull_ref(key.number),
            head: adopted.head,
            last_opened: OpenWith::Nothing,
            took_ms: 0,
            merged_here: false,
            log: Vec::new(),
        })
    }

    pub fn found(&self, key: &PullRequestKey) -> Option<&Load<Found>> {
        self.clones.found(key)
    }

    /// Looks for the clone of `key`'s repository as `query` says.
    pub fn look_up(&mut self, key: &PullRequestKey, query: Query, background: &Background) {
        self.clones.look_up(key, query, background);
    }

    /// Repositories looked up in this app run, by their settings key.
    pub fn repositories(&self) -> &std::collections::BTreeMap<String, KnownRepository> {
        self.clones.repositories()
    }

    /// The default search folders (those that exist), once they are known.
    pub fn default_folders(&mut self, background: &Background) -> Option<&Vec<std::path::PathBuf>> {
        self.clones.defaults(background)
    }

    pub fn ask_folder(
        &mut self,
        purpose: FolderPurpose,
        title: &'static str,
        background: &Background,
    ) {
        self.clones.ask_folder(purpose, title, background);
    }

    pub fn is_asking_folder(&self) -> bool {
        self.clones.is_asking()
    }

    pub fn take_picked(&mut self, purpose: &FolderPurpose) -> Option<std::path::PathBuf> {
        self.clones.take_picked(purpose)
    }

    /// The next folder dialog answers `answer` (`None`: cancelled).
    #[cfg(any(test, feature = "test-support"))]
    pub fn answer_folder_dialog(&self, answer: Option<std::path::PathBuf>) {
        self.clones.answer_folder_dialog(answer);
    }
}

/// A working tree found with the pull request's branch.
pub struct Adopted {
    pub clone: std::path::PathBuf,
    pub path: std::path::PathBuf,
    pub branch: String,
    pub head: String,
}

/// The key of `key`'s repository in [`CheckoutSettings`] (pinned clones, remembered choices).
pub fn settings_key(key: &PullRequestKey) -> String {
    CheckoutSettings::repository_key(&key.connection_id, &key.repository_key)
}

/// "Claude Code", "Codex", or the terminal's name ("Windows Terminal").
pub fn tool_name(with: OpenWith, settings: &CheckoutSettings) -> String {
    match with {
        OpenWith::Nothing => "Nothing".to_owned(),
        OpenWith::Terminal => terminal_name(settings),
        OpenWith::Agent(kind) => agent_names::cli_name(kind).to_owned(),
    }
}

/// Where the clone keeps the pull request's head.
fn pull_ref(number: u64) -> String {
    format!("refs/codeview/pull/{number}/head")
}

/// The name of the terminal Settings chose, as the platform calls it.
pub fn terminal_name(settings: &CheckoutSettings) -> String {
    platform::TERMINALS
        .iter()
        .find(|(choice, _)| *choice == settings.terminal)
        .or(platform::TERMINALS.first())
        .map_or_else(|| "Terminal".to_owned(), |(_, name)| (*name).to_owned())
}

/// Moves finished checkouts, updates, removals, lookups and folder dialogs into the state; a
/// finished checkout opens what it was asked to. Call once per frame.
pub(crate) fn poll(services: &mut Services) {
    services.checkouts.clones.poll();
    poll_finished(services);
    let changes = services.checkouts.upkeep.poll();
    for change in changes {
        let records = &mut services.checkouts.records;
        let saved = match change {
            upkeep::Change::Updated { key, head, log } => records
                .change(&key.connection_id, &key.repository_key, key.number, |record| {
                    record.head = head;
                    record.append_log(log);
                })
                .map(drop),
            upkeep::Change::Stayed { key, log } => records
                .change(&key.connection_id, &key.repository_key, key.number, |record| {
                    record.append_log(log);
                })
                .map(drop),
            upkeep::Change::Removed(key) => {
                records.remove(&key.connection_id, &key.repository_key, key.number)
            }
        };
        if let Err(error) = saved {
            save_failed(services, &error);
        }
    }
    let mut opened = Vec::new();
    let checkouts = &mut services.checkouts;
    for (key, job) in &mut checkouts.jobs {
        if let Some(job::Finished { done, took, log }) = job.poll() {
            let plan = &job.plan;
            let record = CheckoutRecord {
                connection_id: key.connection_id.clone(),
                repository_key: key.repository_key.clone(),
                number: key.number,
                clone: plan.clone.clone(),
                path: done.path,
                kind: match plan.place {
                    Place::Worktree(_) => CheckoutKind::Worktree,
                    Place::Clone => CheckoutKind::Clone,
                    Place::NewClone { .. } => CheckoutKind::NewClone,
                },
                branch: done.branch,
                is_own_branch: done.is_new_branch,
                pull_ref: plan.pull_ref(),
                head: plan.head.clone(),
                last_opened: OpenWith::Nothing,
                took_ms: u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
                merged_here: false,
                log,
            };
            opened.push((key.clone(), record, plan.then));
        }
    }
    for (key, record, then) in opened {
        services.checkouts.jobs.remove(&key);
        if record.kind == CheckoutKind::NewClone {
            // The new clone is the repository's clone from now on, wherever it was put.
            let pinned = &mut services.settings.checkouts.pinned_clones;
            pinned.insert(settings_key(&key), record.clone.clone());
            services.save_settings();
        }
        if let Err(error) = services.checkouts.records.put(record) {
            save_failed(services, &error);
        }
        open_with(services, &key, then);
    }
}

/// The pull request `key` was seen merged or closed (its page, or the state asked at start): its
/// checkout gets what Settings says ([`finish`]). Once per pull request and app run; not while
/// its checkout, an update or a removal runs.
pub fn finished(services: &mut Services, key: &PullRequestKey) {
    let checkouts = &mut services.checkouts;
    let Some(record) = checkouts.record(key).cloned() else {
        return;
    };
    if checkouts.finish.is_handled(key)
        || checkouts.jobs.contains_key(key)
        || checkouts.is_busy(key)
    {
        return;
    }
    checkouts.finish.handled(key);
    match finish::decide(services.settings.checkouts.when_finished, &record) {
        finish::Decision::Remove => checkouts.finish.remove(&record, key, &services.background),
        finish::Decision::Ask => checkouts.finish.ask(key),
        finish::Decision::Keep => {}
        finish::Decision::Forget => {
            checkouts.upkeep.forget(key);
            if let Err(error) =
                checkouts.records.remove(&key.connection_id, &key.repository_key, key.number)
            {
                save_failed(services, &error);
            }
        }
    }
}

/// The pull request `key` is being merged from Code View: once it shows merged, its page asks
/// before the worktree goes, whatever Settings says for merges elsewhere.
pub fn merging_here(services: &mut Services, key: &PullRequestKey) {
    if services.checkouts.record(key).is_none_or(|record| record.merged_here) {
        return;
    }
    let changed = services.checkouts.records.change(
        &key.connection_id,
        &key.repository_key,
        key.number,
        |record| record.merged_here = true,
    );
    if let Err(error) = changed {
        save_failed(services, &error);
    }
}

/// At start, asks the state of every recorded pull request once; applies what came in finished
/// and what the removals of finished pull requests did.
fn poll_finished(services: &mut Services) {
    if !services.checkouts.finish.has_asked_states() {
        let records = services.checkouts.records.all().to_vec();
        let mut providers = HashMap::new();
        for record in &records {
            let id = record.connection_id.clone();
            if let std::collections::hash_map::Entry::Vacant(entry) = providers.entry(id) {
                let provider = services.provider(entry.key());
                entry.insert(provider);
            }
        }
        let provider_of = |id: &str| providers.get(id).cloned().flatten();
        services.checkouts.finish.ask_states(&records, provider_of, &services.background);
    }
    for key in services.checkouts.finish.finished_states() {
        finished(services, &key);
    }
    for change in services.checkouts.finish.poll() {
        let records = &mut services.checkouts.records;
        let saved = match change {
            finish::Change::Removed(key) => {
                services.checkouts.upkeep.forget(&key);
                records.remove(&key.connection_id, &key.repository_key, key.number)
            }
            finish::Change::Kept { key, line } => records
                .change(&key.connection_id, &key.repository_key, key.number, |record| {
                    record.append_log([line]);
                })
                .map(drop),
        };
        if let Err(error) = saved {
            save_failed(services, &error);
        }
    }
}

/// Opens `with` in the checkout's folder and remembers it as the last used; a failure toasts with
/// the folder as the way on.
pub fn open_with(services: &mut Services, key: &PullRequestKey, with: OpenWith) {
    let Some(record) = services.checkouts.record(key) else {
        return;
    };
    let path = record.path.clone();
    let opened = match with {
        OpenWith::Nothing => return,
        OpenWith::Terminal => services
            .agents
            .open_shell(&path, services.settings.checkouts.terminal)
            .map_err(|error| error.to_string()),
        OpenWith::Agent(kind) => match services.agents.cli(kind, &services.background) {
            Some(cli) => services
                .agents
                .open_terminal(&path, &cli.program.to_string_lossy(), &[])
                .map_err(|error| error.to_string()),
            None => Err(format!("{} is not installed", agent_names::cli_name(kind))),
        },
    };
    if let Err(error) = opened {
        let name = tool_name(with, &services.settings.checkouts);
        let folder = ToastAction::open_folder(&path);
        services.toasts.error(format!("Could not open {name}: {error}"), folder);
        return;
    }
    let changed = services.checkouts.records.change(
        &key.connection_id,
        &key.repository_key,
        key.number,
        |record| record.last_opened = with,
    );
    if let Err(error) = changed {
        save_failed(services, &error);
    }
    if services.settings.checkouts.last_opened != with {
        services.settings.checkouts.last_opened = with;
        services.save_settings();
    }
}

fn save_failed(services: &mut Services, error: &std::io::Error) {
    let folder = ToastAction::open_folder(services.paths.config_dir());
    services.toasts.error(save_message(error), folder);
}

fn save_message(error: &std::io::Error) -> String {
    format!("Could not save the list of local checkouts: {error}")
}
