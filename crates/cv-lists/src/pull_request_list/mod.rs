//! Pull request lists: the overview across every connection (all the user can see, or created by
//! / assigned to them), or all pull requests of one repository.
//!
//! The overview groups rows in one card per connection. Hosts answer at their own pace and fail on
//! their own; a card per host keeps a slow or failing host's skeleton or error inside its card, so
//! rows of the other hosts never reshuffle when it arrives (a flat list merged across hosts would
//! jump). Within a card rows are newest first, with the repository in its own column. Above them
//! the Favorites card mixes the starred repositories of every host (see [`favorites`]); it shows
//! its rows once all of them answered, so it never reshuffles either. With the Favorites lane
//! hidden (an option) the card is gone and asks nothing; each card lists the rows of starred
//! repositories first instead, marked with a star. Another option, on by default, groups each
//! card's rows by repository under a header per repository (`list_rows::cards::sections`); the
//! page header's books tool switches it, as does Settings.
//!
//! Each row shows its checks and approvals ([`statuses`]): what the listing brought at once, the
//! rest asked for the rows on screen only and asked again when a refresh lists them changed.
//!
//! Only what is on screen loads. A card's listing goes out only while the card is on screen
//! (`list_rows::Painted::cards`, remembered from the last frame the page was shown) and open:
//! every load, automatic (the first one, a revisit, a warm, the scheduler's beat) or the user's
//! (Refresh, Retry), is owed to the others until they scroll in or are expanded. A page never shown
//! yet knows no screen, so a warm before its first frame loads nothing. The size column, the
//! checks and the read-ahead ask for rows on screen only ([`statuses`], [`warm`],
//! [`cv_services::diff_stats`]).
//!
//! While shown, every open card on screen refreshes through the refresh scheduler
//! ([`queries::LIST_INTERVAL`]): paused while the window is inactive, caught up once when it is
//! active again, and at once after a return to the window (the scheduler's wake). Each beat also
//! renews the checks and votes of the rows on screen ([`statuses::RowStatuses::renew`]).

mod changes;
mod favorites;
mod filters;
mod interest;
mod list_rows;
mod pauses;
mod queries;
mod spread;
mod statuses;
mod toolbar;
mod warm;

use std::collections::BTreeSet;

use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, style};
use cv_pr_session::session::hand_over_row;
use cv_runtime::route::{Navigator, Route};
use cv_services::Services;
use cv_services::diff_stats::{self, StatRow};
use cv_services::self_review::{Identities, RowRequests};
use cv_services::writes::{self, Added, Effect};
use cv_widgets::refresh_status::RefreshStatus;
use cv_widgets::states::{self, Action};
use cv_widgets::{elide, scroll, star};
use egui::RichText;
use review_core::{ProviderKind, PullRequestScope, PullRequestSummary, StateFilter};
use review_store::{AppSettings, ListFilters};

use self::favorites::FavoriteQueries;
use self::filters::{STAR_GAP, narrows, overview_scope};
use self::list_rows::{EmptyOffer, FavoritesLane, ListAction, ListInput, RowFilter, SIDE_MARGIN};
use self::pauses::Pauses;
use self::queries::ConnectionQueries;
pub use self::queries::{ANONYMOUS_LIST_INTERVAL, LIST_INTERVAL, cache_stamp};
use self::statuses::RowStatuses;
pub use self::toolbar::{FAVORITES_LANE, GROUP_BY_REPOSITORY};
use self::toolbar::{
    HEADER_ROW, HeaderAsks, HeaderLayout, HeaderStar, OverviewTools, TITLE_GAP, header_tools,
};
use self::warm::Warming;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListSource {
    Mine,
    Repository { connection_id: String, repository_key: String },
}

impl ListSource {
    /// The overview across every connection, not one repository's list.
    pub fn is_overview(&self) -> bool {
        matches!(self, Self::Mine)
    }
}

pub const SEARCH_TOOLTIP: &str = concat!("Search (", shortcut!(command "K"), ")");

