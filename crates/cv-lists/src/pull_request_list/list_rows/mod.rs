//! The virtualized rows of a pull request list: one card per connection, rows painted directly
//! (no widget tree per row), so thousands of rows cost only the ones on screen. The overview leads
//! with the Favorites card, whose rows come from every connection's starred repositories.
//!
//! Row anatomy, left to right: author avatar; title over a meta line (number, author, age); in wide
//! windows the repository over its branches and the size (lines added and deleted over a bar, and
//! the changed files); status pill; approvals; checks ([`indicators`]); and, set apart by a gap and
//! a hairline, a button that opens the pull request on its Files tab. The right columns have fixed widths, so
//! they line up from row to row and across cards. The size column keeps its place while its
//! numbers load ([`cv_services::diff_stats`]), so nothing moves when they arrive. A row the
//! pointer rests on ([`dwell`]) goes to the page, which warms its pull request. J and K move the
//! keyboard focus from row to row ([`keys`]). A 2 px edge on a row's left says how current it is
//! ([`edge`]).
//!
//! The overview can group each card's rows by repository ([`cards::sections`], an option on by
//! default):
//! a header per repository names it once, so the repository's column shows only the branches.

mod cards;
mod column;
mod dwell;
mod edge;
mod filter;
mod header;
mod indicators;
mod item;
mod keys;
mod leaving;
mod notes;
mod pending;
mod size;

use std::collections::BTreeSet;
use std::sync::Arc;

use cv_base::theme::{self, icons, style};
use cv_runtime::route::{Landing, Navigator, PullRequestKey, Route};
use cv_services::diff_stats::{DiffStats, StatRow};
use cv_services::self_review::{Identities, RowAsk, RowRequests};
use cv_services::writes::flights::Flights;
use cv_widgets::numbers::number_prefix;
use cv_widgets::states::{self, Action};
use egui::{Color32, Galley, Painter, Rect, Stroke, StrokeKind, Ui, pos2, vec2};
use review_core::{ProviderKind, PullRequestSummary};
use review_store::{Favorites, RepositorySections, SectionKey};

use self::cards::sections::{self, Section};
use self::column::{Column, Columns};
pub(crate) use self::filter::RowFilter;
pub(super) use self::filter::matches_search;
use self::item::{ItemRow, RepositoryName, RowPick};
use self::keys::RowKeys;
use super::favorites::{FavoriteQueries, HiddenFavorites};
use super::interest::Interest;
use super::pauses::Pauses;
use super::queries::{ConnectionQueries, ConnectionResult, key_of};
use super::statuses::{RowStatuses, StatusRow};
use super::warm::AheadRow;

const ROW_HEIGHT: f32 = 72.0;
const HEADER_HEIGHT: f32 = 48.0;
const EMPTY_HEIGHT: f32 = 56.0;
const MESSAGE_HEIGHT: f32 = 112.0;
const GROUP_GAP: f32 = 16.0;
/// Left of the cards; on the right the scroll gutter comes on top (see [`style::SCROLL_GUTTER`]).
pub(super) const SIDE_MARGIN: f32 = 24.0;
const SKELETON_ROWS: usize = 4;
/// Inner padding of a row, left and right.
const ROW_PADDING: f32 = 20.0;
const COLUMN_GAP: f32 = 20.0;
const AVATAR_SIZE: f32 = 32.0;
/// Title and meta line centers sit this far above and below the row's middle.
const LINE_OFFSET: f32 = 10.5;
const STATUS_WIDTH: f32 = 92.0;
const REPOSITORY_WIDTH: f32 = 240.0;
const SIZE_WIDTH: f32 = 96.0;
/// Narrower cards move the repository and size columns into the meta line (the size as its
/// colored "+N −M"), then drop the status column and the checks' count.
const STATUS_MIN_CARD: f32 = 640.0;
const REPOSITORY_MIN_CARD: f32 = 1040.0;
/// Gap between the platform mark and the meta line's text.
const META_MARK_GAP: f32 = 5.0;
/// The Files button: a square hit target, inset from the card's right edge.
const FILES_BUTTON: f32 = 32.0;
const FILES_INSET: f32 = 12.0;
/// Room between the Files button and the checks column; a hairline divider sits in its middle,
/// so a click aimed at the row never lands on the button by accident.
const FILES_GAP: f32 = 24.0;
/// A chip's height, shared by a card header's and a section header's row count and a row's labels;
/// egui clamps [`style::RADIUS_CHIP`] to half of it, so the ends are round.
const CHIP_HEIGHT: f32 = 18.0;
/// The room left and right of a chip's text, together.
const CHIP_PADDING_X: f32 = 14.0;

/// The width of a chip around `galley`; never narrower than high, so one digit sits in a circle.
fn chip_width(galley: &Galley) -> f32 {
    (galley.size().x + CHIP_PADDING_X).max(CHIP_HEIGHT)
}

