//! Which merge strategy the merge button preselects, why, and how each is named.
//!
//! Candidates in order, each skipped when the platform does not allow it:
//! 1. the strategy of an auto-complete (Azure DevOps) or auto-merge (GitHub) set on the pull request,
//! 2. the only allowed strategy (forced by the platform),
//! 3. the strategy last merged with in this repository,
//! 4. the user's favourite for this repository,
//! 5. the user's favourite for the platform,
//! 6. the platform's default,
//! 7. the first allowed strategy.

use review_core::{MergePolicy, MergeStrategy, ProviderKind};
use review_store::MergePreferences;

/// Why a strategy was preselected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    AutoComplete,
    OnlyAllowed,
    LastUsed,
    RepositoryFavourite,
    PlatformFavourite,
    PlatformDefault,
    FirstAllowed,
}

impl Reason {
    /// The quiet line under the preselected option; `None` where the badge says enough.
    pub fn explain(self, kind: ProviderKind) -> Option<String> {
        let auto = match kind {
            ProviderKind::AzureDevOps => "Set by this pull request's auto-complete",
            ProviderKind::GitHub => "Set by this pull request's auto-merge",
            ProviderKind::GitLab => "Set by this merge request's auto-merge",
        };
        match self {
            Reason::AutoComplete => Some(auto.to_owned()),
            Reason::OnlyAllowed => {
                Some(format!("The only merge type {} allows here", kind.label()))
            }
            Reason::LastUsed => Some("Last used in this repository".to_owned()),
            Reason::RepositoryFavourite => Some("Your favourite for this repository".to_owned()),
            Reason::PlatformFavourite => Some(format!("Your favourite for {}", kind.label())),
            Reason::PlatformDefault | Reason::FirstAllowed => None,
        }
    }
}

/// Where the user's preferences for this pull request live.
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    pub kind: ProviderKind,
    pub connection_id: &'a str,
    pub repository: &'a str,
}

/// The strategy to preselect and why. `None` only for a policy without strategies (never built).
pub fn preselect(
    policy: &MergePolicy,
    auto_complete: Option<MergeStrategy>,
    preferences: &MergePreferences,
    scope: Scope<'_>,
) -> Option<(MergeStrategy, Reason)> {
    let only = match policy.allowed.as_slice() {
        [only] => Some(*only),
        _ => None,
    };
    let candidates = [
        (auto_complete, Reason::AutoComplete),
        (only, Reason::OnlyAllowed),
        (preferences.last_used(scope.connection_id, scope.repository), Reason::LastUsed),
        (
            preferences.repository_favourite(scope.connection_id, scope.repository),
            Reason::RepositoryFavourite,
        ),
        (preferences.platform_favourite(scope.kind), Reason::PlatformFavourite),
        (policy.default, Reason::PlatformDefault),
        (policy.allowed.first().copied(), Reason::FirstAllowed),
    ];
    candidates.into_iter().find_map(|(strategy, reason)| {
        strategy
            .filter(|strategy| policy.allowed.contains(strategy))
            .map(|strategy| (strategy, reason))
    })
}

/// The strategy's name as the platform calls it. GitLab fast-forwards without rebasing and calls
/// its semi-linear method a merge commit with semi-linear history.
pub fn name(strategy: MergeStrategy, kind: ProviderKind) -> &'static str {
    match (strategy, kind) {
        (MergeStrategy::Merge, ProviderKind::GitHub) => "Create a merge commit",
        (MergeStrategy::Merge, ProviderKind::GitLab) => "Merge commit",
        (MergeStrategy::Merge, ProviderKind::AzureDevOps) => "Merge (no fast-forward)",
        (MergeStrategy::Squash, ProviderKind::GitHub) => "Squash and merge",
        (MergeStrategy::Squash, _) => "Squash commit",
        (MergeStrategy::Rebase, ProviderKind::GitHub) => "Rebase and merge",
        (MergeStrategy::Rebase, ProviderKind::GitLab) => "Fast-forward merge",
        (MergeStrategy::Rebase, ProviderKind::AzureDevOps) => "Rebase and fast-forward",
        (MergeStrategy::SemiLinear, ProviderKind::GitLab) => "Merge commit, semi-linear history",
        (MergeStrategy::SemiLinear, _) => "Semi-linear merge",
    }
}

