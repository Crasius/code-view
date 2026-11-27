//! What the checks table, the run switcher and the overview's merge status agree on: what a check
//! is (a run, or a job of a run), how checks sort, which status group they belong to, where they
//! come from and how they are summed up. Plain data, no UI.
//!
//! Sums speak for the head commit only ([`PipelineRun::is_head`]): a run an older push started,
//! or one a newer run of the same workflow superseded, stays listed as "older run" but never makes
//! the pull request read as failed or running.
//!
//! The overview shows the pull request's pipelines only ([`latest_pipelines`]): the latest run of
//! each, never its stages, jobs or the status checks a CI posts per job. One pipeline in the pull
//! request is one row there; the Pipelines tab keeps everything.

use std::collections::BTreeSet;

use cv_widgets::numbers::counted;
use review_core::{
    PipelineJob, PipelineRun, ProviderKind, RerunAction, RerunKind, RunStatus, Timestamp,
};
use review_store::CheckFilter;

use super::tree::jobs_of;

pub const FILTERS: [(CheckFilter, &str); 4] = [
    (CheckFilter::Failed, "Failed"),
    (CheckFilter::Running, "Running"),
    (CheckFilter::Succeeded, "Succeeded"),
    (CheckFilter::Neutral, "Neutral"),
];

/// The filter chip a run counts under.
pub fn group(status: RunStatus) -> CheckFilter {
    match status {
        RunStatus::Failed | RunStatus::Canceled => CheckFilter::Failed,
        RunStatus::Running | RunStatus::Queued => CheckFilter::Running,
        RunStatus::Succeeded => CheckFilter::Succeeded,
        RunStatus::Skipped | RunStatus::Neutral => CheckFilter::Neutral,
    }
}

/// Which check: a run, or one of its jobs. Survives reloads, unlike references into the data.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CheckKey {
    pub run_id: String,
    pub job_id: Option<String>,
}

/// One row of the checks table and one option of the run switcher: a run, or a job of a run
/// with the stage it belongs to.
#[derive(Clone, Copy)]
pub struct Check<'a> {
    pub run: &'a PipelineRun,
    /// `(stage, job)`; a stage without jobs of its own is its own job.
    pub job: Option<(&'a PipelineJob, &'a PipelineJob)>,
}

impl<'a> Check<'a> {
    pub fn key(&self) -> CheckKey {
        CheckKey { run_id: self.run.id.clone(), job_id: self.job.map(|(_, job)| job.id.clone()) }
    }

    pub fn is(&self, key: &CheckKey) -> bool {
        self.run.id == key.run_id
            && self.job.map(|(_, job)| job.id.as_str()) == key.job_id.as_deref()
    }

    pub fn name(&self) -> &'a str {
        self.job.map_or(&self.run.name, |(_, job)| &job.name)
    }

    pub fn status(&self) -> RunStatus {
        self.job.map_or(self.run.status, |(_, job)| job.status)
    }

    pub fn started_at(&self) -> Option<Timestamp> {
        self.job.map_or(self.run.started_at, |(_, job)| job.started_at)
    }

    pub fn finished_at(&self) -> Option<Timestamp> {
        self.job.map_or(self.run.finished_at, |(_, job)| job.finished_at)
    }

    pub fn reruns(&self) -> &'a [RerunAction] {
        self.job.map_or(&self.run.reruns, |(_, job)| &job.reruns)
    }

    pub fn web_url(&self) -> Option<&'a str> {
        self.job.map_or(self.run.web_url.as_deref(), |(stage, job)| {
            job.web_url.as_deref().or(stage.web_url.as_deref())
        })
    }

    /// A job that waits for a person to start it (GitLab's manual jobs): neutral, never started.
    pub fn is_manual(&self) -> bool {
        self.job
            .is_some_and(|(_, job)| job.status == RunStatus::Neutral && job.started_at.is_none())
    }

    /// The stage a job belongs to; `None` for a run and for a stage that is its own job.
    pub fn stage_name(&self) -> Option<&'a str> {
        self.job.filter(|(stage, job)| stage.id != job.id).map(|(stage, _)| stage.name.as_str())
    }

    /// The accessible name: a job names its run too, as job names repeat across runs, and a
    /// retried job how its attempts went; a run that is not the head commit's says so.
    pub fn label(&self) -> String {
        let status = cv_widgets::badges::run_status_label(self.status());
        match self.job {
            None if !self.run.is_head => format!("{}, {status}, older run", self.run.name),
            None => format!("{}, {status}", self.run.name),
            Some((_, job)) => match super::attempts::outcome(job) {
                Some(outcome) => format!("{} in {}, {status}, {outcome}", job.name, self.run.name),
                None => format!("{} in {}, {status}", job.name, self.run.name),
            },
        }
    }
}

