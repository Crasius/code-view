//! Title and state of referenced issues and pull requests.
//!
//! With a token, one GraphQL query per [`GRAPHQL_BATCH`] references asks every repository's
//! `issueOrPullRequest(number:)` through aliases, including the pull request's
//! `statusCheckRollup`. GraphQL needs a token on GitHub, so anonymous connections fall back to
//! `GET /repos/{owner}/{repo}/issues/{number}` (pull requests are issues there), at most
//! [`ANONYMOUS_LIMIT`] of them: the anonymous quota is 60 requests an hour.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use review_core::{ItemKind, ItemRef, LinkedItem, LinkedState, PullRequestState, Result};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::client::GitHubClient;
use crate::graphql;
use crate::mapping::run_status;
use crate::repo_key::RepoKey;

const GRAPHQL_BATCH: usize = 50;
const ANONYMOUS_LIMIT: usize = 10;
/// `fmt::Write` for `String` never returns an error.
const STRING_WRITE: &str = "writing to a String cannot fail";

const ITEM_FRAGMENT: &str = "fragment item on IssueOrPullRequest {
  __typename
  ... on PullRequest { number title url state isDraft repository { nameWithOwner } statusCheckRollup { state } }
  ... on Issue { number title url state stateReason repository { nameWithOwner } }
}";

/// One batch's answer: `r{i}` per repository, `i{j}` per item in it; GitHub answers an unknown
/// repository or number with `null`.
type BatchData = HashMap<String, Option<HashMap<String, Option<GraphNode>>>>;

/// Where one item of a batch sits in [`BatchData`]: repository index, item index.
type Alias = (usize, usize, ItemRef);

