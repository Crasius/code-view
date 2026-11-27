//! One checkout while it runs: its steps with their durations, its live log, the way to cancel it,
//! and how it ended. The git work runs on the [`Background`]; the page reads the shared progress
//! each frame. A failed or cancelled run takes away what it made (`review_git::checkout::undo`)
//! before it reports.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use review_git::GitMirror;
use review_git::checkout::{self, CheckoutError, Done, Event, Made, Request, Target, Upstream};
use review_store::checkouts::OpenWith;
use tokio::sync::oneshot;

use cv_runtime::background::{Background, TASK_LOST, Task, TaskPoll};
use cv_runtime::route::PullRequestKey;
use cv_widgets::progress_log::{LineKind, ProgressLog, format_duration};

/// Where a checkout goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// A new worktree of the clone, in this folder.
    Worktree(PathBuf),
    /// The clone itself switches to the branch.
    Clone,
    /// A new clone from the mirror, its `origin` set to this URL (without credentials).
    NewClone { origin: String },
}

/// Everything a checkout needs, decided on the page.
#[derive(Clone)]
pub struct Plan {
    pub key: PullRequestKey,
    pub mirror: Arc<GitMirror>,
    /// The user's clone, or the folder a new clone goes to.
    pub clone: PathBuf,
    pub place: Place,
    pub head: String,
    pub branch: String,
    pub upstream: Option<Upstream>,
    /// What opens once the checkout is done.
    pub then: OpenWith,
    /// The name of what opens ("Claude Code"), for the last step's label.
    pub then_name: Option<String>,
}

impl Plan {
    /// Where the clone keeps the pull request's head.
    pub(crate) fn pull_ref(&self) -> String {
        super::pull_ref(self.key.number)
    }
}

/// A checkout's steps as the progress view lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStep {
    FindClone,
    Fetch,
    CreateWorktree,
    CheckOut,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Waiting,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub struct StepRow {
    pub step: JobStep,
    pub label: String,
    pub state: StepState,
    pub took: Option<Duration>,
}

/// Written by the checkout's task, read by the page each frame.
pub struct JobProgress {
    pub steps: Vec<StepRow>,
    pub log: ProgressLog,
    started: Instant,
    step_started: Instant,
    /// Set once the run ended; freezes the elapsed time.
    ended_after: Option<Duration>,
}

