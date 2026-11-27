//! The listings of a pull request list: one request per connection, whose results arrive
//! independently so fast hosts show first. Each keeps its last answer on screen while a newer one
//! loads (stale-while-revalidate). The user's own writes ([`cv_services::writes`]) apply to its
//! rows as they succeed and to every answer that arrives while the host has not caught up; a row a
//! write moved out of the list leaves it eased ([`Leaving`]). An answer also updates the rows other
//! lists of the connection show ([`spread`]).
//!
//! Answers rest in the page cache under a key per connection, repository, role and state
//! ([`query_cache_key`]); its connection part ([`cache_stamp`]) digests the connection's
//! [`signature`], so an edited host, organization or token presence lists under a key of its own.

use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;

use cv_base::platform::is_window_active;
use cv_runtime::cache::page_cache::{self, PageCache};
use cv_runtime::load::Live;
use cv_runtime::refresh::{ANONYMOUS_PIPELINE_INTERVAL, WhenInactive};
use cv_runtime::route::PullRequestKey;
use cv_services::Services;
use review_core::http::Api;
use review_core::{
    Connection, ProviderKind, PullRequestQuery, PullRequestScope, PullRequestState,
    PullRequestSummary, StateFilter,
};

use super::ListSource;
use super::changes::RowChanges;
use super::pauses::Pauses;
use super::spread::spread;

/// The page size of every listing, per repository where a request names several.
pub(super) const PAGE_SIZE: usize = 100;

/// A card on screen asks its listing again this often with a token. Our number: the page shown
/// stays current without the user's F5 (a return to the window refreshes it at once anyway), at one
/// listing per card on screen, plus the checks of its rows on screen where the listing cannot show
/// a vote ([`super::statuses`]).
pub const LIST_INTERVAL: Duration = Duration::from_secs(120);
/// The same without a token. Our number, the pipelines' anonymous one: GitHub allows 60 anonymous
/// requests an hour, which the pull request page's probe shares.
pub(crate) const ANONYMOUS_LIST_INTERVAL: Duration = ANONYMOUS_PIPELINE_INTERVAL;

/// One connection's listing, its answer and the rows on their way out (see the module doc).
pub(crate) struct ConnectionResult {
    pub connection_id: String,
    pub connection_name: String,
    pub kind: ProviderKind,
    /// The connection has a token: how often the card refreshes on its own, and how its rows'
    /// lines are counted.
    pub(super) has_token: bool,
    /// The connection's web root, for the platform mark's host.
    pub base_url: String,
    pub load: Live<Vec<PullRequestSummary>>,
    /// When the provider last answered ([`Services::now`]); 0 while the rows come from the page
    /// cache. Rows ask for their checks again once it moves (see [`super::statuses`]).
    pub answered_at: f64,
    pub(super) cache_key: String,
    request: ListRequest,
    /// When the running (or last) request was asked for ([`Services::now`]).
    asked_at: f64,
    /// The last write of the user's the rows show ([`cv_services::writes::Writes::last`]).
    writes_seen: u64,
    /// The page cache's [`PageCache::patches`] the rows show.
    patches_seen: u64,
    /// Rows a write of the user's just moved out of the list, while they leave it.
    pub leaving: Vec<Leaving>,
    /// The rows the host changed since the user last saw them.
    pub changes: RowChanges,
    /// A load is owed: it goes out once the card is on screen and open ([`Self::load_owed`]).
    pub(super) owed: bool,
}

/// A row on its way out of the list: a write of the user's moved it out of the state filter (a
/// merge out of the open list). It collapses where it was instead of vanishing at once.
pub(crate) struct Leaving {
    pub row: PullRequestSummary,
    /// Its place among the rows it left.
    pub index: usize,
    /// When it started to leave ([`Services::now`]).
    pub since: f64,
}

/// How long a row takes to leave; it is gone for the lists after this, and the rows paint it shorter
/// on the way ([`super::list_rows`]).
pub(crate) const LEAVE_SECONDS: f32 = cv_base::theme::motion::MEDIUM;