pub(crate) async fn items(
    client: &GitHubClient,
    references: &[ItemRef],
) -> Result<Vec<LinkedItem>> {
    let references = resolvable(references);
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

/// The references GitHub can answer: no work items, and an `owner/repo` key.
fn resolvable(references: &[ItemRef]) -> Vec<(RepoKey<'_>, &ItemRef)> {
    references
        .iter()
        .filter(|reference| reference.kind != ItemKind::WorkItem)
        .filter_map(|reference| Some((RepoKey::parse(&reference.repository_key).ok()?, reference)))
        .collect()
}

/// The query for one batch: one aliased `repository` per repository, one aliased item per number.
/// Owner and name travel as variables; numbers are integers and go inline.
fn batch_query(references: &[(RepoKey<'_>, &ItemRef)]) -> (String, Value, Vec<Alias>) {
    let mut repositories: BTreeMap<&str, (RepoKey<'_>, Vec<&ItemRef>)> = BTreeMap::new();
    for (repo, reference) in references {
        repositories
            .entry(reference.repository_key.as_str())
            .or_insert_with(|| (*repo, Vec::new()))
            .1
            .push(reference);
    }
    let mut declarations = String::new();
    let mut body = String::new();
    let mut variables = Map::new();
    let mut aliases = Vec::new();
    for (repository_index, (repo, items)) in repositories.values().enumerate() {
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
            write!(
                body,
                "i{item_index}: issueOrPullRequest(number: {}) {{ ...item }} ",
                item.number
            )
            .expect(STRING_WRITE);
            aliases.push((repository_index, item_index, (*item).clone()));
        }
        body.push_str("} ");
    }
    let declarations = declarations.trim_end().trim_end_matches(',');
    let query = format!("query({declarations}) {{ {body}}} {ITEM_FRAGMENT}");
    (query, Value::Object(variables), aliases)
}

/// A missing repository or number is `null` with a `NOT_FOUND` error, and the items GitHub
/// found still count; any other error fails the lookup.
async fn graphql_batch(
    client: &GitHubClient,
    batch: &[(RepoKey<'_>, &ItemRef)],
) -> Result<Vec<LinkedItem>> {
    let (query, variables, aliases) = batch_query(batch);
    let data: BatchData =
        graphql::execute_found(client, "Reading linked items", &query, variables).await?;
    Ok(read_batch(data, aliases))
}

fn read_batch(mut data: BatchData, aliases: Vec<Alias>) -> Vec<LinkedItem> {
    aliases
        .into_iter()
        .filter_map(|(repository, item, reference)| {
            let items = data.get_mut(&format!("r{repository}"))?.as_mut()?;
            let node = items.remove(&format!("i{item}"))??;
            Some(node.into_item(reference))
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
    state_reason: Option<String>,
    repository: GraphRepository,
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
            (
                ItemKind::PullRequest,
                LinkedState::of_pull_request(pull_request_state(&self.state), self.is_draft),
            )
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
            checks: self
                .status_check_rollup
                .map(|rollup| run_status::from_rollup_state(&rollup.state)),
            item_type: None,
            web_url: self.url,
        }
    }
}

/// GitHub's pull request state word: GraphQL's `MERGED`, `CLOSED`, `OPEN` or REST's lower case.
fn pull_request_state(state: &str) -> PullRequestState {
    match state.to_ascii_uppercase().as_str() {
        "MERGED" => PullRequestState::Merged,
        "CLOSED" => PullRequestState::Closed,
        _ => PullRequestState::Open,
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

#[derive(Debug, Deserialize)]
struct RestIssue {
    title: String,
    html_url: String,
    state: String,
    state_reason: Option<String>,
    #[serde(default)]
    draft: bool,
    pull_request: Option<RestPullRef>,
}

#[derive(Debug, Deserialize)]
struct RestPullRef {
    merged_at: Option<String>,
}

async fn anonymous(
    client: &GitHubClient,
    references: &[(RepoKey<'_>, &ItemRef)],
) -> Result<Vec<LinkedItem>> {
    let mut items = Vec::new();
    for (repo, reference) in references.iter().take(ANONYMOUS_LIMIT) {
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
        Some(pull) => {
            let state = if pull.merged_at.is_some() {
                PullRequestState::Merged
            } else {
                pull_request_state(&issue.state)
            };
            (ItemKind::PullRequest, LinkedState::of_pull_request(state, issue.draft))
        }
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
    use serde_json::{Value, json};

    use super::{BatchData, batch_query, read_batch, resolvable};

    fn reference(kind: ItemKind, repository: &str, number: u64) -> ItemRef {
        ItemRef { kind, repository_key: repository.to_owned(), number }
    }

    fn batch(answer: Value) -> BatchData {
        serde_json::from_value(answer).unwrap_or_else(|error| panic!("answer: {error}"))
    }

    fn pull_request(number: u64, rollup: &str) -> Value {
        json!({ "__typename": "PullRequest", "number": number, "title": "Fix it",
                "url": format!("https://github.com/acme/app/pull/{number}"), "state": "OPEN",
                "isDraft": false, "repository": { "nameWithOwner": "acme/app" },
                "statusCheckRollup": { "state": rollup } })
    }

    #[test]
    fn batches_by_repository_with_variables() {
        let asked = [
            reference(ItemKind::IssueOrPullRequest, "acme/app", 3),
            reference(ItemKind::PullRequest, "acme/app", 9),
            reference(ItemKind::Issue, "fabrikam/web", 1),
            reference(ItemKind::Issue, "not-a-key", 2),
        ];
        let (query, variables, aliases) = batch_query(&resolvable(&asked));
        assert!(
            query.starts_with("query($o0: String!, $n0: String!, $o1: String!, $n1: String!)"),
            "{query}"
        );
        assert!(query.contains("r0: repository(owner: $o0, name: $n0) { i0: issueOrPullRequest(number: 3) { ...item } i1: issueOrPullRequest(number: 9)"));
        assert_eq!(variables["o1"], "fabrikam");
        assert_eq!(
            aliases.iter().map(|(repository, item, _)| (*repository, *item)).collect::<Vec<_>>(),
            [(0, 0), (0, 1), (1, 0)]
        );
    }

    #[test]
    fn reads_partial_answers() {
        let first = reference(ItemKind::IssueOrPullRequest, "acme/app", 3);
        let missing = reference(ItemKind::Issue, "acme/app", 4);
        let gone = reference(ItemKind::Issue, "fabrikam/gone", 1);
        let asked = [first.clone(), missing, gone];
        let (_, _, aliases) = batch_query(&resolvable(&asked));
        let mut draft = pull_request(3, "FAILURE");
        draft["isDraft"] = json!(true);
        let data = batch(json!({ "r0": { "i0": draft, "i1": null }, "r1": null }));
        let items = read_batch(data, aliases);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].reference, first);
        assert_eq!(
            (items[0].kind, items[0].state, items[0].checks),
            (ItemKind::PullRequest, LinkedState::Draft, Some(RunStatus::Failed))
        );
    }

    /// Linked pull requests read `EXPECTED` and unknown rollup states as Running, while the same
    /// pull request's list row read Queued or Neutral.
    #[test]
    fn linked_checks_read_like_the_list_row() {
        let asked = [
            reference(ItemKind::PullRequest, "acme/app", 1),
            reference(ItemKind::PullRequest, "acme/app", 2),
        ];
        let (_, _, aliases) = batch_query(&resolvable(&asked));
        let data = batch(json!({ "r0": {
            "i0": pull_request(1, "EXPECTED"),
            "i1": pull_request(2, "SOMETHING_NEW")
        } }));
        let checks: Vec<_> = read_batch(data, aliases).iter().map(|item| item.checks).collect();
        assert_eq!(checks, [Some(RunStatus::Queued), Some(RunStatus::Neutral)]);
    }
}