/// Paints the chip `rect` filled with `fill`, inside `stroke` when given, with `galley` centered
/// in `text`.
fn paint_chip(
    painter: &Painter,
    rect: Rect,
    galley: Arc<Galley>,
    (fill, text): (Color32, Color32),
    stroke: Option<Stroke>,
) {
    painter.rect_filled(rect, style::RADIUS_CHIP, fill);
    if let Some(stroke) = stroke {
        painter.rect_stroke(rect, style::RADIUS_CHIP, stroke, StrokeKind::Inside);
    }
    painter.galley(rect.center() - galley.size() / 2.0, galley, text);
}

/// What a click in the list asks the page to do.
pub(crate) enum ListAction {
    Retry,
    /// Back to the overview's default filters: every role, active, all repositories.
    ClearFilters,
    /// Switch to the tab that lists pull requests in every state.
    ShowAll,
    /// Collapse or expand the overview group of this connection.
    ToggleConnectionCard(String),
    /// Collapse or expand the Favorites card.
    ToggleFavoritesCard,
    /// The Favorites card's menu: hide the lane.
    HideFavoritesLane,
    /// Star or unstar a repository (a row's context menu).
    ToggleStar {
        connection_id: String,
        repository_key: String,
    },
    /// A row's "Count lines": count its pull request's lines now.
    CountLines(StatRow),
    /// A row's "Add me as reviewer".
    AddMe(PullRequestKey),
    /// Close or open a repository's section.
    ToggleSection(SectionKey),
}

/// What an empty card offers besides its note, so it never ends the way.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmptyOffer {
    Nothing,
    /// The filters narrow the view: [`ListAction::ClearFilters`].
    ClearFilters,
    /// A repository's state tab: [`ListAction::ShowAll`].
    ShowAll,
}

enum Row<'a> {
    Message(&'a ConnectionResult),
    /// The query worked but returned nothing; true when the repository filter hid everything.
    Empty {
        is_filtered: bool,
    },
    /// The first answer waits for the host's rate limit to lift (the banner above has the actions).
    Waiting(ProviderKind),
    Skeleton(usize),
    Item(&'a ConnectionResult, &'a PullRequestSummary),
    /// A row a write of the user's moved out of the list, and how far it is gone ([`leaving`]).
    Leaving(&'a ConnectionResult, &'a PullRequestSummary, f32),
    /// The Favorites card before anything is starred: how to star, and the way to the repositories.
    NoFavorites,
    /// Starred repositories of a connection the role hides (no token), and the way to a token.
    HiddenFavorites(&'a HiddenFavorites),
    /// The header of a repository's rows ([`cards::sections`]).
    Section(Section<'a>),
}

/// What a card's header shows.
enum Header<'a> {
    Connection(&'a ConnectionResult),
    /// The Favorites card: how many repositories it lists and hides, and a failed refresh of one of them.
    Favorites {
        repositories: usize,
        hidden: usize,
        refresh_error: Option<&'a str>,
    },
}

/// One card: its header and the rows below it.
struct Group<'a> {
    header: Header<'a>,
    /// Matching rows of the last answer; `None` before the first one.
    count: Option<usize>,
    body: Vec<Row<'a>>,
    /// 1 expanded, 0 collapsed, in between while the card opens or closes.
    opening: f32,
    /// How far the first answer has faded in.
    opacity: f32,
}

impl Group<'_> {
    /// Stable per card: ids of its animations and widgets.
    fn key(&self) -> &str {
        match self.header {
            Header::Connection(result) => &result.connection_id,
            Header::Favorites { .. } => FAVORITES_KEY,
        }
    }

    fn body_height(&self) -> f32 {
        self.body.iter().map(row_height).sum()
    }

    /// The card's height this frame: the body grows and shrinks with `opening`.
    fn height(&self) -> f32 {
        HEADER_HEIGHT + self.body_height() * self.opening
    }

    /// The pull request rows that can take the keyboard: none while the card is closed.
    fn focusable_rows(&self) -> usize {
        if self.opening > 0.0 {
            self.body.iter().filter(|row| matches!(row, Row::Item(..))).count()
        } else {
            0
        }
    }
}

/// What painting the rows hands back: the pages they open, the rows painted, the keyboard's row.
struct Drawn<'a> {
    navigator: &'a mut Navigator,
    painted: &'a mut Painted,
    keys: RowKeys,
    /// The place of the next focusable row, counted over the cards as they are drawn.
    next_row: usize,
}