/// Every run followed by its jobs, in the platform's order. `stages` gives the stages known for
/// a run; runs without jobs, or whose stages are not known yet, stand alone.
pub fn list<'a>(
    runs: &'a [PipelineRun],
    stages: impl Fn(&PipelineRun) -> Option<&'a [PipelineJob]>,
) -> Vec<Check<'a>> {
    let mut checks = Vec::new();
    for run in runs {
        checks.push(Check { run, job: None });
        for stage in stages(run).unwrap_or_default() {
            for job in jobs_of(stage) {
                checks.push(Check { run, job: Some((stage, job)) });
            }
        }
    }
    checks
}

/// The head commit's checks first, each part failed first, then running, then in the order the
/// platform lists them.
pub fn sort(checks: &mut [Check<'_>]) {
    checks.sort_by_key(order);
}

/// The overview's pipelines in the checks table's order: the latest run of each of the head
/// commit's pipelines (runs with jobs), one per name, as platforms list a pipeline run again under
/// its name (a retried GitLab pipeline, a workflow on `push` and `pull_request`). Providers list
/// the current run first, so the first of a name wins. A pull request whose CI only posts status
/// checks shows those instead, so the overview is never blank while the tab lists checks.
pub fn latest_pipelines(runs: &[PipelineRun]) -> Vec<&PipelineRun> {
    let has_pipelines = runs.iter().any(|run| run.is_head && run.has_jobs);
    let mut names = BTreeSet::new();
    let mut latest: Vec<&PipelineRun> = runs
        .iter()
        .filter(|run| run.is_head && run.has_jobs == has_pipelines)
        .filter(|run| names.insert(run.name.as_str()))
        .collect();
    latest.sort_by_key(|run| rank(run.status));
    latest
}

/// The sort key of the checks: older runs last, then [`rank`].
pub fn order(check: &Check<'_>) -> (bool, u8) {
    (!check.run.is_head, rank(check.status()))
}

/// 0 failed (canceled too, as the Failed chip counts it), 1 running, 2 the rest.
pub fn rank(status: RunStatus) -> u8 {
    match group(status) {
        CheckFilter::Failed => 0,
        CheckFilter::Running => 1,
        CheckFilter::Succeeded | CheckFilter::Neutral => 2,
    }
}

/// The head commit's checks per filter chip, in [`FILTERS`] order.
pub fn counts<'a>(checks: impl Iterator<Item = &'a Check<'a>>) -> [usize; 4] {
    let mut counts = [0; 4];
    for check in checks.filter(|check| check.run.is_head) {
        counts[filter_index(group(check.status()))] += 1;
    }
    counts
}

/// Where `filter` sits in [`FILTERS`].
const fn filter_index(filter: CheckFilter) -> usize {
    match filter {
        CheckFilter::Failed => 0,
        CheckFilter::Running => 1,
        CheckFilter::Succeeded => 2,
        CheckFilter::Neutral => 3,
    }
}

const _: () = {
    let mut index = 0;
    while index < FILTERS.len() {
        assert!(filter_index(FILTERS[index].0) == index, "FILTERS and filter_index disagree");
        index += 1;
    }
};

/// True when `check` passes the status chips and the text filter, which matches the name, the
/// line under it and the source. No chip selected shows everything; a chip shows the head
/// commit's checks of its status, as it counts them.
pub fn matches(
    check: &Check<'_>,
    filter: &BTreeSet<CheckFilter>,
    text: &str,
    platform: ProviderKind,
) -> bool {
    let is_status_shown =
        filter.is_empty() || (check.run.is_head && filter.contains(&group(check.status())));
    let needle = text.trim().to_lowercase();
    let is_text_shown = needle.is_empty()
        || check.name().to_lowercase().contains(&needle)
        || check_sub_line(check, None).to_lowercase().contains(&needle)
        || source(check.run, platform).to_lowercase().contains(&needle);
    is_status_shown && is_text_shown
}

