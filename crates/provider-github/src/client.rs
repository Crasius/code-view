//! Request building: API roots, GitHub headers, auth and `Link`-header pagination.

use std::collections::HashMap;
use std::sync::Mutex;

use reqwest::header::ACCEPT;
use reqwest::{Client, Method, RequestBuilder, Url};
use review_core::{Connection, ProviderError, Result, RowStatus, http};
use serde::de::DeserializeOwned;

use crate::owners::{OwnerKind, Reach};
use crate::sso::Blocked;

const API_VERSION: &str = "2022-11-28";
const MEDIA_TYPE: &str = "application/vnd.github+json";
const PUBLIC_WEB_ROOTS: [&str; 2] = ["https://github.com", "https://www.github.com"];
const PUBLIC_API_ROOT: &str = "https://api.github.com";

/// Page size for every REST and GraphQL listing; GitHub's maximum.
pub(crate) const PER_PAGE: usize = 100;

/// REST and GraphQL endpoints derived from the web root the user configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiRoots {
    pub rest: String,
    pub graphql: String,
}

impl ApiRoots {
    /// `https://github.com` → `api.github.com`; any other host is GitHub Enterprise Server.
    pub fn for_base_url(base_url: &str) -> Self {
        let base = base_url.trim_end_matches('/');
        let is_public = PUBLIC_WEB_ROOTS.iter().any(|root| base.eq_ignore_ascii_case(root));
        if is_public {
            return Self {
                rest: PUBLIC_API_ROOT.to_owned(),
                graphql: format!("{PUBLIC_API_ROOT}/graphql"),
            };
        }
        Self { rest: format!("{base}/api/v3"), graphql: format!("{base}/api/graphql") }
    }
}

/// How far a listing pages: stop at `limit` items or after `max_pages` requests.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Paging {
    pub limit: usize,
    pub max_pages: usize,
}

impl Paging {
    pub const fn pages(max_pages: usize) -> Self {
        Self { limit: usize::MAX, max_pages }
    }
}

pub(crate) struct GitHubClient {
    http: Client,
    connection: Connection,
    roots: ApiRoots,
    owner_kinds: Mutex<HashMap<String, OwnerKind>>,
    /// The signed-in user and their organizations; an async lock so concurrent listings share one lookup.
    reach: tokio::sync::Mutex<Option<Reach>>,
    organization_logins: Mutex<HashMap<u64, String>>,
    /// Organizations the latest pull request listing could not include (SAML single sign-on).
    partial: Mutex<Vec<Blocked>>,
    /// The checks of the rows the searches listed, by repository and number: what a list row's
    /// checks are answered from without a request. Each search answer replaces its rows' entries;
    /// at most [`MAX_LISTED_STATUSES`] are kept.
    listed_statuses: Mutex<HashMap<(String, u64), RowStatus>>,
}

/// Search results are capped at 1000 per search; a few searches' worth of rows stay known.
const MAX_LISTED_STATUSES: usize = 5000;