/// Per-connection state the cards show besides their rows.
pub(crate) struct Hosts<'a> {
    /// Overview groups shown closed (ignored for a repository list, which has no collapsible header).
    pub collapsed: &'a BTreeSet<String>,
    /// The requests a host's rate limit holds back.
    pub pauses: &'a Pauses,
    /// Starred repositories, for the rows' context menu.
    pub favorites: &'a Favorites,
    /// The overview without its Favorites lane: starred repositories' rows lead each card, starred.
    pub lifts_favorites: bool,
    /// The overview groups each card's rows by repository: the sections the user closed.
    pub sections: Option<&'a RepositorySections>,
    /// What the size columns show.
    pub stats: &'a DiffStats,
    /// The rows' "Add me as reviewer" requests.
    pub requests: &'a RowRequests,
    /// What the checks and approvals indicators show.
    pub statuses: &'a RowStatuses,
    /// The user's writes on their way: their rows show as pending ([`pending`]).
    pub flights: &'a Flights,
    /// Who the user is per connection, once a rest asked ([`super::warm`]): how likely they open a
    /// row depends on it.
    pub identities: &'a Identities,
}

impl Hosts<'_> {
    /// Where the rows name their repository: in the section headers while the overview groups.
    fn repository_name(&self) -> RepositoryName {
        if self.sections.is_some() { RepositoryName::InSection } else { RepositoryName::InRow }
    }
}

/// The pull request rows painted this frame: their lines may be counted and their checks asked for.
#[derive(Default)]
pub(crate) struct Painted {
    pub statuses: Vec<StatusRow>,
    /// The row a click opened this frame: its page paints the header from it until the detail is in.
    pub opened: Option<(PullRequestKey, PullRequestSummary)>,
    /// The rows painted, in order, with how likely each is opened: the list reads ahead around
    /// the row the pointer rests on.
    pub ahead: Vec<AheadRow>,
    /// The row the pointer rests on, of a connection with a token: its place in `ahead`.
    pub rested: Option<usize>,
    /// The row the pointer rests on, of any connection: the user saw what changed in it.
    pub seen: Option<PullRequestKey>,
    /// The cards on screen, open or not: their connection ids, [`FAVORITES_KEY`] for the
    /// Favorites card. Only their listings load.
    pub cards: BTreeSet<String>,
}

/// The overview's Favorites card.
pub(crate) struct FavoritesLane<'a> {
    pub queries: &'a FavoriteQueries,
    /// Starred repositories of connections that exist, which the card lists.
    pub repositories: usize,
    /// Those the role hides, per connection.
    pub hidden: Vec<HiddenFavorites>,
    pub is_collapsed: bool,
}

impl FavoritesLane<'_> {
    fn hidden_repositories(&self) -> usize {
        self.hidden.iter().map(|hidden| hidden.repositories).sum()
    }
}

/// The key the Favorites card uses where a connection card uses its connection id.
pub(super) const FAVORITES_KEY: &str = "\u{0}favorites";

/// What the list shows: the connections' cards with their state, the overview's Favorites card
/// above them (`lane`), and the way forward an empty card shows when no filter of its own hid
/// its rows (`offer`).
pub(crate) struct ListInput<'a> {
    pub queries: &'a ConnectionQueries,
    pub lane: Option<&'a FavoritesLane<'a>>,
    pub hosts: &'a Hosts<'a>,
    pub filter: &'a RowFilter,
    pub offer: EmptyOffer,
}

