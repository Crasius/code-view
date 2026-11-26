//! Which merge strategies a pull request's target branch allows, and which one the platform
//! preselects. Read lazily (only where a merge is offered) and cached per repository and branch.

use super::MergeStrategy;

/// Where [`MergePolicy::allowed`] comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicySource {
    /// The platform reported the branch's or repository's merge settings.
    Reported,
    /// The platform did not say; `allowed` is everything its API accepts, narrowed only by what
    /// was readable (GitHub branch rules).
    NotReported(Unreported),
}

/// Why the platform did not report its merge settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unreported {
    /// The connection may not read them (anonymous, or a token without write access).
    NoPermission,
    /// Reading them failed; the message says why.
    Failed(String),
    /// The platform has no API for them.
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergePolicy {
    /// Allowed strategies in the platform's own order. Never empty.
    pub allowed: Vec<MergeStrategy>,
    /// The strategy the platform preselects; `None` where it has no default (GitHub with several
    /// methods allowed) or did not report one.
    pub default: Option<MergeStrategy>,
    pub source: PolicySource,
    /// Where squashing is a flag on top of the project's merge method (GitLab): how the squash
    /// commit lands on the target ([`MergeStrategy::Merge`], [`MergeStrategy::SemiLinear`] or
    /// [`MergeStrategy::Rebase`] for a fast-forward). `None` where squash is its own strategy.
    pub squash_lands_as: Option<MergeStrategy>,
    /// The platform only fast-forwards or merges semi-linearly when the source branch is already
    /// rebased onto the target and does not rebase it while merging (GitLab).
    pub needs_rebased_source: bool,
    /// The platform page where these settings are changed (branch policies, merge settings).
    pub settings_url: Option<String>,
}

impl MergePolicy {
    /// What the platform reported. `default` outside `allowed` is dropped; with one allowed
    /// strategy, that one is the default.
    pub fn reported(allowed: Vec<MergeStrategy>, default: Option<MergeStrategy>) -> Self {
        let default =
            default.filter(|strategy| allowed.contains(strategy)).or_else(|| single(&allowed));
        Self {
            allowed,
            default,
            source: PolicySource::Reported,
            squash_lands_as: None,
            needs_rebased_source: false,
            settings_url: None,
        }
    }

    /// The platform did not report its settings: everything its API accepts (`capable`), no default.
    pub fn not_reported(capable: &[MergeStrategy], reason: Unreported) -> Self {
        Self {
            allowed: capable.to_vec(),
            default: single(capable),
            source: PolicySource::NotReported(reason),
            squash_lands_as: None,
            needs_rebased_source: false,
            settings_url: None,
        }
    }

    #[must_use]
    pub fn with_settings_url(mut self, url: Option<String>) -> Self {
        self.settings_url = url;
        self
    }

    pub fn is_reported(&self) -> bool {
        self.source == PolicySource::Reported
    }
}

fn single(allowed: &[MergeStrategy]) -> Option<MergeStrategy> {
    match allowed {
        [only] => Some(*only),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_allowed_strategy_is_the_default() {
        let policy = MergePolicy::reported(vec![MergeStrategy::Squash], None);
        assert_eq!(policy.default, Some(MergeStrategy::Squash));
        let policy = MergePolicy::reported(
            vec![MergeStrategy::Merge, MergeStrategy::Squash],
            Some(MergeStrategy::Rebase),
        );
        assert_eq!(policy.default, None, "a default the branch forbids is dropped");
    }

    #[test]
    fn unreported_has_no_default() {
        let policy = MergePolicy::not_reported(
            &[MergeStrategy::Merge, MergeStrategy::Squash],
            Unreported::NoPermission,
        );
        assert!(!policy.is_reported());
        assert_eq!(policy.default, None);
    }
}
