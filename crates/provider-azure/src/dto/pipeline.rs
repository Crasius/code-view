use std::collections::HashMap;

use review_core::{PipelineRun, RunStatus, Timestamp};
use serde::Deserialize;

use super::Href;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct StatusContext {
    #[serde(default)]
    pub name: String,
    pub genre: Option<String>,
}

/// `GitPullRequestStatus`: a status posted on the pull request by a policy, bot or external CI.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PullRequestStatus {
    pub id: u64,
    pub state: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub context: StatusContext,
    pub target_url: Option<String>,
    #[serde(default, deserialize_with = "super::date::optional")]
    pub creation_date: Option<Timestamp>,
    #[serde(default, deserialize_with = "super::date::optional")]
    pub updated_date: Option<Timestamp>,
    /// Set when the status was posted on one iteration (push) rather than the pull request.
    pub iteration_id: Option<u32>,
}

impl PullRequestStatus {
    fn context_key(&self) -> String {
        match self.context.genre.as_deref().filter(|genre| !genre.is_empty()) {
            Some(genre) => format!("{genre}/{}", self.context.name),
            None => self.context.name.clone(),
        }
    }

    /// `latest_iteration` is the pull request's newest iteration, when a status names one.
    pub fn to_run(&self, latest_iteration: Option<u32>) -> PipelineRun {
        let status = status_state_from_ado(self.state.as_deref());
        PipelineRun {
            id: format!("status:{}", self.id),
            name: self.context_key(),
            status,
            description: self.description.clone().filter(|description| !description.is_empty()),
            web_url: self.target_url.clone().filter(|url| !url.is_empty()),
            started_at: self.creation_date,
            finished_at: if status.is_finished() {
                self.updated_date.or(self.creation_date)
            } else {
                None
            },
            has_jobs: false,
            // Posted on the pull request as it is now, unless on an iteration before the latest.
            is_head: self
                .iteration_id
                .zip(latest_iteration)
                .is_none_or(|(iteration, latest)| iteration >= latest),
            reruns: Vec::new(),
        }
    }
}

/// Statuses are append-only; the highest id per context (genre/name) is the current one. One
/// posted on an iteration before `latest_iteration` is an older run.
pub(crate) fn latest_statuses(
    statuses: &[PullRequestStatus],
    latest_iteration: Option<u32>,
) -> Vec<PipelineRun> {
    let mut latest: HashMap<String, &PullRequestStatus> = HashMap::new();
    for status in statuses {
        let entry = latest.entry(status.context_key()).or_insert(status);
        if status.id > entry.id {
            *entry = status;
        }
    }
    let mut runs: Vec<PipelineRun> =
        latest.values().map(|status| status.to_run(latest_iteration)).collect();
    runs.sort_by(|a, b| a.name.cmp(&b.name));
    runs
}

