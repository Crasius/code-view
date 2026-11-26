use review_core::RepositoryInfo;
use serde::Deserialize;

use super::branch_name;
use crate::client::RepoKey;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ProjectRef {
    /// The project GUID; policy evaluations name their pull request by it.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// `GitRepository`, also the `repository` reference inside pull requests.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Repository {
    /// The repository GUID; policy configurations are filtered by it.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub project: ProjectRef,
    pub default_branch: Option<String>,
    pub web_url: Option<String>,
    #[serde(default)]
    pub is_disabled: bool,
}

impl Repository {
    pub fn key(&self, organization: &str) -> String {
        RepoKey::format(organization, &self.project.name, &self.name)
    }

    /// `fallback_web_url` is used when the API omits `webUrl` (older Server versions).
    pub fn to_info(&self, organization: &str, fallback_web_url: String) -> RepositoryInfo {
        RepositoryInfo {
            key: self.key(organization),
            name: self.name.clone(),
            description: None,
            web_url: self.web_url.clone().unwrap_or(fallback_web_url),
            default_branch: self.default_branch.as_deref().map(branch_name),
        }
    }
}

/// `GitRef` of `Refs - List`: a full ref name and the commit it points at.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GitRef {
    pub name: String,
    pub object_id: String,
}

/// Case-insensitive substring match on the repository or project name.
pub(crate) fn matches_search(repository: &Repository, search: Option<&str>) -> bool {
    let Some(search) = search.map(str::trim).filter(|search| !search.is_empty()) else {
        return true;
    };
    let needle = search.to_lowercase();
    repository.name.to_lowercase().contains(&needle)
        || repository.project.name.to_lowercase().contains(&needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_repository_with_project_in_key() {
        let json = r#"{"id":"r1","name":"web","project":{"id":"p1","name":"My Project"},
            "defaultBranch":"refs/heads/main","webUrl":"https://dev.azure.com/o/My%20Project/_git/web"}"#;
        let repository: Repository = serde_json::from_str(json).unwrap();
        let info = repository.to_info("o", String::new());
        assert_eq!(info.key, "o/My Project/web");
        assert_eq!(info.default_branch.as_deref(), Some("main"));
        assert!(matches_search(&repository, Some("WE")));
        assert!(matches_search(&repository, Some("project")));
        assert!(!matches_search(&repository, Some("api")));
        assert!(matches_search(&repository, Some("  ")));
    }
}
