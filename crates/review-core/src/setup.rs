//! Connection setup: which features a token unlocks, which organizations it reaches, how to
//! create a token that covers everything, and what a denied request means.
//!
//! Providers fill these through [`crate::Provider::inspect_access`],
//! [`crate::Provider::discover_scopes`] and [`crate::Provider::explain_denial`]. Every probe
//! behind them is a read; checking a token never changes anything on the platform.

use crate::error::ProviderError;
use crate::model::UserIdentity;

/// What the app does with a connection, in the order the setup checklist shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    ReadPullRequests,
    Vote,
    Comment,
    Labels,
    Merge,
    Pipelines,
    GitClone,
}

impl Feature {
    pub const ALL: [Feature; 7] = [
        Feature::ReadPullRequests,
        Feature::Vote,
        Feature::Comment,
        Feature::Labels,
        Feature::Merge,
        Feature::Pipelines,
        Feature::GitClone,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Feature::ReadPullRequests => "Read pull requests",
            Feature::Vote => "Approve and vote",
            Feature::Comment => "Comment and resolve threads",
            Feature::Labels => "Edit labels",
            Feature::Merge => "Merge and close",
            Feature::Pipelines => "Pipelines and logs",
            Feature::GitClone => "Fetch code over git",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Granted,
    /// Works, but not everywhere (public repositories only, for example).
    Limited,
    Missing,
    /// Only a write could prove it, and checks never write. The detail names the permission to confirm.
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureCheck {
    pub feature: Feature,
    pub access: Access,
    /// Why, in the platform's own permission names ("Needs the api scope").
    pub detail: String,
}

impl FeatureCheck {
    pub fn new(feature: Feature, access: Access, detail: impl Into<String>) -> Self {
        Self { feature, access, detail: detail.into() }
    }
}

/// The result of checking a token: who it signs in as and what it can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessReport {
    pub user: UserIdentity,
    /// "Classic personal access token", "Fine-grained personal access token", …
    pub token_kind: String,
    /// Scopes the platform reported for the token; empty when it does not report them.
    pub scopes: Vec<String>,
    /// Expiry date as the platform reports it (`2026-12-31`), when known.
    pub expires_at: Option<String>,
    /// One entry per [`Feature`], in [`Feature::ALL`] order.
    pub checks: Vec<FeatureCheck>,
    /// Extra hints that are not a single feature (organization approval, SSO, refresh).
    pub notes: Vec<String>,
}

impl AccessReport {
    /// A report that only confirms the sign-in, for platforms without permission probes.
    pub fn unverified(user: UserIdentity, token_kind: impl Into<String>) -> Self {
        let checks = Feature::ALL
            .iter()
            .map(|feature| {
                FeatureCheck::new(
                    *feature,
                    Access::Unverified,
                    "This platform does not report token permissions.",
                )
            })
            .collect();
        Self {
            user,
            token_kind: token_kind.into(),
            scopes: Vec::new(),
            expires_at: None,
            checks,
            notes: Vec::new(),
        }
    }

    pub fn check(&self, feature: Feature) -> Option<&FeatureCheck> {
        self.checks.iter().find(|check| check.feature == feature)
    }

    /// True when pull requests can be read at all; everything else is optional.
    pub fn is_usable(&self) -> bool {
        self.check(Feature::ReadPullRequests).is_some_and(|check| {
            matches!(check.access, Access::Granted | Access::Limited | Access::Unverified)
        })
    }

    pub fn has_missing(&self) -> bool {
        self.checks.iter().any(|check| check.access == Access::Missing)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    /// The signed-in user's own namespace.
    Account,
    Organization,
    Group,
    /// An Azure DevOps Server project collection.
    Collection,
}

impl ScopeKind {
    pub fn label(self) -> &'static str {
        match self {
            ScopeKind::Account => "Your account",
            ScopeKind::Organization => "Organization",
            ScopeKind::Group => "Group",
            ScopeKind::Collection => "Collection",
        }
    }
}

/// An organization, group or collection the token reaches, offered for the connection's scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeCandidate {
    /// The value stored in [`crate::Connection::organizations`].
    pub key: String,
    pub display_name: String,
    pub kind: ScopeKind,
}

