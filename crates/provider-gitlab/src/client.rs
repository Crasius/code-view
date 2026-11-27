//! Request building for the GitLab REST API v4: API root, `PRIVATE-TOKEN` auth, project id
//! encoding and `Link`-header pagination.

use reqwest::{Method, RequestBuilder, Response};
use review_core::{Connection, ProviderError, Result, http};
use serde::de::DeserializeOwned;
use tokio::sync::OnceCell;

use crate::dto::UserDto;
use crate::graphql::{Envelope, SchemaGaps};
use crate::pipeline_names::KnownLabels;

const MAX_PER_PAGE: usize = 100;

pub(crate) type Query<'a> = [(&'a str, String)];

pub(crate) struct ApiClient {
    http: reqwest::Client,
    web_root: String,
    api_root: String,
    token: Option<String>,
    /// The token's user, read once per connection for listings filtered by user id.
    me: OnceCell<UserDto>,
    /// What the host's GraphQL schema lacks, learned from its answers.
    schema_gaps: SchemaGaps,
    /// Pipeline names read once, as a pipeline keeps its name.
    pipeline_labels: KnownLabels,
}

impl ApiClient {
    pub(crate) fn new(connection: &Connection, http: reqwest::Client) -> Self {
        let web_root = connection.base_url().to_owned();
        let api_root = format!("{web_root}/api/v4");
        let token = connection.token().map(str::to_owned);
        Self {
            http,
            web_root,
            api_root,
            token,
            me: OnceCell::new(),
            schema_gaps: SchemaGaps::default(),
            pipeline_labels: KnownLabels::default(),
        }
    }

    pub(crate) fn web_root(&self) -> &str {
        &self.web_root
    }

    pub(crate) fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// `GET /user` once per connection; listings need the id on every load.
    pub(crate) async fn me(&self) -> Result<&UserDto> {
        self.require_token("Reading the current user")?;
        self.me.get_or_try_init(|| self.get::<UserDto>("/user", &[])).await
    }

    /// Fails with [`ProviderError::requires_token`] on anonymous connections.
    pub(crate) fn require_token(&self, action: &str) -> Result<()> {
        match self.token {
            Some(_) => Ok(()),
            None => Err(ProviderError::requires_token(action)),
        }
    }

