use super::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum RunStatus {
    #[default]
    Queued,
    Running,
    Succeeded,
    Failed,
    Canceled,
    Skipped,
    /// Finished without pass/fail meaning (GitHub neutral, Azure DevOps "notApplicable", warnings).
    Neutral,
}

impl RunStatus {
    pub fn is_finished(self) -> bool {
        !matches!(self, RunStatus::Queued | RunStatus::Running)
    }

    /// The status of a group (a stage of jobs): failure, then activity, then cancellation win;
    /// success beats skipped and neutral; an empty group is neutral.
    pub fn combined(statuses: impl IntoIterator<Item = RunStatus>) -> RunStatus {
        const PRECEDENCE: [RunStatus; 6] = [
            RunStatus::Failed,
            RunStatus::Running,
            RunStatus::Queued,
            RunStatus::Canceled,
            RunStatus::Succeeded,
            RunStatus::Skipped,
        ];
        // One pass: the strongest status seen wins; neutral ones rank below everything listed.
        statuses
            .into_iter()
            .filter_map(|status| PRECEDENCE.iter().position(|wanted| *wanted == status))
            .min()
            .and_then(|rank| PRECEDENCE.get(rank).copied())
            .unwrap_or(RunStatus::Neutral)
    }
}

/// What a platform's log API serves for a log whose job (or task) is still running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunningLogs {
    /// Nothing until the job finished: the log is one file the platform writes at the end
    /// (GitHub: `GET /actions/jobs/{id}/logs` redirects to a file that exists only then). The
    /// platform's own page shows the job live.
    WhenFinished,
    /// What the job has written so far, longer on each request (GitLab's job trace, an Azure DevOps
    /// log the agent already uploaded). Fetching it again shows the job live.
    Growing,
}

/// A pipeline run or a status check of the pull request: those of its head commit, and older runs
/// where the platform lists them with it (GitLab pipelines of earlier commits, an Azure DevOps
/// definition's latest build of an earlier push, a GitHub workflow run triggered again since).
/// GitHub workflow runs / check runs / commit statuses, GitLab pipelines / external statuses,
/// Azure DevOps builds / PR statuses all map here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PipelineRun {
    pub id: String,
    pub name: String,
    pub status: RunStatus,
    pub description: Option<String>,
    pub web_url: Option<String>,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    /// True when [`crate::Provider::pipeline_jobs`] returns something for this run.
    pub has_jobs: bool,
    /// The run is one of the pull request's current runs: it ran on the head commit (where the
    /// platform names no commit, it is the newest run). Only these runs' jobs load with the
    /// Pipelines tab; an older run's load when the user opens it, as each costs requests from the
    /// platform's rate limit.
    pub is_head: bool,
    /// Reruns of the whole run the platform documents and the run's state allows.
    pub reruns: Vec<RerunAction>,
}

/// Stage, job or step. Leaves (and some inner nodes) carry a `log_id` for [`crate::Provider::download_log`].
///
/// The top level of [`crate::Provider::pipeline_jobs`] are the stages; their children the jobs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PipelineJob {
    pub id: String,
    pub name: String,
    pub status: RunStatus,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub log_id: Option<String>,
    pub web_url: Option<String>,
    pub children: Vec<PipelineJob>,
    /// Ids of the nodes on the same level this node follows: for a stage, the stages before it in
    /// the platform's stage graph (stages without any start in the first column); for a job, the
    /// jobs it explicitly needs, which may sit in other stages (GitLab `needs`). Empty on a job
    /// that is scheduled by its stage.
    pub depends_on: Vec<String>,
    /// Reruns of this node the platform documents and its state allows.
    pub reruns: Vec<RerunAction>,
    /// The platform's number of the attempt this node shows (Azure DevOps "Attempt #2", the
    /// GitHub run attempt, the n-th try of a GitLab job). 0 where the platform reports none,
    /// which reads as the first attempt.
    pub attempt: u32,
    /// The attempts before this one, oldest first; empty when the node ran once. The node itself
    /// is always the latest attempt, so statuses and counts stay those of the latest attempt.
    pub previous_attempts: Vec<PipelineAttempt>,
}

