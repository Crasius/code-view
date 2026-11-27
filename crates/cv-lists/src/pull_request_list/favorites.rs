//! The overview's Favorites lane: the pull requests of every starred repository, across all
//! connections and platforms, in one card.
//!
//! The overview's filters apply as to every other card: the state and the role are part of the
//! requests (a role asks each starred repository for the user's part of it), the repository
//! filter picks among the rows.
//!
//! Requests: one per starred repository, through the same listing (and page cache entry) as that
//! repository's own list, except where a provider lists several repositories in one request
//! (GitHub's search with a token): there one request covers ten of the connection's favorites, and
//! its rows bring their line counts along.
//! The lane asks nothing while it is collapsed, scrolled off screen or the overview is not shown:
//! its loads are owed until then (see the page's loading in [`super`]). Its automatic loads (the
//! first one, a revisit's revalidation, the scheduler's beat) also wait while the window is
//! inactive and go out once when it is active again; Refresh, Retry and expanding the lane are user
//! actions and go at once.

use cv_runtime::cache::page_cache;
use cv_services::Services;
use review_core::{PullRequestQuery, PullRequestScope, PullRequestSummary, StateFilter};

use super::pauses::Pauses;
use super::queries::{self, ConnectionResult, ListRequest, PAGE_SIZE};

/// What the results were built for; a change rebuilds them.
#[derive(Clone, PartialEq, Eq)]
struct FavoritesKey {
    /// The overview's role: everything, or the user's part.
    role: PullRequestScope,
    state: StateFilter,
    /// (connection id, repository key) of every favorite.
    favorites: Vec<(String, String)>,
    /// Signature of every connection that has favorites (a new token changes how it lists).
    connections: Vec<String>,
}

#[derive(Default)]
pub(crate) struct FavoriteQueries {
    key: Option<FavoritesKey>,
    pub results: Vec<ConnectionResult>,
}

/// Starred repositories of one connection that a role hides: the connection has no token, so it
/// cannot tell the user's part. The card says so, with the way to a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HiddenFavorites {
    pub connection_id: String,
    pub connection_name: String,
    pub repositories: usize,
}

/// The starred repositories of existing connections under `role`: (connection id, repository key)
/// of those the card lists, and per connection those it hides (see [`HiddenFavorites`]).
pub(crate) fn split(
    services: &Services,
    role: PullRequestScope,
) -> (Vec<(String, String)>, Vec<HiddenFavorites>) {
    let mut listed = Vec::new();
    let mut hidden: Vec<HiddenFavorites> = Vec::new();
    for favorite in services.settings.favorites.repositories() {
        let Some(connection) = services.connection(&favorite.connection_id) else {
            continue;
        };
        if queries::knows_me(connection, role) {
            listed.push((favorite.connection_id.clone(), favorite.repository_key.clone()));
        } else if let Some(known) =
            hidden.iter_mut().find(|known| known.connection_id == connection.id)
        {
            known.repositories += 1;
        } else {
            hidden.push(HiddenFavorites {
                connection_id: connection.id.clone(),
                connection_name: connection.name.clone(),
                repositories: 1,
            });
        }
    }
    (listed, hidden)
}

impl FavoriteQueries {
    /// Builds the results when the favorites, the role or state filter or their connections
    /// changed. Remembered answers show at once; missing and stale ones are owed a load. A
    /// connection that cannot tell the user's part (no token) lists nothing for a role, as its own
    /// card is left out then; the card names what it hides ([`split`]).
    pub(crate) fn ensure(
        &mut self,
        services: &mut Services,
        role: PullRequestScope,
        state: StateFilter,
    ) {
        let (favorites, _) = split(services, role);
        // Favorites are sorted by connection, so equal ids are neighbours.
        let mut connection_ids: Vec<String> =
            favorites.iter().map(|(connection_id, _)| connection_id.clone()).collect();
        connection_ids.dedup();
        let connections = connection_ids
            .iter()
            .filter_map(|id| services.connection(id))
            .map(queries::signature)
            .collect();
        let key = FavoritesKey { role, state, favorites, connections };
        if self.key.as_ref() == Some(&key) {
            return;
        }
        self.results.clear();
        for connection_id in &connection_ids {
            let Some(connection) = services.connection(connection_id).cloned() else {
                continue;
            };
            let keys: Vec<String> = key
                .favorites
                .iter()
                .filter(|(id, _)| id == connection_id)
                .map(|(_, repository_key)| repository_key.clone())
                .collect();
            let connection_id = connection_id.as_str();
            // Also for one favorite: the search's rows carry their line counts, a repository's own
            // listing does not.
            let lists_together = services
                .provider(connection_id)
                .is_some_and(|provider| provider.lists_repositories_together());
            if lists_together {
                let cache_key = page_cache::list_part(
                    &queries::cache_stamp(&connection),
                    &format!("favorites:{}", keys.join(",")),
                    role,
                    state,
                );
                self.results.push(ConnectionResult::new(
                    services,
                    &connection,
                    cache_key,
                    ListRequest::Repositories { keys, role, state },
                ));
                continue;
            }
            for repository_key in keys {
                // Without a role the repository's own list asks exactly this, so both share the page cache.
                let query = PullRequestQuery::in_repository(repository_key, role, state, PAGE_SIZE);
                let cache_key = queries::query_cache_key(
                    &connection,
                    query.repository_key.as_deref(),
                    query.scope,
                    state,
                );
                self.results.push(ConnectionResult::new(
                    services,
                    &connection,
                    cache_key,
                    ListRequest::Query(query),
                ));
            }
        }
        for result in &mut self.results {
            result.owed = result.is_due(services);
        }
        self.key = Some(key);
    }

