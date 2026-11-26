//! The target branch's "Require a merge strategy" (limit merge types) policy.
//!
//! `Policy Configurations - Get` with `repositoryId` and `refName` returns every configuration
//! that applies to the branch (exact, prefix and repository-wide scopes); `policyType` keeps only
//! the merge-strategy type. Enabled, not deleted configurations restrict the allowed types; several
//! of them intersect. Without one, every type is allowed. The settings are `allowNoFastForward`,
//! `allowSquash`, `allowRebase` and `allowRebaseMerge`; configurations from before those existed
//! only carry `useSquashMerge` (squash required).
//!
//! Azure DevOps has no stored default: it completes with no-fast-forward unless told otherwise, and
//! its completion dialog preselects the first allowed type in its own order (Merge, Squash, Rebase,
//! Semi-linear), which is the order kept here.
//!
//! Reading policies needs sign-in even on public projects; anonymous connections (and tokens that
//! may not read them) get the full list, marked as not reported.

use review_core::{MergePolicy, MergeStrategy, ProviderError, Result, Unreported};
use serde::Deserialize;

use crate::client::{Client, RepoKey};
use crate::dto::ValueList;
use crate::dto::repository::Repository;

/// Policy type id of "Require a merge strategy".
pub(crate) const MERGE_STRATEGY_POLICY: &str = "fa4e907d-c16b-4a4c-9dfa-4916e5d171ab";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolicyConfiguration {
    #[serde(default)]
    is_enabled: bool,
    #[serde(default)]
    is_deleted: bool,
    #[serde(rename = "type")]
    policy_type: Option<PolicyTypeRef>,
    #[serde(default)]
    settings: MergeTypeSettings,
}

#[derive(Debug, Deserialize)]
struct PolicyTypeRef {
    id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeTypeSettings {
    allow_no_fast_forward: Option<bool>,
    allow_squash: Option<bool>,
    allow_rebase: Option<bool>,
    allow_rebase_merge: Option<bool>,
    use_squash_merge: Option<bool>,
}

impl MergeTypeSettings {
    fn allowed(&self) -> Vec<MergeStrategy> {
        let flags = [
            (MergeStrategy::Merge, self.allow_no_fast_forward),
            (MergeStrategy::Squash, self.allow_squash),
            (MergeStrategy::Rebase, self.allow_rebase),
            (MergeStrategy::SemiLinear, self.allow_rebase_merge),
        ];
        if flags.iter().all(|(_, flag)| flag.is_none()) {
            return if self.use_squash_merge == Some(true) {
                vec![MergeStrategy::Squash]
            } else {
                MergeStrategy::ALL.to_vec()
            };
        }
        flags
            .into_iter()
            .filter(|(_, flag)| *flag == Some(true))
            .map(|(strategy, _)| strategy)
            .collect()
    }
}

pub(crate) async fn policy(client: &Client, repo: &str, branch: &str) -> Result<MergePolicy> {
    let key = RepoKey::parse(repo)?;
    let denied = |settings_url: String| {
        MergePolicy::not_reported(&MergeStrategy::ALL, Unreported::NoPermission)
            .with_settings_url(Some(settings_url))
    };
    let repository: Repository = match client.get(&client.repository_resource_api(key), &[]).await {
        Ok(repository) => repository,
        Err(error) if is_denied(&error) => return Ok(denied(client.branches_web_url(key))),
        Err(error) => return Err(error),
    };
    let settings_url = client.branch_policies_web_url(key, &repository.id, branch);
    let query = [
        ("repositoryId", repository.id.clone()),
        ("refName", format!("refs/heads/{branch}")),
        ("policyType", MERGE_STRATEGY_POLICY.to_owned()),
    ];
    let configurations: ValueList<PolicyConfiguration> =
        match client.get(&client.project_api(key, "git/policy/configurations"), &query).await {
            Ok(configurations) => configurations,
            Err(error) if is_denied(&error) => return Ok(denied(settings_url)),
            Err(error) => return Err(error),
        };
    Ok(from_configurations(&configurations.value).with_settings_url(Some(settings_url)))
}

fn from_configurations(configurations: &[PolicyConfiguration]) -> MergePolicy {
    let mut allowed = MergeStrategy::ALL.to_vec();
    let applying = configurations.iter().filter(|configuration| {
        configuration.is_enabled
            && !configuration.is_deleted
            && configuration.policy_type.as_ref().is_some_and(|policy_type| {
                policy_type.id.eq_ignore_ascii_case(MERGE_STRATEGY_POLICY)
            })
    });
    for configuration in applying {
        let permitted = configuration.settings.allowed();
        allowed.retain(|strategy| permitted.contains(strategy));
    }
    if allowed.is_empty() {
        // Policies that together forbid every type: the platform blocks completion; show them all
        // rather than nothing, the platform's message explains the rest.
        allowed = MergeStrategy::ALL.to_vec();
    }
    let default = allowed.first().copied();
    MergePolicy::reported(allowed, default)
}

/// 401 (also the sign-in redirect of anonymous calls) and 403: this connection may not read it.
fn is_denied(error: &ProviderError) -> bool {
    matches!(error.status(), Some(401 | 403))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configuration(json: serde_json::Value) -> PolicyConfiguration {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn enabled_policies_intersect_and_others_are_ignored() {
        let squash_or_rebase = configuration(serde_json::json!({
            "isEnabled": true, "isBlocking": true, "type": { "id": MERGE_STRATEGY_POLICY },
            "settings": { "allowNoFastForward": false, "allowSquash": true, "allowRebase": true, "allowRebaseMerge": false }
        }));
        let squash_only_disabled = configuration(serde_json::json!({
            "isEnabled": false, "type": { "id": MERGE_STRATEGY_POLICY }, "settings": { "allowSquash": true }
        }));
        let policy = from_configurations(&[squash_or_rebase, squash_only_disabled]);
        assert_eq!(policy.allowed, [MergeStrategy::Squash, MergeStrategy::Rebase]);
        assert_eq!(policy.default, Some(MergeStrategy::Squash));
        assert!(policy.is_reported());
    }

    #[test]
    fn no_policy_allows_every_type_with_no_fast_forward_first() {
        let policy = from_configurations(&[]);
        assert_eq!(policy.allowed, MergeStrategy::ALL);
        assert_eq!(policy.default, Some(MergeStrategy::Merge));
    }

    #[test]
    fn legacy_squash_policy_requires_squash() {
        let legacy = configuration(serde_json::json!({
            "isEnabled": true, "type": { "id": MERGE_STRATEGY_POLICY }, "settings": { "useSquashMerge": true }
        }));
        assert_eq!(from_configurations(&[legacy]).allowed, [MergeStrategy::Squash]);
    }
}