/// The cards of `input`; the pull request rows painted this frame go into `visible`, so their
/// lines can be counted and their checks asked for.
pub(crate) fn show(
    ui: &mut Ui,
    input: &ListInput<'_>,
    navigator: &mut Navigator,
    visible: &mut Painted,
) -> Option<ListAction> {
    let ListInput { queries, lane, hosts, filter, .. } = *input;
    let mut groups: Vec<Group<'_>> = queries
        .results
        .iter()
        .map(|result| cards::connection(ui, queries, hosts, filter, result))
        .collect();
    // Without any card of its own the overview keeps its message below, unless something is starred.
    if let Some(lane) =
        lane.filter(|lane| !groups.is_empty() || lane.repositories + lane.hidden_repositories() > 0)
    {
        groups.insert(0, cards::favorites(ui, lane, hosts, filter));
    }
    if groups.is_empty() {
        nothing_to_show(ui, navigator);
        return None;
    }

    let keys_id = ui.scope_id().with("pull_request_row_keys");
    let mut keys = RowKeys::load(ui.ctx(), keys_id);
    keys.read(ui, || groups.iter().map(Group::focusable_rows).sum());
    let mut drawn = Drawn { navigator, painted: visible, keys, next_row: 0 };
    let mut action = None;
    egui::ScrollArea::vertical().id_salt("pull_request_rows").auto_shrink(false).show_viewport(
        ui,
        |ui, viewport| {
            let total: f32 =
                GROUP_GAP + groups.iter().map(|group| group.height() + GROUP_GAP).sum::<f32>();
            ui.set_min_height(total);
            let column = Column { origin: ui.max_rect().min, width: ui.available_width() };
            let viewport = viewport.translate(column.origin.to_vec2());
            if let Some(rect) = drawn.keys.target().and_then(|place| column.row(&groups, place)) {
                ui.scroll_to_rect(rect, None);
            }
            let palette = theme::palette(ui.ctx());
            let mut top = column.origin.y + GROUP_GAP;
            for group in &groups {
                let card = column.card(top, group);
                top = card.bottom() + GROUP_GAP;
                if !card.intersects(viewport) {
                    drawn.next_row += group.focusable_rows();
                    continue;
                }
                drawn.painted.cards.insert(group.key().to_owned());
                ui.painter().add(style::card_shadow(&palette).as_shape(card, style::RADIUS_CARD));
                ui.painter().rect_filled(card, style::RADIUS_CARD, palette.surface);
                let header = Rect::from_min_size(card.min, vec2(card.width(), HEADER_HEIGHT));
                let is_collapsible =
                    queries.is_collapsible() || matches!(group.header, Header::Favorites { .. });
                action = action.take().or(header::paint(ui, header, group, is_collapsible));
                if group.opening > 0.0 {
                    ui.painter().hline(
                        card.x_range(),
                        header.bottom() - 0.5,
                        Stroke::new(1.0, palette.border),
                    );
                    // Rows are clipped to the card, so opening and closing slide them under its edge.
                    let mut body = ui.new_child(
                        egui::UiBuilder::new()
                            .id_salt(("pull_request_group", group.key()))
                            .max_rect(card),
                    );
                    body.set_clip_rect(card.intersect(ui.clip_rect()));
                    let picked = paint_body(&mut body, card, viewport, group, input, &mut drawn);
                    action = action.take().or(picked);
                }
                ui.painter().rect_stroke(
                    card,
                    style::RADIUS_CARD,
                    Stroke::new(1.0, palette.border),
                    StrokeKind::Inside,
                );
            }
        },
    );
    drawn.keys.store(ui.ctx(), keys_id);
    action
}

/// No card at all: why, and the ways to rows.
fn nothing_to_show(ui: &mut Ui, navigator: &mut Navigator) {
    let actions = [
        Action::primary(icons::BOOKS, "Browse repositories"),
        Action::secondary(icons::GEAR, "Add a token"),
    ];
    let picked = states::message_with_actions(
        ui,
        icons::GIT_PULL_REQUEST,
        "Nothing to show",
        "Connections without a token can only list all pull requests of the organizations or groups they name. Browse a repository, or add a token to see yours.",
        &actions,
    );
    match picked {
        Some(0) => navigator.go(Route::Repositories),
        Some(_) => navigator.go(Route::Settings),
        None => {}
    }
}

/// The rows below the card's header, painted only where they meet the viewport; the pull
/// requests among them go into `drawn`.
fn paint_body(
    ui: &mut Ui,
    card: Rect,
    viewport: Rect,
    group: &Group<'_>,
    input: &ListInput<'_>,
    drawn: &mut Drawn<'_>,
) -> Option<ListAction> {
    let palette = theme::palette(ui.ctx());
    let hosts = input.hosts;
    let mut top = card.top() + HEADER_HEIGHT;
    let visible = card.intersect(viewport);
    let opacity = group.opacity * group.opening;
    ui.multiply_opacity(group.opening);
    let mut action = None;
    for (index, row) in group.body.iter().enumerate() {
        let height = row_height(row);
        let rect = Rect::from_min_size(pos2(card.left(), top), vec2(card.width(), height));
        top += height;
        let place = drawn.next_row;
        if matches!(row, Row::Item(..)) {
            drawn.next_row += 1;
        }
        if !rect.intersects(visible) {
            continue;
        }
        let is_last = index + 1 == group.body.len();
        match row {
            Row::Message(result) => {
                action = action.take().or(notes::paint_message(ui, rect, result));
            }
            Row::Empty { is_filtered } => {
                action = action.take().or(notes::paint_empty(ui, rect, *is_filtered, input.offer));
            }
            Row::Waiting(kind) => notes::paint_waiting(ui, rect, *kind),
            Row::Skeleton(seed) => notes::paint_skeleton(ui, rect, *seed),
            Row::NoFavorites => {
                if notes::paint_no_favorites(ui, rect) {
                    drawn.navigator.go(Route::Repositories);
                }
            }
            Row::HiddenFavorites(hidden) => notes::paint_hidden_favorites(ui, rect, hidden),
            Row::Section(section) => {
                let toggled =
                    sections::paint(ui, rect, section, (is_last, opacity), drawn.navigator);
                action = action.take().or(toggled);
            }
            Row::Item(result, pull_request) => {
                let slot = Slot { rect, is_last, opacity, place };
                let picked = paint_pull_request(ui, slot, result, pull_request, hosts, drawn);
                action = action.take().or(picked);
            }
            Row::Leaving(result, pull_request, gone) => {
                let slot = Slot { rect, is_last, opacity, place };
                leaving::paint(ui, slot, *gone, (result, pull_request), hosts, drawn);
            }
        }
        if !is_last {
            ui.painter().hline(
                rect.x_range(),
                rect.bottom() - 0.5,
                Stroke::new(1.0, palette.border),
            );
        }
    }
    action
}

