//! `owner/repo` keys and the REST paths built from them.

// RS-DONT-04: parses provider data, so indexing and narrowing casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use std::fmt;

use review_core::{ProviderError, Result};
use urlencoding::encode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RepoKey<'a> {
    pub owner: &'a str,
    pub name: &'a str,
}

impl<'a> RepoKey<'a> {
    pub fn parse(key: &'a str) -> Result<Self> {
        let parsed = key
            .split_once('/')
            .filter(|(owner, name)| !owner.is_empty() && !name.is_empty() && !name.contains('/'));
        match parsed {
            Some((owner, name)) => Ok(Self { owner, name }),
            None => Err(ProviderError::Local(format!("'{key}' is not an owner/repository key."))),
        }
    }

    /// `/repos/{owner}/{repo}`
    pub fn api_path(&self) -> String {
        format!("/repos/{}/{}", encode(self.owner), encode(self.name))
    }

    /// `/repos/{owner}/{repo}/pulls/{number}`
    pub fn pull_path(&self, number: u64) -> String {
        format!("{}/pulls/{number}", self.api_path())
    }

    /// `/repos/{owner}/{repo}/issues/{number}`; GitHub pull requests are issues too.
    pub fn issue_path(&self, number: u64) -> String {
        format!("{}/issues/{number}", self.api_path())
    }
}

/// `owner/repo`, the key it was parsed from.
impl fmt::Display for RepoKey<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.owner, self.name)
    }
}

/// Encodes a branch name for `/git/refs/heads/{ref}`, keeping its `/` separators.
pub(crate) fn encode_branch(branch: &str) -> String {
    branch.split('/').map(|segment| encode(segment).into_owned()).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use super::{RepoKey, encode_branch};

    #[test]
    fn parses_owner_and_name() {
        let key = RepoKey::parse("contoso/app").expect("valid key");
        assert_eq!(key.api_path(), "/repos/contoso/app");
        assert_eq!(key.pull_path(7), "/repos/contoso/app/pulls/7");
        assert_eq!(key.issue_path(7), "/repos/contoso/app/issues/7");
    }

    #[test]
    fn rejects_malformed_keys() {
        assert!(RepoKey::parse("contoso").is_err());
        assert!(RepoKey::parse("/contoso").is_err());
        assert!(RepoKey::parse("a/b/c").is_err());
    }

    #[test]
    fn encodes_branch_segments() {
        assert_eq!(encode_branch("feature/a b#1"), "feature/a%20b%231");
    }
}