/// The merge button's label: exactly what a click runs.
pub fn button_label(strategy: MergeStrategy, kind: ProviderKind) -> &'static str {
    match (strategy, kind) {
        (MergeStrategy::Merge, _) => "Merge commit",
        (MergeStrategy::Squash, ProviderKind::AzureDevOps) => "Squash merge",
        (MergeStrategy::Squash, _) => "Squash and merge",
        (MergeStrategy::Rebase, ProviderKind::GitHub) => "Rebase and merge",
        (MergeStrategy::Rebase, ProviderKind::GitLab) => "Fast-forward merge",
        (MergeStrategy::Rebase, ProviderKind::AzureDevOps) => "Rebase and fast-forward",
        (MergeStrategy::SemiLinear, _) => "Semi-linear merge",
    }
}

/// The armed merge button's label, e.g. "Click again to squash merge".
pub fn confirm_label(strategy: MergeStrategy, kind: ProviderKind) -> String {
    let verb = match (strategy, kind) {
        (MergeStrategy::Merge, _) => "merge",
        (MergeStrategy::Squash, ProviderKind::AzureDevOps) => "squash merge",
        (MergeStrategy::Squash, _) => "squash and merge",
        (MergeStrategy::Rebase, ProviderKind::GitHub) => "rebase and merge",
        (MergeStrategy::Rebase, ProviderKind::GitLab) => "fast-forward",
        (MergeStrategy::Rebase, ProviderKind::AzureDevOps) => "rebase and fast-forward",
        (MergeStrategy::SemiLinear, _) => "merge semi-linear",
    };
    format!("Click again to {verb}")
}

/// The label of the commit message field; `None` where the strategy writes no commit of its own.
pub fn message_label(strategy: MergeStrategy) -> Option<&'static str> {
    match strategy {
        MergeStrategy::Squash => Some("Squash commit message"),
        MergeStrategy::Merge | MergeStrategy::SemiLinear => Some("Merge commit message"),
        MergeStrategy::Rebase => None,
    }
}

/// The repository's short name for "Always use for `<repository>`": its last path segment.
pub fn repository_name(repository: &str) -> &str {
    repository.rsplit('/').next().unwrap_or(repository)
}

/// One line on what the strategy does to `target`, honest about the platform's own rules.
pub fn explain(
    strategy: MergeStrategy,
    kind: ProviderKind,
    policy: &MergePolicy,
    target: &str,
) -> String {
    let rebased_first = if policy.needs_rebased_source {
        " The source branch must be rebased onto it first."
    } else {
        ""
    };
    match strategy {
        MergeStrategy::Merge => {
            format!("Keeps every commit and joins the branches with a merge commit on {target}.")
        }
        MergeStrategy::Squash => match policy.squash_lands_as {
            Some(MergeStrategy::Rebase) => format!(
                "Combines all commits into one, fast-forwarded onto {target}.{rebased_first}"
            ),
            Some(MergeStrategy::SemiLinear) => format!(
                "Combines all commits into one, then adds a merge commit on {target}.{rebased_first}"
            ),
            Some(_) => {
                format!("Combines all commits into one, then adds a merge commit on {target}.")
            }
            None => format!("Combines all commits into a single new commit on {target}."),
        },
        MergeStrategy::Rebase if kind == ProviderKind::GitLab => {
            format!(
                "Moves {target} up to the source commits in a straight line, no merge commit.{rebased_first}"
            )
        }
        MergeStrategy::Rebase => {
            format!("Replays each commit onto {target} in a straight line, no merge commit.")
        }
        MergeStrategy::SemiLinear if kind == ProviderKind::GitLab => {
            format!("Adds a merge commit on {target} while history stays linear.{rebased_first}")
        }
        MergeStrategy::SemiLinear => {
            format!("Replays the commits onto {target}, then adds a merge commit.")
        }
    }
}

/// A choice exists: more than one strategy is allowed.
pub fn is_choice(policy: &MergePolicy) -> bool {
    policy.allowed.len() > 1
}