    /// `path` starts with `/` and is relative to `{base}/api/v4`.
    pub(crate) fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.request_url(method, &format!("{}{path}", self.api_root))
    }

    fn request_url(&self, method: Method, url: &str) -> RequestBuilder {
        let request = self.http.request(method, url);
        match &self.token {
            Some(token) => request.header("PRIVATE-TOKEN", token),
            None => request,
        }
    }

    /// Sends and maps errors; an anonymous 401 becomes a "needs a token" message, because
    /// gitlab.com answers several read endpoints (discussions, labels, job logs) only with a token.
    pub(crate) async fn send(&self, request: RequestBuilder) -> Result<Response> {
        self.explain_anonymous(http::send(request).await)
    }

    /// Like [`ApiClient::send`] with the body read: identical GETs in flight share one request and
    /// repeated ones revalidate with `If-None-Match`.
    async fn fetch(&self, request: RequestBuilder) -> Result<http::Fetched> {
        self.explain_anonymous(http::fetch(request).await)
    }

    fn explain_anonymous<T>(&self, result: Result<T>) -> Result<T> {
        match result {
            Err(ProviderError::Status { status: 401, path, .. }) if self.token.is_none() => {
                Err(ProviderError::Unsupported(format!(
                    "{path} needs a personal access token on this GitLab host."
                )))
            }
            other => other,
        }
    }

    pub(crate) async fn json<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T> {
        self.fetch(request).await?.json()
    }

    pub(crate) async fn unit(&self, request: RequestBuilder) -> Result<()> {
        self.send(request).await?;
        Ok(())
    }

    pub(crate) async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &Query<'_>,
    ) -> Result<T> {
        self.json(self.request(Method::GET, path).query(query)).await
    }

    /// What the host's GraphQL schema is known to lack.
    pub(crate) fn schema_gaps(&self) -> &SchemaGaps {
        &self.schema_gaps
    }

    pub(crate) fn pipeline_labels(&self) -> &KnownLabels {
        &self.pipeline_labels
    }

    /// `POST {base}/api/graphql` (<https://docs.gitlab.com/api/graphql/>) of a fixed `query`; see
    /// [`ApiClient::graphql_fitted`].
    pub(crate) async fn graphql<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<T> {
        self.graphql_fitted(|_| query.to_owned(), variables).await
    }

    /// `POST {base}/api/graphql` with the token as a Bearer header. `build` writes the query for
    /// what the host's schema is known to lack. GraphQL reports errors with status 200: when they
    /// name a field or argument the host lacks, the gap is kept and the query is built and sent
    /// again if that changes it; every other error becomes a provider error.
    pub(crate) async fn graphql_fitted<T: DeserializeOwned>(
        &self,
        build: impl Fn(&SchemaGaps) -> String,
        variables: serde_json::Value,
    ) -> Result<T> {
        let url = format!("{}/api/graphql", self.web_root);
        let mut query = build(&self.schema_gaps);
        loop {
            let request = self
                .http
                .post(&url)
                .json(&serde_json::json!({ "query": query, "variables": variables }));
            let request = match &self.token {
                Some(token) => request.bearer_auth(token),
                None => request,
            };
            let Envelope { data, errors } = self.json::<Envelope<T>>(request).await?;
            let Some(error) = errors.first() else {
                return data.ok_or_else(|| {
                    ProviderError::Local("GitLab GraphQL answered without data.".to_owned())
                });
            };
            // The gaps only grow and `build` leaves out a finite set of parts, so this ends.
            let fitted = self.schema_gaps.learn(&errors).then(|| build(&self.schema_gaps));
            match fitted {
                Some(fitted) if fitted != query => query = fitted,
                _ => {
                    return Err(ProviderError::Local(format!("GitLab GraphQL: {}", error.message)));
                }
            }
        }
    }

    /// Follows `rel="next"` links until `limit` items are collected or the pages run out.
    pub(crate) async fn get_pages<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &Query<'_>,
        limit: usize,
    ) -> Result<Vec<T>> {
        let per_page = limit.clamp(1, MAX_PER_PAGE).to_string();
        let first = self.request(Method::GET, path).query(query).query(&[("per_page", per_page)]);
        let mut items = Vec::new();
        let mut request = Some(first);
        while let Some(current) = request.take() {
            let fetched = self.fetch(current).await?;
            let page: Vec<T> = fetched.json()?;
            let is_empty = page.is_empty();
            items.extend(page);
            if items.len() >= limit || is_empty {
                break;
            }
            request = fetched.next_link.map(|url| self.request_url(Method::GET, &url));
        }
        items.truncate(limit);
        Ok(items)
    }
}

/// `/projects/:id` with the `path_with_namespace` repository key URL-encoded as `:id`.
pub(crate) fn project_path(repo: &str) -> String {
    format!("/projects/{}", urlencoding::encode(repo))
}

pub(crate) fn merge_request_path(repo: &str, iid: u64) -> String {
    format!("{}/merge_requests/{iid}", project_path(repo))
}

pub(crate) fn group_path(group: &str) -> String {
    format!("/groups/{}", urlencoding::encode(group))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_nested_project_paths() {
        assert_eq!(project_path("group/sub/project"), "/projects/group%2Fsub%2Fproject");
        assert_eq!(merge_request_path("a/b", 7), "/projects/a%2Fb/merge_requests/7");
        assert_eq!(group_path("acme/ci"), "/groups/acme%2Fci");
    }

    #[test]
    fn requests_go_to_the_api_root_and_an_empty_token_is_anonymous() {
        let connection = Connection {
            id: "c".into(),
            name: "n".into(),
            kind: review_core::ProviderKind::GitLab,
            base_url: "https://gitlab.example.com/".into(),
            organizations: Vec::new(),
            token: Some(String::new()),
        };
        let client = ApiClient::new(&connection, reqwest::Client::new());
        let request = client.request(Method::GET, "/user").build().expect("request");
        assert_eq!(request.url().as_str(), "https://gitlab.example.com/api/v4/user");
        assert!(
            request.headers().get("private-token").is_none(),
            "empty token counts as anonymous"
        );
        assert!(matches!(client.require_token("Merging"), Err(ProviderError::Unsupported(_))));
    }
}
