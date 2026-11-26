use review_core::{GitRemote, RepositoryInfo, Result};

use crate::client::{Client, RepoKey};
use crate::dto::ValueList;
use crate::dto::repository::{Repository, matches_search};

const MAX_REPOSITORIES: usize = 100;

/// All projects' repositories of every configured organization, filtered client-side by name.
pub(crate) async fn list(client: &Client, search: Option<&str>) -> Result<Vec<RepositoryInfo>> {
    let mut repositories = Vec::new();
    for organization in client.organizations()? {
        let url = client.organization_api(organization, "git/repositories");
        let page: ValueList<Repository> = client.get(&url, &[]).await?;
        let mut matching: Vec<RepositoryInfo> = page
            .value
            .iter()
            .filter(|repository| !repository.is_disabled && matches_search(repository, search))
            .map(|repository| {
                let key = RepoKey {
                    organization,
                    project: &repository.project.name,
                    repository: &repository.name,
                };
                repository.to_info(organization, client.clone_url(key))
            })
            .collect();
        matching.sort_by_cached_key(|repository| repository.key.to_lowercase());
        repositories.extend(matching);
        if repositories.len() >= MAX_REPOSITORIES {
            break;
        }
    }
    repositories.truncate(MAX_REPOSITORIES);
    Ok(repositories)
}

/// Built from the key; no request needed.
pub(crate) fn git_remote(client: &Client, repo: &str) -> Result<GitRemote> {
    let key = RepoKey::parse(repo)?;
    Ok(GitRemote {
        clone_url: client.clone_url(key),
        authorization_header: client.authorization_header(),
    })
}
