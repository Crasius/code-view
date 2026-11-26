use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct RepositoryDto {
    pub full_name: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub html_url: String,
    #[serde(default)]
    pub default_branch: Option<String>,
    /// The merge settings come only with write access (`contents:write`); absent otherwise.
    #[serde(default)]
    pub allow_merge_commit: Option<bool>,
    #[serde(default)]
    pub allow_squash_merge: Option<bool>,
    #[serde(default)]
    pub allow_rebase_merge: Option<bool>,
}

/// `GET /repos/{o}/{r}/rules/branches/{branch}` item: a rule that applies to the branch.
#[derive(Debug, Deserialize)]
pub(crate) struct BranchRuleDto {
    #[serde(rename = "type")]
    pub rule_type: String,
    #[serde(default)]
    pub parameters: Option<BranchRuleParameters>,
}

/// Only the `pull_request` rule's `allowed_merge_methods` (`merge`, `squash`, `rebase`) is read.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct BranchRuleParameters {
    #[serde(default)]
    pub allowed_merge_methods: Option<Vec<String>>,
}
