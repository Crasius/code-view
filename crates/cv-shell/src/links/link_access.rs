//! Whether Code View can open a pull request link: [`locate`] finds the saved connection that
//! serves it, then [`AccessCheck`] asks that connection's token to read the pull request. The
//! copied-link check runs both before it opens a page; anything else that asks "can Code View open
//! this URL?" (a browser extension) asks the same two steps. Together they answer one of
//! open ([`Verdict::Open`]), no access ([`Verdict::NoAccess`]), not connected
//! ([`Target::NotConnected`]) and not a pull request ([`Target::NotAPullRequest`]), or that the
//! host gave no clear answer ([`Verdict::Unknown`]).
//!
//! The check is one read, the pull request's detail through the provider, on the background
//! runtime. A detail it read goes to the page cache, where the page that opens next finds it
//! fresh and does not read it again (`PullRequestSession::open`). The read does not go through the
//! connection's watched provider: a refusal here concerns one pull request and is told in the
//! caller's answer, so it must not raise the connection-wide access banner.
//!
//! Only a clear refusal counts as no access: 401 (the host refused the token), 403 (the token
//! lacks the permission) and 404 (GitHub and GitLab answer a private repository the token cannot
//! see with 404, so a missing pull request and a hidden one look the same). A 5xx, a network
//! failure, a rate limit or no answer within [`BUDGET`] leaves the access unknown: the host may
//! well serve the pull request a moment later, so the caller opens the page anyway, which shows
//! the failure with its own Retry, instead of holding back a link that probably works.

use std::time::Duration;

use review_core::{Connection, ProviderError, PullRequestDetail};

use cv_runtime::background::{Task, TaskPoll};
use cv_runtime::cache::page_cache::pull_request_part;
use cv_runtime::intents::AddRequest;
use cv_runtime::links::pull_request_link;
use cv_runtime::route::PullRequestKey;
use cv_services::Services;
use cv_services::writes::Answer;

/// How long the check waits for the host before it calls the access unknown. Our own budget: a
/// detail read takes well under a second on a healthy host, and the user is waiting for the jump.
pub const BUDGET: Duration = Duration::from_secs(4);

/// What a URL names, before any request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// A pull request on a saved connection; `web_url` is its canonical page, the same for every
    /// form of the link (`/files`, a fragment, the `visualstudio.com` host).
    PullRequest { key: PullRequestKey, web_url: String },
    /// A pull request on a platform's cloud host that no connection serves yet.
    NotConnected(AddRequest),
    /// Another URL, or a pull request on a host whose platform is unknown without a connection.
    NotAPullRequest,
}

/// The connection and pull request `url` names; sends nothing.
pub(crate) fn locate(url: &str, connections: &[Connection]) -> Target {
    let Some(key) = pull_request_link::resolve(url, connections) else {
        return pull_request_link::unconnected_cloud_link(url)
            .map_or(Target::NotAPullRequest, Target::NotConnected);
    };
    let connection = connections
        .iter()
        .find(|connection| connection.id == key.connection_id)
        .expect("resolve keys a pull request to one of `connections`");
    let web_url = pull_request_link::web_url(connection, &key.repository_key, key.number);
    Target::PullRequest { key, web_url }
}

/// What the host's answer to the read says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The connection read the pull request; its page opens without reading it again.
    Open(PullRequestKey),
    /// The host refused the read; `key.connection_id` names the connection whose token it refused.
    NoAccess { key: PullRequestKey, denial: Denial },
    /// No clear answer (5xx, network, rate limit, over [`BUDGET`]): the access is not known.
    Unknown(PullRequestKey),
}

/// Why the host refused the read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Denial {
    /// 401: the token is invalid or expired.
    TokenRefused,
    /// 403: the token lacks the permission for this repository.
    Forbidden,
    /// 404: the pull request does not exist, or the token cannot see its repository.
    NotFound,
}

/// One read of a pull request's detail through its connection, under way or answered.
pub(crate) struct AccessCheck {
    key: PullRequestKey,
    /// `None` once the connection was gone at the start; `Some(None)` from the task is a read over
    /// [`BUDGET`].
    read: Option<Task<Option<review_core::Result<PullRequestDetail>>>>,
    /// When the read started ([`Services::now`]).
    asked_at: f64,
}

