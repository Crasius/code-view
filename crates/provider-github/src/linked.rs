//! Title and state of referenced issues and pull requests.
//!
//! With a token, one GraphQL query per [`GRAPHQL_BATCH`] references asks every repository's
//! `issueOrPullRequest(number:)` through aliases, including the pull request's
//! `statusCheckRollup`. GraphQL needs a token on GitHub, so anonymous connections fall back to
//! `GET /repos/{owner}/{repo}/issues/{number}` (pull requests are issues there), at most
//! [`ANONYMOUS_LIMIT`] of them: the anonymous quota is 60 requests an hour.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use reqwest::Method;
use review_core::{ItemKind, ItemRef, LinkedItem, LinkedState, Result, RunStatus, http};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::client::GitHubClient;
use crate::repo_key::RepoKey;

pub(crate) const GRAPHQL_BATCH: usize = 50;
pub(crate) const ANONYMOUS_LIMIT: usize = 10;
/// `fmt::Write` for `String` never returns an error.
const STRING_WRITE: &str = "writing to a String cannot fail";

const ITEM_FRAGMENT: &str = "fragment item on IssueOrPullRequest {
  __typename
  ... on PullRequest { number title url state isDraft repository { nameWithOwner } statusCheckRollup { state } }
  ... on Issue { number title url state stateReason repository { nameWithOwner } }
}";

pub(crate) async fn items(
    client: &GitHubClient,
    references: &[ItemRef],
) -> Result<Vec<LinkedItem>> {
    let references: Vec<&ItemRef> = references
        .iter()
        .filter(|reference| {
            reference.kind != ItemKind::WorkItem
                && RepoKey::parse(&reference.repository_key).is_ok()
        })
        .collect();
    if references.is_empty() {
        return Ok(Vec::new());
    }
    if !client.has_token() {
        return anonymous(client, &references).await;
    }
    let mut items = Vec::new();
    for batch in references.chunks(GRAPHQL_BATCH) {
        items.extend(graphql_batch(client, batch).await?);
    }
    Ok(items)
}

/// The query for one batch: one aliased `repository` per repository, one aliased item per number.
/// Owner and name travel as variables; numbers are integers and go inline.
pub(crate) fn batch_query(references: &[&ItemRef]) -> (String, Value, Vec<(String, ItemRef)>) {
    let mut repositories: BTreeMap<&str, Vec<&ItemRef>> = BTreeMap::new();
    for reference in references {
        repositories.entry(reference.repository_key.as_str()).or_default().push(reference);
    }
    let mut declarations = String::new();
    let mut body = String::new();
    let mut variables = Map::new();
    let mut aliases = Vec::new();
    for (repository_index, (key, items)) in repositories.iter().enumerate() {
        let Ok(repo) = RepoKey::parse(key) else {
            continue;
        };
        write!(declarations, "$o{repository_index}: String!, $n{repository_index}: String!, ")
            .expect(STRING_WRITE);
        variables.insert(format!("o{repository_index}"), json!(repo.owner));
        variables.insert(format!("n{repository_index}"), json!(repo.name));
        write!(
            body,
            "r{repository_index}: repository(owner: $o{repository_index}, name: $n{repository_index}) {{ "
        )
        .expect(STRING_WRITE);
        for (item_index, item) in items.iter().enumerate() {
            let alias = format!("i{item_index}");
            write!(body, "{alias}: issueOrPullRequest(number: {}) {{ ...item }} ", item.number)
                .expect(STRING_WRITE);
            aliases.push((format!("r{repository_index}.{alias}"), (*item).clone()));
        }
        body.push_str("} ");
    }
    let declarations = declarations.trim_end().trim_end_matches(',');
    let query = format!("query({declarations}) {{ {body}}} {ITEM_FRAGMENT}");
    (query, Value::Object(variables), aliases)
}

/// GraphQL answers a missing repository or number with `errors` next to partial `data`; the
/// items it found still count.
async fn graphql_batch(client: &GitHubClient, batch: &[&ItemRef]) -> Result<Vec<LinkedItem>> {
    let (query, variables, aliases) = batch_query(batch);
    let request = client
        .request(Method::POST, client.graphql_url())
        .json(&json!({ "query": query, "variables": variables }));
    let envelope: Value = http::json(request).await?;
    Ok(read_batch(&envelope, &aliases))
}