/// A denied request explained in terms of the token, for "Fix token" prompts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessProblem {
    pub status: u16,
    /// One line: "The token was rejected", "The token lacks Merge Request: Read".
    pub title: String,
    /// What to change on the token page, in the platform's own words.
    pub fix: String,
    /// Pages that fix the problem directly ("Authorize for acme"), shown as buttons beside "Fix token".
    /// Empty when the token page is the way out.
    pub actions: Vec<ProblemAction>,
}

/// A link that resolves an [`AccessProblem`] on the platform's own page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProblemAction {
    /// Button text: "Authorize for acme".
    pub label: String,
    pub url: String,
}

/// One way to create a token, with the exact settings to choose on the platform's page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRecipe {
    /// "Classic personal access token".
    pub title: String,
    /// The platform's token page, pre-filled with name and permissions when `is_prefilled`.
    pub create_url: String,
    pub is_prefilled: bool,
    /// Why this kind of token, in one or two sentences.
    pub summary: String,
    /// The settings to choose, each with the reason the app needs it.
    pub settings: Vec<TokenSetting>,
    /// Limits worth knowing before choosing this recipe.
    pub caveats: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSetting {
    /// As the platform labels it: "repo", "Code: Read & write", "Merge Request: Approve".
    pub name: String,
    pub reason: String,
}

impl TokenSetting {
    pub fn new(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self { name: name.into(), reason: reason.into() }
    }
}

/// The simplest token that covers every feature, plus the least-privilege alternative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenGuide {
    pub recommended: TokenRecipe,
    pub alternative: Option<TokenRecipe>,
}

/// Name the platforms show for tokens the app creates links for.
pub const TOKEN_NAME: &str = crate::APP_NAME;
pub const TOKEN_DESCRIPTION: &str = "Pull request review desktop app";

/// The platform-neutral reading of 401 and 403 answers; providers refine it with their permission names.
pub fn generic_denial(error: &ProviderError) -> Option<AccessProblem> {
    match error.status()? {
        401 => Some(AccessProblem {
            status: 401,
            title: "The token was rejected. It may have expired or been revoked.".to_owned(),
            fix: "Create a new token and paste it into this connection.".to_owned(),
            actions: Vec::new(),
        }),
        403 => Some(AccessProblem {
            status: 403,
            title: format!("Access denied: {}", error.server_message().unwrap_or_else(|| "no details from the server".to_owned())),
            fix: "Give the token the missing permission, or create a new token with the recommended settings.".to_owned(),
            actions: Vec::new(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> UserIdentity {
        UserIdentity {
            id: "1".into(),
            display_name: "Ada".into(),
            login: Some("ada".into()),
            avatar_url: None,
        }
    }

    #[test]
    fn unverified_report_lists_every_feature_and_stays_usable() {
        let report = AccessReport::unverified(user(), "Token");
        assert_eq!(report.checks.len(), Feature::ALL.len());
        assert!(report.is_usable());
        assert!(!report.has_missing());
    }

    #[test]
    fn missing_read_access_is_not_usable() {
        let mut report = AccessReport::unverified(user(), "Token");
        report.checks[0].access = Access::Missing;
        assert!(!report.is_usable());
        assert!(report.has_missing());
    }

    #[test]
    fn explains_401_and_403_only() {
        let status = |status| ProviderError::Status {
            status,
            path: "/x".into(),
            body: r#"{"message":"Nope"}"#.into(),
        };
        assert_eq!(generic_denial(&status(401)).map(|problem| problem.status), Some(401));
        assert_eq!(
            generic_denial(&status(403)).map(|problem| problem.title),
            Some("Access denied: Nope".to_owned())
        );
        assert!(generic_denial(&status(404)).is_none());
        assert!(generic_denial(&ProviderError::Network("x".into())).is_none());
    }
}
