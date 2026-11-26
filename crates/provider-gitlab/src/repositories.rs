//! Account, project listing, the project lookup and git remote.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use review_core::{GitRemote, RepositoryInfo, Result, UserIdentity};

use crate::client::{ApiClient, group_path, project_path};
use crate::dto::{ProjectDto, UserDto};

const MAX_REPOSITORIES: usize = 100;

pub(crate) async fn current_user_dto(client: &ApiClient) -> Result<UserDto> {
    client.require_token("Reading the current user")?;
    client.get("/user", &[]).await
}

pub(crate) async fn current_user(client: &ApiClient) -> Result<UserIdentity> {
    Ok(current_user_dto(client).await?.to_identity())
}

/// Group projects (with subgroups) per organization, otherwise the projects the token is a
/// member of. Anonymous connections without groups list public projects matching `search`.
pub(crate) async fn list_repositories(
    client: &ApiClient,
    organizations: &[String],
    search: Option<&str>,
) -> Result<Vec<RepositoryInfo>> {
    let mut query =
        vec![("simple", "true".to_owned()), ("order_by", "last_activity_at".to_owned())];
    if let Some(search) = search.filter(|search| !search.is_empty()) {
        query.push(("search", search.to_owned()));
    }
    let mut projects: Vec<ProjectDto> = Vec::new();
    if organizations.is_empty() {
        if client.token().is_some() {
            query.push(("membership", "true".to_owned()));
        }
        projects = client.get_pages("/projects", &query, MAX_REPOSITORIES).await?;
    } else {
        query.push(("include_subgroups", "true".to_owned()));
        for group in organizations {
            let remaining = MAX_REPOSITORIES - projects.len();
            if remaining == 0 {
                break;
            }
            let path = format!("{}/projects", group_path(group));
            projects.extend(client.get_pages::<ProjectDto>(&path, &query, remaining).await?);
        }
    }
    Ok(projects.into_iter().map(to_repository).collect())
}

pub(crate) async fn git_remote(
    client: &ApiClient,
    projects: &Projects,
    repo: &str,
) -> Result<GitRemote> {
    let project = projects.get(client, repo).await?;
    Ok(GitRemote {
        clone_url: project.http_url_to_repo,
        authorization_header: client.token().map(basic_authorization),
    })
}

/// How long one answer of `GET /projects/:project` serves: our own number, long enough for a
/// merge request page that opens (its git remote first, its merge policy once the detail is in),
/// short enough that a changed merge setting shows on the next page.
const PROJECT_FRESH: Duration = Duration::from_secs(60);

/// `GET /projects/:project` shared by the git remote and the merge policy, which a page opening a
/// merge request asks a moment apart, so it sends one request instead of two. Questions at the
/// same time share one request anyway (the HTTP client's in-flight sharing). Holds one answer per
/// project asked for within [`PROJECT_FRESH`].
#[derive(Default)]
pub(crate) struct Projects {
    answers: Mutex<HashMap<String, (Instant, ProjectDto)>>,
}

impl Projects {
    pub async fn get(&self, client: &ApiClient, repo: &str) -> Result<ProjectDto> {
        let fresh = self
            .answers()
            .get(repo)
            .filter(|(asked, _)| asked.elapsed() < PROJECT_FRESH)
            .map(|(_, project)| project.clone());
        if let Some(project) = fresh {
            return Ok(project);
        }
        let project: ProjectDto = client.get(&project_path(repo), &[]).await?;
        let mut answers = self.answers();
        answers.retain(|_, (asked, _)| asked.elapsed() < PROJECT_FRESH);
        answers.insert(repo.to_owned(), (Instant::now(), project.clone()));
        Ok(project)
    }

    fn answers(&self) -> MutexGuard<'_, HashMap<String, (Instant, ProjectDto)>> {
        self.answers.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Git over HTTPS accepts a personal access token as password with any non-blank user name.
fn basic_authorization(token: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("oauth2:{token}")))
}

fn to_repository(project: ProjectDto) -> RepositoryInfo {
    RepositoryInfo {
        key: project.path_with_namespace,
        name: project.name,
        description: project.description.filter(|description| !description.is_empty()),
        web_url: project.web_url,
        default_branch: project.default_branch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_simple_project() {
        let json = r#"{"id":1,"name":"cli","path_with_namespace":"gitlab-org/cli","description":"",
            "web_url":"https://gitlab.com/gitlab-org/cli","default_branch":"main",
            "http_url_to_repo":"https://gitlab.com/gitlab-org/cli.git"}"#;
        let project: ProjectDto = serde_json::from_str(json).expect("fixture");
        let repository = to_repository(project);
        assert_eq!(repository.key, "gitlab-org/cli");
        assert_eq!(repository.description, None);
        assert_eq!(repository.default_branch.as_deref(), Some("main"));
    }

    #[test]
    fn builds_basic_header_with_oauth2_user() {
        // base64("oauth2:abc")
        assert_eq!(basic_authorization("abc"), "Basic b2F1dGgyOmFiYw==");
    }
}
