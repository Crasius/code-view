//! Which merge methods a pull request into a branch may use.
//!
//! Two sources, both documented under "Repositories": the repository's `allow_merge_commit`,
//! `allow_squash_merge` and `allow_rebase_merge` (`GET /repos/{owner}/{repo}`, returned only with
//! write access), and the `allowed_merge_methods` of `pull_request` rules that apply to the branch
//! (`GET /repos/{owner}/{repo}/rules/branches/{branch}`, readable by everyone who can read the
//! repository). A method must pass both. Without the repository settings the policy is not
//! reported: every method GitHub accepts, narrowed by the branch rules.
//!
//! GitHub stores no default method; only a single allowed one counts as the default.

use review_core::{MergePolicy, MergeStrategy, ProviderError, Result, Unreported};

use crate::client::{GitHubClient, Paging};
use crate::dto::repository::{BranchRuleDto, RepositoryDto};
use crate::mapping::pull_request::merge_method_from_github;
use crate::pull_requests::auto_merge;
use crate::repo_key::{RepoKey, encode_branch};

/// Rules per branch are few; three pages are far more than any repository has.
const RULE_PAGES: usize = 3;

pub(crate) async fn policy(
    client: &GitHubClient,
    key: &str,
    branch: &str,
    capable: &[MergeStrategy],
) -> Result<MergePolicy> {
    let repo = RepoKey::parse(key)?;
    let repository: RepositoryDto = client.get_json(&repo.api_path()).await?;
    let rules = branch_rules(client, &repo, branch).await?;
    let ruled = allowed_by_rules(&rules);
    let permitted =
        |strategy: &MergeStrategy| ruled.as_ref().is_none_or(|ruled| ruled.contains(strategy));
    let settings_url = Some(auto_merge::settings_url(client, &repo));
    let flags = [
        (MergeStrategy::Merge, repository.allow_merge_commit),
        (MergeStrategy::Squash, repository.allow_squash_merge),
        (MergeStrategy::Rebase, repository.allow_rebase_merge),
    ];
    if flags.iter().any(|(_, flag)| flag.is_none()) {
        let narrowed: Vec<MergeStrategy> = capable.iter().copied().filter(permitted).collect();
        let allowed = if narrowed.is_empty() { capable.to_vec() } else { narrowed };
        return Ok(MergePolicy::not_reported(&allowed, Unreported::NoPermission)
            .with_settings_url(settings_url));
    }
    let mut allowed: Vec<MergeStrategy> = flags
        .into_iter()
        .filter(|(_, flag)| *flag == Some(true))
        .map(|(strategy, _)| strategy)
        .filter(permitted)
        .collect();
    if allowed.is_empty() {
        // Settings and rules that leave nothing: GitHub refuses every merge and says why.
        allowed = flags
            .into_iter()
            .filter(|(_, flag)| *flag == Some(true))
            .map(|(strategy, _)| strategy)
            .collect();
    }
    Ok(MergePolicy::reported(allowed, None).with_settings_url(settings_url))
}

/// The rules that apply to `branch`. Where rulesets do not exist no rule applies: older Enterprise
/// Server answers 404, github.com answers a private repository on GitHub Free with 403 ("Upgrade
/// to GitHub Pro or make this repository public"). Any reader may read the rules otherwise, and a
/// token denied the repository already failed its read above.
async fn branch_rules(
    client: &GitHubClient,
    repo: &RepoKey<'_>,
    branch: &str,
) -> Result<Vec<BranchRuleDto>> {
    let path = format!("{}/rules/branches/{}", repo.api_path(), encode_branch(branch));
    let query = [("per_page", crate::client::PER_PAGE.to_string())];
    match client
        .collect_pages(&path, &query, Paging::pages(RULE_PAGES), |page: Vec<BranchRuleDto>| page)
        .await
    {
        Ok(rules) => Ok(rules),
        Err(ProviderError::Status { status: 403 | 404, .. }) => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// The methods every `pull_request` rule with `allowed_merge_methods` permits; `None` without one.
fn allowed_by_rules(rules: &[BranchRuleDto]) -> Option<Vec<MergeStrategy>> {
    let mut allowed: Option<Vec<MergeStrategy>> = None;
    for rule in rules.iter().filter(|rule| rule.rule_type == "pull_request") {
        let Some(methods) = rule
            .parameters
            .as_ref()
            .and_then(|parameters| parameters.allowed_merge_methods.as_ref())
        else {
            continue;
        };
        let permitted: Vec<MergeStrategy> =
            methods.iter().filter_map(|method| merge_method_from_github(method)).collect();
        allowed = Some(match allowed {
            None => permitted,
            Some(known) => {
                known.into_iter().filter(|strategy| permitted.contains(strategy)).collect()
            }
        });
    }
    allowed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(json: serde_json::Value) -> BranchRuleDto {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn pull_request_rules_intersect() {
        let rules = [
            rule(serde_json::json!({ "type": "deletion" })),
            rule(
                serde_json::json!({ "type": "pull_request", "parameters": { "allowed_merge_methods": ["merge", "squash"] } }),
            ),
            rule(
                serde_json::json!({ "type": "pull_request", "parameters": { "allowed_merge_methods": ["squash", "rebase"] } }),
            ),
            rule(
                serde_json::json!({ "type": "merge_queue", "parameters": { "merge_method": "SQUASH" } }),
            ),
        ];
        assert_eq!(allowed_by_rules(&rules), Some(vec![MergeStrategy::Squash]));
        assert_eq!(allowed_by_rules(&rules[..1]), None);
    }
}