/// "Rebase and fast-forward isn't allowed on main.": what the platform can merge with but this
/// branch forbids, or `None` when nothing is missing or the platform did not say.
pub fn disallowed(
    policy: &MergePolicy,
    capable: &[MergeStrategy],
    kind: ProviderKind,
    target: &str,
) -> Option<String> {
    if !policy.is_reported() {
        return None;
    }
    let missing: Vec<&str> = capable
        .iter()
        .filter(|strategy| !policy.allowed.contains(strategy))
        .map(|strategy| button_label(*strategy, kind))
        .collect();
    match missing.as_slice() {
        [] => None,
        [one] => Some(format!("{one} isn't allowed on {target}.")),
        [rest @ .., last] => {
            Some(format!("{} and {last} aren't allowed on {target}.", rest.join(", ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use review_core::Unreported;

    use super::*;

    const SCOPE: Scope<'static> =
        Scope { kind: ProviderKind::GitHub, connection_id: "gh", repository: "octo/app" };

    fn policy(allowed: &[MergeStrategy], default: Option<MergeStrategy>) -> MergePolicy {
        MergePolicy::reported(allowed.to_vec(), default)
    }

    #[test]
    fn follows_the_order_and_skips_what_the_platform_forbids() {
        use MergeStrategy::*;
        let mut preferences = MergePreferences::default();
        let all = policy(&[Merge, Squash, Rebase], Some(Merge));
        assert_eq!(
            preselect(&all, None, &preferences, SCOPE),
            Some((Merge, Reason::PlatformDefault))
        );
        preferences.set_platform_favourite(ProviderKind::GitHub, Some(Rebase));
        assert_eq!(
            preselect(&all, None, &preferences, SCOPE),
            Some((Rebase, Reason::PlatformFavourite))
        );
        preferences.set_repository_favourite("gh", "octo/app", Some(Squash));
        assert_eq!(
            preselect(&all, None, &preferences, SCOPE),
            Some((Squash, Reason::RepositoryFavourite))
        );
        preferences.remember_last_used("gh", "octo/app", Merge);
        assert_eq!(preselect(&all, None, &preferences, SCOPE), Some((Merge, Reason::LastUsed)));
        assert_eq!(
            preselect(&all, Some(Rebase), &preferences, SCOPE),
            Some((Rebase, Reason::AutoComplete))
        );

        // The branch only allows squash: everything else is skipped, the forced one wins.
        let squash_only = policy(&[Squash], None);
        assert_eq!(
            preselect(&squash_only, Some(Rebase), &preferences, SCOPE),
            Some((Squash, Reason::OnlyAllowed))
        );
        assert!(!is_choice(&squash_only));

        // Last used is forbidden now: the repository favourite (squash) is next.
        let no_merge = policy(&[Squash, Rebase], None);
        assert_eq!(
            preselect(&no_merge, None, &preferences, SCOPE),
            Some((Squash, Reason::RepositoryFavourite))
        );
    }

    #[test]
    fn without_preferences_or_default_the_first_allowed_is_taken() {
        let unreported = MergePolicy::not_reported(
            &[MergeStrategy::Merge, MergeStrategy::Squash],
            Unreported::NoPermission,
        );
        assert_eq!(
            preselect(&unreported, None, &MergePreferences::default(), SCOPE),
            Some((MergeStrategy::Merge, Reason::FirstAllowed))
        );
    }

    #[test]
    fn names_what_the_branch_forbids_only_when_the_platform_said_so() {
        use MergeStrategy::*;
        let all = [Merge, Squash, Rebase, SemiLinear];
        let ado = ProviderKind::AzureDevOps;
        let squash_only = policy(&[Squash], None);
        assert_eq!(
            disallowed(&squash_only, &all, ado, "main").as_deref(),
            Some(
                "Merge commit, Rebase and fast-forward and Semi-linear merge aren't allowed on main."
            )
        );
        let no_rebase = policy(&[Merge, Squash, SemiLinear], None);
        assert_eq!(
            disallowed(&no_rebase, &all, ado, "main").as_deref(),
            Some("Rebase and fast-forward isn't allowed on main.")
        );
        assert_eq!(disallowed(&policy(&all, None), &all, ado, "main"), None);
        let unreported = MergePolicy::not_reported(&[Merge], Unreported::NoPermission);
        assert_eq!(
            disallowed(&unreported, &all, ado, "main"),
            None,
            "an unreported policy forbids nothing for sure"
        );
    }
}
