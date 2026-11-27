//! Build timelines: flat `TimelineRecord`s linked by `parentId` → Stage/Job/Task tree.

use std::collections::{HashMap, HashSet};

use review_core::{
    AttemptSource, PipelineAttempt, PipelineJob, RerunAction, RerunKind, RunStatus, Timestamp,
    as_earlier_attempt, infer_dependencies_from_timing,
};
use serde::Deserialize;

use super::pipeline::result_from_ado;

#[derive(Debug, Deserialize)]
pub(crate) struct LogRef {
    pub id: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimelineRecord {
    pub id: String,
    pub parent_id: Option<String>,
    /// Stable across attempts; for stages the `stageRefName` that stage retries address.
    pub identifier: Option<String>,
    #[serde(rename = "type", default)]
    pub record_type: String,
    #[serde(default)]
    pub name: String,
    pub state: Option<String>,
    pub result: Option<String>,
    pub order: Option<i64>,
    #[serde(default, deserialize_with = "super::date::optional")]
    pub start_time: Option<Timestamp>,
    #[serde(default, deserialize_with = "super::date::optional")]
    pub finish_time: Option<Timestamp>,
    pub log: Option<LogRef>,
    /// 1 until the record's stage or job is retried; every retry counts it up.
    pub attempt: Option<u32>,
    /// The records of the earlier attempts, each in the timeline of that attempt
    /// (`GET build/builds/{id}/timeline/{timelineId}`). `[]` or `null` when there were none.
    pub previous_attempts: Option<Vec<PreviousAttempt>>,
}

/// `TimelineAttempt`: where an earlier attempt of a record lives.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviousAttempt {
    pub attempt: u32,
    pub timeline_id: String,
    pub record_id: String,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Timeline {
    #[serde(default)]
    pub records: Vec<TimelineRecord>,
}

enum Role {
    /// Shown as a node.
    Node,
    /// Hidden; its children attach to its parent. Phases and the implicit `__default` stage,
    /// matching what the Azure DevOps web UI shows.
    Transparent,
    /// Hidden with its subtree (checkpoints, approvals and other bookkeeping records).
    Dropped,
}

/// `phases_with_jobs` are the phases that have Job records below them.
fn role(record: &TimelineRecord, phases_with_jobs: &HashSet<&str>) -> Role {
    match record.record_type.as_str() {
        "Stage" if record.name == "__default" => Role::Transparent,
        // A phase stands for its jobs. One that never got any (skipped by its condition, or not
        // expanded yet) is the job, as the web UI lists it.
        "Phase" if phases_with_jobs.contains(record.id.as_str()) => Role::Transparent,
        "Stage" | "Job" | "Task" | "Phase" => Role::Node,
        _ => Role::Dropped,
    }
}

pub(crate) fn record_status(state: Option<&str>, result: Option<&str>) -> RunStatus {
    match state {
        Some("completed") => result_from_ado(result),
        Some("inProgress") => RunStatus::Running,
        _ => RunStatus::Queued,
    }
}

/// `build_web_url` is the build's results page; jobs and tasks get deep links into its log view.
/// Log ids are `"{build_id}:{log_id}"`.
pub(crate) fn build_job_tree(
    build_id: u64,
    records: &[TimelineRecord],
    build_web_url: Option<&str>,
) -> Vec<PipelineJob> {
    let by_id: HashMap<&str, &TimelineRecord> =
        records.iter().map(|record| (record.id.as_str(), record)).collect();
    let phases_with_jobs: HashSet<&str> = records
        .iter()
        .filter(|record| record.record_type == "Job")
        .filter_map(|record| record.parent_id.as_deref())
        .collect();
    let mut children: HashMap<Option<&str>, Vec<&TimelineRecord>> = HashMap::new();
    for record in records {
        if !matches!(role(record, &phases_with_jobs), Role::Node) {
            continue;
        }
        if let Placement::Below(parent) =
            visible_parent(record, &by_id, &phases_with_jobs, records.len())
        {
            children.entry(parent).or_default().push(record);
        }
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|a, b| {
            (a.order.unwrap_or(i64::MAX), a.start_time, &a.name).cmp(&(
                b.order.unwrap_or(i64::MAX),
                b.start_time,
                &b.name,
            ))
        });
    }
    let roots = children.get(&None).map(Vec::as_slice).unwrap_or_default();
    let is_finished = roots.iter().all(|record| {
        record_status(record.state.as_deref(), record.result.as_deref()).is_finished()
    });
    let context = TreeContext { build_id, build_web_url, children: &children, is_finished };
    let mut nodes = context.nodes(None);
    // Timelines carry no stage graph: `order` numbers stages one after another even when they ran
    // in parallel, and every stage is a root record. Start and finish times show which stages
    // waited for which, so the graph is derived from them.
    infer_dependencies_from_timing(&mut nodes);
    nodes
}