impl PipelineJob {
    /// The attempt this node shows, from 1.
    pub fn attempt_number(&self) -> u32 {
        self.attempt.max(1)
    }

    /// Attempts in total: the earlier ones and this one.
    pub fn attempt_count(&self) -> usize {
        self.previous_attempts.len() + 1
    }
}

/// An earlier attempt of a stage or job.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineAttempt {
    /// The platform's number of the attempt, from 1.
    pub number: u32,
    pub source: AttemptSource,
}

/// Where an earlier attempt's stage or job comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum AttemptSource {
    /// Reported with the run (GitHub, GitLab): the attempt's node with its steps, times and logs.
    /// It offers no reruns and lists no attempts of its own.
    Loaded(Box<PipelineJob>),
    /// Listed by id only (Azure DevOps): [`crate::Provider::pipeline_attempt`] loads the node with
    /// this opaque target when the user opens the attempt.
    Lazy(String),
}

impl PipelineAttempt {
    /// The attempt's node when the platform reported it with the run.
    pub fn job(&self) -> Option<&PipelineJob> {
        match &self.source {
            AttemptSource::Loaded(job) => Some(job),
            AttemptSource::Lazy(_) => None,
        }
    }

    /// The target for [`crate::Provider::pipeline_attempt`] when the attempt still has to be loaded.
    pub fn lazy_target(&self) -> Option<&str> {
        match &self.source {
            AttemptSource::Loaded(_) => None,
            AttemptSource::Lazy(target) => Some(target),
        }
    }
}

/// Makes `job` the node of an earlier attempt: no reruns (the platforms rerun the latest attempt
/// only) and no attempts of its own, in the whole subtree.
pub fn as_earlier_attempt(mut job: PipelineJob, number: u32) -> PipelineJob {
    fn strip(job: &mut PipelineJob) {
        job.reruns.clear();
        job.previous_attempts.clear();
        job.children.iter_mut().for_each(strip);
    }
    strip(&mut job);
    job.attempt = number;
    job
}

/// What a rerun covers. Labels are the UI's business; the platform terms are in the docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RerunKind {
    /// Every job of a stage again (Azure DevOps "Rerun just this stage").
    StageAll,
    /// The failed, canceled and skipped jobs of a stage (Azure DevOps "Retry stage").
    StageFailed,
    /// One job (GitHub "Re-run job", which also reruns the jobs that need it; GitLab "Retry job").
    Job,
    /// The failed jobs of the whole run (GitHub "Re-run failed jobs", GitLab "Retry pipeline").
    RunFailed,
    /// Every job of the run (GitHub "Re-run all jobs").
    RunAll,
}

/// A rerun offered on a run, stage or job; passed back to [`crate::Provider::rerun`] as is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RerunAction {
    pub kind: RerunKind,
    /// Opaque to the app: what the provider needs to address the call.
    pub target: String,
}

impl RerunAction {
    pub fn new(kind: RerunKind, target: impl Into<String>) -> Self {
        Self { kind, target: target.into() }
    }
}

/// Every stage waits for the one before it: the fallback where a platform does not report
/// dependencies.
pub fn chain_sequentially(stages: &mut [PipelineJob]) {
    let ids: Vec<String> = stages.iter().map(|stage| stage.id.clone()).collect();
    for (index, stage) in stages.iter_mut().enumerate() {
        stage.depends_on =
            index.checked_sub(1).map(|previous| ids[previous].clone()).into_iter().collect();
    }
}