/// Stars or unstars a repository and saves the choice; the Favorites card follows on its next frame.
pub fn toggle_favorite(services: &mut Services, connection_id: &str, repository_key: &str) {
    let is_favorite = services.settings.favorites.toggle(connection_id, repository_key);
    services.save_settings();
    let text = if is_favorite {
        format!("{repository_key} added to favorites.")
    } else {
        format!("{repository_key} removed from favorites.")
    };
    services.toasts.success(text);
}

/// Shows or hides the overview's Favorites lane and saves the choice; the page follows on its next
/// frame.
fn show_lane(services: &mut Services, is_shown: bool) {
    services.settings.favorites.is_lane_shown = is_shown;
    services.save_settings();
}

/// A row this page has loaded, for the title bar's jump box.
pub struct LoadedPullRequest<'a> {
    pub connection_id: &'a str,
    pub kind: ProviderKind,
    pub pull_request: &'a PullRequestSummary,
}

/// The role, state and repository filters live in the settings (`ListFilters`): the same on every
/// visit and after a restart. Finding a pull request by text is the title bar's search (Ctrl+K),
/// which the header's search button opens.
pub struct PullRequestListPage {
    source: ListSource,
    queries: ConnectionQueries,
    /// The overview's Favorites card; unused by a repository's list.
    favorites: FavoriteQueries,
    /// "Add me as reviewer" asked from rows.
    requests: RowRequests,
    /// The checks and votes of rows, beyond what their listing brought.
    statuses: RowStatuses,
    /// The pages around the row the pointer rests on, read ahead of the click.
    warming: Warming,
    /// The cards on screen the last frame the page was shown (`list_rows::Painted::cards`);
    /// `None` before its first frame.
    on_screen: Option<BTreeSet<String>>,
}

impl PullRequestListPage {
    pub fn new(source: ListSource) -> Self {
        Self {
            source,
            queries: ConnectionQueries::default(),
            favorites: FavoriteQueries::default(),
            requests: RowRequests::default(),
            statuses: RowStatuses::default(),
            warming: Warming::default(),
            on_screen: None,
        }
    }

    pub fn source(&self) -> &ListSource {
        &self.source
    }

