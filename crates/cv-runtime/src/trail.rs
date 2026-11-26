//! The way to the page shown, as the breadcrumb trail names it below the app's root crumb.
//!
//! Where the user came on a way, the trail shows that way: a pull request opened from the list of
//! "My pull requests" sits below that list, one opened from another pull request's description
//! below that pull request's trail. Where there is no way to show (a link from outside, the
//! search, the rail's sections, the first page after a start), it is the route's structure:
//! connection, repository, pull request.
//!
//! The rule, for a page entered from the page a trail ends on:
//! - a section's root (the lists of the rail) starts over from its structure;
//! - a page that is a structural level of the new route (the repositories page, a connection, a
//!   repository's list) leads down through the levels between, as the structure names them;
//! - any other page carries its trail and the new route follows it.
//!
//! A route already in the trail cuts it back to that level, so a crumb click goes up the trail
//! and a way in a circle leaves no loop. The history keeps each place's trail with it, so back
//! and forward restore it ([`super::history`]).

use super::route::Route;

/// Levels kept below the root; a longer way loses its oldest levels.
const LIMIT: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trail {
    /// From the level below the root down to the one above the page.
    above: Vec<Route>,
    route: Route,
}

impl Trail {
    /// The route's levels by structure: a pull request below its repository below its connection.
    pub fn structural(route: &Route) -> Self {
        let above = match route {
            Route::MyPullRequests
            | Route::Repositories
            | Route::Connection { .. }
            | Route::Settings => Vec::new(),
            Route::RepositoryPullRequests { connection_id, .. } => vec![connection(connection_id)],
            Route::PullRequest(key) => vec![
                connection(&key.connection_id),
                Route::RepositoryPullRequests {
                    connection_id: key.connection_id.clone(),
                    repository_key: key.repository_key.clone(),
                },
            ],
        };
        Self { above, route: route.clone() }
    }

    /// The trail of `route` entered from the page this trail ends on (see the module's rule).
    #[must_use]
    pub fn entered(&self, route: &Route) -> Self {
        if is_section_root(route) {
            return Self::structural(route);
        }
        let structure = Self::structural(route);
        let mut levels = self.above.clone();
        match structural_depth(&self.route, &structure.above) {
            // Down the structure from the page left: the levels between follow it.
            Some(depth) => {
                for level in structure.above.iter().skip(depth) {
                    push_level(&mut levels, level);
                }
            }
            None => push_level(&mut levels, &self.route),
        }
        // The route itself, unless the way led back to a level above.
        if let Some(index) = levels.iter().position(|level| level == route) {
            levels.truncate(index);
        }
        if levels.len() >= LIMIT {
            levels.drain(..=levels.len() - LIMIT);
        }
        Self { above: levels, route: route.clone() }
    }

    /// The page the trail ends on.
    pub fn route(&self) -> &Route {
        &self.route
    }

    /// From the level below the root down to the page.
    pub fn levels(&self) -> impl Iterator<Item = &Route> {
        self.above.iter().chain(std::iter::once(&self.route))
    }

    /// Drops the levels of `connection_id`, whose pages are gone: a trail with any of them
    /// becomes its route's structure.
    pub fn forget_connection(&mut self, connection_id: &str) {
        if self.levels().any(|level| level.connection_id() == Some(connection_id)) {
            *self = Self::structural(&self.route);
        }
    }
}

fn connection(connection_id: &str) -> Route {
    Route::Connection { connection_id: connection_id.to_owned() }
}

/// The lists the rail leads to: they head a trail of their own.
fn is_section_root(route: &Route) -> bool {
    matches!(route, Route::MyPullRequests | Route::Repositories | Route::Settings)
}

/// Where `left` stands in `structure`, the levels above the route entered from it. The
/// repositories page (all connections, or one picked in it) heads every structure it leads into,
/// so a switch to another connection there replaces the one picked.
fn structural_depth(left: &Route, structure: &[Route]) -> Option<usize> {
    match left {
        Route::Repositories | Route::Connection { .. } => Some(0),
        _ => structure.iter().position(|level| level == left),
    }
}

