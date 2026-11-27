//! Work items and referenced pull requests (api-version 7.1, all documented on learn.microsoft.com):
//!
//! - `GET {org}/{project}/_apis/git/repositories/{repo}/pullRequests/{id}/workitems` lists the
//!   work items linked to a pull request.
//! - `POST {org}/_apis/wit/workitemsbatch` with `ids` (at most 200), `fields` and
//!   `errorPolicy: omit` reads them in one request; the project segment is optional.
//! - `GET {org}/_apis/git/pullrequests/{id}` reads a pull request by its organization-wide id,
//!   project optional, so `!12` resolves whichever repository it lives in. There is no batch
//!   variant; at most [`MAX_PULL_REQUESTS`] are read per call.

use std::collections::BTreeMap;

use reqwest::Method;
use review_core::{ItemKind, ItemRef, LinkedItem, LinkedState, PullRequestState, Result};
use serde::Deserialize;
use serde_json::json;

use crate::client::{Client, RepoKey};
use crate::dto::ValueList;
use crate::dto::pull_request::{PullRequest, state_from_ado};

const MAX_WORK_ITEMS: usize = 200;
const MAX_PULL_REQUESTS: usize = 20;
const FIELDS: [&str; 5] =
    ["System.Id", "System.Title", "System.State", "System.WorkItemType", "System.TeamProject"];

#[derive(Debug, Deserialize)]
struct ResourceRef {
    id: String,
}

#[derive(Debug, Deserialize)]
struct WorkItem {
    id: u64,
    #[serde(default)]
    fields: WorkItemFields,
}

#[derive(Debug, Default, Deserialize)]
struct WorkItemFields {
    #[serde(rename = "System.Title", default)]
    title: String,
    #[serde(rename = "System.State", default)]
    state: String,
    #[serde(rename = "System.WorkItemType")]
    work_item_type: Option<String>,
    #[serde(rename = "System.TeamProject")]
    project: Option<String>,
}

pub(crate) async fn references(client: &Client, repo: &str, number: u64) -> Result<Vec<ItemRef>> {
    let key = RepoKey::parse(repo)?;
    let list: ValueList<ResourceRef> =
        client.get(&client.pull_request_api(key, number, "workitems"), &[]).await?;
    Ok(list
        .value
        .iter()
        .filter_map(|item| item.id.parse().ok())
        .map(|id| ItemRef {
            kind: ItemKind::WorkItem,
            repository_key: format!("{}/{}", key.organization, key.project),
            number: id,
        })
        .collect())
}

pub(crate) async fn items(client: &Client, references: &[ItemRef]) -> Result<Vec<LinkedItem>> {
    let mut items = Vec::new();
    let mut work_items: BTreeMap<&str, Vec<&ItemRef>> = BTreeMap::new();
    let mut pull_requests = Vec::new();
    for reference in references {
        let Some(organization) = reference
            .repository_key
            .split('/')
            .next()
            .filter(|organization| !organization.is_empty())
        else {
            continue;
        };
        match reference.kind {
            ItemKind::WorkItem => work_items.entry(organization).or_default().push(reference),
            ItemKind::PullRequest => pull_requests.push((organization, reference)),
            ItemKind::Issue | ItemKind::IssueOrPullRequest => {}
        }
    }
    for (organization, references) in work_items {
        // Work item ids are unique per organization: references that differ only in the project
        // name one item, read once and answered for each of them.
        let mut ids: Vec<u64> = Vec::new();
        for reference in &references {
            if !ids.contains(&reference.number) {
                ids.push(reference.number);
            }
        }
        for chunk in ids.chunks(MAX_WORK_ITEMS) {
            items.extend(work_item_batch(client, organization, chunk, &references).await?);
        }
    }
    for (organization, reference) in pull_requests.into_iter().take(MAX_PULL_REQUESTS) {
        let url = client
            .organization_api(organization, &format!("git/pullrequests/{}", reference.number));
        let pull_request: PullRequest = match client.get(&url, &[]).await {
            Ok(pull_request) => pull_request,
            Err(error) if matches!(error.status(), Some(401 | 403 | 404)) => continue,
            Err(error) => return Err(error),
        };
        items.push(pull_request_item(client, organization, reference.clone(), &pull_request));
    }
    Ok(items)
}