    /// The overview with `rows` loaded from one GitHub connection, `connection_id`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn loaded_for_test(
        connection_id: &str,
        rows: Vec<review_core::PullRequestSummary>,
    ) -> Self {
        Self::new_for_test(
            ConnectionQueries::loaded_for_test(
                connection_id,
                review_core::ProviderKind::GitHub,
                rows,
            ),
            FavoriteQueries::default(),
        )
    }

    /// The overview with `rows` loaded from one GitHub connection and `favorites` loaded in its
    /// Favorites card.
    #[cfg(any(test, feature = "test-support"))]
    pub fn loaded_with_favorites_for_test(
        connection_id: &str,
        rows: Vec<review_core::PullRequestSummary>,
        favorites: Vec<review_core::PullRequestSummary>,
    ) -> Self {
        let kind = review_core::ProviderKind::GitHub;
        let favorites =
            FavoriteQueries::loaded_for_test(vec![queries::ConnectionResult::loaded_for_test(
                connection_id,
                kind,
                favorites,
            )]);
        Self::new_for_test(ConnectionQueries::loaded_for_test(connection_id, kind, rows), favorites)
    }

    /// Every card counts as on screen.
    #[cfg(any(test, feature = "test-support"))]
    fn new_for_test(queries: ConnectionQueries, favorites: FavoriteQueries) -> Self {
        let mut on_screen: BTreeSet<String> =
            queries.results.iter().map(|result| result.connection_id.clone()).collect();
        on_screen.insert(list_rows::FAVORITES_KEY.to_owned());
        Self {
            source: ListSource::Mine,
            queries,
            favorites,
            requests: RowRequests::default(),
            statuses: RowStatuses::default(),
            warming: Warming::default(),
            on_screen: Some(on_screen),
        }
    }

    /// Polls the cards' loads as a frame does; true while one of them still runs.
    #[cfg(any(test, feature = "test-support"))]
    pub fn poll_for_test(&mut self, services: &mut Services) -> bool {
        self.land(services);
        self.is_refreshing(&Pauses::default())
    }

    /// The rows on their way out of the cards after a write of the user's.
    #[cfg(any(test, feature = "test-support"))]
    pub fn leaving_for_test(&self) -> usize {
        let results = self.queries.results.iter().chain(&self.favorites.results);
        results.map(|result| result.leaving.len()).sum()
    }

    /// Loaded rows that match `search` by title, author, repository, label or number, in list
    /// order: the Favorites card's first. A pull request listed by two cards counts once.
    pub fn loaded_matching(&self, search: &str) -> Vec<LoadedPullRequest<'_>> {
        let query = search.trim().to_lowercase();
        let mut found: Vec<LoadedPullRequest<'_>> = Vec::new();
        let favorites = self.favorites.rows().into_iter();
        let listed = self.queries.results.iter().flat_map(|result| {
            result
                .load
                .ready()
                .into_iter()
                .flatten()
                .map(move |pull_request| (result, pull_request))
        });
        for (result, pull_request) in favorites
            .chain(listed)
            .filter(|(_, pull_request)| list_rows::matches_search(pull_request, &query))
        {
            let is_known = found.iter().any(|known| {
                known.connection_id == result.connection_id
                    && known.pull_request.repository_key == pull_request.repository_key
                    && known.pull_request.number == pull_request.number
            });
            if !is_known {
                found.push(LoadedPullRequest {
                    connection_id: &result.connection_id,
                    kind: result.kind,
                    pull_request,
                });
            }
        }
        found
    }

    /// Each frame the page is hidden: the cards' answers land as they arrive (a Refresh just
    /// before the user left, a [`Self::warm`]). A finished load wakes the UI with one frame, so
    /// that frame's time is the answer's arrival, which the page cache counts its freshness from.
    /// Taken in only at the revisit, an answer that came 25 s before would count as new, and the
    /// revisit would not ask again.
    pub fn land(&mut self, services: &mut Services) {
        writes::land(services);
        self.queries.poll(services);
        self.favorites.poll(services);
    }

    /// The page is shown again: its rows stay, a background refresh brings newer ones for the cards
    /// on screen whose answer is older than the page cache's freshness window.
    pub fn revisit(&mut self, services: &mut Services) {
        self.land(services);
        self.queries.revalidate(services, false);
        self.favorites.revalidate(services, false);
        self.load_owed(services, false);
    }

    /// While another page is shown and the user will likely come here next: asks for what is
    /// stale (or was never listed) on the cards that were on screen, in the background, so the
    /// rows are current when they arrive; their checks and votes are renewed then.
    pub fn warm(&mut self, services: &mut Services) {
        let (scope, state) = self.query(&services.settings.list_filters);
        self.queries.ensure(services, &self.source, scope, state);
        if self.has_lane(services) {
            self.favorites.ensure(services, scope, state);
        }
        self.statuses.renew(services.now());
        self.revisit(services);
    }

    /// A card's or the Favorites card's newer listing is under way, unless `pauses` hold it back.
    fn is_refreshing(&self, pauses: &Pauses) -> bool {
        self.queries.is_refreshing(pauses) || self.favorites.is_refreshing(pauses)
    }

    /// The overview with its Favorites lane shown.
    fn has_lane(&self, services: &Services) -> bool {
        self.source.is_overview() && services.settings.favorites.is_lane_shown
    }

    /// Sends the owed loads of the open cards on screen ([`Self::on_screen`]); automatic ones only
    /// while the window is active, the user's (`by_user`) always.
    fn load_owed(&mut self, services: &mut Services, by_user: bool) {
        let Some(on_screen) = &self.on_screen else {
            return;
        };
        self.queries.load_owed(services, on_screen, by_user);
        let is_lane_on_screen = on_screen.contains(list_rows::FAVORITES_KEY);
        self.favorites.load_owed(services, is_lane_on_screen, by_user);
    }

    /// After the frame's cards were painted: remembers which are on screen, lets the ones that
    /// came on screen and the scheduler's beats owe loads, and sends what is owed.
    fn follow(&mut self, services: &mut Services, cards: BTreeSet<String>) {
        let before = self.on_screen.take();
        let on_screen = &*self.on_screen.insert(cards);
        let mut has_beat = self.queries.follow(services, on_screen, before.as_ref());
        if on_screen.contains(list_rows::FAVORITES_KEY) {
            let came = before.is_none_or(|before| !before.contains(list_rows::FAVORITES_KEY));
            has_beat |= self.favorites.follow(services, came);
        }
        if has_beat {
            self.statuses.renew(services.now());
        }
        self.load_owed(services, false);
    }

    fn query(&self, filters: &ListFilters) -> (PullRequestScope, StateFilter) {
        match self.source {
            ListSource::Mine => (overview_scope(filters.scope), filters.state),
            ListSource::Repository { .. } => {
                (PullRequestScope::Repository, filters.repository_state)
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, services: &mut Services, navigator: &mut Navigator) {
        let (scope, state) = self.query(&services.settings.list_filters);
        self.queries.ensure(services, &self.source, scope, state);
        // A write that landed meanwhile, also one of a page the user left: before the rows poll.
        writes::land(services);
        self.queries.poll(services);
        diff_stats::poll(services);
        self.warming.poll(services);
        self.statuses.poll(&services.writes, services.now());
        if self.has_lane(services) {
            self.favorites.ensure(services, scope, state);
            self.favorites.poll(services);
        } else {
            // A hidden lane keeps no rows: they would still turn up in the search and the pickers.
            self.favorites = FavoriteQueries::default();
        }
        self.poll_requests(ui.ctx(), services);
        let pauses = Pauses::of(services);
        let asks = self.header(ui, &mut services.settings, &pauses);
        if asks.wants_refresh {
            self.reload_by_hand(services);
        }
        if asks.toggles_sections {
            let sections = &mut services.settings.repository_sections;
            sections.is_grouped = !sections.is_grouped;
        }
        if asks.filters_changed || asks.toggles_sections {
            services.save_settings();
        }
        match (asks.star, &self.source) {
            (Some(HeaderStar::Lane), _) => {
                let is_shown = !services.settings.favorites.is_lane_shown;
                show_lane(services, is_shown);
            }
            (
                Some(HeaderStar::Repository),
                ListSource::Repository { connection_id, repository_key },
            ) => toggle_favorite(services, connection_id, repository_key),
            _ => {}
        }

        if services.connections.all().is_empty() {
            let picked = states::message_with_actions(
                ui,
                icons::PLUGS,
                "No connections",
                "Connect Azure DevOps, GitHub or GitLab to see your pull requests here.",
                &[Action::primary(icons::PLUS, "Add a connection")],
            );
            if picked.is_some() {
                navigator.go(Route::Settings);
            }
            return;
        }
        let cards = self.list(ui, services, navigator, scope, &pauses);
        self.follow(services, cards);
    }

    /// The header: title (with the repository's star), filters and tools, on one row or with the
    /// filters on a second one; returns what it asked for.
    fn header(&self, ui: &mut egui::Ui, settings: &mut AppSettings, pauses: &Pauses) -> HeaderAsks {
        let palette = theme::palette(ui.ctx());
        let mut asks = HeaderAsks::default();
        egui::Frame::new()
            .fill(palette.surface)
            // Lined up with the cards below, which keep the scroll gutter free on the right.
            .inner_margin(scroll::with_gutter(egui::Margin {
                left: SIDE_MARGIN as i8,
                right: SIDE_MARGIN as i8,
                top: 16,
                bottom: 0,
            }))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let title = match &self.source {
                    ListSource::Mine => "Pull requests".to_owned(),
                    ListSource::Repository { repository_key, .. } => repository_key.clone(),
                };
                let heading = egui::TextStyle::Heading.resolve(ui.style());
                let title_width = elide::text_width(ui, &title, &heading);
                let layout = HeaderLayout::plan(
                    ui.available_width(),
                    title_width,
                    filters::fixed_width(ui, &self.source),
                    self.source.is_overview(),
                );
                let failure = self
                    .queries
                    .refresh_error()
                    .or_else(|| self.favorites.refresh_error())
                    .map(str::to_owned);
                let status = RefreshStatus::of(self.is_refreshing(pauses), failure.as_deref());
                // One row: title, filters, then the tools from the right edge. Every width is planned
                // above, so nothing moves while the window keeps its size.
                asks = self.header_row(ui, settings, &title, layout, status);
                if layout.filters_wrap {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.set_min_height(HEADER_ROW);
                        asks.filters_changed = filters::show(
                            ui,
                            &self.source,
                            &mut settings.list_filters,
                            layout.repository_width,
                            || self.known_repositories(),
                        );
                    });
                }
                if self.source.is_overview() {
                    ui.add_space(style::SPACE_S);
                }
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.min_rect().bottom(),
            egui::Stroke::new(1.0, palette.border),
        );
        asks
    }

    /// The header's first row: the title, the repository's star, the filters (unless they wrap)
    /// and, from the right edge, refresh, search and the refresh note.
    fn header_row(
        &self,
        ui: &mut egui::Ui,
        settings: &mut AppSettings,
        title: &str,
        layout: HeaderLayout,
        status: RefreshStatus<'_>,
    ) -> HeaderAsks {
        let mut asks = HeaderAsks::default();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.set_min_height(HEADER_ROW);
            let size = egui::vec2(layout.title_width, HEADER_ROW);
            ui.allocate_ui_with_layout(
                size,
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.add(egui::Label::new(RichText::new(title).heading()).truncate());
                },
            );
            if let ListSource::Repository { connection_id, repository_key } = &self.source {
                ui.add_space(STAR_GAP);
                let is_favorite = settings.favorites.contains(connection_id, repository_key);
                if star::toggle(ui, is_favorite, repository_key).clicked() {
                    asks.star = Some(HeaderStar::Repository);
                }
            }
            if !layout.filters_wrap {
                ui.add_space(TITLE_GAP);
                asks.filters_changed = filters::show(
                    ui,
                    &self.source,
                    &mut settings.list_filters,
                    layout.repository_width,
                    || self.known_repositories(),
                );
            }
            let overview = self.source.is_overview().then_some(OverviewTools {
                is_lane_shown: settings.favorites.is_lane_shown,
                is_grouped: settings.repository_sections.is_grouped,
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                header_tools(ui, layout, status, overview, &mut asks);
            });
        });
        asks
    }

    /// The cards of rows, and what their buttons asked for; returns the cards on screen.
    fn list(
        &mut self,
        ui: &mut egui::Ui,
        services: &mut Services,
        navigator: &mut Navigator,
        scope: PullRequestScope,
        pauses: &Pauses,
    ) -> BTreeSet<String> {
        let filters = &services.settings.list_filters;
        let offer = match self.source {
            ListSource::Repository { .. } if filters.repository_state != StateFilter::All => {
                EmptyOffer::ShowAll
            }
            ListSource::Mine if narrows(filters) => EmptyOffer::ClearFilters,
            ListSource::Repository { .. } | ListSource::Mine => EmptyOffer::Nothing,
        };
        // Repository lists show one repository already; the overview's repository filter is not theirs.
        let repository = filters.repository.as_ref().filter(|_| self.source.is_overview());
        let filter = RowFilter::new(repository);
        let is_overview = self.source.is_overview();
        let hosts = list_rows::Hosts {
            collapsed: &services.settings.collapsed_connections,
            pauses,
            favorites: &services.settings.favorites,
            lifts_favorites: is_overview && !services.settings.favorites.is_lane_shown,
            sections: Some(&services.settings.repository_sections)
                .filter(|sections| is_overview && sections.is_grouped),
            stats: &services.diff_stats,
            requests: &self.requests,
            statuses: &self.statuses,
            flights: &services.writes.flights,
            identities: &Identities::of(ui.ctx()),
        };
        let lane = (is_overview && services.settings.favorites.is_lane_shown).then(|| {
            let (listed, hidden) = favorites::split(services, scope);
            FavoritesLane {
                queries: &self.favorites,
                repositories: listed.len(),
                hidden,
                is_collapsed: services.settings.favorites.is_collapsed,
            }
        });
        let mut visible = list_rows::Painted::default();
        let input = ListInput {
            queries: &self.queries,
            lane: lane.as_ref(),
            hosts: &hosts,
            filter: &filter,
            offer,
        };
        let action = list_rows::show(ui, &input, navigator, &mut visible);
        if let Some(action) = action {
            self.apply(services, action);
        }
        // The page opens at the end of this frame and reads the row from the page cache.
        if let Some((key, summary)) = visible.opened.take() {
            visible.seen = Some(key.clone());
            hand_over_row(services, &key, summary);
        }
        if let Some(key) = visible.seen.take() {
            queries::seen(&mut self.queries.results, &key);
            queries::seen(&mut self.favorites.results, &key);
        }
        self.warming.rest(services, &visible.ahead, visible.rested);
        let painted = visible.ahead.iter().map(|ahead| &ahead.row);
        diff_stats::want(services, painted, |row| self.project_rows(row));
        statuses::want(&mut self.statuses, services, &visible.statuses);
        visible.cards
    }

    /// Carries out what a card's button asked for.
    fn apply(&mut self, services: &mut Services, action: ListAction) {
        match action {
            ListAction::ToggleConnectionCard(connection_id) => {
                let collapsed = &mut services.settings.collapsed_connections;
                if !collapsed.remove(&connection_id) {
                    collapsed.insert(connection_id.clone());
                }
                services.save_settings();
                self.queries.expand(services, &connection_id);
            }
            ListAction::ToggleFavoritesCard => {
                let favorites = &mut services.settings.favorites;
                favorites.is_collapsed = !favorites.is_collapsed;
                let is_expanded = !favorites.is_collapsed;
                services.save_settings();
                if is_expanded {
                    self.favorites.expand(services);
                }
            }
            ListAction::HideFavoritesLane => show_lane(services, false),
            ListAction::ToggleStar { connection_id, repository_key } => {
                toggle_favorite(services, &connection_id, &repository_key);
            }
            ListAction::Retry => self.reload_by_hand(services),
            ListAction::ClearFilters => {
                let filters = &mut services.settings.list_filters;
                let defaults = ListFilters::default();
                filters.scope = defaults.scope;
                filters.state = defaults.state;
                filters.repository = None;
                services.save_settings();
            }
            ListAction::ShowAll => {
                services.settings.list_filters.repository_state = StateFilter::All;
                services.save_settings();
            }
            ListAction::CountLines(row) => {
                diff_stats::count(services, &row, |row| self.project_rows(row));
            }
            ListAction::AddMe(key) => self.requests.start(services, key),
            ListAction::ToggleSection(key) => {
                services.settings.repository_sections.toggle(key);
                services.save_settings();
            }
        }
    }

    /// Refresh and Retry: every open card on screen asks again at once, the others when they come
    /// on screen, and the rows take the host's word over the user's writes it may not show yet.
    fn reload_by_hand(&mut self, services: &mut Services) {
        services.writes.lists_reloaded_by_hand();
        self.queries.revalidate(services, true);
        self.favorites.revalidate(services, true);
        self.load_owed(services, true);
    }

    /// Loads who the user is where a row's menu asked, and logs every "Add me as reviewer" that
    /// worked as a write of the user's, which every card that lists the pull request shows at once.
    fn poll_requests(&mut self, ctx: &egui::Context, services: &mut Services) {
        Identities::of(ctx).start_asked(services);
        let added = self.requests.poll(services);
        if added.is_empty() {
            return;
        }
        for (key, me) in added {
            writes::record(services, &key, Effect::ReviewerAdded(Added::Me), Some(me));
        }
        self.queries.poll(services);
        self.favorites.poll(services);
        self.statuses.poll(&services.writes, services.now());
    }

    /// Every loaded row of `row`'s repository (each once), whose lines a platform that batches
    /// counts together.
    fn project_rows(&self, row: &StatRow) -> Vec<StatRow> {
        let (connection_id, repository_key) =
            (row.connection_id.as_str(), row.repository_key.as_str());
        let mut rows: Vec<StatRow> = self
            .queries
            .results
            .iter()
            .chain(&self.favorites.results)
            .filter(|result| result.connection_id == connection_id)
            .flat_map(|result| {
                let listed = result.load.ready().into_iter().flatten();
                listed.filter(|pull_request| pull_request.repository_key == repository_key).map(
                    move |pull_request| {
                        list_rows::stat_row(result, pull_request, result.has_token, row.is_favorite)
                    },
                )
            })
            .collect();
        rows.sort_by_key(|row| row.number);
        rows.dedup_by_key(|row| row.number);
        rows
    }

    /// Repositories in the rows loaded so far: (connection id, connection name, repository key).
    fn known_repositories(&self) -> Vec<(String, String, String)> {
        let mut repositories: Vec<(String, String, String)> = self
            .queries
            .results
            .iter()
            .chain(&self.favorites.results)
            .flat_map(|result| {
                result.load.ready().into_iter().flatten().map(|pull_request| {
                    (
                        result.connection_id.clone(),
                        result.connection_name.clone(),
                        pull_request.repository_key.clone(),
                    )
                })
            })
            .collect();
        // Sorted down to the id and the exact key, so equal entries are neighbours for `dedup`
        // also where two connections share a name.
        repositories.sort_by_cached_key(|(id, name, key)| {
            (key.to_lowercase(), name.clone(), id.clone(), key.clone())
        });
        repositories.dedup();
        repositories
    }
}