/// Stage retry (`PATCH build/builds/{id}/stages/{stageRefName}`) once the run has finished:
/// "Rerun just this stage" reruns every job, "Retry stage" the failed, canceled and skipped ones.
fn stage_reruns(
    build_id: u64,
    record: &TimelineRecord,
    status: RunStatus,
    is_run_finished: bool,
) -> Vec<RerunAction> {
    let Some(identifier) =
        record.identifier.as_deref().filter(|_| record.record_type == "Stage" && is_run_finished)
    else {
        return Vec::new();
    };
    let target = format!("{build_id}:{identifier}");
    let kinds: &[RerunKind] = match status {
        RunStatus::Succeeded => &[RerunKind::StageAll],
        RunStatus::Skipped => &[RerunKind::StageFailed],
        RunStatus::Failed | RunStatus::Canceled | RunStatus::Neutral => {
            &[RerunKind::StageFailed, RerunKind::StageAll]
        }
        RunStatus::Queued | RunStatus::Running => &[],
    };
    kinds.iter().map(|kind| RerunAction::new(*kind, target.clone())).collect()
}

/// Parses a `"{build_id}:{stageRefName}"` stage rerun target.
pub(crate) fn parse_stage_target(target: &str) -> Option<(u64, &str)> {
    let (build, stage) = target.split_once(':')?;
    let build = build.parse().ok()?;
    (!stage.is_empty()).then_some((build, stage))
}

/// Where a visible record goes in the job tree.
enum Placement<'a> {
    /// Below the record with this id; `None` is the root.
    Below(Option<&'a str>),
    /// Inside a dropped subtree (or a parent cycle): not shown.
    Hidden,
}

fn visible_parent<'a>(
    record: &'a TimelineRecord,
    by_id: &HashMap<&str, &'a TimelineRecord>,
    phases_with_jobs: &HashSet<&str>,
    max_depth: usize,
) -> Placement<'a> {
    let mut parent_id = record.parent_id.as_deref();
    for _ in 0..=max_depth {
        let Some(parent) = parent_id.and_then(|id| by_id.get(id)) else {
            return Placement::Below(None);
        };
        match role(parent, phases_with_jobs) {
            Role::Node => return Placement::Below(Some(parent.id.as_str())),
            Role::Transparent => parent_id = parent.parent_id.as_deref(),
            Role::Dropped => return Placement::Hidden,
        }
    }
    Placement::Hidden
}

struct TreeContext<'a> {
    build_id: u64,
    build_web_url: Option<&'a str>,
    children: &'a HashMap<Option<&'a str>, Vec<&'a TimelineRecord>>,
    is_finished: bool,
}

impl TreeContext<'_> {
    fn nodes(&self, parent: Option<&str>) -> Vec<PipelineJob> {
        let Some(records) = self.children.get(&parent) else {
            return Vec::new();
        };
        records.iter().map(|record| self.node(record, parent)).collect()
    }

    fn node(&self, record: &TimelineRecord, parent: Option<&str>) -> PipelineJob {
        let status = record_status(record.state.as_deref(), record.result.as_deref());
        PipelineJob {
            id: record.id.clone(),
            name: record.name.clone(),
            status,
            started_at: record.start_time,
            finished_at: record.finish_time,
            log_id: record.log.as_ref().map(|log| format!("{}:{}", self.build_id, log.id)),
            web_url: self.web_url(record, parent),
            children: self.nodes(Some(record.id.as_str())),
            depends_on: Vec::new(),
            reruns: stage_reruns(self.build_id, record, status, self.is_finished),
            attempt: record.attempt.unwrap_or(0),
            previous_attempts: previous_attempts(self.build_id, record),
        }
    }

    fn web_url(&self, record: &TimelineRecord, parent: Option<&str>) -> Option<String> {
        let base = self.build_web_url?;
        match record.record_type.as_str() {
            "Job" => Some(format!("{base}&view=logs&j={}", record.id)),
            "Task" => parent.map(|job| format!("{base}&view=logs&j={job}&t={}", record.id)),
            _ => Some(base.to_owned()),
        }
    }
}