/// Where a run comes from: the platform's CI for runs with jobs, else a reported status.
pub fn source(run: &PipelineRun, platform: ProviderKind) -> &'static str {
    if !run.has_jobs {
        return "Status check";
    }
    match platform {
        ProviderKind::GitLab => "GitLab CI",
        ProviderKind::GitHub => "GitHub Actions",
        ProviderKind::AzureDevOps => "Azure Pipelines",
    }
}

/// The line under a check's name: a job's stage ("Cleanup · manual"), else [`sub_line`].
pub fn check_sub_line(check: &Check<'_>, stages: Option<&[PipelineJob]>) -> String {
    if check.job.is_none() {
        return sub_line(check.run, stages);
    }
    let place = check.stage_name().unwrap_or(&check.run.name);
    let retry = check.job.and_then(|(_, job)| super::attempts::outcome(job));
    let manual = check.is_manual().then(|| "manual".to_owned());
    std::iter::once(place.to_owned()).chain(manual).chain(retry).collect::<Vec<_>>().join(" · ")
}

/// The line under a run's name: its trigger, "older run" for one that is not the head commit's,
/// then how many stages and jobs it has once known.
pub fn sub_line(run: &PipelineRun, stages: Option<&[PipelineJob]>) -> String {
    let mut parts: Vec<String> = Vec::new();
    match &run.description {
        Some(description) => parts.push(description.clone()),
        None if !run.has_jobs => parts.push("Status check".to_owned()),
        None => {}
    }
    if !run.is_head {
        parts.push("older run".to_owned());
    }
    if let Some(stages) = stages.filter(|stages| !stages.is_empty()) {
        let jobs: usize = stages.iter().map(|stage| jobs_of(stage).len()).sum();
        parts.push(counted(stages.len(), "stage", "stages"));
        parts.push(counted(jobs, "job", "jobs"));
        let retried =
            stages.iter().flat_map(jobs_of).filter(|job| !job.previous_attempts.is_empty()).count();
        if retried > 0 {
            parts.push(format!("{} retried", counted(retried, "job", "jobs")));
        }
    }
    parts.join(" · ")
}

/// "1 reviewer", "3 reviewers": [`counted`] with the plural an added "s".
pub fn plural(count: usize, noun: &str) -> String {
    counted(count, noun, &format!("{noun}s"))
}

/// "14m 25s", counting up while running; `None` before it started.
pub fn duration(run: &PipelineRun) -> Option<String> {
    super::duration(run.started_at, run.finished_at)
}

/// [`duration`] of a run or a job.
pub fn check_duration(check: &Check<'_>) -> Option<String> {
    super::duration(check.started_at(), check.finished_at())
}

/// The head commit's failed runs that offer rerunning their failed jobs, for "Rerun failed".
pub fn failed_reruns(runs: &[PipelineRun]) -> Vec<(String, RerunAction)> {
    runs.iter()
        .filter(|run| run.is_head && group(run.status) == CheckFilter::Failed)
        .filter_map(|run| {
            let action = run.reruns.iter().find(|action| action.kind == RerunKind::RunFailed)?;
            Some((run.id.clone(), action.clone()))
        })
        .collect()
}

/// The overview's one-line verdict on the head commit: the worst state wins.
pub struct Summary {
    pub status: RunStatus,
    pub headline: String,
    pub detail: String,
}

