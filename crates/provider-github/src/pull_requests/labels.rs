//! Repository label catalog and pull request labels.

use reqwest::Method;
use review_core::{Label, Result, http};
use serde_json::json;

use crate::client::{GitHubClient, PER_PAGE, Paging};
use crate::dto::label::LabelDto;
use crate::mapping::identity;
use crate::repo_key::RepoKey;

const MAX_LABEL_PAGES: usize = 20;

pub(crate) async fn available(client: &GitHubClient, key: &str) -> Result<Vec<Label>> {
    let repo = RepoKey::parse(key)?;
    let query = [("per_page", PER_PAGE.to_string())];
    let path = format!("{}/labels", repo.api_path());
    client
        .collect_pages(&path, &query, Paging::pages(MAX_LABEL_PAGES), |page: Vec<LabelDto>| {
            page.into_iter().map(identity::label).collect()
        })
        .await
}

/// `PUT` replaces the whole set; an empty list clears all labels.
pub(crate) async fn set(
    client: &GitHubClient,
    key: &str,
    number: u64,
    labels: &[String],
) -> Result<()> {
    client.require_token("Changing labels")?;
    let repo = RepoKey::parse(key)?;
    let request = client
        .rest(Method::PUT, &format!("{}/labels", repo.issue_path(number)))
        .json(&json!({ "labels": labels }));
    http::unit(request).await
}