/// What a result asks its provider for.
#[derive(Clone)]
pub(super) enum ListRequest {
    Query(PullRequestQuery),
    /// The pull requests of several repositories at once that `role` keeps
    /// ([`review_core::Provider::list_repositories_pull_requests`]).
    Repositories {
        keys: Vec<String>,
        role: PullRequestScope,
        state: StateFilter,
    },
}

/// What the results were built for; a change rebuilds them.
#[derive(Clone, PartialEq, Eq)]
struct QueryKey {
    scope: PullRequestScope,
    state: StateFilter,
    /// Id, platform, host, organizations and whether a token is set, per connection.
    connections: Vec<String>,
}

#[derive(Default)]
pub(crate) struct ConnectionQueries {
    key: Option<QueryKey>,
    pub results: Vec<ConnectionResult>,
    /// The overview groups can collapse (and then load nothing); a repository list cannot.
    collapsible: bool,
}

impl ConnectionQueries {
    /// Builds the results when the filter or the connections changed. Remembered answers show at
    /// once; only stale ones are owed a load.
    pub(crate) fn ensure(
        &mut self,
        services: &mut Services,
        source: &ListSource,
        scope: PullRequestScope,
        state: StateFilter,
    ) {
        let key = QueryKey {
            scope,
            state,
            connections: services.connections.all().iter().map(signature).collect(),
        };
        if self.key.as_ref() == Some(&key) {
            return;
        }
        self.key = Some(key);
        self.results.clear();
        self.collapsible = source.is_overview();

        let (connections, repository_key): (Vec<Connection>, Option<String>) = match source {
            ListSource::Mine => {
                let listed = services
                    .connections
                    .all()
                    .iter()
                    .filter(|connection| takes_part(connection, scope))
                    .cloned()
                    .collect();
                (listed, None)
            }
            ListSource::Repository { connection_id, repository_key } => (
                services.connection(connection_id).cloned().into_iter().collect(),
                Some(repository_key.clone()),
            ),
        };
        for connection in connections {
            let query = PullRequestQuery {
                scope,
                state,
                repository_key: repository_key.clone(),
                top: PAGE_SIZE,
            };
            let cache_key = query_cache_key(&connection, repository_key.as_deref(), scope, state);
            // A missing or stale answer is owed: it goes out once the card is on screen and open.
            let mut result =
                ConnectionResult::new(services, &connection, cache_key, ListRequest::Query(query));
            result.owed = result.is_due(services);
            self.results.push(result);
        }
    }

    /// A group was expanded (a user action): load it unless a fresh answer is on screen or on its
    /// way.
    pub(crate) fn expand(&mut self, services: &mut Services, connection_id: &str) {
        let Some(result) =
            self.results.iter_mut().find(|result| result.connection_id == connection_id)
        else {
            return;
        };
        if (result.owed || result.is_due(services)) && !result.load.is_loading() {
            result.start(services);
        }
        result.owed = false;
    }

    /// Moves arrived answers on screen and into the page cache.
    pub(crate) fn poll(&mut self, services: &mut Services) {
        poll_results(&mut self.results, services);
    }

    /// Refreshes in the background, keeping the rows on screen: owes a load to every card
    /// (`force`: Refresh, Retry) or to those whose answer is older than the cache's freshness window
    /// (a revisit). Only the open cards on screen send it ([`Self::load_owed`]).
    pub(crate) fn revalidate(&mut self, services: &Services, force: bool) {
        let now = services.now();
        for result in &mut self.results {
            result.owed |= force || services.cache.is_stale(&result.cache_key, now);
        }
    }

    /// Sends the owed loads of the open cards in `on_screen` (connection ids); the others keep
    /// theirs until they scroll in or are expanded. `by_user`: also while the window is inactive.
    pub(crate) fn load_owed(
        &mut self,
        services: &mut Services,
        on_screen: &BTreeSet<String>,
        by_user: bool,
    ) {
        let shown = self.shown(services, on_screen);
        for (result, is_shown) in self.results.iter_mut().zip(shown) {
            if is_shown {
                result.load_owed(services, by_user);
            }
        }
    }

