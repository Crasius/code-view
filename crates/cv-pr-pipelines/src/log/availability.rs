//! Whether the log pane can show a log now, from what the platform's API serves of a running job
//! ([`RunningLogs`]) and the state of the job the log belongs to. A log the platform does not serve
//! yet is a state with its way forward, never an error.

use std::time::Duration;

use review_core::{RunStatus, RunningLogs};

/// How long after its job finished a missing log counts as still on its way: the platform may
/// publish the file a moment after it reports the job done.
pub(crate) const PUBLISH_GRACE: Duration = Duration::from_secs(60);

/// What the log pane does for the open item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Availability {
    /// No log to fetch: the item has none (yet), or has not started. The pane explains its state.
    Absent,
    /// The platform serves the log only once its job finished, and it still runs: the pane says
    /// so, offers watching it live on the platform and fetches the log once the job finished.
    AfterFinish,
    /// Fetch the log; `is_live` while its job runs, so what it adds is fetched as it comes.
    Fetch { is_live: bool },
}

/// `has_log` is whether the item has a log id; `owner` the state of the job (or task) the log
/// belongs to, as the stages loaded last report it.
pub(crate) fn of(running_logs: RunningLogs, has_log: bool, owner: RunStatus) -> Availability {
    match (has_log, owner) {
        // GitLab lists a log for a pending job; there is nothing in it before the job starts.
        (false, _) | (true, RunStatus::Queued) => Availability::Absent,
        (true, RunStatus::Running) => match running_logs {
            RunningLogs::WhenFinished => Availability::AfterFinish,
            RunningLogs::Growing => Availability::Fetch { is_live: true },
        },
        (true, _) => Availability::Fetch { is_live: false },
    }
}

/// What a fetch that the platform answered with an error status means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Missing {
    /// The platform serves the log once the job finished, which it has not (the stages were
    /// older than the answer): the same state as [`Availability::AfterFinish`].
    AfterFinish,
    /// The running job has not written its log yet: try again when the next fetch is due.
    NotYet,
    /// The job just finished and its log is not published yet: try again when due.
    Publishing,
    /// A real failure, explained with its ways out.
    Failed,
}

/// `finished_ago` is the time since the job finished, when it did.
pub(crate) fn missing(
    running_logs: RunningLogs,
    owner: RunStatus,
    status: Option<u16>,
    finished_ago: Option<Duration>,
) -> Missing {
    if status != Some(404) {
        return Missing::Failed;
    }
    if !owner.is_finished() {
        return match running_logs {
            RunningLogs::WhenFinished => Missing::AfterFinish,
            RunningLogs::Growing => Missing::NotYet,
        };
    }
    match finished_ago {
        Some(ago) if ago < PUBLISH_GRACE => Missing::Publishing,
        _ => Missing::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GITHUB: RunningLogs = RunningLogs::WhenFinished;
    const GROWING: RunningLogs = RunningLogs::Growing;

    #[test]
    fn github_serves_a_job_log_once_the_job_finished() {
        assert_eq!(
            of(GITHUB, true, RunStatus::Running),
            Availability::AfterFinish,
            "no request while the job runs"
        );
        for finished in
            [RunStatus::Succeeded, RunStatus::Failed, RunStatus::Canceled, RunStatus::Neutral]
        {
            assert_eq!(
                of(GITHUB, true, finished),
                Availability::Fetch { is_live: false },
                "{finished:?}: fetched once"
            );
        }
        assert_eq!(of(GITHUB, true, RunStatus::Queued), Availability::Absent);
    }

    /// The bug: `GET /actions/jobs/{id}/logs` of a running job redirects to a blob that answers
    /// 404 `BlobNotFound`. That is the job still running, not an error.
    #[test]
    fn github_404_of_a_running_job_is_the_live_state_not_an_error() {
        assert_eq!(missing(GITHUB, RunStatus::Running, Some(404), None), Missing::AfterFinish);
        assert_eq!(
            missing(GITHUB, RunStatus::Succeeded, Some(404), Some(Duration::from_secs(5))),
            Missing::Publishing
        );
        assert_eq!(
            missing(GITHUB, RunStatus::Succeeded, Some(404), Some(Duration::from_secs(600))),
            Missing::Failed
        );
        assert_eq!(missing(GITHUB, RunStatus::Succeeded, Some(404), None), Missing::Failed);
        assert_eq!(
            missing(GITHUB, RunStatus::Running, Some(403), None),
            Missing::Failed,
            "a denied log stays an error"
        );
        assert_eq!(
            missing(GITHUB, RunStatus::Running, None, None),
            Missing::Failed,
            "network errors stay errors"
        );
    }

    #[test]
    fn growing_logs_are_fetched_live_while_their_job_runs() {
        assert_eq!(of(GROWING, true, RunStatus::Running), Availability::Fetch { is_live: true });
        assert_eq!(of(GROWING, true, RunStatus::Failed), Availability::Fetch { is_live: false });
        assert_eq!(
            of(GROWING, true, RunStatus::Queued),
            Availability::Absent,
            "a pending GitLab job's log is empty"
        );
        assert_eq!(
            of(GROWING, false, RunStatus::Running),
            Availability::Absent,
            "an Azure DevOps task without a log yet"
        );
        assert_eq!(missing(GROWING, RunStatus::Running, Some(404), None), Missing::NotYet);
    }
}
