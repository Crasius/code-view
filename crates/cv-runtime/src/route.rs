use super::history::Step;

/// Identifies one pull request across connections.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PullRequestKey {
    pub connection_id: String,
    pub repository_key: String,
    pub number: u64,
}

impl PullRequestKey {
    /// Stable key for local state (viewed files) that survives restarts.
    pub fn storage_key(&self) -> String {
        format!("{}|{}|{}", self.connection_id, self.repository_key, self.number)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    MyPullRequests,
    Repositories,
    /// The repositories of one connection.
    Connection {
        connection_id: String,
    },
    RepositoryPullRequests {
        connection_id: String,
        repository_key: String,
    },
    PullRequest(PullRequestKey),
    Settings,
}

impl Route {
    /// The connection whose page this is; `None` for the pages of all connections.
    pub fn connection_id(&self) -> Option<&str> {
        match self {
            Route::Connection { connection_id }
            | Route::RepositoryPullRequests { connection_id, .. } => Some(connection_id),
            Route::PullRequest(key) => Some(&key.connection_id),
            Route::MyPullRequests | Route::Repositories | Route::Settings => None,
        }
    }

    pub fn section(&self) -> Section {
        match self {
            Route::MyPullRequests | Route::PullRequest(_) => Section::PullRequests,
            Route::Repositories
            | Route::Connection { .. }
            | Route::RepositoryPullRequests { .. } => Section::Repositories,
            Route::Settings => Section::Settings,
        }
    }
}

/// Top-level areas in the navigation rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    PullRequests,
    Repositories,
    Settings,
}

/// The tabs of a pull request page; the history keeps the one shown with each visit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestTab {
    Overview,
    Files,
    Commits,
    Pipelines,
}

impl PullRequestTab {
    pub fn name(self) -> &'static str {
        match self {
            PullRequestTab::Overview => "Overview",
            PullRequestTab::Files => "Files",
            PullRequestTab::Commits => "Commits",
            PullRequestTab::Pipelines => "Pipelines",
        }
    }
}

/// Where a pull request opens: on the tab it showed last (Overview the first time), or on Files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Landing {
    #[default]
    Default,
    Files,
}

/// How a route was asked for: from the page shown, whose trail it then continues, or from
/// outside it (a link from another app, the search, the rail), with a trail of its own structure
/// ([`super::trail`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Page,
    Outside,
}

/// A route asked for, where it lands and how it was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub route: Route,
    pub landing: Landing,
    pub origin: Origin,
}

/// Views ask for navigation through this; the app applies it before the page runs (the rail, the
/// title bar) or after the frame's UI (the page itself). Back and forward come from Alt+Left/Right,
/// the mouse's side buttons and the title bar's arrows ([`super::history`]); the breadcrumb bar
/// links every level above.
#[derive(Default)]
pub struct Navigator {
    requested: Option<Request>,
    step: Option<Step>,
}

impl Navigator {
    /// Goes to `route` from the page shown: the breadcrumb trail shows the way.
    pub fn go(&mut self, route: Route) {
        self.ask(route, Landing::Default, Origin::Page);
    }

    /// Goes to `route` from outside the page shown (a link from another app, the search, the
    /// rail): the trail is the route's structure.
    pub fn jump(&mut self, route: Route) {
        self.ask(route, Landing::Default, Origin::Outside);
    }

    /// Opens a pull request straight on its Files tab; history and breadcrumb as for [`Self::go`].
    pub fn open_files(&mut self, key: PullRequestKey) {
        self.ask(Route::PullRequest(key), Landing::Files, Origin::Page);
    }

    pub fn back(&mut self) {
        self.step = Some(Step::Back);
    }

    pub fn forward(&mut self) {
        self.step = Some(Step::Forward);
    }

    /// What was asked for since the last call: a step through the history wins over a route.
    pub fn take(&mut self) -> (Option<Request>, Option<Step>) {
        (self.requested.take(), self.step.take())
    }

    fn ask(&mut self, route: Route, landing: Landing, origin: Origin) {
        self.requested = Some(Request { route, landing, origin });
    }
}