/// Where one pull request row goes and how far its card has faded in.
#[derive(Clone, Copy)]
struct Slot {
    rect: Rect,
    is_last: bool,
    opacity: f32,
    /// Its place among the focusable rows.
    place: usize,
}

/// One pull request row; opens what it was asked for and returns what the page has to do. The row
/// goes into `drawn`, so its lines can be counted, and takes the keyboard when J or K moved to it.
fn paint_pull_request(
    ui: &mut Ui,
    slot: Slot,
    result: &ConnectionResult,
    pull_request: &PullRequestSummary,
    hosts: &Hosts<'_>,
    drawn: &mut Drawn<'_>,
) -> Option<ListAction> {
    let Slot { rect, is_last, opacity, place } = slot;
    let connection_id = &result.connection_id;
    let id = ui.scope_id().with((connection_id, &pull_request.repository_key, pull_request.number));
    let is_favorite = hosts.favorites.contains(connection_id, &pull_request.repository_key);
    let has_token = result.has_token;
    let stat_row = stat_row(result, pull_request, is_favorite);
    let shown = hosts.stats.shown(&stat_row);
    let (is_running, failure) =
        hosts.requests.state_of(connection_id, &pull_request.repository_key, pull_request.number);
    let identities = hosts.identities;
    let me = identities.known(connection_id);
    let ask = RowAsk { connection_id, has_token, is_running, failure, me: me.as_ref(), identities };
    let status_row = StatusRow::new(
        connection_id,
        result.kind,
        has_token,
        stat_row.storage_key.clone(),
        result.answered_at,
        pull_request,
    );
    let checks = hosts.statuses.checks(&status_row, pull_request.checks);
    let reviewers = hosts.statuses.reviewers(&status_row, &pull_request.reviewers);
    let required_approvals = hosts.statuses.required_approvals(&status_row);
    let me = me.as_ref().filter(|_| has_token);
    let interest = Interest::of(pull_request, reviewers, required_approvals, me);
    let pending =
        hosts.flights.pending_on(connection_id, &pull_request.repository_key, pull_request.number);
    let item = ItemRow {
        repository: hosts.repository_name(),
        pending,
        rect,
        id,
        is_last,
        opacity,
        is_favorite,
        is_starred: is_favorite && hosts.lifts_favorites,
        shown: &shown,
        ask,
        checks,
        reviewers,
        required_approvals,
    };
    let picked = item::paint(ui, &item, result, pull_request);
    let number = format!("{}{}", number_prefix(result.kind), pull_request.number);
    let edge = edge::of(result, hosts.statuses, hosts.pauses, &status_row);
    let place_of_edge = edge::Place { rect, id, is_last, opacity, number: &number };
    let mut action = edge::paint(ui, &place_of_edge, edge).then_some(ListAction::Retry);
    drawn.keys.drawn(ui, place, id);
    match picked {
        Some(RowPick::Open(landing)) => {
            let key = key_of(connection_id, pull_request);
            drawn.painted.opened = Some((key.clone(), pull_request.clone()));
            match landing {
                Landing::Files => drawn.navigator.open_files(key),
                Landing::Default => drawn.navigator.go(Route::PullRequest(key)),
            }
        }
        Some(RowPick::Repository) => drawn.navigator.go(Route::RepositoryPullRequests {
            connection_id: connection_id.clone(),
            repository_key: pull_request.repository_key.clone(),
        }),
        Some(RowPick::ToggleFavorite) => {
            action = Some(ListAction::ToggleStar {
                connection_id: connection_id.clone(),
                repository_key: pull_request.repository_key.clone(),
            });
        }
        Some(RowPick::CountLines) => action = Some(ListAction::CountLines(stat_row.clone())),
        Some(RowPick::AddMe) => {
            action = Some(ListAction::AddMe(key_of(connection_id, pull_request)));
        }
        None => {}
    }
    dwell::note_rest(ui, id, rect, drawn.painted, &stat_row);
    drawn.painted.ahead.push(AheadRow { row: stat_row, interest });
    drawn.painted.statuses.push(status_row);
    action
}

fn row_height(row: &Row<'_>) -> f32 {
    match row {
        Row::Empty { .. } | Row::Waiting(_) | Row::NoFavorites | Row::HiddenFavorites(_) => {
            EMPTY_HEIGHT
        }
        Row::Message(_) => MESSAGE_HEIGHT,
        Row::Item(..) | Row::Skeleton(_) => ROW_HEIGHT,
        Row::Leaving(.., gone) => leaving::height(*gone),
        Row::Section(section) => section.height(),
    }
}