    /// Each frame: an open card in `on_screen` that was not in `before` owes a load when its answer
    /// is stale, and every open card on screen follows the scheduler's beat. True on a beat.
    pub(crate) fn follow(
        &mut self,
        services: &Services,
        on_screen: &BTreeSet<String>,
        before: Option<&BTreeSet<String>>,
    ) -> bool {
        let shown = self.shown(services, on_screen);
        let mut has_beat = false;
        for (result, is_shown) in self.results.iter_mut().zip(shown) {
            if is_shown {
                let came = before.is_none_or(|before| !before.contains(&result.connection_id));
                has_beat |= result.follow(services, came);
            }
        }
        has_beat
    }

    /// Per result: its card is open and in `on_screen`.
    fn shown(&self, services: &Services, on_screen: &BTreeSet<String>) -> Vec<bool> {
        let results = self.results.iter();
        results
            .map(|result| {
                !self.is_collapsed(services, &result.connection_id)
                    && on_screen.contains(&result.connection_id)
            })
            .collect()
    }

    /// True for a collapsed overview group.
    fn is_collapsed(&self, services: &Services, connection_id: &str) -> bool {
        self.collapsible && services.settings.collapsed_connections.contains(connection_id)
    }

    pub(crate) fn is_collapsible(&self) -> bool {
        self.collapsible
    }

    /// A card's newer listing is under way ([`ConnectionResult::is_refreshing`]).
    pub(crate) fn is_refreshing(&self, pauses: &Pauses) -> bool {
        self.results.iter().any(|result| result.is_refreshing(pauses))
    }

    /// The first failed refresh whose older rows stayed on screen.
    pub(crate) fn refresh_error(&self) -> Option<&str> {
        self.results.iter().find_map(|result| result.load.refresh_error())
    }

    /// One connection's rows as if they had loaded, for tests of what reads them.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn loaded_for_test(
        connection_id: &str,
        kind: ProviderKind,
        rows: Vec<PullRequestSummary>,
    ) -> Self {
        Self {
            key: None,
            results: vec![ConnectionResult::loaded_for_test(connection_id, kind, rows)],
            collapsible: false,
        }
    }
}

impl ConnectionResult {
    /// A newer listing is under way: not while the host's REST pause, which every listing goes
    /// through, holds it back. Then it waits, which the rows' edges and the banner say, and a
    /// spinner would turn for the whole pause.
    pub(crate) fn is_refreshing(&self, pauses: &Pauses) -> bool {
        self.load.is_refreshing() && !pauses.holds(&self.connection_id, Api::Rest)
    }

