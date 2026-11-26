use serde::Deserialize;

/// `/projects` item; the `simple=true` representation carries all of these.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ProjectDto {
    pub path_with_namespace: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub web_url: String,
    #[serde(default)]
    pub default_branch: Option<String>,
    pub http_url_to_repo: String,
    /// `merge`, `rebase_merge` or `ff`. GitLab leaves the merge settings out of anonymous answers
    /// (gitlab.com, public projects), and the fields it returns depend on the user's role.
    #[serde(default)]
    pub merge_method: Option<String>,
    /// `never`, `always`, `default_on` or `default_off`.
    #[serde(default)]
    pub squash_option: Option<String>,
}
