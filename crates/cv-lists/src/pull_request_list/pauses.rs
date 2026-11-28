//! The rate-limit pauses in force, per connection and kind of request
//! ([`review_core::http::rate_limits`], the banner above the page says why and for how long). A
//! request a pause holds back is in flight for as long as the pause lasts, up to an hour, while
//! nothing can happen: what waits for it shows as waiting, which draws no frames, not as under way.

use std::collections::BTreeSet;

use cv_services::Services;
use review_core::http::Api;

#[derive(Debug, Default)]
pub(crate) struct Pauses(BTreeSet<(String, Api)>);

impl Pauses {
    pub(crate) fn of(services: &Services) -> Self {
        Self(
            services
                .rate_limited()
                .into_iter()
                .map(|limited| (limited.connection.id, limited.api))
                .collect(),
        )
    }

    /// The connection's requests of `api` wait.
    pub(crate) fn holds(&self, connection_id: &str, api: Api) -> bool {
        self.0.iter().any(|(id, held)| id == connection_id && *held == api)
    }

    /// Some of the connection's requests wait, of either kind.
    pub(crate) fn holds_any(&self, connection_id: &str) -> bool {
        self.0.iter().any(|(id, _)| id == connection_id)
    }

    /// The connection's requests of `api` waiting.
    #[cfg(test)]
    pub(super) fn of_test(connection_id: &str, api: Api) -> Self {
        Self(BTreeSet::from([(connection_id.to_owned(), api)]))
    }
}