    /// One connection's rows as if they had loaded.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn loaded_for_test(
        connection_id: &str,
        kind: ProviderKind,
        rows: Vec<PullRequestSummary>,
    ) -> Self {
        let query = PullRequestQuery {
            scope: PullRequestScope::Visible,
            state: StateFilter::Open,
            repository_key: None,
            top: PAGE_SIZE,
        };
        Self {
            connection_id: connection_id.to_owned(),
            connection_name: connection_id.to_owned(),
            kind,
            has_token: true,
            base_url: String::new(),
            load: Live::seeded(Some(rows)),
            answered_at: 0.0,
            cache_key: String::new(),
            request: ListRequest::Query(query),
            asked_at: 0.0,
            writes_seen: 0,
            patches_seen: 0,
            leaving: Vec::new(),
            changes: RowChanges::default(),
            owed: false,
        }
    }

    /// A result for `connection` showing the page cache's answer for `cache_key` until its own arrives.
    pub(super) fn new(
        services: &mut Services,
        connection: &Connection,
        cache_key: String,
        request: ListRequest,
    ) -> Self {
        let cached: Option<Vec<PullRequestSummary>> = services.cache.get(&cache_key);
        Self {
            connection_id: connection.id.clone(),
            connection_name: connection.name.clone(),
            kind: connection.kind,
            has_token: connection.token().is_some(),
            base_url: connection.base_url().to_owned(),
            changes: RowChanges::new(cached.as_deref()),
            load: Live::seeded(cached),
            answered_at: 0.0,
            cache_key,
            request,
            asked_at: 0.0,
            // Seeded rows catch up with the writes on the first poll.
            writes_seen: 0,
            // Seeded with the patches so far.
            patches_seen: services.cache.patches(),
            leaving: Vec::new(),
            owed: false,
        }
    }

    /// The `dropped` rows that were on screen start to leave; a row it never showed just stays out.
    fn leave(&mut self, dropped: Vec<PullRequestSummary>, now: f64) {
        let Some(shown) = self.load.ready() else {
            return;
        };
        for row in dropped {
            let place = shown.iter().position(|old| {
                old.number == row.number && old.repository_key == row.repository_key
            });
            if let Some(index) = place {
                self.leaving.push(Leaving { row, index, since: now });
            }
        }
    }

    /// Asks the provider now; the card's next beat comes a full interval later.
    pub(super) fn start(&mut self, services: &mut Services) {
        let Some(provider) = services.provider(&self.connection_id) else {
            return;
        };
        services.refresh.restart(&services.background.ctx(), &self.refresh_key());
        let request = self.request.clone();
        self.asked_at = services.now();
        self.load.start(&services.background, async move {
            match request {
                ListRequest::Query(query) => provider.list_pull_requests(&query).await,
                ListRequest::Repositories { keys, role, state } => {
                    provider.list_repositories_pull_requests(&keys, role, state, PAGE_SIZE).await
                }
            }
        });
    }

    /// Sends the owed load unless one is on its way already; automatic ones only while the window is
    /// active, the user's (`by_user`) always.
    pub(super) fn load_owed(&mut self, services: &mut Services, by_user: bool) {
        let ctx = services.background.ctx();
        if !self.owed || !(by_user || ctx.input(|input| is_window_active(input.viewport()))) {
            return;
        }
        if !self.load.is_loading() {
            self.start(services);
        }
        self.owed = false;
    }

    /// Each frame its card is on screen and open: owes a load when the card just `came` on screen
    /// with a stale answer, or on the scheduler's beat ([`LIST_INTERVAL`], caught up once after the
    /// window was inactive, at once after a return). True on a beat.
    pub(super) fn follow(&mut self, services: &Services, came: bool) -> bool {
        if came && self.is_due(services) {
            self.owed = true;
        }
        let interval = if self.has_token { LIST_INTERVAL } else { ANONYMOUS_LIST_INTERVAL };
        let ctx = services.background.ctx();
        let has_beat =
            services.refresh.due(&ctx, &self.refresh_key(), interval, WhenInactive::Pause);
        self.owed |= has_beat;
        has_beat
    }

    /// The scheduler's key, per listing: a repository's list and the Favorites card that share one
    /// share its beat.
    fn refresh_key(&self) -> String {
        format!("list|{}", self.cache_key)
    }

    /// The repositories the request names, for messages about it; `None` for a connection's listing.
    pub(crate) fn repositories_label(&self) -> Option<String> {
        match &self.request {
            ListRequest::Query(query) => query.repository_key.clone(),
            ListRequest::Repositories { keys, .. } => Some(keys.join(", ")),
        }
    }

    /// The state filter the rows are listed by.
    fn state(&self) -> StateFilter {
        match &self.request {
            ListRequest::Query(query) => query.state,
            ListRequest::Repositories { state, .. } => *state,
        }
    }

    /// Shows the page cache's rows when another list's answer patched them since the last look; a
    /// change the host made counts as one ([`RowChanges::answered`]).
    fn catch_up(&mut self, cache: &mut PageCache) {
        if self.patches_seen == cache.patches() {
            return;
        }
        self.patches_seen = cache.patches();
        let Some(rows) = cache.get::<Vec<PullRequestSummary>>(&self.cache_key) else {
            return;
        };
        if self.load.ready() != Some(&rows) {
            self.changes.answered(&rows);
            self.load.set(rows);
        }
    }

    /// No answer yet, or the shown one is older than the page cache's freshness window.
    pub(super) fn is_due(&self, services: &Services) -> bool {
        self.load.ready().is_none() || services.cache.is_stale(&self.cache_key, services.now())
    }
}

