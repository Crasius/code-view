//! What the app sends, per connection: the request counter behind Settings → Network activity
//! (and any frame-stats overlay), and the hosts that currently rate-limit a connection.
//! Only counts, methods and path templates; never URLs with queries, headers or tokens.

use std::time::Duration;

use review_core::http::{self, Api, ConnectionTraffic, RateLimitNotice};
use review_core::{Connection, ProviderKind};
use review_store::ConnectionStore;

/// Requests of one stored connection.
#[derive(Debug, Clone)]
pub struct ConnectionRate {
    pub connection_id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub traffic: ConnectionTraffic,
}

/// A stored connection whose host asked the app to wait.
#[derive(Debug, Clone)]
pub struct RateLimited {
    pub connection: Connection,
    pub origin: String,
    /// The kind of requests that wait; the host's other kind goes on.
    pub api: Api,
    pub resumes_in: Duration,
}

/// Requests per minute (and totals per endpoint) for every connection in `connections` that sent
/// any.
pub fn request_rates(connections: &ConnectionStore) -> Vec<ConnectionRate> {
    connections
        .all()
        .iter()
        .filter_map(|connection| {
            let traffic = http::traffic::of(&connection.id)?;
            Some(ConnectionRate {
                connection_id: connection.id.clone(),
                name: connection.name.clone(),
                kind: connection.kind,
                base_url: connection.base_url().to_owned(),
                traffic,
            })
        })
        .collect()
}

/// Connections in `connections` that are paused by a rate limit right now, soonest resume first.
pub fn rate_limited(connections: &ConnectionStore) -> Vec<RateLimited> {
    http::rate_limits()
        .into_iter()
        .flat_map(|RateLimitNotice { origin, api, connection_ids, resumes_in }| {
            connection_ids.into_iter().filter_map(|id| connections.find(&id).cloned()).map(
                move |connection| RateLimited {
                    connection,
                    origin: origin.clone(),
                    api,
                    resumes_in,
                },
            )
        })
        .collect()
}

/// "Retry now": ends the host's pauses so queued requests go out at once.
pub fn retry_now(origin: &str) {
    http::resume(origin);
}

/// The platform's documentation of its rate limits, for the banner's help link. Self-managed GitLab
/// sets its limits in the admin area; gitlab.com publishes its own.
pub fn rate_limit_docs(connection: &Connection) -> &'static str {
    match connection.kind {
        ProviderKind::GitLab
            if connection
                .base_url()
                .eq_ignore_ascii_case(ProviderKind::GitLab.default_base_url()) =>
        {
            "https://docs.gitlab.com/user/gitlab_com/rate_limits/"
        }
        ProviderKind::GitLab => {
            "https://docs.gitlab.com/administration/settings/user_and_ip_rate_limits/"
        }
        ProviderKind::GitHub => {
            "https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api"
        }
        ProviderKind::AzureDevOps => {
            "https://learn.microsoft.com/en-us/azure/devops/integrate/concepts/rate-limits"
        }
    }
}