/// The verdict on [`latest_pipelines`]. Status checks are not rows there, but one that failed
/// still fails the verdict and is named, so a failure never hides behind a green pipeline.
pub fn summary(runs: &[PipelineRun]) -> Summary {
    let pipelines = latest_pipelines(runs);
    let checks: Vec<Check<'_>> = pipelines.iter().map(|run| Check { run, job: None }).collect();
    let [failed, running, succeeded, neutral] = counts(checks.iter());
    let failed_checks = runs
        .iter()
        .filter(|run| run.is_head && !run.has_jobs && group(run.status) == CheckFilter::Failed)
        .filter(|run| !pipelines.iter().any(|shown| shown.id == run.id))
        .count();
    let (status, headline) = if pipelines.is_empty() {
        (RunStatus::Neutral, "No pipelines for this commit".to_owned())
    } else if failed > 0 {
        (RunStatus::Failed, format!("{} failed", counted(failed, "pipeline", "pipelines")))
    } else if failed_checks > 0 {
        (
            RunStatus::Failed,
            format!("{} failed", counted(failed_checks, "status check", "status checks")),
        )
    } else if running > 0 {
        (RunStatus::Running, format!("{} running", counted(running, "pipeline", "pipelines")))
    } else if succeeded > 0 && neutral == 0 {
        (RunStatus::Succeeded, "All pipelines succeeded".to_owned())
    } else if succeeded > 0 {
        (RunStatus::Succeeded, "Pipelines succeeded".to_owned())
    } else {
        (RunStatus::Neutral, "Pipelines finished without a result".to_owned())
    };
    let mut detail = Vec::new();
    if !pipelines.is_empty() {
        detail.push(counted(pipelines.len(), "pipeline", "pipelines"));
    }
    for (count, word) in
        [(failed, "failed"), (running, "running"), (succeeded, "succeeded"), (neutral, "neutral")]
    {
        if count > 0 {
            detail.push(format!("{count} {word}"));
        }
    }
    if failed_checks > 0 {
        detail.push(format!("{} failed", counted(failed_checks, "status check", "status checks")));
    }
    Summary { status, headline, detail: detail.join(" · ") }
}

/// Runs and jobs named by their id, for the tests of the checks and the run header.
#[cfg(test)]
pub(crate) mod fixtures {
    use review_core::{PipelineJob, PipelineRun, RunStatus};

    /// A run of the head commit.
    pub fn run(name: &str, status: RunStatus, has_jobs: bool) -> PipelineRun {
        PipelineRun {
            id: name.to_owned(),
            name: name.to_owned(),
            status,
            has_jobs,
            is_head: true,
            ..PipelineRun::default()
        }
    }

