//! Title and state of referenced merge requests and issues: one list request per project and
//! kind, filtered with the documented `iids[]` attribute of
//! `GET /projects/:id/merge_requests` and `GET /projects/:id/issues`.

use std::collections::BTreeMap;

use review_core::{ItemKind, ItemRef, LinkedItem, LinkedState, Result};
use serde::Deserialize;

use crate::client::{ApiClient, project_path};
use crate::merge_requests::pull_request_state;

/// GitLab's page size limit; more references per project are listed in further requests.
const MAX_IIDS: usize = 100;

#[derive(Debug, Deserialize)]
struct ItemDto {
    iid: u64,
    title: String,
    /// Merge requests: `opened`, `closed`, `locked`, `merged`. Issues: `opened`, `closed`.
    state: String,
    #[serde(default)]
    draft: bool,
    web_url: String,
}

pub(crate) async fn items(client: &ApiClient, references: &[ItemRef]) -> Result<Vec<LinkedItem>> {
    let mut groups: BTreeMap<(&str, bool), Vec<&ItemRef>> = BTreeMap::new();
    for reference in references {
        let is_merge_request = match reference.kind {
            ItemKind::PullRequest => true,
            ItemKind::Issue => false,
            ItemKind::IssueOrPullRequest | ItemKind::WorkItem => continue,
        };
        groups
            .entry((reference.repository_key.as_str(), is_merge_request))
            .or_default()
            .push(reference);
    }
    let mut items = Vec::new();
    for ((project, is_merge_request), references) in groups {
        for chunk in references.chunks(MAX_IIDS) {
            let resource = if is_merge_request { "merge_requests" } else { "issues" };
            let path = format!("{}/{resource}", project_path(project));
            let query: Vec<(&str, String)> =
                chunk.iter().map(|reference| ("iids[]", reference.number.to_string())).collect();
            let found: Vec<ItemDto> = match client.get_pages(&path, &query, chunk.len()).await {
                Ok(found) => found,
                // A project that is gone or private to this token: its references stay unknown.
                Err(error) if matches!(error.status(), Some(403 | 404)) => continue,
                Err(error) => return Err(error),
            };
            for dto in found {
                if let Some(reference) = chunk.iter().find(|reference| reference.number == dto.iid)
                {
                    items.push(to_item((*reference).clone(), is_merge_request, dto));
                }
            }
        }
    }
    Ok(items)
}

fn to_item(reference: ItemRef, is_merge_request: bool, dto: ItemDto) -> LinkedItem {
    let (kind, (state, label)) = if is_merge_request {
        (
            ItemKind::PullRequest,
            LinkedState::of_pull_request(pull_request_state(&dto.state), dto.draft),
        )
    } else {
        let state = if dto.state == "closed" {
            (LinkedState::Done, "Closed")
        } else {
            (LinkedState::Open, "Open")
        };
        (ItemKind::Issue, state)
    };
    LinkedItem {
        repository_key: reference.repository_key.clone(),
        reference,
        kind,
        title: dto.title,
        state,
        state_label: label.to_owned(),
        checks: None,
        item_type: None,
        web_url: dto.web_url,
    }
}

#[cfg(test)]
mod tests {
    use review_core::PullRequestState;

    use super::pull_request_state;

    #[test]
    fn decodes_merge_request_states() {
        assert_eq!(pull_request_state("merged"), PullRequestState::Merged);
        assert_eq!(pull_request_state("closed"), PullRequestState::Closed);
        assert_eq!(pull_request_state("opened"), PullRequestState::Open);
        assert_eq!(pull_request_state("locked"), PullRequestState::Open);
    }
}