impl JobProgress {
    fn new(plan: &Plan, now: Instant) -> Self {
        let folder = |path: &PathBuf| {
            path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            )
        };
        let mut steps = vec![match &plan.place {
            Place::NewClone { .. } => {
                row(JobStep::FindClone, format!("Create {}", folder(&plan.clone)))
            }
            Place::Worktree(_) | Place::Clone => row(JobStep::FindClone, "Find your clone".into()),
        }];
        steps.push(row(JobStep::Fetch, format!("Fetch #{} from the mirror", plan.key.number)));
        if let Place::Worktree(path) = &plan.place {
            steps.push(row(JobStep::CreateWorktree, format!("Create worktree {}", folder(path))));
        }
        steps.push(row(JobStep::CheckOut, format!("Check out {}", plan.branch)));
        if let Some(name) = &plan.then_name {
            steps.push(row(JobStep::Open, format!("Open {name}")));
        }
        Self {
            steps,
            log: ProgressLog::default(),
            started: now,
            step_started: now,
            ended_after: None,
        }
    }

    /// The running step, or the one that failed.
    pub fn current(&self) -> Option<&StepRow> {
        self.steps.iter().find(|row| matches!(row.state, StepState::Running | StepState::Failed))
    }

    pub fn done_count(&self) -> usize {
        self.steps.iter().filter(|row| row.state == StepState::Done).count()
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        self.ended_after.unwrap_or_else(|| now.saturating_duration_since(self.started))
    }

    pub fn step_elapsed(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.step_started)
    }

    pub(crate) fn is_ended(&self) -> bool {
        self.ended_after.is_some()
    }

    fn enter(&mut self, step: JobStep, now: Instant) {
        if self.is_ended() || self.current().is_some_and(|row| row.step == step) {
            return;
        }
        self.finish_running(now);
        if let Some(row) = self.steps.iter_mut().find(|row| row.step == step) {
            row.state = StepState::Running;
            self.log.push(LineKind::Step, &format!("{}…", row.label));
        }
        self.step_started = now;
    }

    fn finish_running(&mut self, now: Instant) {
        let took = now.saturating_duration_since(self.step_started);
        if let Some(row) = self.steps.iter_mut().find(|row| row.state == StepState::Running) {
            row.state = StepState::Done;
            row.took = Some(took);
            self.log
                .push(LineKind::Step, &format!("{} done in {}", row.label, format_duration(took)));
        }
    }

    /// The checkout itself is done; the Open step runs once the record is saved.
    fn succeed(&mut self, now: Instant) {
        self.finish_running(now);
        self.ended_after = Some(now.saturating_duration_since(self.started));
    }

    fn fail(&mut self, message: &str, now: Instant) {
        if self.is_ended() {
            return;
        }
        let took = now.saturating_duration_since(self.step_started);
        if let Some(row) = self.steps.iter_mut().find(|row| row.state == StepState::Running) {
            row.state = StepState::Failed;
            row.took = Some(took);
            let line = format!("{} failed after {}", row.label, format_duration(took));
            self.log.push(LineKind::Error, &line);
        }
        for line in message.lines().filter(|line| !line.trim().is_empty()) {
            self.log.push(LineKind::Error, &format!("error: {line}"));
        }
        self.ended_after = Some(now.saturating_duration_since(self.started));
    }
}

fn row(step: JobStep, label: String) -> StepRow {
    StepRow { step, label, state: StepState::Waiting, took: None }
}