    pub fn job(name: &str, status: RunStatus) -> PipelineJob {
        PipelineJob { id: name.to_owned(), name: name.to_owned(), status, ..PipelineJob::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{job, run};
    use super::*;

    #[test]
    fn failed_then_running_then_platform_order() {
        let runs = vec![
            run("lint", RunStatus::Succeeded, false),
            run("deploy", RunStatus::Running, true),
            run("docs", RunStatus::Neutral, false),
            run("tests", RunStatus::Failed, true),
            run("build", RunStatus::Queued, true),
        ];
        let names: Vec<&str> =
            latest_pipelines(&runs).iter().map(|run| run.name.as_str()).collect();
        assert_eq!(names, ["tests", "deploy", "build"], "pipelines only, failed first");
        let checks = list(&runs, |_| None);
        assert_eq!(counts(checks.iter()), [1, 2, 1, 1]);
    }

    #[test]
    fn canceled_checks_sort_with_the_failed_ones_they_count_under() {
        let runs =
            vec![run("lint", RunStatus::Succeeded, false), run("e2e", RunStatus::Canceled, false)];
        let mut checks = list(&runs, |_| None);
        sort(&mut checks);
        let names: Vec<&str> = checks.iter().map(Check::name).collect();
        assert_eq!(names, ["e2e", "lint"]);
        assert_eq!(counts(checks.iter())[0], 1, "the Failed chip counts it");
    }

    #[test]
    fn jobs_follow_their_run_and_sort_with_everything_else() {
        let stage = |id: &str, children: Vec<PipelineJob>| PipelineJob {
            id: id.to_owned(),
            name: id.to_owned(),
            children,
            ..PipelineJob::default()
        };
        let stages = vec![
            stage(
                "Build",
                vec![job("compile", RunStatus::Succeeded), job("unit", RunStatus::Failed)],
            ),
            stage("Cleanup", vec![job("stop", RunStatus::Neutral)]),
            PipelineJob { status: RunStatus::Running, ..stage("Deploy", Vec::new()) },
        ];
        let runs = vec![
            run("pipeline", RunStatus::Failed, true),
            run("scan", RunStatus::Succeeded, false),
        ];
        let mut checks = list(&runs, |run| run.has_jobs.then_some(stages.as_slice()));
        let names: Vec<&str> = checks.iter().map(Check::name).collect();
        assert_eq!(
            names,
            ["pipeline", "compile", "unit", "stop", "Deploy", "scan"],
            "a stage without jobs is its own job"
        );
        sort(&mut checks);
        let names: Vec<&str> = checks.iter().map(Check::name).collect();
        assert_eq!(names, ["pipeline", "unit", "Deploy", "compile", "stop", "scan"]);
        let stop = checks.iter().find(|check| check.name() == "stop").expect("the manual job");
        assert_eq!(check_sub_line(stop, None), "Cleanup · manual");
        assert_eq!(stop.label(), "stop in pipeline, Neutral");
        assert!(stop.is(&stop.key()) && !checks[0].is(&stop.key()));
    }

    #[test]
    fn retried_jobs_say_how_their_attempts_went_and_counts_stay_the_latest() {
        use review_core::{AttemptSource, PipelineAttempt};
        let earlier = |status: RunStatus| PipelineAttempt {
            number: 1,
            source: AttemptSource::Loaded(Box::new(PipelineJob {
                status,
                attempt: 1,
                ..PipelineJob::default()
            })),
        };
        let retried = |id: &str, status: RunStatus| PipelineJob {
            id: id.to_owned(),
            name: id.to_owned(),
            status,
            attempt: 2,
            previous_attempts: vec![earlier(RunStatus::Failed)],
            ..PipelineJob::default()
        };
        let stage = PipelineJob {
            id: "Test".to_owned(),
            name: "Test".to_owned(),
            children: vec![
                retried("unit", RunStatus::Succeeded),
                retried("e2e", RunStatus::Failed),
            ],
            ..PipelineJob::default()
        };
        let stages = vec![stage];
        let runs = vec![run("pipeline", RunStatus::Failed, true)];
        let checks = list(&runs, |_| Some(stages.as_slice()));
        let lines: Vec<String> =
            checks.iter().map(|check| check_sub_line(check, Some(&stages))).collect();
        assert_eq!(
            lines,
            [
                "1 stage · 2 jobs · 2 jobs retried",
                "Test · passed on retry",
                "Test · failed after 2 attempts"
            ]
        );
        assert_eq!(counts(checks.iter()), [2, 0, 1, 0], "the latest attempts count");
    }

    #[test]
    fn filters_combine_status_chips_and_text() {
        let tests = run("Unit tests", RunStatus::Failed, true);
        let tests = Check { run: &tests, job: None };
        let mut chips = BTreeSet::new();
        assert!(matches(&tests, &chips, "", ProviderKind::GitLab), "no chip shows everything");
        chips.insert(CheckFilter::Running);
        assert!(!matches(&tests, &chips, "", ProviderKind::GitLab));
        chips.insert(CheckFilter::Failed);
        assert!(
            matches(&tests, &chips, "UNIT", ProviderKind::GitLab),
            "case-insensitive name match"
        );
        assert!(
            matches(&tests, &chips, "gitlab ci", ProviderKind::GitLab),
            "the source matches too"
        );
        assert!(!matches(&tests, &chips, "deploy", ProviderKind::GitLab));
    }

    #[test]
    fn sub_lines_and_summaries_read_like_the_platform() {
        let mut pipeline = run("Pipeline #2279", RunStatus::Succeeded, true);
        pipeline.description =
            Some("merge_request_event on refs/merge-requests/17/head".to_owned());
        let stage = |jobs: usize| PipelineJob {
            children: vec![PipelineJob::default(); jobs],
            ..PipelineJob::default()
        };
        let stages = vec![stage(3), stage(1), stage(0)];
        assert_eq!(
            sub_line(&pipeline, Some(&stages)),
            "merge_request_event on refs/merge-requests/17/head · 3 stages · 5 jobs"
        );
        assert_eq!(
            sub_line(&run("Security scan", RunStatus::Succeeded, false), None),
            "Status check"
        );
        let older = PipelineRun { is_head: false, ..pipeline.clone() };
        assert_eq!(
            sub_line(&older, None),
            "merge_request_event on refs/merge-requests/17/head · older run",
            "jobs load once it is opened"
        );
        assert_eq!(
            Check { run: &older, job: None }.label(),
            "Pipeline #2279, Succeeded, older run"
        );

        let all_good = vec![pipeline.clone(), run("scan", RunStatus::Succeeded, false)];
        let verdict = summary(&all_good);
        assert_eq!(
            (verdict.headline.as_str(), verdict.detail.as_str()),
            ("All pipelines succeeded", "1 pipeline · 1 succeeded")
        );
        let failing = vec![
            pipeline,
            run("scan", RunStatus::Failed, false),
            run("docs", RunStatus::Running, true),
        ];
        let verdict = summary(&failing);
        assert_eq!(verdict.status, RunStatus::Failed, "a failed status check still fails it");
        assert_eq!(
            (verdict.headline.as_str(), verdict.detail.as_str()),
            (
                "1 status check failed",
                "2 pipelines · 1 running · 1 succeeded · 1 status check failed"
            )
        );
    }

    #[test]
    fn older_runs_stay_listed_but_never_count() {
        let older = |name: &str, status: RunStatus| PipelineRun {
            is_head: false,
            ..run(name, status, true)
        };
        let runs = vec![
            older("Pipeline #2275", RunStatus::Failed),
            run("Pipeline #2279", RunStatus::Succeeded, true),
            older("Pipeline #2277", RunStatus::Running),
            run("scan", RunStatus::Succeeded, false),
        ];
        let verdict = summary(&runs);
        assert_eq!(
            verdict.status,
            RunStatus::Succeeded,
            "a superseded failure does not fail the pull request"
        );
        assert_eq!(
            (verdict.headline.as_str(), verdict.detail.as_str()),
            ("All pipelines succeeded", "1 pipeline · 1 succeeded")
        );

        let checks = list(&runs, |_| None);
        assert_eq!(counts(checks.iter()), [0, 0, 2, 0], "the chips count the head commit's checks");
        let failed: BTreeSet<CheckFilter> = [CheckFilter::Failed].into();
        assert!(
            !matches(&checks[0], &failed, "", ProviderKind::GitLab),
            "a chip shows what it counts"
        );
        assert!(
            matches(&checks[0], &BTreeSet::new(), "", ProviderKind::GitLab),
            "no chip shows the older runs too"
        );
        assert!(failed_reruns(&runs).is_empty(), "\"Rerun failed\" is for the head commit");

        let names: Vec<&str> =
            latest_pipelines(&runs).iter().map(|run| run.name.as_str()).collect();
        assert_eq!(names, ["Pipeline #2279"], "the overview shows the latest run only");
        let mut checks = checks;
        sort(&mut checks);
        let names: Vec<&str> = checks.iter().map(Check::name).collect();
        assert_eq!(names, ["Pipeline #2279", "scan", "Pipeline #2275", "Pipeline #2277"]);

        let only_older = vec![older("Pipeline #2275", RunStatus::Failed)];
        let verdict = summary(&only_older);
        assert_eq!(verdict.status, RunStatus::Neutral);
        assert_eq!(
            (verdict.headline.as_str(), verdict.detail.as_str()),
            ("No pipelines for this commit", "")
        );
    }

    /// One pipeline in the pull request is one overview row: its latest run, not the earlier run
    /// of the same commit and not the status checks its CI posts per job.
    #[test]
    fn the_overview_shows_each_pipeline_once() {
        let runs = vec![
            PipelineRun { id: "new".into(), ..run("CI", RunStatus::Running, true) },
            PipelineRun { id: "retried".into(), ..run("CI", RunStatus::Failed, true) },
            run("build", RunStatus::Failed, false),
            run("test", RunStatus::Succeeded, false),
        ];
        let ids: Vec<&str> = latest_pipelines(&runs).iter().map(|run| run.id.as_str()).collect();
        assert_eq!(ids, ["new"]);
        let verdict = summary(&runs);
        assert_eq!(
            (verdict.headline.as_str(), verdict.detail.as_str()),
            ("1 status check failed", "1 pipeline · 1 running · 1 status check failed")
        );

        let statuses_only = vec![
            run("jenkins", RunStatus::Succeeded, false),
            run("sonar", RunStatus::Failed, false),
        ];
        let names: Vec<&str> =
            latest_pipelines(&statuses_only).iter().map(|run| run.name.as_str()).collect();
        assert_eq!(names, ["sonar", "jenkins"], "without pipelines the status checks stand in");
        assert_eq!(summary(&statuses_only).headline, "1 pipeline failed");
    }
}