pub(crate) fn read_batch(envelope: &Value, aliases: &[(String, ItemRef)]) -> Vec<LinkedItem> {
    let data = &envelope["data"];
    aliases
        .iter()
        .filter_map(|(path, reference)| {
            let (repository, item) = path.split_once('.')?;
            let node = data.get(repository)?.get(item)?;
            let node: GraphNode = serde_json::from_value(node.clone()).ok()?;
            Some(node.into_item(reference.clone()))
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphNode {
    #[serde(rename = "__typename")]
    typename: String,
    title: String,
    url: String,
    state: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    state_reason: Option<String>,
    repository: GraphRepository,
    #[serde(default)]
    status_check_rollup: Option<GraphRollup>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphRepository {
    name_with_owner: String,
}

#[derive(Debug, Deserialize)]
struct GraphRollup {
    state: String,
}

impl GraphNode {
    fn into_item(self, reference: ItemRef) -> LinkedItem {
        let is_pull_request = self.typename == "PullRequest";
        let (kind, (state, label)) = if is_pull_request {
            (ItemKind::PullRequest, pull_request_state(&self.state, self.is_draft))
        } else {
            (ItemKind::Issue, issue_state(&self.state, self.state_reason.as_deref()))
        };
        LinkedItem {
            reference,
            kind,
            repository_key: self.repository.name_with_owner,
            title: self.title,
            state,
            state_label: label.to_owned(),
            checks: self.status_check_rollup.map(|rollup| rollup_status(&rollup.state)),
            item_type: None,
            web_url: self.url,
        }
    }
}

/// The app's words for pull request states, the same on every platform.
fn pull_request_state(state: &str, is_draft: bool) -> (LinkedState, &'static str) {
    match state.to_ascii_uppercase().as_str() {
        "MERGED" => (LinkedState::Merged, "Merged"),
        "CLOSED" => (LinkedState::Closed, "Abandoned"),
        _ if is_draft => (LinkedState::Draft, "Draft"),
        _ => (LinkedState::Open, "Active"),
    }
}

/// Closed issues count as done unless GitHub says they were not planned or duplicates.
fn issue_state(state: &str, reason: Option<&str>) -> (LinkedState, &'static str) {
    if !state.eq_ignore_ascii_case("closed") {
        return (LinkedState::Open, "Open");
    }
    let reason = reason.unwrap_or_default().to_ascii_uppercase();
    if matches!(reason.as_str(), "NOT_PLANNED" | "DUPLICATE") {
        (LinkedState::Closed, "Closed")
    } else {
        (LinkedState::Done, "Closed")
    }
}

/// `StatusState`: SUCCESS, FAILURE, ERROR, PENDING, EXPECTED.
fn rollup_status(state: &str) -> RunStatus {
    match state {
        "SUCCESS" => RunStatus::Succeeded,
        "FAILURE" | "ERROR" => RunStatus::Failed,
        _ => RunStatus::Running,
    }
}

#[derive(Debug, Deserialize)]
struct RestIssue {
    title: String,
    html_url: String,
    state: String,
    #[serde(default)]
    state_reason: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    pull_request: Option<RestPullRef>,
}

#[derive(Debug, Deserialize)]
struct RestPullRef {
    #[serde(default)]
    merged_at: Option<String>,
}

async fn anonymous(client: &GitHubClient, references: &[&ItemRef]) -> Result<Vec<LinkedItem>> {
    let mut items = Vec::new();
    for reference in references.iter().take(ANONYMOUS_LIMIT) {
        let Ok(repo) = RepoKey::parse(&reference.repository_key) else {
            continue;
        };
        let issue: RestIssue = match client.get_json(&repo.issue_path(reference.number)).await {
            Ok(issue) => issue,
            Err(error) if matches!(error.status(), Some(404 | 410)) => continue,
            Err(error) => return Err(error),
        };
        items.push(rest_item((*reference).clone(), issue));
    }
    Ok(items)
}

fn rest_item(reference: ItemRef, issue: RestIssue) -> LinkedItem {
    let (kind, (state, label)) = match &issue.pull_request {
        Some(pull) if pull.merged_at.is_some() => {
            (ItemKind::PullRequest, (LinkedState::Merged, "Merged"))
        }
        Some(_) => (ItemKind::PullRequest, pull_request_state(&issue.state, issue.draft)),
        None => (ItemKind::Issue, issue_state(&issue.state, issue.state_reason.as_deref())),
    };
    LinkedItem {
        repository_key: reference.repository_key.clone(),
        reference,
        kind,
        title: issue.title,
        state,
        state_label: label.to_owned(),
        checks: None,
        item_type: None,
        web_url: issue.html_url,
    }
}

#[cfg(test)]
mod tests {
    use review_core::{ItemKind, ItemRef, LinkedState, RunStatus};
    use serde_json::json;

    use super::{batch_query, read_batch};

    fn reference(kind: ItemKind, repository: &str, number: u64) -> ItemRef {
        ItemRef { kind, repository_key: repository.to_owned(), number }
    }

    #[test]
    fn batches_by_repository_with_variables() {
        let first = reference(ItemKind::IssueOrPullRequest, "cli/cli", 3);
        let second = reference(ItemKind::PullRequest, "cli/cli", 9);
        let third = reference(ItemKind::Issue, "rust-lang/rust", 1);
        let (query, variables, aliases) = batch_query(&[&first, &second, &third]);
        assert!(
            query.starts_with("query($o0: String!, $n0: String!, $o1: String!, $n1: String!)"),
            "{query}"
        );
        assert!(query.contains("r0: repository(owner: $o0, name: $n0) { i0: issueOrPullRequest(number: 3) { ...item } i1: issueOrPullRequest(number: 9)"));
        assert_eq!(variables["o1"], "rust-lang");
        assert_eq!(
            aliases.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
            ["r0.i0", "r0.i1", "r1.i0"]
        );
    }

    #[test]
    fn reads_partial_answers() {
        let first = reference(ItemKind::IssueOrPullRequest, "cli/cli", 3);
        let missing = reference(ItemKind::Issue, "cli/cli", 4);
        let (_, _, aliases) = batch_query(&[&first, &missing]);
        let envelope = json!({
            "data": { "r0": {
                "i0": { "__typename": "PullRequest", "number": 3, "title": "Fix it", "url": "https://github.com/cli/cli/pull/3",
                        "state": "OPEN", "isDraft": true, "repository": { "nameWithOwner": "cli/cli" },
                        "statusCheckRollup": { "state": "FAILURE" } },
                "i1": null
            } },
            "errors": [{ "type": "NOT_FOUND", "message": "Could not resolve to an issue or pull request with the number of 4." }]
        });
        let items = read_batch(&envelope, &aliases);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].reference, first);
        assert_eq!(
            (items[0].kind, items[0].state, items[0].checks),
            (ItemKind::PullRequest, LinkedState::Draft, Some(RunStatus::Failed))
        );
    }
}