/// Derives stage dependencies from start and finish times where the API reports no graph.
///
/// A stage that started waits for the stages that had finished by then, reduced to the latest ones
/// (a stage that finished before another of them started is implied through that one). A stage that
/// started before anything finished has no dependency. Stages that never started (pending, or
/// skipped without times) wait for the stage listed before them, the platforms' default order.
/// Timing can add an edge the definition does not have (a parallel stage that happened to finish
/// first), but it never places a stage before one it waited for.
pub fn infer_dependencies_from_timing(stages: &mut [PipelineJob]) {
    let times: Vec<(Option<Timestamp>, Option<Timestamp>)> = stages
        .iter()
        .map(|stage| (stage.started_at, stage.finished_at.filter(|_| stage.status.is_finished())))
        .collect();
    let ids: Vec<String> = stages.iter().map(|stage| stage.id.clone()).collect();
    for (index, stage) in stages.iter_mut().enumerate() {
        let Some(started) = times[index].0 else {
            stage.depends_on =
                index.checked_sub(1).map(|previous| ids[previous].clone()).into_iter().collect();
            continue;
        };
        let finished_before: Vec<usize> = (0..times.len())
            .filter(|&other| {
                other != index && times[other].1.is_some_and(|finished| finished <= started)
            })
            .collect();
        let precedes = |earlier: usize, later: usize| match (times[earlier].1, times[later].0) {
            (Some(finished), Some(started)) => finished <= started,
            _ => false,
        };
        stage.depends_on = finished_before
            .iter()
            .filter(|&&candidate| {
                !finished_before
                    .iter()
                    .any(|&other| other != candidate && precedes(candidate, other))
            })
            .map(|&candidate| ids[candidate].clone())
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn stage(id: &str, started: Option<Timestamp>, finished: Option<Timestamp>) -> PipelineJob {
        PipelineJob {
            id: id.to_owned(),
            name: id.to_owned(),
            status: if finished.is_some() { RunStatus::Succeeded } else { RunStatus::Queued },
            started_at: started,
            finished_at: finished,
            ..PipelineJob::default()
        }
    }

    fn dependencies(stages: &[PipelineJob]) -> Vec<Vec<&str>> {
        stages.iter().map(|stage| stage.depends_on.iter().map(String::as_str).collect()).collect()
    }

    #[test]
    fn timing_finds_parallel_stages_and_joins() {
        // Two builds in parallel, then E2E, as an Azure DevOps timeline reports them.
        let mut stages = vec![
            stage(
                "windows",
                Some(datetime!(2026-10-18 19:05:24 UTC)),
                Some(datetime!(2026-10-18 19:08:09 UTC)),
            ),
            stage(
                "osx",
                Some(datetime!(2026-10-18 19:05:22 UTC)),
                Some(datetime!(2026-10-18 19:09:01 UTC)),
            ),
            stage(
                "e2e",
                Some(datetime!(2026-10-18 19:09:24 UTC)),
                Some(datetime!(2026-10-18 19:22:15 UTC)),
            ),
            stage(
                "publish",
                Some(datetime!(2026-10-18 19:23:00 UTC)),
                Some(datetime!(2026-10-18 19:24:00 UTC)),
            ),
            stage("cleanup", None, None),
        ];
        infer_dependencies_from_timing(&mut stages);
        assert_eq!(
            dependencies(&stages),
            vec![vec![], vec![], vec!["windows", "osx"], vec!["e2e"], vec!["publish"]]
        );
    }

    #[test]
    fn a_group_shows_its_strongest_status() {
        use RunStatus::{Canceled, Failed, Neutral, Queued, Running, Skipped, Succeeded};
        let cases: [(&[RunStatus], RunStatus); 8] = [
            (&[Succeeded, Failed, Running], Failed),
            (&[Succeeded, Running, Queued], Running),
            (&[Succeeded, Queued, Canceled], Queued),
            (&[Canceled, Succeeded], Canceled),
            (&[Skipped, Succeeded, Neutral], Succeeded),
            (&[Neutral, Skipped], Skipped),
            (&[Neutral], Neutral),
            (&[], Neutral),
        ];
        for (statuses, expected) in cases {
            assert_eq!(RunStatus::combined(statuses.iter().copied()), expected, "{statuses:?}");
        }
    }

    #[test]
    fn every_stage_waits_for_the_one_listed_before_it() {
        let mut stages =
            vec![stage("a", None, None), stage("b", None, None), stage("c", None, None)];
        chain_sequentially(&mut stages);
        assert_eq!(dependencies(&stages), vec![vec![], vec!["a"], vec!["b"]]);
    }
}
