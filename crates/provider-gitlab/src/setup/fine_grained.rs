//! Access checks of fine-grained tokens. Reads are probed with `GET` requests on the most recently
//! active project; writes are read from `granular_scopes` when GitLab reports them (19.5+), else
//! they stay unverified, because only reads may run during a check.

use reqwest::Method;
use review_core::setup::{Access, Feature, FeatureCheck};
use review_core::{ProviderError, Result};
use serde::Deserialize;

use crate::client::{ApiClient, project_path};

/// Fine-grained permissions the app uses, as GitLab's token page names them.
pub(super) const PERMISSIONS: [(&str, &str); 14] = [
    ("Group and project access", "The groups or projects you review."),
    ("User: Read", "Sign in and show who you are."),
    ("Group: Read", "List your groups."),
    ("Project: Read", "List projects and repositories."),
    ("Merge Request: Read", "Read merge requests, approvals, discussions, diffs and pipelines."),
    ("Merge Request: Approve", "Approve and revoke approvals."),
    ("Merge Request: Create", "Start discussions and reply."),
    ("Merge Request: Update", "Resolve threads, edit labels, title and draft state."),
    ("Merge Request: Merge", "Merge."),
    ("Label: Read", "Offer the project's labels."),
    ("Pipeline: Read", "Pipelines and their jobs."),
    ("Job: Read", "Job logs."),
    ("Code: Download", "Fetch code over git."),
    ("Personal Access Token: Read", "Let the app check this token."),
];

#[derive(Debug, Deserialize)]
struct ProjectRefDto {
    id: u64,
    path_with_namespace: String,
}

#[derive(Debug, Deserialize)]
struct MergeRequestRefDto {
    iid: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Probe {
    Allowed,
    Denied,
}

/// The checks of a fine-grained token whose `granular_scopes` list `granted` (empty when GitLab
/// does not report them).
pub(super) async fn checks(client: &ApiClient, granted: &[String]) -> Result<Vec<FeatureCheck>> {
    let project = match sample_project(client).await? {
        Ok(project) => project,
        Err(missing) => {
            return Ok(Feature::ALL
                .iter()
                .map(|feature| FeatureCheck::new(*feature, Access::Missing, missing))
                .collect());
        }
    };
    let base = project_path(&project.id.to_string());
    let on = &project.path_with_namespace;
    let read = merge_request_read(client, &base).await?;
    let (labels, pipelines, jobs) = tokio::try_join!(
        probe(client, format!("{base}/labels")),
        probe(client, format!("{base}/pipelines")),
        probe(client, format!("{base}/jobs")),
    )?;
    let granted = Granted::new(granted);
    Ok(vec![
        read_check(read, on),
        granted.write_check(Feature::Vote, &["Merge Request: Approve"]),
        granted.write_check(Feature::Comment, &["Merge Request: Create", "Merge Request: Update"]),
        labels_check(labels, &granted),
        granted.write_check(Feature::Merge, &["Merge Request: Merge"]),
        pipelines_check(pipelines, jobs, on),
        granted.write_check(Feature::GitClone, &["Code: Download"]),
    ])
}

/// The most recently active project the token sees, or why there is none to probe on.
async fn sample_project(
    client: &ApiClient,
) -> Result<std::result::Result<ProjectRefDto, &'static str>> {
    let query = [
        ("membership", "true".to_owned()),
        ("simple", "true".to_owned()),
        ("order_by", "last_activity_at".to_owned()),
        ("per_page", "1".to_owned()),
    ];
    match client.get::<Vec<ProjectRefDto>>("/projects", &query).await {
        Ok(projects) => Ok(projects.into_iter().next().ok_or(
            "The token sees no project. Add groups or projects under Group and project access.",
        )),
        Err(ProviderError::Status { status: 403, .. }) => Ok(Err("Needs Project: Read.")),
        Err(error) => Err(error),
    }
}