    /// Sends the owed loads while the lane is open and `is_on_screen`; `by_user`: also while the
    /// window is inactive.
    pub(crate) fn load_owed(&mut self, services: &mut Services, is_on_screen: bool, by_user: bool) {
        if services.settings.favorites.is_collapsed || !is_on_screen {
            return;
        }
        for result in &mut self.results {
            result.load_owed(services, by_user);
        }
    }

    /// Each frame the lane is open and on screen: `came` on screen, it owes stale answers a load,
    /// and each of its listings follows the scheduler's beat. True on a beat.
    pub(crate) fn follow(&mut self, services: &Services, came: bool) -> bool {
        if services.settings.favorites.is_collapsed {
            return false;
        }
        let mut has_beat = false;
        for result in &mut self.results {
            has_beat |= result.follow(services, came);
        }
        has_beat
    }

    /// The lane was expanded (a user action): loads what is owed, missing or stale at once.
    pub(crate) fn expand(&mut self, services: &mut Services) {
        for result in &mut self.results {
            if (result.owed || result.is_due(services)) && !result.load.is_loading() {
                result.start(services);
            }
            result.owed = false;
        }
    }

    /// `force` (Refresh, Retry) owes every listing a load; a revisit owes one to answers older than
    /// the cache's freshness window.
    pub(crate) fn revalidate(&mut self, services: &Services, force: bool) {
        for result in &mut self.results {
            result.owed |= force || result.is_due(services);
        }
    }

    /// Moves arrived answers on screen and into the page cache.
    pub(crate) fn poll(&mut self, services: &mut Services) {
        queries::poll_results(&mut self.results, services);
    }

    /// A starred repository's newer listing is under way ([`ConnectionResult::is_refreshing`]).
    pub(crate) fn is_refreshing(&self, pauses: &Pauses) -> bool {
        self.results.iter().any(|result| result.is_refreshing(pauses))
    }

    pub(crate) fn refresh_error(&self) -> Option<&str> {
        self.results.iter().find_map(|result| result.load.refresh_error())
    }

    /// Every loaded row with its result, most recently updated first; a pull request that two
    /// results list shows once.
    pub(crate) fn rows(&self) -> Vec<(&ConnectionResult, &PullRequestSummary)> {
        let mut rows: Vec<(&ConnectionResult, &PullRequestSummary)> = Vec::new();
        for result in &self.results {
            for pull_request in result.load.ready().into_iter().flatten() {
                let is_known = rows.iter().any(|(known, row)| {
                    known.connection_id == result.connection_id
                        && row.repository_key == pull_request.repository_key
                        && row.number == pull_request.number
                });
                if !is_known {
                    rows.push((result, pull_request));
                }
            }
        }
        rows.sort_by_key(|(_, pull_request)| std::cmp::Reverse(pull_request.last_activity()));
        rows
    }

    /// Results as if they had loaded, for tests of what reads them.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn loaded_for_test(results: Vec<ConnectionResult>) -> Self {
        Self { key: None, results }
    }
}

#[cfg(test)]
mod tests {
    use review_core::{ProviderKind, PullRequestSummary};
    use time::macros::datetime;

    use super::FavoriteQueries;
    use crate::pull_request_list::queries::ConnectionResult;

    fn row(repository: &str, number: u64, updated: time::OffsetDateTime) -> PullRequestSummary {
        PullRequestSummary {
            repository_key: repository.into(),
            repository_name: repository.rsplit('/').next().unwrap_or(repository).into(),
            updated_at: Some(updated),
            ..crate::pull_request_list::list_rows::tests::row(number)
        }
    }

    #[test]
    fn rows_of_every_platform_mix_newest_updated_first_without_duplicates() {
        let github = ConnectionResult::loaded_for_test(
            "gh",
            ProviderKind::GitHub,
            vec![
                row("acme/app", 3, datetime!(2026-10-18 10:00 UTC)),
                row("acme/app", 1, datetime!(2026-10-10 10:00 UTC)),
            ],
        );
        let gitlab = ConnectionResult::loaded_for_test(
            "gl",
            ProviderKind::GitLab,
            vec![row("group/api", 7, datetime!(2026-10-15 10:00 UTC))],
        );
        let again = ConnectionResult::loaded_for_test(
            "gh",
            ProviderKind::GitHub,
            vec![row("acme/app", 3, datetime!(2026-10-18 10:00 UTC))],
        );
        let queries = FavoriteQueries::loaded_for_test(vec![github, gitlab, again]);
        let order: Vec<(&str, u64)> = queries
            .rows()
            .iter()
            .map(|(result, row)| (result.connection_id.as_str(), row.number))
            .collect();
        assert_eq!(order, [("gh", 3), ("gl", 7), ("gh", 1)]);
    }
}
