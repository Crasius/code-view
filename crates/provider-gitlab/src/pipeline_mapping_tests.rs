use super::*;

fn pipeline(id: u64, sha: &str, status: &str) -> PipelineDto {
    serde_json::from_value(serde_json::json!({
        "id": id, "project_id": 7, "sha": sha, "ref": "refs/merge-requests/1/head", "status": status,
        "source": "merge_request_event", "created_at": "2026-10-16T02:00:31.422Z",
        "updated_at": "2026-10-16T02:08:45.909Z", "web_url": format!("https://gitlab.com/p/-/pipelines/{id}")
    }))
    .expect("fixture")
}

#[test]
fn maps_job_statuses() {
    let cases = [
        ("created", false, RunStatus::Queued),
        ("pending", false, RunStatus::Queued),
        ("waiting_for_resource", false, RunStatus::Queued),
        ("preparing", false, RunStatus::Queued),
        ("scheduled", false, RunStatus::Queued),
        ("manual", false, RunStatus::Neutral),
        ("running", false, RunStatus::Running),
        ("success", false, RunStatus::Succeeded),
        ("failed", false, RunStatus::Failed),
        ("failed", true, RunStatus::Neutral),
        ("canceled", false, RunStatus::Canceled),
        ("skipped", false, RunStatus::Skipped),
        ("something_new", false, RunStatus::Neutral),
    ];
    for (status, allow_failure, expected) in cases {
        assert_eq!(
            run_status(status, allow_failure),
            expected,
            "{status} allow_failure={allow_failure}"
        );
    }
}

#[test]
fn orders_head_pipeline_first_then_head_sha_then_rest() {
    let listed = vec![
        pipeline(5, "merge", "success"),
        pipeline(4, "head", "failed"),
        pipeline(3, "old", "success"),
    ];
    let selected = pipeline_runs(listed, Some(pipeline(5, "merge", "success")), Some("head"), 10);
    let ids: Vec<(&str, bool)> =
        selected.iter().map(|run| (run.id.as_str(), run.is_head)).collect();
    assert_eq!(
        ids,
        vec![("pipeline:7:5", true), ("pipeline:7:4", true), ("pipeline:7:3", false)],
        "only the head commit's runs are head runs"
    );
    let limited = pipeline_runs(
        vec![pipeline(2, "a", "success"), pipeline(1, "b", "success")],
        None,
        Some("b"),
        1,
    );
    assert_eq!((limited[0].id.as_str(), limited[0].is_head), ("pipeline:7:1", true));
}

#[test]
fn without_a_head_the_newest_pipeline_is_the_head_run() {
    let listed = vec![pipeline(9, "a", "running"), pipeline(8, "b", "success")];
    let runs = pipeline_runs(listed, None, None, 10);
    assert_eq!(runs.iter().map(|run| run.is_head).collect::<Vec<_>>(), [true, false]);
    let pushed = pipeline_runs(vec![pipeline(8, "old", "success")], None, Some("new"), 10);
    assert!(!pushed[0].is_head, "a head commit without a pipeline yet leaves the older ones lazy");
}

#[test]
fn maps_pipeline_run() {
    let run = to_run(&pipeline(42, "head", "success"), true);
    assert_eq!(run.id, "pipeline:7:42");
    assert_eq!(run.name, "Merge request pipeline", "named by its kind, not its number");
    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(
        run.description.as_deref(),
        Some("#42 · merge_request_event on refs/merge-requests/1/head")
    );
    assert!(run.has_jobs);
    assert_eq!(
        run.started_at,
        Some(time::macros::datetime!(2026-10-16 02:00:31.422 UTC)),
        "without started_at the run starts at created_at"
    );
    assert_eq!(
        run.finished_at,
        Some(time::macros::datetime!(2026-10-16 02:08:45.909 UTC)),
        "a finished pipeline ends at updated_at"
    );
    let running = to_run(&pipeline(43, "head", "running"), true);
    assert_eq!(running.finished_at, None, "updated_at is no finish time while running");
}