/// Merge Request: Read, confirmed on the approvals of the newest merge request when there is one.
async fn merge_request_read(client: &ApiClient, base: &str) -> Result<Probe> {
    let listed = client
        .get::<Vec<MergeRequestRefDto>>(
            &format!("{base}/merge_requests"),
            &[("per_page", "1".to_owned())],
        )
        .await;
    match listed {
        Ok(merge_requests) => match merge_requests.first() {
            Some(merge_request) => {
                probe(client, format!("{base}/merge_requests/{}/approvals", merge_request.iid))
                    .await
            }
            None => Ok(Probe::Allowed),
        },
        Err(ProviderError::Status { status: 403, .. }) => Ok(Probe::Denied),
        Err(error) => Err(error),
    }
}

async fn probe(client: &ApiClient, path: String) -> Result<Probe> {
    match client.send(client.request(Method::GET, &path).query(&[("per_page", "1")])).await {
        Ok(_) => Ok(Probe::Allowed),
        Err(ProviderError::Status { status: 403, .. }) => Ok(Probe::Denied),
        Err(error) => Err(error),
    }
}

/// `"Merge Request: Read"` → `"read_merge_request"`, the form `granular_scopes` lists.
pub(super) fn permission_key(permission: &str) -> String {
    let (resource, action) = permission.split_once(':').unwrap_or((permission, ""));
    let resource = resource.trim().to_lowercase().replace(' ', "_");
    format!("{}_{resource}", action.trim().to_lowercase())
}

/// The permissions `granular_scopes` reported.
struct Granted<'a> {
    keys: &'a [String],
    /// Trust the reported list only when it uses the permission names the app knows.
    is_reported: bool,
}

impl<'a> Granted<'a> {
    fn new(keys: &'a [String]) -> Self {
        let mut granted = Self { keys, is_reported: false };
        granted.is_reported = PERMISSIONS.iter().any(|(permission, _)| granted.lists(permission));
        granted
    }

    fn lists(&self, permission: &str) -> bool {
        let key = permission_key(permission);
        self.keys.contains(&key)
    }

    /// A write feature: granted or missing when the token reports its permissions, else
    /// unverified.
    fn write_check(&self, feature: Feature, permissions: &[&str]) -> FeatureCheck {
        let names = permissions.join(" and ");
        if !self.is_reported {
            return FeatureCheck::new(
                feature,
                Access::Unverified,
                format!(
                    "Needs {names}. Only a write could confirm it, so make sure it is selected."
                ),
            );
        }
        if permissions.iter().all(|permission| self.lists(permission)) {
            FeatureCheck::new(feature, Access::Granted, format!("{names} granted."))
        } else {
            FeatureCheck::new(feature, Access::Missing, format!("Needs {names}."))
        }
    }
}

fn read_check(read: Probe, on: &str) -> FeatureCheck {
    match read {
        Probe::Allowed => FeatureCheck::new(
            Feature::ReadPullRequests,
            Access::Granted,
            format!("Merge Request: Read confirmed on {on}."),
        ),
        Probe::Denied => FeatureCheck::new(
            Feature::ReadPullRequests,
            Access::Missing,
            "Needs Merge Request: Read.",
        ),
    }
}

fn labels_check(labels: Probe, granted: &Granted<'_>) -> FeatureCheck {
    match labels {
        Probe::Denied => FeatureCheck::new(
            Feature::Labels,
            Access::Missing,
            "Needs Label: Read and Merge Request: Update.",
        ),
        Probe::Allowed => granted.write_check(Feature::Labels, &["Merge Request: Update"]),
    }
}

fn pipelines_check(pipelines: Probe, jobs: Probe, on: &str) -> FeatureCheck {
    match (pipelines, jobs) {
        (Probe::Allowed, Probe::Allowed) => FeatureCheck::new(
            Feature::Pipelines,
            Access::Granted,
            format!("Pipeline: Read and Job: Read confirmed on {on}."),
        ),
        (Probe::Denied, Probe::Denied) => FeatureCheck::new(
            Feature::Pipelines,
            Access::Missing,
            "Needs Pipeline: Read and Job: Read.",
        ),
        (Probe::Denied, Probe::Allowed) => {
            FeatureCheck::new(Feature::Pipelines, Access::Missing, "Needs Pipeline: Read.")
        }
        (Probe::Allowed, Probe::Denied) => FeatureCheck::new(
            Feature::Pipelines,
            Access::Limited,
            "Pipelines show, but job logs need Job: Read.",
        ),
    }
}
