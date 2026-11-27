//! The project's merge method and squash option (`GET /projects/:id`, `merge_method` and
//! `squash_option`), mapped onto merge strategies.
//!
//! GitLab has no per-merge choice of method: the project merges with a merge commit (`merge`), a
//! merge commit on semi-linear history (`rebase_merge`) or a fast-forward (`ff`). Squashing is a
//! flag on top: `never` hides it, `always` forces it, `default_on` / `default_off` preselect it or
//! not. For `rebase_merge` and `ff`, GitLab merges only once the source branch is rebased onto the
//! target and does not rebase it itself (the merge request reports `need_rebase` until then).
//! Merge settings apply to the whole project, so the branch does not matter here.

use review_core::{MergePolicy, MergeStrategy, ProviderError, Result, Unreported};

use crate::client::{ApiClient, project_path};
use crate::dto::ProjectDto;
use crate::repositories::Projects;

/// The project's merge method as a strategy and whether squashing is allowed, preselected or forced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProjectMerge {
    pub method: MergeStrategy,
    pub squash: Squash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Squash {
    Never,
    Always,
    DefaultOn,
    DefaultOff,
}

impl ProjectMerge {
    /// `None` when the answer left the settings out (or names a method this app does not know).
    pub fn of(project: &ProjectDto) -> Option<Self> {
        let method = match project.merge_method.as_deref()? {
            "merge" => MergeStrategy::Merge,
            "rebase_merge" => MergeStrategy::SemiLinear,
            "ff" => MergeStrategy::Rebase,
            _ => return None,
        };
        // Older hosts without the option allowed squashing, unticked.
        let squash = match project.squash_option.as_deref() {
            Some("never") => Squash::Never,
            Some("always") => Squash::Always,
            Some("default_on") => Squash::DefaultOn,
            _ => Squash::DefaultOff,
        };
        Some(Self { method, squash })
    }

    pub fn allowed(self) -> Vec<MergeStrategy> {
        match self.squash {
            Squash::Never => vec![self.method],
            Squash::Always => vec![MergeStrategy::Squash],
            Squash::DefaultOn | Squash::DefaultOff => vec![self.method, MergeStrategy::Squash],
        }
    }

    pub fn policy(self) -> MergePolicy {
        let default = match self.squash {
            Squash::Always | Squash::DefaultOn => MergeStrategy::Squash,
            Squash::Never | Squash::DefaultOff => self.method,
        };
        let mut policy = MergePolicy::reported(self.allowed(), Some(default));
        policy.squash_lands_as = Some(self.method);
        policy.needs_rebased_source = self.method != MergeStrategy::Merge;
        policy
    }

    /// Refuses a strategy GitLab would not carry out as asked.
    pub fn check(self, strategy: MergeStrategy) -> Result<()> {
        if self.allowed().contains(&strategy) {
            return Ok(());
        }
        Err(ProviderError::Unsupported(format!(
            "This GitLab project merges with {}{}; it has no per-merge choice of method.",
            method_name(self.method),
            match self.squash {
                Squash::Always => " and always squashes",
                Squash::Never => " and never squashes",
                Squash::DefaultOn | Squash::DefaultOff => "",
            }
        )))
    }
}

fn method_name(method: MergeStrategy) -> &'static str {
    match method {
        MergeStrategy::Merge | MergeStrategy::Squash => "a merge commit",
        MergeStrategy::SemiLinear => "a merge commit on semi-linear history",
        MergeStrategy::Rebase => "fast-forward merges",
    }
}

pub(crate) async fn project_merge(client: &ApiClient, repo: &str) -> Result<Option<ProjectMerge>> {
    let project: ProjectDto = client.get(&project_path(repo), &[]).await?;
    Ok(ProjectMerge::of(&project))
}

/// The policy for the merge button: from the project lookup the page's git remote shared a
/// moment before. A merge itself reads the settings afresh ([`project_merge`]).
pub(crate) async fn policy(
    client: &ApiClient,
    projects: &Projects,
    repo: &str,
    capable: &[MergeStrategy],
) -> Result<MergePolicy> {
    let settings_url = Some(format!("{}/{repo}/-/settings/merge_requests", client.web_root()));
    let project = projects.get(client, repo).await?;
    let policy = match ProjectMerge::of(&project) {
        Some(merge) => merge.policy(),
        None => MergePolicy::not_reported(capable, Unreported::NoPermission),
    };
    Ok(policy.with_settings_url(settings_url))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(method: Option<&str>, squash: Option<&str>) -> ProjectDto {
        serde_json::from_value(serde_json::json!({
            "path_with_namespace": "g/p", "name": "p", "web_url": "https://gitlab.example.com/g/p",
            "http_url_to_repo": "https://gitlab.example.com/g/p.git", "merge_method": method, "squash_option": squash,
        }))
        .unwrap()
    }

    #[test]
    fn maps_methods_and_squash_options() {
        let merge = |method, squash| {
            ProjectMerge::of(&project(Some(method), Some(squash))).unwrap().policy()
        };
        let policy = merge("merge", "default_on");
        assert_eq!(
            (policy.allowed.as_slice(), policy.default),
            (&[MergeStrategy::Merge, MergeStrategy::Squash][..], Some(MergeStrategy::Squash))
        );
        assert!(!policy.needs_rebased_source);
        let policy = merge("rebase_merge", "default_off");
        assert_eq!(
            (policy.allowed.as_slice(), policy.default),
            (
                &[MergeStrategy::SemiLinear, MergeStrategy::Squash][..],
                Some(MergeStrategy::SemiLinear)
            )
        );
        assert!(policy.needs_rebased_source);
        assert_eq!(policy.squash_lands_as, Some(MergeStrategy::SemiLinear));
        let policy = merge("ff", "never");
        assert_eq!(
            (policy.allowed.as_slice(), policy.default),
            (&[MergeStrategy::Rebase][..], Some(MergeStrategy::Rebase))
        );
        let policy = merge("ff", "always");
        assert_eq!(
            (policy.allowed.as_slice(), policy.default),
            (&[MergeStrategy::Squash][..], Some(MergeStrategy::Squash))
        );
        assert_eq!(
            policy.squash_lands_as,
            Some(MergeStrategy::Rebase),
            "the squash commit is fast-forwarded"
        );
    }

    #[test]
    fn absent_settings_are_not_reported() {
        assert_eq!(ProjectMerge::of(&project(None, None)), None);
        assert_eq!(
            ProjectMerge::of(&project(Some("merge"), None)).unwrap().squash,
            Squash::DefaultOff
        );
    }

    #[test]
    fn refuses_what_gitlab_would_do_differently() {
        let ff = ProjectMerge::of(&project(Some("ff"), Some("default_off"))).unwrap();
        assert!(ff.check(MergeStrategy::Rebase).is_ok());
        assert!(ff.check(MergeStrategy::Squash).is_ok());
        assert!(
            matches!(ff.check(MergeStrategy::Merge), Err(ProviderError::Unsupported(_))),
            "a fast-forward project writes no merge commit"
        );
        let always = ProjectMerge::of(&project(Some("merge"), Some("always"))).unwrap();
        assert!(matches!(always.check(MergeStrategy::Merge), Err(ProviderError::Unsupported(_))));
    }
}