/// `workflow:name` wins; without it the merge request event type, then the source names the kind.
#[test]
fn names_pipelines_by_workflow_name_else_by_kind() {
    let named =
        PipelineDto { name: Some("Release build".into()), ..pipeline(1, "head", "success") };
    assert_eq!(to_run(&named, true).name, "Release build");
    let merged = PipelineDto {
        merge_request_event_type: Some("MERGED_RESULT".into()),
        ..pipeline(2, "head", "success")
    };
    assert_eq!(to_run(&merged, true).name, "Merged results pipeline");
    let pushed = PipelineDto { source: Some("push".into()), ..pipeline(3, "head", "success") };
    assert_eq!(to_run(&pushed, true).name, "Branch pipeline");
    let unknown = PipelineDto { source: None, git_ref: None, ..pipeline(4, "head", "success") };
    let run = to_run(&unknown, true);
    assert_eq!((run.name.as_str(), run.description.as_deref()), ("Pipeline", Some("#4")));
}

#[test]
fn keeps_only_external_statuses() {
    let statuses: Vec<CommitStatusDto> = serde_json::from_str(
        r#"[{"id":1,"name":"rspec","status":"success","target_url":"https://gitlab.com/p/-/jobs/91"},
            {"id":2,"name":"old-ci","status":"success","target_url":"https://gitlab.example.com/p/builds/91"},
            {"id":3,"name":"sonar","status":"failed","description":"Quality gate","target_url":"https://sonar.example.com/x"},
            {"id":4,"name":"deploy-check","status":"pending","target_url":null}]"#,
    )
    .expect("fixture");
    let runs: Vec<PipelineRun> =
        statuses.iter().filter(|status| !is_ci_job_status(status)).map(status_run).collect();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].id, "status:3");
    assert_eq!(runs[0].status, RunStatus::Failed);
    assert!(!runs[0].has_jobs);
    assert_eq!(runs[1].status, RunStatus::Queued);
}

#[test]
fn groups_jobs_into_stages_in_pipeline_order() {
    let jobs: Vec<JobDto> = serde_json::from_str(
        r#"[{"id":13,"name":"deploy","stage":"deploy","status":"manual","allow_failure":true},
            {"id":12,"name":"test-b","stage":"test","status":"failed","allow_failure":false,
             "started_at":"2026-01-01T10:05:00Z","finished_at":"2026-01-01T10:09:00Z"},
            {"id":11,"name":"test-a","stage":"test","status":"success",
             "started_at":"2026-01-01T10:04:00Z","finished_at":"2026-01-01T10:06:00Z"},
            {"id":10,"name":"compile","stage":"build","status":"success","web_url":"https://gitlab.com/p/-/jobs/10"}]"#,
    )
    .expect("fixture");
    let bridges: Vec<JobDto> = serde_json::from_str(
        r#"[{"id":14,"name":"trigger-docs","stage":"deploy","status":"running",
             "downstream_pipeline":{"web_url":"https://gitlab.com/docs/-/pipelines/1"}}]"#,
    )
    .expect("fixture");
    let stages = group_stages(99, 7, jobs, bridges, None);
    let names: Vec<&str> = stages.iter().map(|stage| stage.name.as_str()).collect();
    assert_eq!(names, vec!["build", "test", "deploy"]);
    assert_eq!(
        stage_dependencies(&stages),
        vec![vec![], vec!["stage:99:build"], vec!["stage:99:test"]],
        "no needs: one after another"
    );
    let retries: Vec<(&str, usize)> =
        stages[1].children.iter().map(|job| (job.name.as_str(), job.reruns.len())).collect();
    assert_eq!(retries, vec![("test-a", 1), ("test-b", 1)]);
    assert_eq!(stages[1].children[1].reruns[0], RerunAction::new(RerunKind::Job, "7:12"));
    assert!(
        stages[2].children.iter().all(|job| job.reruns.is_empty()),
        "manual jobs and bridges are not retried"
    );
    let test = &stages[1];
    assert_eq!(test.id, "stage:99:test");
    assert_eq!(test.status, RunStatus::Failed);
    assert_eq!(
        test.children.iter().map(|job| job.name.as_str()).collect::<Vec<_>>(),
        vec!["test-a", "test-b"]
    );
    assert_eq!(test.children[0].log_id.as_deref(), Some("7:11"));
    assert!(test.started_at < test.finished_at);
    let deploy = &stages[2];
    assert_eq!(deploy.status, RunStatus::Running);
    assert_eq!(deploy.finished_at, None);
    assert_eq!(deploy.children[1].log_id, None, "bridges have no log");
    assert_eq!(
        deploy.children[1].web_url.as_deref(),
        Some("https://gitlab.com/docs/-/pipelines/1")
    );
}