#[cfg(test)]
mod tests {
    use review_core::{ProviderKind, PullRequestScope, StateFilter};
    use review_store::ListFilters;

    use super::{ListSource, PullRequestListPage};
    use crate::pull_request_list::favorites::FavoriteQueries;
    use crate::pull_request_list::queries::{ConnectionQueries, ConnectionResult};

    #[test]
    fn the_repository_picker_lists_each_repository_of_a_connection_once() {
        let rows = || vec![super::list_rows::tests::row(1)];
        // Two accounts on one platform, both named after it, list the same repository; the first
        // one's Favorites card lists it again.
        let named = |connection_id: &str| {
            let mut result =
                ConnectionResult::loaded_for_test(connection_id, ProviderKind::GitHub, rows());
            result.connection_name = "GitHub".into();
            result
        };
        let mut queries = ConnectionQueries::default();
        queries.results = vec![named("gh-1"), named("gh-2")];
        let favorites = FavoriteQueries::loaded_for_test(vec![named("gh-1")]);
        let page = PullRequestListPage::new_for_test(queries, favorites);
        let listed: Vec<(String, String, String)> = page.known_repositories();
        let ids: Vec<&str> = listed.iter().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(ids, ["gh-1", "gh-2"], "{listed:?}");
    }

    #[test]
    fn every_list_asks_with_the_saved_filters() {
        let saved = ListFilters {
            scope: PullRequestScope::CreatedByMe,
            state: StateFilter::Merged,
            repository: None,
            repository_state: StateFilter::Closed,
        };
        // A new page (another visit, a restart) starts from what was picked last.
        assert_eq!(
            PullRequestListPage::new(ListSource::Mine).query(&saved),
            (PullRequestScope::CreatedByMe, StateFilter::Merged)
        );
        let repository = ListSource::Repository {
            connection_id: "ado".into(),
            repository_key: "Shop/web".into(),
        };
        assert_eq!(
            PullRequestListPage::new(repository).query(&saved),
            (PullRequestScope::Repository, StateFilter::Closed)
        );
        // A saved role the overview does not offer falls back to everything visible.
        let odd = ListFilters { scope: PullRequestScope::Repository, ..saved };
        assert_eq!(
            PullRequestListPage::new(ListSource::Mine).query(&odd).0,
            PullRequestScope::Visible
        );
    }
}