impl AccessCheck {
    /// Starts the read of `key` on the background runtime.
    pub(crate) fn start(services: &mut Services, key: PullRequestKey) -> Self {
        let connection = services.connection(&key.connection_id).cloned();
        let read = connection.map(|connection| {
            let provider = services.providers.build_uncached(&connection);
            let (repository, number) = (key.repository_key.clone(), key.number);
            services.background.spawn(async move {
                tokio::time::timeout(BUDGET, provider.pull_request(&repository, number)).await.ok()
            })
        });
        Self { key, read, asked_at: services.now() }
    }

    /// A read of `key` asked for at input time `asked_at` that answers `detail`.
    #[cfg(test)]
    pub fn answering_for_test(
        services: &Services,
        key: PullRequestKey,
        asked_at: f64,
        detail: PullRequestDetail,
    ) -> Self {
        let read = services.background.spawn(async move { Some(Ok(detail)) });
        Self { key, read: Some(read), asked_at }
    }

    /// The answer once it is in. A detail that was read goes to the page cache first, stamped with
    /// when it was asked for and with the user's writes the host may not show yet applied.
    pub(crate) fn poll(&mut self, services: &mut Services) -> Option<Verdict> {
        let answer = match self.read.as_mut().map(Task::poll) {
            Some(TaskPoll::Pending) => return None,
            Some(TaskPoll::Done(answer)) => answer,
            Some(TaskPoll::Lost) | None => None,
        };
        let key = self.key.clone();
        match answer {
            Some(Ok(mut detail)) => {
                let (now, asked_at) = (services.now(), self.asked_at);
                services.writes.settle_detail(&key, &mut detail, Answer::Host { asked_at }, now);
                let part = pull_request_part(&key.storage_key(), "detail");
                services.cache.put_answer(&part, detail, asked_at, now);
                Some(Verdict::Open(key))
            }
            Some(Err(error)) => Some(verdict(key, Some(&error))),
            None => Some(verdict(key, None)),
        }
    }
}

/// The access a failed read shows; `None` is no answer at all (over the budget, or the task
/// stopped).
pub(crate) fn verdict(key: PullRequestKey, error: Option<&ProviderError>) -> Verdict {
    let denial = match error.and_then(ProviderError::status) {
        Some(401) => Denial::TokenRefused,
        Some(403) => Denial::Forbidden,
        Some(404) => Denial::NotFound,
        _ => return Verdict::Unknown(key),
    };
    Verdict::NoAccess { key, denial }
}

#[cfg(test)]
mod tests {
    use review_core::ProviderError;

    use super::{Denial, Verdict, verdict};
    use cv_runtime::route::PullRequestKey;

    fn key() -> PullRequestKey {
        PullRequestKey {
            connection_id: "gh".to_owned(),
            repository_key: "acme/widgets".to_owned(),
            number: 9,
        }
    }

    fn status(status: u16) -> ProviderError {
        ProviderError::Status { status, path: "/repos".to_owned(), body: String::new() }
    }

    #[test]
    fn only_clear_refusals_count_as_no_access() {
        for (code, denial) in
            [(401, Denial::TokenRefused), (403, Denial::Forbidden), (404, Denial::NotFound)]
        {
            assert_eq!(
                verdict(key(), Some(&status(code))),
                Verdict::NoAccess { key: key(), denial },
                "{code}"
            );
        }
        let unclear = [
            Some(status(500)),
            Some(status(502)),
            Some(ProviderError::Network("timed out".to_owned())),
            Some(ProviderError::RateLimited {
                host: "github.com".to_owned(),
                retry_after_secs: 60,
            }),
            Some(ProviderError::Decode("eof".to_owned())),
            // Over the budget: no answer at all.
            None,
        ];
        for error in unclear {
            assert_eq!(verdict(key(), error.as_ref()), Verdict::Unknown(key()), "{error:?}");
        }
    }
}
