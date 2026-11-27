use serde::Deserialize;

/// A project from the `simple=true` listing or from `GET /projects/:id`; only the single read
/// carries the merge settings.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ProjectDto {
    pub path_with_namespace: String,
    pub name: String,
    pub description: Option<String>,
    pub web_url: String,
    pub default_branch: Option<String>,
    pub http_url_to_repo: String,
    /// `merge`, `rebase_merge` or `ff`. GitLab leaves the merge settings out of anonymous answers
    /// (gitlab.com, public projects), and the fields it returns depend on the user's role.
    pub merge_method: Option<String>,
    /// `never`, `always`, `default_on` or `default_off`.
    pub squash_option: Option<String>,
}