/// The row of `pull_request`, as far as counting its lines goes; `is_favorite`: its repository is starred.
pub(super) fn stat_row(
    result: &ConnectionResult,
    pull_request: &PullRequestSummary,
    is_favorite: bool,
) -> StatRow {
    let storage_key = key_of(&result.connection_id, pull_request).storage_key();
    StatRow::new(
        &result.connection_id,
        result.kind,
        result.has_token,
        is_favorite,
        storage_key,
        pull_request,
    )
}

#[cfg(test)]
pub(super) mod tests {
    use std::collections::BTreeSet;

    use cv_runtime::route::Navigator;
    use cv_runtime::route::Route;
    use cv_services::diff_stats::{DiffStats, StatRow};
    use cv_services::self_review::{Identities, RowRequests};
    use cv_services::writes::flights::Flights;
    use cv_testkit::{Clicks, samples};
    use egui::accesskit::Role;
    use egui::{Rect, pos2, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use review_core::{DiffStat, ProviderKind, PullRequestSummary, UserIdentity};
    use review_store::{Favorites, RepositorySections, SectionCard, SectionKey};

    use super::{
        Columns, EmptyOffer, Hosts, ListAction, ListInput, Painted, ROW_HEIGHT, RowFilter, show,
    };
    use crate::pull_request_list::pauses::Pauses;
    use crate::pull_request_list::queries::ConnectionQueries;
    use crate::pull_request_list::statuses::RowStatuses;

    /// Row `number` of `group/app`, by "Ada" from `feature`: the meta line and branch tests read
    /// both.
    pub(in crate::pull_request_list) fn row(number: u64) -> PullRequestSummary {
        PullRequestSummary {
            author: UserIdentity { display_name: "Ada".into(), ..UserIdentity::default() },
            source_branch: "feature".into(),
            head_sha: Some(format!("head{number}")),
            ..samples::summary("group/app", number)
        }
    }

    /// One GitLab card with two merge requests, as the list paints it.
    struct Scene {
        queries: ConnectionQueries,
        stats: DiffStats,
        favorites: Favorites,
        lifts_favorites: bool,
        /// The overview's repository sections; `None`: rows ungrouped.
        sections: Option<RepositorySections>,
        collapsed: BTreeSet<String>,
        navigator: Navigator,
        painted: Painted,
        asked_to_count: bool,
        /// The last section a header asked to close or open.
        toggled: Option<SectionKey>,
        /// Retries the rows asked for.
        retries: usize,
        requests: RowRequests,
        statuses: RowStatuses,
        flights: Flights,
        pauses: Pauses,
    }

    fn scene(has_token: bool) -> Scene {
        let mut queries =
            ConnectionQueries::loaded_for_test("gl", ProviderKind::GitLab, vec![row(1), row(2)]);
        queries.results[0].has_token = has_token;
        Scene {
            queries,
            stats: DiffStats::default(),
            favorites: Favorites::default(),
            lifts_favorites: false,
            sections: None,
            collapsed: BTreeSet::new(),
            navigator: Navigator::default(),
            painted: Painted::default(),
            asked_to_count: false,
            toggled: None,
            retries: 0,
            requests: RowRequests::default(),
            statuses: RowStatuses::default(),
            flights: Flights::default(),
            pauses: Pauses::default(),
        }
    }

    fn harness(width: f32, scene: Scene) -> Harness<'static, Scene> {
        Harness::builder()
            .with_size([width, 600.0])
            .with_context_setup(cv_base::theme::fonts::install)
            .build_ui_state(
                |ui, scene: &mut Scene| {
                    let hosts = Hosts {
                        collapsed: &scene.collapsed,
                        pauses: &scene.pauses,
                        favorites: &scene.favorites,
                        lifts_favorites: scene.lifts_favorites,
                        sections: scene.sections.as_ref(),
                        stats: &scene.stats,
                        requests: &scene.requests,
                        statuses: &scene.statuses,
                        flights: &scene.flights,
                        identities: &Identities::default(),
                    };
                    scene.painted = Painted::default();
                    let input = ListInput {
                        queries: &scene.queries,
                        lane: None,
                        hosts: &hosts,
                        filter: &RowFilter::new(None),
                        offer: EmptyOffer::Nothing,
                    };
                    let action = show(ui, &input, &mut scene.navigator, &mut scene.painted);
                    scene.asked_to_count |= matches!(action, Some(ListAction::CountLines(_)));
                    scene.retries += usize::from(matches!(action, Some(ListAction::Retry)));
                    if let Some(ListAction::ToggleSection(key)) = action {
                        scene.toggled = Some(key);
                    }
                },
                scene,
            )
    }