/// Earlier attempts by id only, oldest first: their records live in other timelines, loaded when
/// the user opens one. Targets are `"{build_id}:{timeline_id}:{record_id}"`.
fn previous_attempts(build_id: u64, record: &TimelineRecord) -> Vec<PipelineAttempt> {
    let mut attempts: Vec<PipelineAttempt> = record
        .previous_attempts
        .iter()
        .flatten()
        .map(|previous| PipelineAttempt {
            number: previous.attempt,
            source: AttemptSource::Lazy(format!(
                "{build_id}:{}:{}",
                previous.timeline_id, previous.record_id
            )),
        })
        .collect();
    attempts.sort_by_key(|attempt| attempt.number);
    attempts
}

/// Parses a `"{build_id}:{timeline_id}:{record_id}"` attempt target.
pub(crate) fn parse_attempt_target(target: &str) -> Option<(u64, &str, &str)> {
    let mut parts = target.splitn(3, ':');
    let build = parts.next()?.parse().ok()?;
    let (timeline, record) = (parts.next()?, parts.next()?);
    (!timeline.is_empty() && !record.is_empty()).then_some((build, timeline, record))
}

/// The record `record_id` of an earlier attempt's timeline as a node with its children, marked as
/// that attempt (no reruns, no attempts of its own).
pub(crate) fn attempt_node(
    build_id: u64,
    records: &[TimelineRecord],
    record_id: &str,
    build_web_url: Option<&str>,
) -> Option<PipelineJob> {
    fn find(nodes: Vec<PipelineJob>, id: &str) -> Option<PipelineJob> {
        nodes
            .into_iter()
            .find_map(|node| if node.id == id { Some(node) } else { find(node.children, id) })
    }
    let node = find(build_job_tree(build_id, records, build_web_url), record_id)?;
    let number = node.attempt_number();
    Some(as_earlier_attempt(node, number))
}

