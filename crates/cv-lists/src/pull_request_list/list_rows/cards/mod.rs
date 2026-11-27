//! What each card lists this frame: a connection's card ([`connection`]) and the overview's
//! Favorites card ([`favorites`]), as the rows the list paints, grouped by repository under their
//! section headers when the overview asks for it ([`sections`]).

pub(crate) mod sections;

use cv_base::theme::motion;
use egui::{Id, Ui};
use review_core::PullRequestSummary;
use review_store::SectionCard;

use super::super::favorites::FavoriteQueries;
use super::super::queries::{ConnectionQueries, ConnectionResult};
use super::{
    FAVORITES_KEY, FavoritesLane, Group, Header, Hosts, Row, RowFilter, SKELETON_ROWS, filter,
    leaving,
};

/// A connection's card: skeleton rows until its first answer, a note when it failed or waits for
/// the host's rate limit, then its matching rows.
pub(crate) fn connection<'a>(
    ui: &Ui,
    queries: &ConnectionQueries,
    hosts: &Hosts<'_>,
    filter: &RowFilter,
    result: &'a ConnectionResult,
) -> Group<'a> {
    let is_ready = result.load.ready().is_some() || result.load.error().is_some();
    let opacity = motion::reveal(
        ui.ctx(),
        Id::unique(("pull_request_list", &result.connection_id)),
        is_ready,
    );
    let is_open = !(queries.is_collapsible() && hosts.collapsed.contains(&result.connection_id));
    let opening = motion::toggle(
        ui.ctx(),
        Id::unique(("pull_request_group_open", &result.connection_id)),
        is_open,
        motion::MEDIUM,
    );
    let mut body = Vec::new();
    let mut count = None;
    if let Some(pull_requests) = result.load.ready() {
        let mut matching: Vec<&PullRequestSummary> = pull_requests
            .iter()
            .filter(|pull_request| filter.matches(&result.connection_id, pull_request))
            .collect();
        if hosts.lifts_favorites {
            filter::favorites_first(&mut matching, &result.connection_id, hosts.favorites);
        }
        count = Some(matching.len());
        if matching.is_empty() {
            body.push(Row::Empty { is_filtered: !pull_requests.is_empty() });
        }
        body.extend(matching.into_iter().map(|pull_request| Row::Item(result, pull_request)));
        leaving::insert(ui.ctx(), &mut body, 0, result, filter);
        if let Some(grouping) = hosts.sections {
            sections::split(&mut body, SectionCard::Connection, grouping);
        }
    } else if result.load.error().is_some() {
        body.push(Row::Message(result));
    } else if hosts.pauses.holds_any(&result.connection_id) {
        // A pause can last minutes; a still note says why instead of a skeleton shimmering on.
        body.push(Row::Waiting(result.kind));
    } else {
        body.extend((0..SKELETON_ROWS).map(Row::Skeleton));
    }
    Group { header: Header::Connection(result), count, body, opening, opacity }
}

/// The Favorites card: skeleton rows until every starred repository answered once (or failed, or
/// waits for its host's rate limit), so rows that arrive later never reshuffle the ones shown;
/// then all rows mixed, most recently updated first, and below them what failed or waits. What the
/// role hides leads the card from the start, so it stays put while the rest loads.
pub(crate) fn favorites<'a>(
    ui: &Ui,
    lane: &'a FavoritesLane<'a>,
    hosts: &Hosts<'_>,
    filter: &RowFilter,
) -> Group<'a> {
    let queries: &'a FavoriteQueries = lane.queries;
    let is_waiting = |result: &ConnectionResult| hosts.pauses.holds_any(&result.connection_id);
    let is_settled = queries.results.iter().all(|result| {
        result.load.ready().is_some() || result.load.error().is_some() || is_waiting(result)
    });
    let opacity =
        motion::reveal(ui.ctx(), Id::unique(("pull_request_list", FAVORITES_KEY)), is_settled);
    let opening = motion::toggle(
        ui.ctx(),
        Id::unique(("pull_request_group_open", FAVORITES_KEY)),
        !lane.is_collapsed,
        motion::MEDIUM,
    );
    let header = Header::Favorites {
        repositories: lane.repositories,
        hidden: lane.hidden_repositories(),
        refresh_error: queries.refresh_error(),
    };
    let mut body: Vec<Row<'a>> = lane.hidden.iter().map(Row::HiddenFavorites).collect();
    let mut count = None;
    if lane.repositories == 0 && lane.hidden.is_empty() {
        body.push(Row::NoFavorites);
    } else if !is_settled {
        body.extend((0..SKELETON_ROWS).map(Row::Skeleton));
    } else {
        let rows = queries.rows();
        let loaded = rows.len();
        let matching: Vec<_> = rows
            .into_iter()
            .filter(|(result, pull_request)| filter.matches(&result.connection_id, pull_request))
            .collect();
        count = Some(matching.len());
        let unanswered: Vec<Row<'a>> = queries
            .results
            .iter()
            .filter(|result| result.load.ready().is_none())
            .map(|result| {
                if result.load.error().is_some() {
                    Row::Message(result)
                } else {
                    Row::Waiting(result.kind)
                }
            })
            .collect();
        if matching.is_empty() && unanswered.is_empty() {
            body.push(Row::Empty { is_filtered: loaded > 0 });
        }
        let first = body.len();
        body.extend(
            matching.into_iter().map(|(result, pull_request)| Row::Item(result, pull_request)),
        );
        leaving::insert_by_update(ui.ctx(), &mut body, first, &queries.results, filter);
        if let Some(grouping) = hosts.sections {
            sections::split(&mut body, SectionCard::Favorites, grouping);
        }
        body.extend(unanswered);
    }
    Group { header, count, body, opening, opacity }
}