/// Why a checkout stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The clone has changed tracked files (porcelain lines).
    Dirty(Vec<String>),
    /// The branch is checked out in this working tree already.
    CheckedOut(PathBuf),
    /// The user cancelled.
    Cancelled,
    /// Anything else: a title naming what failed ("git worktree add failed") and git's message.
    Git { title: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobPhase {
    Running,
    Failed(Failure),
}

enum Ended {
    Done(Done),
    Failed(CheckoutError),
    Cancelled,
}

/// A checkout that runs, or that failed and waits to be retried or dismissed.
pub struct Job {
    pub plan: Plan,
    progress: Arc<Mutex<JobProgress>>,
    task: Option<Task<Ended>>,
    cancel: Option<oneshot::Sender<()>>,
    phase: JobPhase,
}

/// A checkout that succeeded this frame.
pub(crate) struct Finished {
    pub done: Done,
    pub took: Duration,
    pub log: Vec<String>,
}

impl Job {
    pub(crate) fn start(plan: Plan, background: &Background) -> Self {
        let progress = Arc::new(Mutex::new(JobProgress::new(&plan, Instant::now())));
        let (cancel, cancelled) = oneshot::channel();
        let request = Request {
            clone: plan.clone.clone(),
            target: match &plan.place {
                Place::Worktree(path) => Target::Worktree { path: path.clone() },
                Place::Clone => Target::Clone,
                Place::NewClone { origin } => Target::NewClone { origin: origin.clone() },
            },
            head: plan.head.clone(),
            branch: plan.branch.clone(),
            upstream: plan.upstream.clone(),
            pull_ref: plan.pull_ref(),
        };
        let mirror = plan.mirror.clone();
        let shared = progress.clone();
        let ctx = background.ctx();
        let task = background.spawn(async move {
            let events = move |event: Event| {
                let mut progress = lock(&shared);
                match event {
                    Event::Step(step) => progress.enter(job_step(step), Instant::now()),
                    Event::Git(event) => progress.log.report_git(event),
                }
                ctx.request_repaint();
            };
            let made = Mutex::new(Made::default());
            let ended = tokio::select! {
                result = checkout::check_out(&mirror, &request, &events, &made) => match result {
                    Ok(done) => Ended::Done(done),
                    Err(error) => Ended::Failed(error),
                },
                _cancelled = cancelled => Ended::Cancelled,
            };
            if !matches!(ended, Ended::Done(_)) {
                let made =
                    std::mem::take(&mut *made.lock().unwrap_or_else(PoisonError::into_inner));
                for line in checkout::undo(made).await {
                    events(Event::Git(review_git::GitEvent::Message(line)));
                }
            }
            ended
        });
        Self { plan, progress, task: Some(task), cancel: Some(cancel), phase: JobPhase::Running }
    }

    pub fn progress(&self) -> MutexGuard<'_, JobProgress> {
        lock(&self.progress)
    }

    pub fn phase(&self) -> &JobPhase {
        &self.phase
    }

    pub fn is_running(&self) -> bool {
        self.phase == JobPhase::Running
    }

    /// Stops the git work; what it made is taken away before the job reports "Cancelled".
    pub(crate) fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            // The task may have ended a moment ago; then there is nothing left to stop.
            let _already_ended = cancel.send(());
        }
    }

    /// The run's result once it succeeded; a failure shows in [`Job::phase`].
    pub(crate) fn poll(&mut self) -> Option<Finished> {
        let task = self.task.as_mut()?;
        let ended = match task.poll() {
            TaskPoll::Pending => return None,
            TaskPoll::Done(ended) => Some(ended),
            TaskPoll::Lost => None,
        };
        self.task = None;
        self.cancel = None;
        let now = Instant::now();
        let failure = match ended {
            Some(Ended::Done(done)) => {
                let mut progress = self.progress();
                progress.succeed(now);
                let took = progress.elapsed(now);
                let log = progress.log.text().lines().map(str::to_owned).collect();
                return Some(Finished { done, took, log });
            }
            Some(Ended::Cancelled) => Failure::Cancelled,
            Some(Ended::Failed(error)) => self.failure(error),
            None => {
                Failure::Git { title: "The checkout stopped".into(), message: TASK_LOST.into() }
            }
        };
        let message = match &failure {
            Failure::Git { message, .. } => message.clone(),
            Failure::Cancelled => "Cancelled".into(),
            Failure::Dirty(files) => format!("{} uncommitted files", files.len()),
            Failure::CheckedOut(path) => format!("checked out in {}", path.display()),
        };
        self.progress().fail(&message, now);
        self.phase = JobPhase::Failed(failure);
        None
    }

    fn failure(&self, error: CheckoutError) -> Failure {
        match error {
            CheckoutError::Dirty(files) => Failure::Dirty(files),
            CheckoutError::CheckedOut(path) => Failure::CheckedOut(path),
            other => {
                let step = self.progress().current().map(|row| row.step);
                let is_new_clone = matches!(self.plan.place, Place::NewClone { .. });
                let title = match step {
                    Some(JobStep::FindClone) if is_new_clone => "git init failed",
                    Some(JobStep::FindClone) => "Checking your clone failed",
                    Some(JobStep::Fetch) => "Fetching from the mirror failed",
                    Some(JobStep::CreateWorktree) => "git worktree add failed",
                    Some(JobStep::CheckOut | JobStep::Open) | None => "git checkout failed",
                };
                Failure::Git { title: title.into(), message: other.to_string() }
            }
        }
    }
}

fn job_step(step: checkout::Step) -> JobStep {
    match step {
        checkout::Step::FindClone => JobStep::FindClone,
        checkout::Step::Fetch => JobStep::Fetch,
        checkout::Step::CreateWorktree => JobStep::CreateWorktree,
        checkout::Step::CheckOut => JobStep::CheckOut,
    }
}

fn lock(progress: &Mutex<JobProgress>) -> MutexGuard<'_, JobProgress> {
    // A panic while holding the lock leaves at worst a half-written line.
    progress.lock().unwrap_or_else(PoisonError::into_inner)
}