/// Adds `level` below the trail, or cuts the trail back to it where it is already a level.
fn push_level(levels: &mut Vec<Route>, level: &Route) {
    match levels.iter().position(|known| known == level) {
        Some(index) => levels.truncate(index + 1),
        None => levels.push(level.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::{LIMIT, Trail};
    use crate::route::{PullRequestKey, Route};

    fn pull_request(number: u64) -> Route {
        Route::PullRequest(PullRequestKey {
            connection_id: "gh".into(),
            repository_key: "cli/cli".into(),
            number,
        })
    }

    fn levels(trail: &Trail) -> Vec<Route> {
        trail.levels().cloned().collect()
    }

    fn connection() -> Route {
        Route::Connection { connection_id: "gh".into() }
    }

    fn repository() -> Route {
        Route::RepositoryPullRequests {
            connection_id: "gh".into(),
            repository_key: "cli/cli".into(),
        }
    }

    #[test]
    fn structure_names_connection_repository_and_page() {
        let trail = Trail::structural(&pull_request(7));
        assert_eq!(levels(&trail), [connection(), repository(), pull_request(7)]);
        assert_eq!(levels(&Trail::structural(&Route::MyPullRequests)), [Route::MyPullRequests]);
    }

    #[test]
    fn a_page_off_the_structure_carries_its_trail() {
        let list = Trail::structural(&Route::MyPullRequests);
        let opened = list.entered(&pull_request(12));
        assert_eq!(levels(&opened), [Route::MyPullRequests, pull_request(12)]);
        let linked = opened.entered(&pull_request(13));
        assert_eq!(levels(&linked), [Route::MyPullRequests, pull_request(12), pull_request(13)]);
    }

    #[test]
    fn going_down_the_structure_keeps_it() {
        let repositories = Trail::structural(&Route::Repositories);
        let repository_list = repositories.entered(&repository());
        assert_eq!(levels(&repository_list), [connection(), repository()]);
        let picked = repositories.entered(&connection());
        assert_eq!(levels(&picked), [connection()]);
        let other = picked.entered(&Route::Connection { connection_id: "gl".into() });
        assert_eq!(levels(&other), [Route::Connection { connection_id: "gl".into() }]);
        let opened = picked.entered(&repository()).entered(&pull_request(7));
        assert_eq!(opened, Trail::structural(&pull_request(7)));
    }

    #[test]
    fn a_level_already_in_the_trail_cuts_it_back() {
        let deep = Trail::structural(&pull_request(7)).entered(&pull_request(8));
        assert_eq!(deep.entered(&pull_request(7)), Trail::structural(&pull_request(7)));
        assert_eq!(levels(&deep.entered(&repository())), [connection(), repository()]);
        assert_eq!(levels(&deep.entered(&connection())), [connection()]);
        // Below a repository reached on a way, its pull requests follow that way.
        let mine = Trail::structural(&Route::MyPullRequests).entered(&repository());
        assert_eq!(levels(&mine), [Route::MyPullRequests, repository()]);
        assert_eq!(
            levels(&mine.entered(&pull_request(9))),
            [Route::MyPullRequests, repository(), pull_request(9)]
        );
    }

    #[test]
    fn section_roots_start_over() {
        let deep = Trail::structural(&Route::MyPullRequests).entered(&pull_request(12));
        assert_eq!(levels(&deep.entered(&Route::MyPullRequests)), [Route::MyPullRequests]);
        assert_eq!(levels(&deep.entered(&Route::Settings)), [Route::Settings]);
    }

    #[test]
    fn a_long_way_keeps_its_newest_levels() {
        let mut trail = Trail::structural(&Route::MyPullRequests);
        for number in 0..LIMIT as u64 + 5 {
            trail = trail.entered(&pull_request(number));
        }
        assert_eq!(levels(&trail).len(), LIMIT);
        assert_eq!(trail.route(), &pull_request(LIMIT as u64 + 4));
    }

    #[test]
    fn a_removed_connection_leaves_the_structure() {
        let trail = Trail::structural(&Route::MyPullRequests)
            .entered(&Route::PullRequest(PullRequestKey {
                connection_id: "gl".into(),
                repository_key: "a/b".into(),
                number: 1,
            }))
            .entered(&pull_request(7));
        let mut kept = trail.clone();
        kept.forget_connection("ado");
        assert_eq!(kept, trail);
        let mut forgotten = trail;
        forgotten.forget_connection("gl");
        assert_eq!(forgotten, Trail::structural(&pull_request(7)));
    }
}