    /// The rows the last frame painted, which the page hands on to be counted.
    fn painted_rows(harness: &Harness<'_, Scene>) -> Vec<StatRow> {
        harness.state().painted.ahead.iter().map(|ahead| ahead.row.clone()).collect()
    }

    fn rect_of(harness: &Harness<'_, Scene>, label: &str) -> Rect {
        harness.get_by_label(label).rect()
    }

    /// Rows, their Files buttons and size columns, as rectangles, in a fixed order.
    fn layout(harness: &Harness<'_, Scene>, sizes: [&str; 2]) -> Vec<Rect> {
        let mut rects = vec![
            harness.get_by_role_and_label(Role::Link, "1 Change 1").rect(),
            harness.get_by_role_and_label(Role::Link, "2 Change 2").rect(),
            rect_of(harness, "Open Files (changes) of !1"),
            rect_of(harness, "Open Files (changes) of !2"),
        ];
        rects.extend(sizes.map(|label| rect_of(harness, label)));
        rects
    }

    #[test]
    fn the_size_column_keeps_its_place_while_the_lines_are_counted() {
        let mut harness = harness(1300.0, scene(true));
        harness.run();
        let counting = layout(&harness, ["Counting the lines of !1", "Counting the lines of !2"]);
        let rows = painted_rows(&harness);
        assert_eq!(rows.len(), 2, "both rows are on screen, so both get counted");

        harness.state_mut().stats.counted_for_test(&rows[0], DiffStat::new(1240, 312, 18));
        harness.state_mut().stats.counted_for_test(&rows[1], DiffStat::new(3, 1, 1));
        harness.run();
        let first = "1,552 lines changed in 18 files: 1,240 added, 312 deleted";
        let second = "4 lines changed in 1 file: 3 added, 1 deleted";
        let counted = layout(&harness, [first, second]);
        assert_eq!(counting, counted, "rows, Files buttons and size columns stay where they were");
        assert_eq!(
            counted[4].x_range(),
            counted[5].x_range(),
            "the size column lines up from row to row"
        );
        assert_eq!(counted[0].height(), ROW_HEIGHT);
    }