/// Reads `ids` (distinct) and answers every reference in `references` that names one of them.
async fn work_item_batch(
    client: &Client,
    organization: &str,
    ids: &[u64],
    references: &[&ItemRef],
) -> Result<Vec<LinkedItem>> {
    let body = json!({ "ids": ids, "fields": FIELDS, "errorPolicy": "omit" });
    let url = client.organization_api(organization, "wit/workitemsbatch");
    // With `errorPolicy: omit`, ids the token cannot read come back as `null` entries.
    let answer: ValueList<Option<WorkItem>> = client.send_json(Method::POST, &url, &body).await?;
    Ok(answer
        .value
        .into_iter()
        .flatten()
        .flat_map(|item| {
            let id = item.id;
            references
                .iter()
                .filter(move |reference| reference.number == id)
                .map(move |reference| work_item(client, organization, (*reference).clone(), &item))
        })
        .collect())
}

fn work_item(
    client: &Client,
    organization: &str,
    reference: ItemRef,
    item: &WorkItem,
) -> LinkedItem {
    let project = item
        .fields
        .project
        .clone()
        .or_else(|| reference.repository_key.split('/').nth(1).map(str::to_owned));
    let root = client.organization_root(organization);
    let web_url = match &project {
        Some(project) => {
            format!("{root}/{}/_workitems/edit/{}", urlencoding::encode(project), item.id)
        }
        None => format!("{root}/_workitems/edit/{}", item.id),
    };
    let repository_key = match &project {
        Some(project) => format!("{organization}/{project}"),
        None => organization.to_owned(),
    };
    LinkedItem {
        reference,
        kind: ItemKind::WorkItem,
        repository_key,
        title: item.fields.title.clone(),
        state: work_item_state(&item.fields.state),
        state_label: item.fields.state.clone(),
        checks: None,
        item_type: item.fields.work_item_type.clone(),
        web_url,
    }
}

/// Work item states are per process template; these are the finished ones of Agile, Scrum, CMMI
/// and Basic. Everything else (New, Active, Committed, To Do, Doing) is still open.
fn work_item_state(state: &str) -> LinkedState {
    match state.to_ascii_lowercase().as_str() {
        "closed" | "done" | "resolved" | "completed" => LinkedState::Done,
        "removed" | "cut" => LinkedState::Closed,
        _ => LinkedState::Open,
    }
}

fn pull_request_item(
    client: &Client,
    organization: &str,
    reference: ItemRef,
    pull_request: &PullRequest,
) -> LinkedItem {
    let summary = pull_request
        .to_summary(organization, |key, number| client.pull_request_web_url(key, number));
    let (state, label) = match state_from_ado(&pull_request.status) {
        PullRequestState::Merged => (LinkedState::Merged, "Merged"),
        PullRequestState::Closed => (LinkedState::Closed, "Abandoned"),
        PullRequestState::Open if pull_request.is_draft => (LinkedState::Draft, "Draft"),
        PullRequestState::Open => (LinkedState::Open, "Active"),
    };
    LinkedItem {
        reference,
        kind: ItemKind::PullRequest,
        repository_key: summary.repository_key,
        title: summary.title,
        state,
        state_label: label.to_owned(),
        checks: None,
        item_type: None,
        web_url: summary.web_url,
    }
}

#[cfg(test)]
mod tests {
    use review_core::LinkedState;

    use super::work_item_state;

    #[test]
    fn finished_states_of_every_process_template() {
        for state in ["Closed", "Done", "Resolved"] {
            assert_eq!(work_item_state(state), LinkedState::Done, "{state}");
        }
        assert_eq!(work_item_state("Removed"), LinkedState::Closed);
        for state in ["New", "Active", "Committed", "To Do", "Doing"] {
            assert_eq!(work_item_state(state), LinkedState::Open, "{state}");
        }
    }
}