/// Moves arrived answers on screen and into the page cache, each with the user's writes the host
/// may not show yet applied, and shows the writes logged since the rows last caught up.
/// Rows another list's answer patched into the page cache ([`spread`]) replace the shown ones too.
pub(super) fn poll_results(results: &mut [ConnectionResult], services: &mut Services) {
    let now = services.now();
    for result in results {
        result.leaving.retain(|leaving| now - leaving.since < f64::from(LEAVE_SECONDS));
        result.catch_up(&mut services.cache);
        let is_answered = result.load.poll();
        let is_behind = result.writes_seen != services.writes.last();
        result.writes_seen = services.writes.last();
        if !is_answered && !is_behind {
            continue;
        }
        let Some(mut rows) = result.load.ready().cloned() else {
            continue;
        };
        let mut dropped = Vec::new();
        let writes = &services.writes;
        let is_changed =
            writes.settle_rows(&result.connection_id, result.state(), &mut rows, &mut dropped, now);
        // Where the rows on screen had them.
        result.leave(dropped, now);
        if is_changed {
            result.load.set(rows.clone());
        }
        if !is_answered {
            result.changes.follow(&rows);
            // The page cache had the writes applied when they were logged.
            continue;
        }
        result.changes.answered(&rows);
        result.answered_at = now;
        for key in finished_keys(&result.connection_id, &rows) {
            services.retention.pull_request_finished(&result.connection_id, &key);
        }
        let cache = &mut services.cache;
        spread(cache, &result.connection_id, &result.cache_key, &rows, result.asked_at);
        cache.put_answer(&result.cache_key, rows, result.asked_at, now);
    }
}

/// The user saw the row of `pull_request` (the pointer rested on it, or it was opened): wherever
/// `results` list it, it is no longer marked as changed.
pub(super) fn seen(results: &mut [ConnectionResult], pull_request: &PullRequestKey) {
    let lists =
        results.iter_mut().filter(|result| result.connection_id == pull_request.connection_id);
    for result in lists {
        result.changes.seen(&pull_request.repository_key, pull_request.number);
    }
}

/// The page cache key of a listing; a repository's list and the Favorites lane share theirs.
pub(super) fn query_cache_key(
    connection: &Connection,
    repository_key: Option<&str>,
    scope: PullRequestScope,
    state: StateFilter,
) -> String {
    page_cache::list_part(&cache_stamp(connection), repository_key.unwrap_or("*"), scope, state)
}

/// The part of a page cache key that names the connection as it lists: its id and a digest of its
/// [`signature`]. An edited host, organization or token presence lists other rows, so the answers
/// remembered before the edit are not shown as fresh ones after it.
pub(crate) fn cache_stamp(connection: &Connection) -> String {
    let mut hasher = DefaultHasher::new();
    signature(connection).hash(&mut hasher);
    format!("{}|{:016x}", connection.id, hasher.finish())
}

/// Connections with a token list every scope. Anonymous ones only list "everything" in the
/// organizations they name (public groups and projects); "mine" needs to know who "me" is.
fn takes_part(connection: &Connection, scope: PullRequestScope) -> bool {
    knows_me(connection, scope)
        && (connection.token().is_some() || !connection.organizations.is_empty())
}

/// The connection can tell the user's part of `scope`: every connection for everything, only one
/// with a token for "mine". The Favorites card keeps an anonymous connection's starred
/// repositories out of a "mine" view by this, as the connection's own card is kept out.
pub(super) fn knows_me(connection: &Connection, scope: PullRequestScope) -> bool {
    scope == PullRequestScope::Visible || connection.token().is_some()
}

pub(super) fn signature(connection: &Connection) -> String {
    format!(
        "{}|{:?}|{}|{}|{}",
        connection.id,
        connection.kind,
        connection.base_url,
        connection.organizations.join(","),
        connection.token().is_some()
    )
}

/// Storage keys of the merged and closed rows: their local data goes now (retention keeps an
/// open page's data until it closes).
fn finished_keys(connection_id: &str, rows: &[PullRequestSummary]) -> Vec<String> {
    rows.iter()
        .filter(|row| row.state != PullRequestState::Open)
        .map(|row| key_of(connection_id, row).storage_key())
        .collect()
}