fn dag(needs: &[&str]) -> JobNeeds {
    JobNeeds { is_dag: true, needs: needs.iter().map(|name| (*name).to_owned()).collect() }
}

fn by_stage() -> JobNeeds {
    JobNeeds { is_dag: false, needs: Vec::new() }
}

fn needs_of(entries: &[(&str, JobNeeds)]) -> HashMap<String, JobNeeds> {
    entries.iter().map(|(name, job_needs)| ((*name).to_owned(), job_needs.clone())).collect()
}

fn stage_dependencies(stages: &[PipelineJob]) -> Vec<Vec<&str>> {
    stages.iter().map(|stage| stage.depends_on.iter().map(String::as_str).collect()).collect()
}

/// Every job declares `needs: []`, so stages are lanes, not steps. The stages
/// still form the sequential stage graph; no job waits for another.
#[test]
fn stages_keep_their_order_when_every_job_needs_nothing() {
    let jobs: Vec<JobDto> = serde_json::from_str(
        r#"[{"id":1,"name":"tests:unit","stage":"test","status":"success"},
            {"id":2,"name":"lint","stage":"test","status":"success"},
            {"id":3,"name":"build:linux","stage":"build","status":"success"},
            {"id":4,"name":"docs:check","stage":"documentation","status":"success"}]"#,
    )
    .expect("fixture");
    let needs = needs_of(&[
        ("tests:unit", dag(&[])),
        ("lint", dag(&[])),
        ("build:linux", dag(&[])),
        ("docs:check", dag(&[])),
    ]);
    let stages = group_stages(9, 7, jobs, Vec::new(), Some(&needs));
    assert_eq!(
        stage_dependencies(&stages),
        vec![vec![], vec!["stage:9:test"], vec!["stage:9:build"]]
    );
    assert!(stages.iter().flat_map(|stage| &stage.children).all(|job| job.depends_on.is_empty()));
}

#[test]
fn jobs_that_use_needs_list_the_jobs_they_need() {
    let jobs: Vec<JobDto> = serde_json::from_str(
        r#"[{"id":1,"name":"compile","stage":"build","status":"success"},
            {"id":2,"name":"lint","stage":"test","status":"success"},
            {"id":3,"name":"unit","stage":"test","status":"failed"},
            {"id":4,"name":"docs","stage":"docs","status":"success"},
            {"id":5,"name":"ship","stage":"deploy","status":"skipped"}]"#,
    )
    .expect("fixture");
    let needs = needs_of(&[
        ("compile", by_stage()),
        ("lint", dag(&[])),
        ("unit", dag(&["compile", "lint", "optional-gone"])),
        ("docs", dag(&["unit"])),
        ("ship", by_stage()),
    ]);
    let stages = group_stages(9, 7, jobs, Vec::new(), Some(&needs));
    assert_eq!(
        stage_dependencies(&stages),
        vec![vec![], vec!["stage:9:build"], vec!["stage:9:test"], vec!["stage:9:docs"]]
    );
    let job_dependencies: Vec<(&str, Vec<&str>)> = stages
        .iter()
        .flat_map(|stage| &stage.children)
        .map(|job| (job.name.as_str(), job.depends_on.iter().map(String::as_str).collect()))
        .collect();
    assert_eq!(
        job_dependencies,
        vec![
            ("compile", vec![]),
            ("lint", vec![]),
            ("unit", vec!["job:1", "job:2"]),
            ("docs", vec!["job:3"]),
            ("ship", vec![]),
        ],
        "needed jobs outside the pipeline are left out; stage-scheduled jobs list none"
    );
}