/// Parses a `"{build_id}:{log_id}"` log id.
pub(crate) fn parse_log_id(log_id: &str) -> Option<(u64, u64)> {
    let (build, log) = log_id.split_once(':')?;
    Some((build.parse().ok()?, log.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{"records":[
        {"id":"t2","parentId":"j1","type":"Task","name":"Test","order":2,"state":"completed","result":"failed","log":{"id":12}},
        {"id":"t1","parentId":"j1","type":"Task","name":"Build","order":1,"state":"completed","result":"succeeded","log":{"id":11},
         "startTime":"2026-10-18T15:07:26.6466667Z","finishTime":"0001-01-01T00:00:00"},
        {"id":"j1","parentId":"p1","type":"Job","name":"Linux","order":1,"state":"inProgress","log":{"id":10}},
        {"id":"p1","parentId":"s1","type":"Phase","name":"Linux","order":1,"state":"inProgress"},
        {"id":"s1","parentId":null,"type":"Stage","name":"Build","order":1,"state":"inProgress"},
        {"id":"s2","parentId":null,"type":"Stage","name":"Publish","order":2,"state":"pending"},
        {"id":"c1","parentId":"s2","type":"Checkpoint","name":"Checkpoint","order":1},
        {"id":"a1","parentId":"c1","type":"Checkpoint.Approval","name":"Approval","order":1},
        {"id":"o1","parentId":"missing","type":"Job","name":"Orphan","order":9,"state":"completed","result":"skipped"}
    ]}"#;

    #[test]
    fn builds_stage_job_task_tree_without_phases_and_checkpoints() {
        let timeline: Timeline = serde_json::from_str(FIXTURE).unwrap();
        let tree =
            build_job_tree(42, &timeline.records, Some("https://x/_build/results?buildId=42"));
        assert_eq!(
            tree.iter().map(|job| job.name.as_str()).collect::<Vec<_>>(),
            ["Build", "Publish", "Orphan"]
        );
        let job = &tree[0].children[0];
        assert_eq!((job.name.as_str(), job.status), ("Linux", RunStatus::Running));
        assert_eq!(job.log_id.as_deref(), Some("42:10"));
        assert_eq!(
            job.web_url.as_deref(),
            Some("https://x/_build/results?buildId=42&view=logs&j=j1")
        );
        let tasks: Vec<_> =
            job.children.iter().map(|task| (task.name.as_str(), task.status)).collect();
        assert_eq!(tasks, [("Build", RunStatus::Succeeded), ("Test", RunStatus::Failed)]);
        assert_eq!(
            job.children[0].web_url.as_deref(),
            Some("https://x/_build/results?buildId=42&view=logs&j=j1&t=t1")
        );
        assert!(job.children[0].started_at.is_some() && job.children[0].finished_at.is_none());
        assert!(tree[1].children.is_empty(), "{:?}", tree[1].children);
        assert_eq!(tree[1].status, RunStatus::Queued);
        assert_eq!(tree[2].status, RunStatus::Skipped);
    }

    #[test]
    fn default_stage_is_transparent() {
        let timeline: Timeline = serde_json::from_str(
            r#"{"records":[
            {"id":"s","type":"Stage","name":"__default","order":1},
            {"id":"p","parentId":"s","type":"Phase","name":"Job","order":1},
            {"id":"j","parentId":"p","type":"Job","name":"Job","order":1,"state":"completed","result":"succeededWithIssues"}
        ]}"#,
        )
        .unwrap();
        let tree = build_job_tree(1, &timeline.records, None);
        assert_eq!(tree.len(), 1);
        assert_eq!((tree[0].name.as_str(), tree[0].status), ("Job", RunStatus::Neutral));
        assert_eq!(tree[0].web_url, None);
    }

    #[test]
    fn stages_get_timing_dependencies_and_retries_once_the_run_finished() {
        let timeline: Timeline = serde_json::from_str(
            r#"{"records":[
            {"id":"a","identifier":"Build_Windows","type":"Stage","name":"Build Windows","order":1,"state":"completed","result":"succeeded",
             "startTime":"2026-10-18T19:05:24Z","finishTime":"2026-10-18T19:08:09Z"},
            {"id":"b","identifier":"Build_OSX","type":"Stage","name":"Build OSX","order":2,"state":"completed","result":"failed",
             "startTime":"2026-10-18T19:05:22Z","finishTime":"2026-10-18T19:09:01Z"},
            {"id":"c","identifier":"E2E","type":"Stage","name":"E2E","order":3,"state":"completed","result":"skipped"}
        ]}"#,
        )
        .unwrap();
        let tree = build_job_tree(7, &timeline.records, None);
        let dependencies: Vec<Vec<&str>> = tree
            .iter()
            .map(|stage| stage.depends_on.iter().map(String::as_str).collect())
            .collect();
        assert_eq!(dependencies, vec![Vec::<&str>::new(), vec![], vec!["b"]]);
        assert_eq!(tree[0].reruns, vec![RerunAction::new(RerunKind::StageAll, "7:Build_Windows")]);
        let kinds: Vec<RerunKind> = tree[1].reruns.iter().map(|rerun| rerun.kind).collect();
        assert_eq!(kinds, [RerunKind::StageFailed, RerunKind::StageAll]);
        assert_eq!(tree[2].reruns, vec![RerunAction::new(RerunKind::StageFailed, "7:E2E")]);
        assert_eq!(parse_stage_target("7:E2E"), Some((7, "E2E")));
        assert_eq!(parse_stage_target("7:"), None);
    }

    /// Trimmed from the timeline of a public build, 1639760 (fetched anonymously on 2026-10-19
    /// while it ran): a running job has no log of its own yet
    /// (`log: null`) while its finished tasks have theirs; the running task and the pending ones
    /// have none. Its phase carries a log of its own. A phase whose condition skipped it has no
    /// Job records at all.
    const RUNNING_FIXTURE: &str = r#"{"records":[
        {"id":"25cdcba1","parentId":null,"type":"Stage","name":"MSBuild Tests","identifier":"msbuild_dotnet","state":"inProgress","result":null,"order":6,"log":null,"startTime":"2026-10-19T02:22:38.86Z","finishTime":null},
        {"id":"a6ddfd4c","parentId":"25cdcba1","type":"Phase","name":"macOS > Tests > MSBuild","identifier":"msbuild_dotnet.mac_msbuild_tests","state":"inProgress","result":null,"order":1,"log":{"id":116},"startTime":"2026-10-19T02:22:42.6433333Z","finishTime":null},
        {"id":"51364297","parentId":"a6ddfd4c","type":"Job","name":"macOS > Tests > MSBuild 9","identifier":"msbuild_dotnet.mac_msbuild_tests.Job9","state":"inProgress","result":null,"order":9,"log":null,"startTime":"2026-10-19T02:23:47.0433333Z","finishTime":null,"workerName":"AcesShared 10"},
        {"id":"t-init","parentId":"51364297","type":"Task","name":"Initialize job","state":"completed","result":"succeeded","order":1,"log":{"id":227}},
        {"id":"t-checkout","parentId":"51364297","type":"Task","name":"Checkout dotnet/android@refs/pull/13176/merge to s","state":"completed","result":"succeeded","order":2,"log":{"id":363}},
        {"id":"t-skip","parentId":"51364297","type":"Task","name":"install .NET Core 10.0","state":"completed","result":"skipped","order":4,"log":{"id":370}},
        {"id":"7a5b871e","parentId":"51364297","type":"Task","name":"run Xamarin.Android.Build.Tests - macOS-9","state":"inProgress","result":null,"order":34,"log":null,"startTime":"2026-10-19T02:29:28.5033333Z","finishTime":null},
        {"id":"8d43628a","parentId":"51364297","type":"Task","name":"diagnose hanging processes (macOS/Linux)","state":"pending","result":null,"order":35,"log":null,"startTime":null,"finishTime":null},
        {"id":"e23aca35","parentId":null,"type":"Stage","name":"MAUI Tests","identifier":"maui_tests","state":"completed","result":"succeeded","order":9,"log":null,"startTime":null,"finishTime":"2026-10-19T02:22:20.2666667Z"},
        {"id":"759c1e91","parentId":"e23aca35","type":"Phase","name":"MAUI Integration","identifier":"maui_tests.maui_tests_integration","state":"completed","result":"skipped","order":1,"log":null,"startTime":null,"finishTime":null},
        {"id":"c-maui","parentId":"e23aca35","type":"Checkpoint","name":"Checkpoint","state":"completed","result":"succeeded","order":1,"log":null}
    ]}"#;

    #[test]
    fn running_jobs_have_no_own_log_but_their_finished_tasks_do() {
        let timeline: Timeline = serde_json::from_str(RUNNING_FIXTURE).unwrap();
        let tree = build_job_tree(
            1_639_760,
            &timeline.records,
            Some("https://dev.azure.com/x/_build/results?buildId=1639760"),
        );
        let job = &tree[0].children[0];
        assert_eq!(
            (job.name.as_str(), job.status, job.log_id.as_deref()),
            ("macOS > Tests > MSBuild 9", RunStatus::Running, None)
        );
        let tasks: Vec<(&str, Option<&str>)> =
            job.children.iter().map(|task| (task.id.as_str(), task.log_id.as_deref())).collect();
        assert_eq!(
            tasks,
            [
                ("t-init", Some("1639760:227")),
                ("t-checkout", Some("1639760:363")),
                ("t-skip", Some("1639760:370")),
                ("7a5b871e", None),
                ("8d43628a", None)
            ]
        );
    }

    #[test]
    fn a_phase_without_jobs_is_shown_as_its_job() {
        let timeline: Timeline = serde_json::from_str(RUNNING_FIXTURE).unwrap();
        let tree = build_job_tree(
            1_639_760,
            &timeline.records,
            Some("https://dev.azure.com/x/_build/results?buildId=1639760"),
        );
        assert_eq!(tree[0].children.len(), 1, "a phase with jobs stays hidden");
        let skipped: Vec<(&str, RunStatus, Option<&str>)> = tree[1]
            .children
            .iter()
            .map(|job| (job.name.as_str(), job.status, job.log_id.as_deref()))
            .collect();
        assert_eq!(skipped, [("MAUI Integration", RunStatus::Skipped, None)]);
        assert_eq!(
            tree[1].children[0].web_url.as_deref(),
            Some("https://dev.azure.com/x/_build/results?buildId=1639760")
        );
    }

    #[test]
    fn running_builds_offer_no_stage_retry() {
        let timeline: Timeline = serde_json::from_str(FIXTURE).unwrap();
        let tree = build_job_tree(42, &timeline.records, None);
        assert!(tree.iter().all(|stage| stage.reruns.is_empty()));
    }

    /// A public build, 1639558, fetched anonymously on 2026-10-19 and trimmed: stage "Build" was
    /// retried; its job "Source-Build (Managed)" failed in both attempts, "Code check" ran once and
    /// stays in attempt 1. "Platform tests" passed on its retry.
    const RETRIED_FIXTURE: &str = include_str!("../../tests/fixtures/timeline_1639558.json");
    /// `GET build/builds/1639558/timeline/6884a131-…`: the first attempt of stage "Build".
    const ATTEMPT_1_FIXTURE: &str =
        include_str!("../../tests/fixtures/timeline_1639558_6884a131.json");
    const BUILD_ATTEMPT_1: &str = "6884a131-87da-5381-61f3-d7acc3b91d76";
    const SOURCE_BUILD_ATTEMPT_1: &str = "2f0d093c-1064-5c86-fc5b-b7b1eca8e66a";

    fn lazy(target: String) -> PipelineAttempt {
        PipelineAttempt { number: 1, source: AttemptSource::Lazy(target) }
    }

    #[test]
    fn retried_stages_and_jobs_list_their_earlier_attempts_by_id() {
        let timeline: Timeline = serde_json::from_str(RETRIED_FIXTURE).unwrap();
        let tree = build_job_tree(1_639_558, &timeline.records, None);
        let build = tree.iter().find(|stage| stage.name == "Build").expect("stage Build");
        assert_eq!(
            (build.status, build.attempt_number(), build.attempt_count()),
            (RunStatus::Failed, 2, 2)
        );
        assert_eq!(
            build.previous_attempts,
            [lazy(format!("1639558:{BUILD_ATTEMPT_1}:{BUILD_ATTEMPT_1}"))]
        );
        let source_build = build
            .children
            .iter()
            .find(|job| job.name == "Source-Build (Managed)")
            .expect("the retried job");
        assert_eq!((source_build.status, source_build.attempt), (RunStatus::Failed, 2));
        assert_eq!(
            source_build.previous_attempts,
            [lazy(format!("1639558:{BUILD_ATTEMPT_1}:{SOURCE_BUILD_ATTEMPT_1}"))]
        );
        let code_check = build
            .children
            .iter()
            .find(|job| job.name == "Code check")
            .expect("the job that ran once");
        assert_eq!((code_check.attempt_number(), code_check.previous_attempts.len()), (1, 0));
        let tasks = &source_build.children;
        assert!(
            tasks.iter().all(|task| task.attempt == 1 && task.previous_attempts.is_empty()),
            "tasks count per job attempt"
        );
        let platform =
            tree.iter().find(|stage| stage.name == "Platform tests").expect("stage Platform tests");
        assert_eq!(
            (platform.status, platform.attempt_count()),
            (RunStatus::Succeeded, 2),
            "passed on retry"
        );
        assert_eq!(
            parse_attempt_target(&format!("1639558:{BUILD_ATTEMPT_1}:{SOURCE_BUILD_ATTEMPT_1}")),
            Some((1_639_558, BUILD_ATTEMPT_1, SOURCE_BUILD_ATTEMPT_1))
        );
        assert_eq!(parse_attempt_target("1639558:x"), None);
    }

    #[test]
    fn an_earlier_attempt_comes_from_its_own_timeline_without_reruns() {
        let timeline: Timeline = serde_json::from_str(ATTEMPT_1_FIXTURE).unwrap();
        let web_url = Some("https://dev.azure.com/contoso/public/_build/results?buildId=1639558");
        let job = attempt_node(1_639_558, &timeline.records, SOURCE_BUILD_ATTEMPT_1, web_url)
            .expect("the job's first attempt");
        assert_eq!(
            (job.name.as_str(), job.status, job.attempt, job.log_id.as_deref()),
            ("Source-Build (Managed)", RunStatus::Failed, 1, Some("1639558:95"))
        );
        let tasks: Vec<(&str, RunStatus, Option<&str>)> = job
            .children
            .iter()
            .map(|task| (task.name.as_str(), task.status, task.log_id.as_deref()))
            .collect();
        assert_eq!(
            tasks,
            [
                ("Initialize job", RunStatus::Succeeded, Some("1639558:75")),
                (
                    "Checkout dotnet/aspnetcore@refs/pull/69916/merge to s",
                    RunStatus::Failed,
                    Some("1639558:87")
                ),
                ("Finalize Job", RunStatus::Succeeded, Some("1639558:94")),
            ]
        );
        let stage = attempt_node(1_639_558, &timeline.records, BUILD_ATTEMPT_1, web_url)
            .expect("the stage's first attempt");
        assert_eq!(
            (stage.name.as_str(), stage.status, stage.children.len()),
            ("Build", RunStatus::Failed, 2)
        );
        assert!(stage.reruns.is_empty(), "an earlier attempt is not rerun; the latest is");
        assert_eq!(attempt_node(1_639_558, &timeline.records, "gone", web_url), None);
    }

    #[test]
    fn parses_log_ids() {
        assert_eq!(parse_log_id("1639583:845"), Some((1_639_583, 845)));
        assert_eq!(parse_log_id("845"), None);
        assert_eq!(parse_log_id("a:1"), None);
    }
}