    #[test]
    fn the_size_column_comes_and_goes_with_the_repository_column() {
        let wide = Columns::for_row(Rect::from_min_size(pos2(0.0, 0.0), vec2(1240.0, ROW_HEIGHT)));
        let (repository, size, status) =
            (wide.repository.unwrap(), wide.size.unwrap(), wide.status.unwrap());
        assert!(
            repository.1 < size.0 && size.1 < status.0,
            "repository, size, status from left to right"
        );
        assert_eq!(size.1 - size.0, super::SIZE_WIDTH);
        let narrow = Columns::for_row(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, ROW_HEIGHT)));
        assert!(narrow.size.is_none() && narrow.repository.is_none() && narrow.status.is_some());

        let mut harness = harness(960.0, scene(true));
        harness.run();
        let rows = painted_rows(&harness);
        harness.state_mut().stats.counted_for_test(&rows[0], DiffStat::new(1240, 312, 18));
        harness.run();
        assert!(
            harness.query_by_label_contains("lines changed").is_none(),
            "no size column on a narrow card"
        );
        assert!(harness.query_by_label_contains("Counting the lines").is_none());
    }

    #[test]
    fn rows_without_counts_offer_to_count_them() {
        let mut harness = harness(1300.0, scene(false));
        harness.run();
        assert!(
            harness.query_by_label_contains("Counting").is_none(),
            "anonymous GitLab rows wait to be asked"
        );
        assert!(harness.query_by_label("Lines of !1 not counted").is_some());
        harness.click_at(harness.get_by_label("Count lines of !1").rect());
        harness.run();
        assert!(harness.state().asked_to_count, "the link asks the page to count");
    }

    /// Rows 1 and 3 of `group/app` around row 2 of `group/api`, grouped by repository.
    fn sectioned() -> Scene {
        let mut rows = vec![row(1), row(2), row(3)];
        rows[1].repository_key = "group/api".into();
        let mut scene = scene(true);
        scene.queries = ConnectionQueries::loaded_for_test("gl", ProviderKind::GitLab, rows);
        scene.sections = Some(RepositorySections::default());
        scene
    }

    #[test]
    fn rows_group_under_their_repository_in_the_order_of_their_newest() {
        let mut harness = harness(1300.0, sectioned());
        harness.run();
        let app =
            harness.get_by_role_and_label(Role::Button, "group/app, 2 pull requests, expanded");
        let api =
            harness.get_by_role_and_label(Role::Button, "group/api, 1 pull request, expanded");
        let link = |number: u64| {
            harness.get_by_role_and_label(Role::Link, &format!("{number} Change {number}")).rect()
        };
        let (app, api) = (app.rect(), api.rect());
        assert!(app.bottom() <= link(1).top(), "the first repository's header leads");
        assert!(link(1).bottom() <= link(3).top(), "its rows keep their order");
        assert!(link(3).bottom() <= api.top(), "the next repository follows: {api:?}");
        assert!(api.bottom() <= link(2).top());
        assert_eq!(api.height(), super::cards::sections::SECTION_HEIGHT);
        assert_eq!(link(2).height(), ROW_HEIGHT, "rows keep their height under a header");

        harness.click_at(
            harness.get_by_role_and_label(Role::Link, "Pull requests of group/api").rect(),
        );
        harness.run();
        let (asked, _) = harness.state_mut().navigator.take();
        let expected = Route::RepositoryPullRequests {
            connection_id: "gl".into(),
            repository_key: "group/api".into(),
        };
        assert_eq!(asked.map(|request| request.route), Some(expected), "the arrow opens its list");
        assert_eq!(harness.state().toggled, None, "and closes nothing");
    }

    #[test]
    fn a_closed_section_keeps_its_header_and_count_and_drops_its_rows() {
        let mut harness = harness(1300.0, sectioned());
        harness.run();
        harness.click_at(harness.get_by_label("group/app, 2 pull requests, expanded").rect());
        harness.run();
        let key = SectionKey {
            card: SectionCard::Connection,
            connection_id: "gl".into(),
            repository_key: "group/app".into(),
        };
        assert_eq!(harness.state().toggled, Some(key.clone()), "the header asks to close");

        // The page saves the choice; the next frame lists the section closed.
        if let Some(sections) = harness.state_mut().sections.as_mut() {
            sections.toggle(key);
        }
        harness.run();
        assert!(harness.query_by_label("group/app, 2 pull requests, collapsed").is_some());
        assert!(harness.query_by_label("1 Change 1").is_none(), "its rows are gone");
        assert!(harness.query_by_label("3 Change 3").is_none());
        assert!(harness.query_by_label("2 Change 2").is_some(), "the other section stays open");
        let painted: Vec<u64> = painted_rows(&harness).iter().map(|row| row.number).collect();
        assert_eq!(painted, [2], "closed rows are neither counted nor asked for");
    }

    #[test]
    fn the_left_edge_says_how_current_each_row_is() {
        let edges = |harness: &Harness<'_, Scene>| {
            ["Updating the checks", "was not updated", "changed since"]
                .map(|words| harness.query_by_label_contains(words).is_some())
        };
        let mut harness = harness(1300.0, scene(true));
        harness.run();
        assert_eq!(edges(&harness), [false; 3], "fresh rows have no edge");

        let rows = harness.state().painted.statuses.clone();
        let changed = PullRequestSummary { head_sha: Some("pushed".into()), ..row(2) };
        {
            let scene = harness.state_mut();
            scene.statuses.running_for_test(&rows[0]);
            let changes = &mut scene.queries.results[0].changes;
            changes.answered(&[row(1), row(2)]);
            changes.answered(&[row(1), changed]);
        }
        // The light travels on: a few frames, not until it settles.
        harness.run_steps(3);
        assert!(harness.query_by_label("Updating the checks and approvals of !1").is_some());
        assert!(harness.query_by_label_contains("!2 changed since you last looked").is_some());

        harness.state_mut().statuses.failed_for_test(&rows[1], "403 Forbidden");
        harness.run_steps(3);
        let failed = "!2 was not updated: 403 Forbidden
Click to try again";
        assert!(
            harness.query_by_label_contains("!2 changed since").is_none(),
            "a failure outweighs a change"
        );
        harness.click_at(harness.get_by_label(failed).rect());
        harness.run_steps(3);
        assert_eq!(harness.state().retries, 1, "the failed edge tries again");
        assert_eq!(
            harness.get_by_role_and_label(Role::Link, "2 Change 2").rect().height(),
            ROW_HEIGHT,
            "the edge takes no room of its own"
        );
    }

    /// A request a rate-limit pause holds back waits for up to an hour: its row says so with the
    /// waiting edge's still dots, and the list asks for no frame meanwhile.
    #[test]
    fn a_row_held_by_a_rate_limit_waits_still() {
        let mut harness = harness(1300.0, scene(true));
        harness.run();
        let rows = harness.state().painted.statuses.clone();
        {
            let scene = harness.state_mut();
            scene.statuses.running_for_test(&rows[0]);
            scene.pauses = Pauses::of_test("gl", review_core::http::Api::GraphQl);
        }
        // Settles: `run` fails when a frame keeps asking for the next.
        harness.run();
        assert!(
            harness
                .query_by_label_contains("!1 waits for GitLab to accept requests again")
                .is_some()
        );
        assert!(harness.query_by_label_contains("Updating the checks").is_none());

        harness.state_mut().pauses = Pauses::default();
        harness.run_steps(3);
        assert!(
            harness.query_by_label("Updating the checks and approvals of !1").is_some(),
            "the light again once the request goes out"
        );
    }
}
