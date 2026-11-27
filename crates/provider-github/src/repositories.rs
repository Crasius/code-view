//! Current user, repository listing and git remotes.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use review_core::{Connection, GitRemote, RepositoryInfo, Result, UserIdentity};

use crate::client::{GitHubClient, PER_PAGE, Paging};
use crate::dto::repository::RepositoryDto;
use crate::dto::user::UserDto;
use crate::mapping::identity;
use crate::owners;
use crate::repo_key::RepoKey;

const MAX_REPOSITORIES: usize = 100;
/// Pages scanned per owner while filtering by name.
const MAX_SEARCH_PAGES: usize = 10;

pub(crate) async fn current_user(client: &GitHubClient) -> Result<UserIdentity> {
    client.require_token("Reading the current user")?;
    let user: UserDto = client.get_json("/user").await?;
    Ok(identity::user(user))
}

/// Repositories of the connection's owners, or of the token's user when no owner is configured.
pub(crate) async fn list(
    client: &GitHubClient,
    search: Option<&str>,
) -> Result<Vec<RepositoryInfo>> {
    let needle = search.map(str::trim).filter(|term| !term.is_empty()).map(str::to_lowercase);
    let owners: Vec<&str> = client
        .connection()
        .organizations
        .iter()
        .map(|owner| owner.trim())
        .filter(|owner| !owner.is_empty())
        .collect();
    if owners.is_empty() {
        client.require_token("Listing your repositories")?;
        return collect_matching(client, "/user/repos", needle.as_deref(), MAX_REPOSITORIES).await;
    }
    let mut repositories = Vec::new();
    for owner in owners {
        let remaining = MAX_REPOSITORIES - repositories.len();
        if remaining == 0 {
            break;
        }
        let path = owners::owner_kind(client, owner).await?.repositories_path(owner);
        repositories.extend(collect_matching(client, &path, needle.as_deref(), remaining).await?);
    }
    Ok(repositories)
}

async fn collect_matching(
    client: &GitHubClient,
    path: &str,
    needle: Option<&str>,
    limit: usize,
) -> Result<Vec<RepositoryInfo>> {
    let query = [("per_page", PER_PAGE.to_string()), ("sort", "updated".to_owned())];
    let max_pages = if needle.is_some() { MAX_SEARCH_PAGES } else { 1 };
    client
        .collect_pages(path, &query, Paging { limit, max_pages }, |page: Vec<RepositoryDto>| {
            page.into_iter()
                .filter(|repository| {
                    needle.is_none_or(|needle| repository.full_name.to_lowercase().contains(needle))
                })
                .map(identity::repository)
                .collect()
        })
        .await
}

/// HTTPS clone URL on the web host; tokens authenticate git as `x-access-token`.
pub(crate) fn git_remote(connection: &Connection, repo: &str) -> Result<GitRemote> {
    let key = RepoKey::parse(repo)?;
    let clone_url = format!("{}/{}/{}.git", connection.base_url(), key.owner, key.name);
    let authorization_header = connection
        .token()
        .map(|token| format!("Basic {}", STANDARD.encode(format!("x-access-token:{token}"))));
    Ok(GitRemote { clone_url, authorization_header })
}

#[cfg(test)]
mod tests {
    use super::git_remote;
    use crate::client::test_connection;
    use review_core::Connection;

    fn connection(token: Option<&str>) -> Connection {
        test_connection("https://github.com/", token)
    }

    #[test]
    fn anonymous_remote_has_no_header() {
        let remote = git_remote(&connection(None), "contoso/app").expect("remote");
        assert_eq!(remote.clone_url, "https://github.com/contoso/app.git");
        assert_eq!(remote.authorization_header, None);
        assert!(
            git_remote(&connection(Some("")), "contoso/app")
                .expect("remote")
                .authorization_header
                .is_none()
        );
    }

    #[test]
    fn token_becomes_basic_auth() {
        let remote = git_remote(&connection(Some("abc")), "contoso/app").expect("remote");
        // base64("x-access-token:abc")
        assert_eq!(remote.authorization_header.as_deref(), Some("Basic eC1hY2Nlc3MtdG9rZW46YWJj"));
    }
}