/// A jobs list with `include_retried=true`: `jest-integration` failed, was retried and runs again.
/// A retried job shares name and stage with its retry, REST marks neither.
#[test]
fn retried_jobs_become_the_earlier_attempts_of_their_job() {
    let jobs: Vec<JobDto> =
        serde_json::from_str(include_str!("../tests/fixtures/jobs_include_retried.json"))
            .expect("fixture");
    let listed = jobs.len();
    let stages = group_stages(2_943_559_618, 278_964, jobs, Vec::new(), None);
    let shown: usize = stages.iter().map(|stage| stage.children.len()).sum();
    assert_eq!(shown, listed - 1, "the retried job is no job of its own");
    let frontend =
        stages.iter().find(|stage| stage.name == "test-frontend").expect("stage test-frontend");
    assert_eq!(frontend.status, RunStatus::Running, "the stage counts the latest attempt only");
    let jest = frontend
        .children
        .iter()
        .find(|job| job.name == "jest-integration")
        .expect("the retried job");
    assert_eq!(
        (jest.id.as_str(), jest.status, jest.attempt_number()),
        ("job:17271932626", RunStatus::Running, 2)
    );
    assert_eq!(jest.previous_attempts.len(), 1);
    let earlier = jest.previous_attempts[0].job().expect("reported with the run");
    assert_eq!(jest.previous_attempts[0].number, 1);
    assert_eq!(
        (earlier.id.as_str(), earlier.status, earlier.attempt),
        ("job:17148452815", RunStatus::Failed, 1)
    );
    assert_eq!(earlier.log_id.as_deref(), Some("278964:17148452815"), "its own trace");
    assert!(earlier.reruns.is_empty() && earlier.previous_attempts.is_empty());
    assert!(
        frontend
            .children
            .iter()
            .filter(|job| job.name != "jest-integration")
            .all(|job| job.attempt == 1 && job.previous_attempts.is_empty()),
        "jobs that ran once have no earlier attempt"
    );
}

/// A retry gets a new id, higher than every job of the later stages.
#[test]
fn a_retried_job_keeps_its_stage_and_place_in_pipeline_order() {
    let jobs: Vec<JobDto> = serde_json::from_str(
        r#"[{"id":1,"name":"compile","stage":"build","status":"failed"},
            {"id":2,"name":"lint","stage":"build","status":"success"},
            {"id":3,"name":"unit","stage":"test","status":"skipped"},
            {"id":4,"name":"compile","stage":"build","status":"success"}]"#,
    )
    .expect("fixture");
    let stages = group_stages(9, 7, jobs, Vec::new(), None);
    let names: Vec<&str> = stages.iter().map(|stage| stage.name.as_str()).collect();
    assert_eq!(names, ["build", "test"]);
    assert_eq!(stage_dependencies(&stages), vec![vec![], vec!["stage:9:build"]]);
    let build: Vec<&str> = stages[0].children.iter().map(|job| job.id.as_str()).collect();
    assert_eq!(build, ["job:4", "job:2"], "the retry stands where its first attempt stood");
}

/// With every job of the first stage retried, its retries are the newest jobs of the pipeline.
#[test]
fn a_stage_of_retried_jobs_stays_before_the_stages_that_need_it() {
    let jobs: Vec<JobDto> = serde_json::from_str(
        r#"[{"id":1,"name":"compile","stage":"build","status":"failed"},
            {"id":2,"name":"unit","stage":"test","status":"skipped"},
            {"id":3,"name":"compile","stage":"build","status":"success"}]"#,
    )
    .expect("fixture");
    let stages = group_stages(9, 7, jobs, Vec::new(), None);
    let names: Vec<&str> = stages.iter().map(|stage| stage.name.as_str()).collect();
    assert_eq!(names, ["build", "test"]);
    assert_eq!(stage_dependencies(&stages), vec![vec![], vec!["stage:9:build"]]);
}

#[test]
fn failed_pipelines_offer_a_retry() {
    assert_eq!(
        to_run(&pipeline(42, "head", "failed"), true).reruns,
        vec![RerunAction::new(RerunKind::RunFailed, "7:42")]
    );
    let reruns = to_run(&pipeline(42, "head", "success"), true).reruns;
    assert!(reruns.is_empty(), "{reruns:?}");
}