pub(crate) fn status_state_from_ado(state: Option<&str>) -> RunStatus {
    match state {
        Some("succeeded") => RunStatus::Succeeded,
        Some("failed" | "error") => RunStatus::Failed,
        Some("pending") => RunStatus::Running,
        Some("notApplicable") => RunStatus::Neutral,
        _ => RunStatus::Queued,
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DefinitionRef {
    pub id: u64,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct BuildLinks {
    pub web: Option<Href>,
}

/// The repository a build's pipeline builds: `TfsGit` for Azure Repos, else `GitHub`,
/// `Bitbucket` and so on.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct BuildRepository {
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(clippy::struct_field_names, reason = "fields mirror the Build JSON keys")]
pub(crate) struct Build {
    pub id: u64,
    pub build_number: Option<String>,
    pub status: Option<String>,
    pub result: Option<String>,
    #[serde(default)]
    pub definition: DefinitionRef,
    #[serde(default, deserialize_with = "super::date::optional")]
    pub start_time: Option<Timestamp>,
    #[serde(default, deserialize_with = "super::date::optional")]
    pub finish_time: Option<Timestamp>,
    #[serde(rename = "_links", default)]
    pub links: BuildLinks,
    /// The commit built: for a pull request build the merge commit of `refs/pull/{id}/merge`.
    pub source_version: Option<String>,
    /// Pull request builds name the source commit they validate in `pr.sourceSha`.
    #[serde(default)]
    pub trigger_info: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub repository: BuildRepository,
}

impl Build {
    /// Whether the build ran for an Azure Repos repository. A project's pipelines can build GitHub
    /// repositories too, whose pull request builds use the same `refs/pull/{number}/merge` names:
    /// in a public project that also builds GitHub repositories, each one's pull request #N lands
    /// there. Those are never the builds of an Azure Repos pull request. Builds that name no repository type count as ours.
    pub fn is_azure_repos(&self) -> bool {
        self.repository.kind.as_deref().is_none_or(|kind| kind.eq_ignore_ascii_case("TfsGit"))
    }

    /// The pull request commit the build validates: its source commit, else the commit built.
    fn commit(&self) -> Option<&str> {
        self.trigger_info
            .get("pr.sourceSha")
            .and_then(serde_json::Value::as_str)
            .or(self.source_version.as_deref())
    }

    pub fn to_run(&self, is_head: bool) -> PipelineRun {
        PipelineRun {
            id: format!("build:{}", self.id),
            name: self.definition.name.clone(),
            status: build_status_from_ado(self.status.as_deref(), self.result.as_deref()),
            description: self.build_number.clone(),
            web_url: self.links.web.as_ref().and_then(|web| web.href.clone()),
            started_at: self.start_time,
            finished_at: self.finish_time,
            has_jobs: true,
            is_head,
            reruns: Vec::new(),
        }
    }
}

/// Builds arrive newest first; the first one per definition is that definition's latest run.
/// The newest build validates the pull request's latest commit: the builds of that commit are the
/// head runs, a definition whose latest build validated an earlier push is an older run. A build
/// that names no commit is a head run only as the newest. Builds of other repository types are
/// left out (see [`Build::is_azure_repos`]): `repositoryId` would leave them out on the server, but
/// only given the repository's GUID, which costs another request.
pub(crate) fn latest_builds(builds: &[Build]) -> Vec<PipelineRun> {
    let ours: Vec<&Build> = builds.iter().filter(|build| build.is_azure_repos()).collect();
    let head = ours.first().and_then(|build| build.commit());
    let mut seen = std::collections::HashSet::new();
    ours.iter()
        .enumerate()
        .filter(|(_, build)| seen.insert(build.definition.id))
        .map(|(index, build)| {
            build.to_run(index == 0 || (head.is_some() && build.commit() == head))
        })
        .collect()
}

/// Build `status` (lifecycle) plus `result` (outcome once completed).
pub(crate) fn build_status_from_ado(status: Option<&str>, result: Option<&str>) -> RunStatus {
    match status {
        Some("completed") => result_from_ado(result),
        Some("inProgress" | "cancelling") => RunStatus::Running,
        _ => RunStatus::Queued,
    }
}

/// Outcome shared by builds (`partiallySucceeded`) and timeline records (`succeededWithIssues`, `abandoned`).
pub(crate) fn result_from_ado(result: Option<&str>) -> RunStatus {
    match result {
        Some("succeeded") => RunStatus::Succeeded,
        Some("failed") => RunStatus::Failed,
        Some("canceled" | "abandoned") => RunStatus::Canceled,
        Some("skipped") => RunStatus::Skipped,
        // `partiallySucceeded`, `succeededWithIssues`, and results newer than this mapping.
        _ => RunStatus::Neutral,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_latest_status_per_context() {
        let statuses: super::super::ValueList<PullRequestStatus> = serde_json::from_str(
            r#"{"value":[
              {"id":1,"state":"pending","context":{"name":"build","genre":"ci"},"creationDate":"2026-01-01T10:00:00Z"},
              {"id":2,"state":"succeeded","description":"ok","context":{"name":"build","genre":"ci"},
               "targetUrl":"https://ci/2","creationDate":"2026-01-01T10:05:00Z","updatedDate":"2026-01-01T10:06:00Z"},
              {"id":3,"state":"notApplicable","context":{"name":"lint"},"creationDate":"2026-01-01T10:00:00Z"}
            ]}"#,
        )
        .unwrap();
        let runs = latest_statuses(&statuses.value, None);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, "status:2");
        assert_eq!(runs[0].name, "ci/build");
        assert_eq!(runs[0].status, RunStatus::Succeeded);
        assert!(runs[0].finished_at.is_some() && !runs[0].has_jobs);
        assert_eq!(runs[1].status, RunStatus::Neutral);
    }

    /// A status posted on iteration 1 still is the latest of its context after a push made
    /// iteration 2; it judged the earlier push, so a failure there is no failure of the head.
    #[test]
    fn a_status_of_an_earlier_iteration_is_an_older_run() {
        let statuses: super::super::ValueList<PullRequestStatus> = serde_json::from_str(
            r#"{"value":[
              {"id":1,"state":"failed","iterationId":1,"context":{"name":"sonar"}},
              {"id":2,"state":"succeeded","iterationId":2,"context":{"name":"lint"}},
              {"id":3,"state":"succeeded","context":{"name":"bot"}}
            ]}"#,
        )
        .unwrap();
        let heads: Vec<(String, bool)> = latest_statuses(&statuses.value, Some(2))
            .into_iter()
            .map(|run| (run.name, run.is_head))
            .collect();
        assert_eq!(
            heads,
            [("bot".to_owned(), true), ("lint".to_owned(), true), ("sonar".to_owned(), false)]
        );
    }

    #[test]
    fn keeps_newest_build_per_definition() {
        let builds: super::super::ValueList<Build> = serde_json::from_str(
            r#"{"value":[
              {"id":30,"buildNumber":"20261018.3","status":"inProgress","definition":{"id":1,"name":"ci"},
               "startTime":"2026-10-18T14:39:44.4663546Z","_links":{"web":{"href":"https://dev.azure.com/o/p/_build/results?buildId=30"}}},
              {"id":20,"status":"completed","result":"failed","definition":{"id":1,"name":"ci"}},
              {"id":10,"status":"completed","result":"partiallySucceeded","definition":{"id":2,"name":"docs"}}
            ]}"#,
        )
        .unwrap();
        let runs = latest_builds(&builds.value);
        assert_eq!(
            runs.iter().map(|run| run.id.as_str()).collect::<Vec<_>>(),
            ["build:30", "build:10"]
        );
        assert_eq!(
            runs.iter().map(|run| run.is_head).collect::<Vec<_>>(),
            [true, false],
            "without commits only the newest is a head run"
        );
        assert_eq!(runs[0].status, RunStatus::Running);
        assert_eq!(runs[0].description.as_deref(), Some("20261018.3"));
        assert!(runs[0].web_url.is_some() && runs[0].has_jobs);
        assert_eq!(runs[1].status, RunStatus::Neutral);
    }

    /// The build list shape of a public project (`triggerInfo` of a pull request build, 2026-10-19).
    #[test]
    fn head_runs_validate_the_newest_builds_commit() {
        let builds: super::super::ValueList<Build> = serde_json::from_str(
            r#"{"value":[
              {"id":40,"status":"inProgress","definition":{"id":1,"name":"ci"},"sourceVersion":"merge2",
               "triggerInfo":{"pr.sourceSha":"head2","pr.number":"254","pr.isFork":"False"}},
              {"id":39,"status":"completed","result":"succeeded","definition":{"id":2,"name":"docs"},"sourceVersion":"merge1b",
               "triggerInfo":{"pr.sourceSha":"head2"}},
              {"id":30,"status":"completed","result":"failed","definition":{"id":1,"name":"ci"},"sourceVersion":"merge1",
               "triggerInfo":{"pr.sourceSha":"head1"}},
              {"id":20,"status":"completed","result":"failed","definition":{"id":3,"name":"perf"},"sourceVersion":"merge1",
               "triggerInfo":{"pr.sourceSha":"head1"}},
              {"id":10,"status":"completed","result":"succeeded","definition":{"id":4,"name":"manual"},"sourceVersion":"merge2"}
            ]}"#,
        )
        .unwrap();
        let runs = latest_builds(&builds.value);
        let heads: Vec<(&str, bool)> =
            runs.iter().map(|run| (run.id.as_str(), run.is_head)).collect();
        // docs ran again after the target branch moved (another merge commit, same source commit);
        // perf last ran on the earlier push; the manual build names only the merge commit.
        assert_eq!(
            heads,
            [("build:40", true), ("build:39", true), ("build:20", false), ("build:10", false)]
        );
    }

    /// A public project that also builds GitHub repositories: `branchName=refs/pull/5/merge` there
    /// also lists the builds of each GitHub repository's pull request #5. An Azure Repos pull request
    /// #5 must not show them, nor take its head commit from them.
    #[test]
    fn leaves_out_builds_of_other_repository_types() {
        let builds: super::super::ValueList<Build> = serde_json::from_str(
            r#"{"value":[
              {"id":50,"status":"inProgress","definition":{"id":129,"name":"runtime"},"sourceVersion":"gh-merge",
               "triggerInfo":{"pr.sourceSha":"gh-head"},"repository":{"id":"contoso/api","type":"GitHub"}},
              {"id":41,"status":"completed","result":"succeeded","definition":{"id":7,"name":"wiki"},"sourceVersion":"merge2",
               "triggerInfo":{"pr.sourceSha":"head2"},"repository":{"id":"5f9b6a1e-0000-4000-8000-000000000001","type":"TfsGit"}},
              {"id":40,"status":"completed","result":"failed","definition":{"id":8,"name":"lint"},"sourceVersion":"merge1",
               "triggerInfo":{"pr.sourceSha":"head1"},"repository":{"id":"5f9b6a1e-0000-4000-8000-000000000001","type":"TfsGit"}}
            ]}"#,
        )
        .unwrap();
        let runs = latest_builds(&builds.value);
        let heads: Vec<(&str, bool)> =
            runs.iter().map(|run| (run.id.as_str(), run.is_head)).collect();
        assert_eq!(
            heads,
            [("build:41", true), ("build:40", false)],
            "the GitHub build is gone and the head comes from ours"
        );
    }

    /// Head detection on a live build list: `REVIEW_ADO_BUILDS_JSON` is the answer of an anonymous
    /// `GET {project}/_apis/build/builds?branchName=refs/pull/{n}/merge&queryOrder=queueTimeDescending&$top=50`,
    /// `REVIEW_ADO_HEAD` the pull request's head commit. Prints how every listed build counts.
    #[test]
    #[ignore = "needs a downloaded build list; run explicitly"]
    fn live_build_list_head_detection() {
        let path = std::env::var("REVIEW_ADO_BUILDS_JSON")
            .expect("REVIEW_ADO_BUILDS_JSON names the saved build list");
        let head = std::env::var("REVIEW_ADO_HEAD").expect("REVIEW_ADO_HEAD names the head commit");
        let mut builds: super::super::ValueList<Build> =
            serde_json::from_str(&std::fs::read_to_string(path).expect("build list"))
                .expect("a build list");
        for build in &builds.value {
            let source = build.trigger_info.get("pr.sourceSha").and_then(serde_json::Value::as_str);
            println!(
                "build {} definition {} ({}) repository {:?}: pr.sourceSha {:?}, sourceVersion {:?}",
                build.id,
                build.definition.id,
                build.definition.name,
                build.repository.kind,
                source,
                build.source_version
            );
        }
        let with_source_sha = builds
            .value
            .iter()
            .filter(|build| build.trigger_info.contains_key("pr.sourceSha"))
            .count();
        println!("{with_source_sha} of {} builds carry pr.sourceSha", builds.value.len());
        // Public projects with pull request builds build GitHub pull requests; read them as ours.
        for build in &mut builds.value {
            build.repository.kind = None;
        }
        let commits: HashMap<String, Option<String>> = builds
            .value
            .iter()
            .map(|build| (format!("build:{}", build.id), build.commit().map(str::to_owned)))
            .collect();
        for run in latest_builds(&builds.value) {
            let commit = commits[&run.id].as_deref();
            println!(
                "{} ({}) validated {commit:?}: {}",
                run.id,
                run.name,
                if run.is_head { "head run" } else { "older run" }
            );
            assert_eq!(
                run.is_head,
                commit == Some(head.as_str()),
                "{} is a head run exactly when it validated the head commit",
                run.id
            );
        }
    }

    #[test]
    fn maps_build_lifecycle() {
        assert_eq!(build_status_from_ado(Some("notStarted"), None), RunStatus::Queued);
        assert_eq!(build_status_from_ado(Some("postponed"), None), RunStatus::Queued);
        assert_eq!(build_status_from_ado(Some("completed"), Some("canceled")), RunStatus::Canceled);
        assert_eq!(
            build_status_from_ado(Some("completed"), Some("succeeded")),
            RunStatus::Succeeded
        );
        assert_eq!(status_state_from_ado(Some("error")), RunStatus::Failed);
        assert_eq!(status_state_from_ado(Some("notSet")), RunStatus::Queued);
    }
}
