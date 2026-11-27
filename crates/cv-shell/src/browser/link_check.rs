//! Whether Code View opens a link: the one answer the native host gives the extension. The same
//! two steps as a copied link ([`crate::links::link_access`]): [`link_access::locate`] finds the
//! connection, then one read of the pull request's detail with its token says whether it may open.
//! The host is a process of its own without the app's services, so the caller does the read
//! ([`check`]'s `read`) and [`link_access::verdict`] tells a refusal from no clear answer. A pull
//! request the app itself just opened in the browser ([`super::passes`]) is answered before the
//! read, without one.

use review_core::Connection;

use cv_runtime::route::PullRequestKey;

use crate::links::link_access::{self, Denial, Target, Verdict};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// A pull request Code View itself just opened in the browser: it stays there.
    Pass,
    /// A pull request of a saved connection that its token read, or whose host gave no clear
    /// answer (the page then shows the failure with its own Retry, as a copied link does).
    Open,
    /// The host refused the connection's token for this pull request.
    NoAccess(Denial),
    /// A pull request on a platform's cloud host that no connection serves.
    NotConnected,
    /// Anything else: another site, or a page of a host that is no pull request.
    NotAPullRequest,
}

impl LinkState {
    /// The state's name in the host's answers.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Open => "open",
            Self::NoAccess(_) => "no_access",
            Self::NotConnected => "not_connected",
            Self::NotAPullRequest => "not_a_pull_request",
        }
    }
}

/// The denial's name in the host's answers.
pub fn denial_name(denial: Denial) -> &'static str {
    match denial {
        Denial::TokenRefused => "token_refused",
        Denial::Forbidden => "forbidden",
        Denial::NotFound => "not_found",
    }
}

/// The answer for `url`. `is_passed` says whether the app just opened the pull request in the
/// browser; otherwise `read` reads its detail through the connection once, within
/// [`link_access::BUDGET`], and `None` is no answer in time.
pub fn check(
    url: &str,
    connections: &[Connection],
    is_passed: impl FnOnce(&PullRequestKey) -> bool,
    read: impl FnOnce(&Connection, &PullRequestKey) -> Option<review_core::Result<()>>,
) -> LinkState {
    let key = match link_access::locate(url, connections) {
        Target::PullRequest { key, .. } => key,
        Target::NotConnected(_) => return LinkState::NotConnected,
        Target::NotAPullRequest => return LinkState::NotAPullRequest,
    };
    let connection = connections
        .iter()
        .find(|connection| connection.id == key.connection_id)
        .expect("resolve keys a pull request to one of `connections`");
    if is_passed(&key) {
        return LinkState::Pass;
    }
    let answer = read(connection, &key);
    let error = match &answer {
        Some(Ok(())) => return LinkState::Open,
        Some(Err(error)) => Some(error),
        None => None,
    };
    match link_access::verdict(key, error) {
        Verdict::NoAccess { denial, .. } => LinkState::NoAccess(denial),
        Verdict::Open(_) | Verdict::Unknown(_) => LinkState::Open,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use cv_testkit::samples;
    use review_core::{Connection, ProviderError, ProviderKind};

    use super::{LinkState, check};
    use crate::links::link_access::Denial;

    fn connections() -> [Connection; 1] {
        [samples::connection("work", ProviderKind::GitLab, "https://gitlab.example.com")]
    }

    fn status(status: u16) -> ProviderError {
        ProviderError::Status { status, path: "/merge_requests".to_owned(), body: String::new() }
    }

    #[test]
    fn a_pull_request_opens_unless_its_host_refuses_the_token() {
        let url = "https://gitlab.example.com/team/app/-/merge_requests/4";
        let read = |answer: Option<review_core::Result<()>>| {
            check(
                url,
                &connections(),
                |_| false,
                |connection, key| {
                    assert_eq!((connection.id.as_str(), key.number), ("work", 4));
                    answer
                },
            )
        };
        assert_eq!(read(Some(Ok(()))), LinkState::Open);
        assert_eq!(read(Some(Err(status(404)))), LinkState::NoAccess(Denial::NotFound));
        assert_eq!(read(Some(Err(status(403)))), LinkState::NoAccess(Denial::Forbidden));
        assert_eq!(read(Some(Err(status(401)))), LinkState::NoAccess(Denial::TokenRefused));
        // No clear answer opens, as a copied link does: the page shows the failure itself.
        assert_eq!(read(Some(Err(status(502)))), LinkState::Open);
        assert_eq!(read(None), LinkState::Open);
    }

    #[test]
    fn links_of_no_connection_stay_without_a_read() {
        let reads = Cell::new(0);
        let cases = [
            ("https://github.com/cli/cli/pull/3", LinkState::NotConnected),
            ("https://gitlab.example.com/team/app/-/issues/4", LinkState::NotAPullRequest),
            ("https://example.org/a/b/pull/1", LinkState::NotAPullRequest),
        ];
        for (url, state) in cases {
            let answer = check(
                url,
                &connections(),
                |_| true,
                |_, _| {
                    reads.set(reads.get() + 1);
                    Some(Ok(()))
                },
            );
            assert_eq!(answer, state, "{url}");
        }
        assert_eq!(reads.get(), 0, "nothing is read for them");
    }

    #[test]
    fn a_pull_request_the_app_just_opened_passes_without_a_read() {
        let url = "https://gitlab.example.com/team/app/-/merge_requests/4/diffs";
        let answer = check(
            url,
            &connections(),
            |key| key.number == 4,
            |_, _| panic!("a passed pull request is not read"),
        );
        assert_eq!(answer, LinkState::Pass);
    }
}