impl GitHubClient {
    pub fn new(connection: Connection, http: Client) -> Self {
        let roots = ApiRoots::for_base_url(connection.base_url());
        Self {
            http,
            connection,
            roots,
            owner_kinds: Mutex::new(HashMap::new()),
            reach: tokio::sync::Mutex::new(None),
            organization_logins: Mutex::new(HashMap::new()),
            partial: Mutex::new(Vec::new()),
            listed_statuses: Mutex::new(HashMap::new()),
        }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn graphql_url(&self) -> &str {
        &self.roots.graphql
    }

    pub fn rest_url(&self) -> &str {
        &self.roots.rest
    }

    pub fn has_token(&self) -> bool {
        self.connection.token().is_some()
    }

    pub fn require_token(&self, action: &str) -> Result<()> {
        if self.has_token() {
            return Ok(());
        }
        Err(ProviderError::requires_token(action))
    }

    /// A request to an absolute URL with GitHub headers and, when configured, the bearer token.
    pub fn request(&self, method: Method, url: &str) -> RequestBuilder {
        let builder = self
            .http
            .request(method, url)
            .header(ACCEPT, MEDIA_TYPE)
            .header("X-GitHub-Api-Version", API_VERSION);
        match self.connection.token() {
            Some(token) => builder.bearer_auth(token),
            None => builder,
        }
    }

    /// A request to `path` (starting with `/`) below the REST root.
    pub fn rest(&self, method: Method, path: &str) -> RequestBuilder {
        self.request(method, &format!("{}{path}", self.roots.rest))
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        http::json(self.rest(Method::GET, path)).await
    }

    /// Follows `rel="next"` links, turning each decoded page into items with `extract`.
    pub async fn collect_pages<P, T>(
        &self,
        path: &str,
        query: &[(&str, String)],
        paging: Paging,
        mut extract: impl FnMut(P) -> Vec<T>,
    ) -> Result<Vec<T>>
    where
        P: DeserializeOwned,
    {
        let mut items = Vec::new();
        let mut request = self.rest(Method::GET, path).query(query);
        for _ in 0..paging.max_pages {
            // Conditional GETs: GitHub answers an unchanged page with a 304 that costs no quota.
            let fetched = http::fetch(request).await?;
            let page: P = fetched.json()?;
            items.extend(extract(page));
            if items.len() >= paging.limit {
                items.truncate(paging.limit);
                break;
            }
            let Some(next) = fetched.next_link else {
                break;
            };
            request = self.request(Method::GET, self.same_origin(&next)?);
        }
        Ok(items)
    }

    /// Pagination links must stay on the API host so the token never leaves it. Hosts compare
    /// parsed (case-insensitive), the path below the REST root by whole segments.
    fn same_origin<'a>(&self, url: &'a str) -> Result<&'a str> {
        let below_root = |link: &Url, root: &Url| {
            let root_path = root.path().trim_end_matches('/');
            link.origin() == root.origin()
                && link.path().strip_prefix(root_path).is_some_and(|rest| rest.starts_with('/'))
        };
        if let (Ok(link), Ok(root)) = (Url::parse(url), Url::parse(&self.roots.rest))
            && below_root(&link, &root)
        {
            return Ok(url);
        }
        Err(ProviderError::Decode(format!("pagination link {url} leaves {}", self.roots.rest)))
    }

    pub fn cached_owner_kind(&self, owner: &str) -> Option<OwnerKind> {
        let cache = self.owner_kinds.lock().ok()?;
        cache.get(&owner.to_ascii_lowercase()).copied()
    }

    pub fn remember_owner_kind(&self, owner: &str, kind: OwnerKind) {
        if let Ok(mut cache) = self.owner_kinds.lock() {
            cache.insert(owner.to_ascii_lowercase(), kind);
        }
    }

    pub fn reach_cache(&self) -> &tokio::sync::Mutex<Option<Reach>> {
        &self.reach
    }

    pub fn cached_organization_login(&self, id: u64) -> Option<String> {
        self.organization_logins.lock().ok()?.get(&id).cloned()
    }

    pub fn remember_organization_login(&self, id: u64, login: &str) {
        if let Ok(mut cache) = self.organization_logins.lock() {
            cache.insert(id, login.to_owned());
        }
    }

    pub fn partial(&self) -> Vec<Blocked> {
        self.partial.lock().map(|blocked| blocked.clone()).unwrap_or_default()
    }

    pub fn set_partial(&self, blocked: Vec<Blocked>) {
        if let Ok(mut partial) = self.partial.lock() {
            *partial = blocked;
        }
    }

    /// Keeps the checks a search listed with its rows; a full cache starts over.
    pub fn remember_statuses(&self, statuses: Vec<((String, u64), RowStatus)>) {
        if let Ok(mut cache) = self.listed_statuses.lock() {
            if cache.len() + statuses.len() > MAX_LISTED_STATUSES {
                cache.clear();
            }
            cache.extend(statuses);
        }
    }

    /// Drops what a search listed for `rows`: a later listing without checks (REST) shows them,
    /// so their checks are asked for again instead of answered from an older search.
    pub fn forget_statuses<'a>(&self, rows: impl IntoIterator<Item = (&'a str, u64)>) {
        if let Ok(mut cache) = self.listed_statuses.lock() {
            for (repo, number) in rows {
                cache.remove(&(repo.to_owned(), number));
            }
        }
    }

    /// The checks the latest listing of the pull request `number` of `repo` brought.
    pub fn listed_status(&self, repo: &str, number: u64) -> Option<RowStatus> {
        self.listed_statuses.lock().ok()?.get(&(repo.to_owned(), number)).cloned()
    }
}

/// Number of pages needed for `limit` items at [`PER_PAGE`].
pub(crate) fn pages_for(limit: usize) -> usize {
    limit.div_ceil(PER_PAGE).max(1)
}

/// `per_page` query value for a listing that wants `limit` items.
pub(crate) fn per_page(limit: usize) -> String {
    limit.clamp(1, PER_PAGE).to_string()
}

#[cfg(test)]
mod tests {
    use review_core::{Connection, ProviderKind};

    use super::{ApiRoots, GitHubClient, pages_for, per_page};

    fn client(base_url: &str) -> GitHubClient {
        let connection = Connection {
            id: "gh".to_owned(),
            name: "GitHub".to_owned(),
            kind: ProviderKind::GitHub,
            base_url: base_url.to_owned(),
            organizations: Vec::new(),
            token: None,
        };
        GitHubClient::new(connection, reqwest::Client::new())
    }

    /// GitHub writes its page links with the host in lowercase, whatever case the user typed.
    #[test]
    fn page_links_follow_on_the_api_host_in_any_case() {
        let enterprise = client("https://GHE.Example.com");
        let next = "https://ghe.example.com/api/v3/repos/o/r/pulls?page=2";
        assert_eq!(enterprise.same_origin(next).ok(), Some(next));
        for elsewhere in [
            "https://ghe.example.com.evil.test/api/v3/repos/o/r/pulls?page=2",
            "https://ghe.example.com/api/v3evil/x",
            "http://ghe.example.com/api/v3/x",
        ] {
            assert!(enterprise.same_origin(elsewhere).is_err(), "{elsewhere}");
        }
        let public = client("https://github.com");
        assert!(public.same_origin("https://api.github.com/repositories/1/pulls?page=2").is_ok());
        assert!(public.same_origin("https://api.github.com.evil.test/x").is_err());
    }

    #[test]
    fn public_github_uses_api_host() {
        let roots = ApiRoots::for_base_url("https://github.com/");
        assert_eq!(roots.rest, "https://api.github.com");
        assert_eq!(roots.graphql, "https://api.github.com/graphql");
        assert_eq!(ApiRoots::for_base_url("https://GitHub.com").rest, "https://api.github.com");
    }

    #[test]
    fn enterprise_uses_api_v3_and_api_graphql() {
        let roots = ApiRoots::for_base_url("https://git.example.com");
        assert_eq!(roots.rest, "https://git.example.com/api/v3");
        assert_eq!(roots.graphql, "https://git.example.com/api/graphql");
    }

    #[test]
    fn page_math() {
        assert_eq!(pages_for(0), 1);
        assert_eq!(pages_for(100), 1);
        assert_eq!(pages_for(101), 2);
        assert_eq!(per_page(0), "1");
        assert_eq!(per_page(30), "30");
        assert_eq!(per_page(500), "100");
    }
}