/// The key of `summary`, listed by the connection `connection_id`.
pub(super) fn key_of(connection_id: &str, summary: &PullRequestSummary) -> PullRequestKey {
    PullRequestKey {
        connection_id: connection_id.to_owned(),
        repository_key: summary.repository_key.clone(),
        number: summary.number,
    }
}

#[cfg(test)]
mod tests {
    use review_core::{
        Connection, ProviderKind, PullRequestScope, PullRequestState, PullRequestSummary,
        StateFilter,
    };

    use PullRequestScope::Visible;
    use PullRequestState::Open;

    use super::{
        ConnectionResult, PageCache, RowChanges, finished_keys, page_cache, query_cache_key, spread,
    };

    fn row(number: u64, state: PullRequestState) -> PullRequestSummary {
        let row = crate::pull_request_list::list_rows::tests::row;
        PullRequestSummary {
            state,
            repository_key: "cli/cli".into(),
            repository_name: "cli".into(),
            ..row(number)
        }
    }

    #[test]
    fn an_edited_connection_lists_under_a_cache_key_of_its_own() {
        let connection = Connection {
            id: "gh-1".into(),
            name: "GitHub".into(),
            kind: ProviderKind::GitHub,
            base_url: ProviderKind::GitHub.default_base_url().into(),
            organizations: vec!["acme".into()],
            token: None,
        };
        let key = |connection: &Connection| {
            query_cache_key(connection, None, PullRequestScope::Visible, StateFilter::Open)
        };
        let renamed = Connection { name: "Work".into(), ..connection.clone() };
        assert_eq!(key(&connection), key(&renamed), "a new name lists the same rows");
        let edits = [
            Connection {
                organizations: vec!["acme".into(), "umbrella".into()],
                ..connection.clone()
            },
            Connection { token: Some("token".into()), ..connection.clone() },
            Connection { base_url: "https://github.example.com".into(), ..connection.clone() },
        ];
        for edited in &edits {
            assert_ne!(key(&connection), key(edited), "{:?}", edited.organizations);
            assert!(key(edited).starts_with("pulls|gh-1|"), "{}", key(edited));
        }
    }

    #[test]
    fn merged_and_closed_rows_are_reported_as_finished() {
        let rows = [
            row(1, PullRequestState::Open),
            row(2, PullRequestState::Merged),
            row(3, PullRequestState::Closed),
        ];
        assert_eq!(finished_keys("gh-1", &rows), ["gh-1|cli/cli|2", "gh-1|cli/cli|3"]);
    }

    /// A list kept off screen shows the rows another list's answer updated once it polls again,
    /// marked as changed by the host.
    #[test]
    fn a_shown_list_takes_the_rows_another_answer_patched() {
        let key = page_cache::list_part("gh-1|00ff", "cli/cli", Visible, StateFilter::Open);
        let mut cache = PageCache::default();
        cache.put_answer(&key, vec![row(1, Open), row(2, Open)], 10.0, 11.0);
        let mut result = ConnectionResult::loaded_for_test(
            "gh-1",
            ProviderKind::GitHub,
            vec![row(1, Open), row(2, Open)],
        );
        result.cache_key.clone_from(&key);
        result.changes = RowChanges::new(result.load.ready().map(Vec::as_slice));
        result.catch_up(&mut cache);
        assert_eq!(result.load.ready().map(Vec::len), Some(2), "nothing patched");

        let overview = page_cache::list_part("gh-1|00ff", "*", Visible, StateFilter::Open);
        let pushed = PullRequestSummary { head_sha: Some("pushed".into()), ..row(2, Open) };
        spread(&mut cache, "gh-1", &overview, std::slice::from_ref(&pushed), 20.0);
        result.catch_up(&mut cache);
        assert_eq!(result.load.ready(), Some(&vec![row(1, Open), pushed]));
        assert!(result.changes.is_changed("cli/cli", 2), "pushed on the host");
        assert!(!result.changes.is_changed("cli/cli", 1));
    }
}
