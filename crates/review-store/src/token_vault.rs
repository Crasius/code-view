//! Where the access tokens of a connections file rest, and what to do when the system cannot keep
//! them right now.

use std::fmt;
use std::io;

use crate::platform;

/// Step-by-step setup of the Linux secret store, linked from every [`VaultProblem`]. A post on the
/// product's community page (`docs/release.md`, Community), where the screenshots live.
pub const LINUX_HELP_URL: &str = "https://creative-downloads.com/creators/northbyte/c/code-view/posts/getting-started-with-code-view";

/// What the connections file keeps in front of a token that [`TokenVault::File`] holds.
const IN_FILE: &str = "in-file:";
/// Where [`TokenVault::File`] keeps tokens, in the setup's words.
const FILE_SUMMARY: &str =
    "Tokens stay in this profile's connections file. Code View never shows or logs them.";

/// Where [`crate::ConnectionStore`] keeps tokens, and what the setup says and checks about it.
/// [`crate::AppPaths`] decides: the user's data folders use the system's store, folders a caller
/// picked (tests, fixtures) keep their tokens to themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenVault {
    /// The system's store (see `platform`): DPAPI on Windows, the login Keychain on macOS, the
    /// Secret Service on Linux.
    System,
    /// In the connections file itself, readable. Nothing reaches the system's store, so a test
    /// never reads, overwrites or deletes the user's tokens, never waits on a Keychain prompt and
    /// shares nothing with tests running next to it.
    File,
    /// A system store that refuses every token with `problem`, the way a Linux session without a
    /// keyring does. Nothing reaches a real store: tests walk the setup's fix message with it.
    Refusing(&'static VaultProblem),
}

impl TokenVault {
    /// What the connections file keeps for the token of the connection `account`.
    pub(crate) fn protect(self, account: &str, token: &str) -> io::Result<String> {
        match self {
            Self::System => platform::protect_token(account, token),
            Self::File => Ok(format!("{IN_FILE}{token}")),
            Self::Refusing(problem) => Err(problem.clone().into()),
        }
    }

    /// The token behind what the connections file keeps for `account`.
    pub(crate) fn unprotect(self, account: &str, stored: &str) -> io::Result<String> {
        match self {
            Self::System => platform::unprotect_token(account, stored),
            Self::File => stored.strip_prefix(IN_FILE).map(str::to_owned).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the token was protected by the system's store; add it again",
                )
            }),
            Self::Refusing(problem) => Err(problem.clone().into()),
        }
    }

    /// Drops whatever the vault keeps for `account` outside the connections file. A store that
    /// refuses tokens holds none, so a connection without a token still saves.
    pub(crate) fn forget(self, account: &str) -> io::Result<()> {
        match self {
            Self::System => platform::forget_token(account),
            Self::File | Self::Refusing(_) => Ok(()),
        }
    }

    /// One sentence on where a saved token goes, for the setup's token field.
    pub fn summary(self) -> &'static str {
        match self {
            Self::System | Self::Refusing(_) => platform::VAULT_SUMMARY,
            Self::File => FILE_SUMMARY,
        }
    }

    /// Whether a token could be saved now. Asking the system's store blocks (on Linux a D-Bus round
    /// trip, which can start the keyring daemon), so call it off the UI thread.
    pub fn check(self) -> Result<(), VaultProblem> {
        match self {
            Self::System => platform::check_vault(),
            Self::File => Ok(()),
            Self::Refusing(problem) => Err(problem.clone()),
        }
    }
}

/// Why a token cannot be stored or read right now, and how to fix it. Saving a connection fails
/// with an [`io::Error`] that carries this ([`VaultProblem::of`]), so the setup can show the fix
/// next to the Save button instead of a bare error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultProblem {
    pub title: String,
    pub fix: String,
    /// Shell commands that install or start what is missing, in the order to run them.
    pub commands: Vec<String>,
    /// Where the fix is explained step by step.
    pub help_url: &'static str,
}

impl VaultProblem {
    /// The problem `error` carries, if it came from the token store.
    pub fn of(error: &io::Error) -> Option<&VaultProblem> {
        error.get_ref()?.downcast_ref::<VaultProblem>()
    }
}

impl From<VaultProblem> for io::Error {
    fn from(problem: VaultProblem) -> Self {
        io::Error::other(problem)
    }
}

impl fmt::Display for VaultProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.title, self.fix)
    }
}

impl std::error::Error for VaultProblem {}

#[cfg(test)]
mod tests {
    use super::{TokenVault, VaultProblem};
    use crate::test_support::no_keyring;

    #[test]
    fn the_file_vault_round_trips_without_the_system_store() {
        let stored = TokenVault::File.protect("gh-1", "example-value").unwrap();
        assert_eq!(TokenVault::File.unprotect("gh-1", &stored).unwrap(), "example-value");
        TokenVault::File.forget("gh-1").unwrap();
        assert_eq!(
            TokenVault::File.unprotect("gh-1", &stored).unwrap(),
            "example-value",
            "forgetting leaves the file alone"
        );
        assert!(
            TokenVault::File.unprotect("gh-1", "keychain").is_err(),
            "a system store's marker is no token"
        );
    }

    #[test]
    fn a_refusing_vault_fails_saves_with_its_problem_and_keeps_tokenless_connections() {
        let problem = no_keyring();
        let vault = TokenVault::Refusing(problem);
        assert_eq!(vault.check().as_ref(), Err(problem));
        let error = vault.protect("gh-1", "example-value").unwrap_err();
        assert_eq!(VaultProblem::of(&error), Some(problem));
        assert!(!error.to_string().contains("example-value"), "the token never shows in an error");
        vault.forget("gh-1").unwrap();
    }
}
