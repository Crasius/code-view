//! Names of a merge request's pipelines. REST lists them without one: neither
//! `GET /merge_requests/:iid/pipelines` nor the merge request's `head_pipeline` has `name`
//! (<https://docs.gitlab.com/api/merge_requests/#list-merge-request-pipelines>). GraphQL has
//! `Pipeline.name` (`workflow:name`) and `Pipeline.mergeRequestEventType`
//! (<https://docs.gitlab.com/api/graphql/reference/#pipeline>), for every pipeline of the merge
//! request in one request. A host that lacks both fields is not asked again, and a pipeline named
//! once is not asked about again: reloads of running pipelines cost no GraphQL request.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use review_core::{Result, http};
use serde::Deserialize;
use serde_json::json;

use crate::client::ApiClient;
use crate::dto::PipelineDto;
use crate::graphql::{Nodes, SchemaGaps};

/// Pipelines whose labels a connection keeps (our number; about 100 bytes each).
const MAX_KNOWN: usize = 4_096;

/// What GraphQL tells about one pipeline beyond REST.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Label {
    pub name: Option<String>,
    pub merge_request_event_type: Option<String>,
}

/// The labels a connection read so far, by pipeline id.
#[derive(Default)]
pub(crate) struct KnownLabels(Mutex<HashMap<u64, Label>>);

impl KnownLabels {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Label>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Names `pipelines` of merge request `iid` in project `project_id`; asks GraphQL only when one of
/// them was not named before. Names make the runs readable and nothing depends on them, so the
/// question never waits out a GraphQL rate limit, and a refused or failed one leaves the runs
/// named by their kind until the next load asks again.
pub(crate) async fn name(
    client: &ApiClient,
    project_id: u64,
    iid: u64,
    pipelines: &mut [&mut PipelineDto],
) {
    let known = client.pipeline_labels();
    let is_new = {
        let labels = known.lock();
        pipelines.iter().any(|pipeline| !labels.contains_key(&pipeline.id))
    };
    if is_new && let Ok(mut fresh) = http::without_waiting(labels(client, project_id, iid)).await {
        if !fields(client.schema_gaps()).is_empty() {
            // Pipelines GraphQL did not list have no label to wait for either.
            for pipeline in pipelines.iter() {
                fresh.entry(pipeline.id).or_default();
            }
        }
        let mut labels = known.lock();
        if labels.len() + fresh.len() > MAX_KNOWN {
            labels.clear();
        }
        labels.extend(fresh);
    }
    let labels = known.lock();
    for pipeline in pipelines.iter_mut() {
        apply(pipeline, &labels);
    }
}

/// The labels of the pipelines of merge request `iid` in project `project_id`, by pipeline id.
async fn labels(client: &ApiClient, project_id: u64, iid: u64) -> Result<HashMap<u64, Label>> {
    if fields(client.schema_gaps()).is_empty() {
        return Ok(HashMap::new());
    }
    let variables = json!({
        "project": format!("gid://gitlab/Project/{project_id}"),
        "iid": iid.to_string(),
        "first": crate::pipelines::MAX_LISTED_PIPELINES,
    });
    Ok(labels_of(client.graphql_fitted(query, variables).await?))
}

fn labels_of(data: Data) -> HashMap<u64, Label> {
    let pipelines = data
        .projects
        .into_items()
        .filter_map(|project| project.merge_request)
        .flat_map(|merge_request| merge_request.pipelines.into_items());
    pipelines
        .filter_map(|pipeline| {
            let id = pipeline.id.rsplit('/').next()?.parse().ok()?;
            let label = Label {
                name: pipeline.name.filter(|name| !name.trim().is_empty()),
                merge_request_event_type: pipeline.merge_request_event_type,
            };
            Some((id, label))
        })
        .collect()
}

/// Gives `pipeline` its name and event type where GraphQL knows them.
fn apply(pipeline: &mut PipelineDto, labels: &HashMap<u64, Label>) {
    if let Some(label) = labels.get(&pipeline.id) {
        if label.name.is_some() {
            pipeline.name.clone_from(&label.name);
        }
        pipeline.merge_request_event_type.clone_from(&label.merge_request_event_type);
    }
}

/// The pipeline fields the host is known to have.
fn fields(schema: &SchemaGaps) -> String {
    ["name", "mergeRequestEventType"]
        .into_iter()
        .filter(|field| schema.has_field("Pipeline", field))
        .collect::<Vec<_>>()
        .join(" ")
}

fn query(schema: &SchemaGaps) -> String {
    let fields = fields(schema);
    format!(
        "query($project: ID!, $iid: String!, $first: Int) {{
  projects(ids: [$project]) {{ nodes {{ mergeRequest(iid: $iid) {{
    pipelines(first: $first) {{ nodes {{ id {fields} }} }}
  }} }} }}
}}"
    )
}

#[derive(Deserialize)]
struct Data {
    projects: Nodes<ProjectNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectNode {
    merge_request: Option<MergeRequestNode>,
}

#[derive(Deserialize)]
struct MergeRequestNode {
    pipelines: Nodes<PipelineNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PipelineNode {
    /// A global id: `gid://gitlab/Ci::Pipeline/2695`.
    id: String,
    name: Option<String>,
    merge_request_event_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::GraphqlError;

    fn unknown_pipeline_field(field: &str) -> Vec<GraphqlError> {
        serde_json::from_value(serde_json::json!([{
            "message": format!("Field '{field}' doesn't exist on type 'Pipeline'"),
            "extensions": {"code": "undefinedField", "typeName": "Pipeline", "fieldName": field}
        }]))
        .expect("fixture")
    }

    #[test]
    fn asks_only_for_the_fields_the_host_has() {
        let schema = SchemaGaps::default();
        assert!(schema.learn(&unknown_pipeline_field("name")));
        assert!(query(&schema).contains("nodes { id mergeRequestEventType }"));
        assert!(schema.learn(&unknown_pipeline_field("mergeRequestEventType")));
        assert!(fields(&schema).is_empty(), "a host without both fields is not asked");
    }

    #[test]
    fn names_reach_the_listed_pipelines() {
        let data: Data = serde_json::from_str(
            r#"{"projects": {"nodes": [{"mergeRequest": {"pipelines": {"nodes": [
                {"id": "gid://gitlab/Ci::Pipeline/2695", "name": "Release build", "mergeRequestEventType": "MERGED_RESULT"},
                {"id": "gid://gitlab/Ci::Pipeline/2690", "name": null, "mergeRequestEventType": "DETACHED"}
            ]}}}]}}"#,
        )
        .expect("fixture");
        let labels = labels_of(data);
        let mut pipeline: PipelineDto = serde_json::from_value(serde_json::json!({
            "id": 2690, "project_id": 7, "sha": "a", "status": "success", "name": "kept"
        }))
        .expect("fixture");
        apply(&mut pipeline, &labels);
        assert_eq!(pipeline.name.as_deref(), Some("kept"), "no name over GraphQL keeps REST's");
        assert_eq!(pipeline.merge_request_event_type.as_deref(), Some("DETACHED"));
        assert_eq!(labels[&2695].name.as_deref(), Some("Release build"));
    }
}
