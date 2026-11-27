//! GraphQL transport. GitHub answers GraphQL failures with HTTP 200 and an `errors` array;
//! those become [`ProviderError::Status`] with the closest REST status. Batches that name several
//! objects under aliases ([`execute_found`]) get `null` and a `NOT_FOUND` error for each one
//! GitHub does not know, next to the data of the others; only those errors are dropped there.
//! A check run the token may not read (a fine-grained token without the Checks permission) comes
//! back as a `null` context and a `FORBIDDEN` error next to the data; the rollup's `state` still
//! decides and the context counts as unlisted, so that error is dropped for every query.

use reqwest::{Method, RequestBuilder};
use review_core::{ProviderError, Result, http};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::client::GitHubClient;
use crate::sso;

/// Author fields shared by every query; `id` falls back to the login when the actor has no database id.
pub(crate) const ACTOR_FRAGMENT: &str = " fragment actor on Actor { login avatarUrl ... on User { databaseId name } ... on Bot { databaseId } }";

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Where in `data` the error left a `null`: field names and list indexes.
    #[serde(default)]
    path: Vec<Value>,
}

impl<T> Envelope<T> {
    /// Drops the errors of check contexts hidden from the token when the data came with them.
    fn without_hidden_contexts(mut self) -> Self {
        if self.data.is_some() {
            self.errors.retain(|error| !error.is_hidden_context());
        }
        self
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PageInfo {
    #[serde(default)]
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
    #[serde(default)]
    pub has_previous_page: bool,
    pub start_cursor: Option<String>,
}

/// A GraphQL connection. List items are nullable in GitHub's schema, so nulls are dropped.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Connection<T> {
    #[serde(default)]
    pub page_info: PageInfo,
    /// A plain `default` would make serde require `T: Default`.
    #[serde(default = "Vec::new")]
    pub nodes: Vec<Option<T>>,
}

impl<T> Connection<T> {
    pub(crate) fn into_nodes(self) -> (PageInfo, Vec<T>) {
        (self.page_info, self.nodes.into_iter().flatten().collect())
    }
}

/// Sends one query or mutation. GraphQL always needs a token on GitHub.
pub(crate) async fn execute<T: DeserializeOwned>(
    client: &GitHubClient,
    action: &str,
    query: &str,
    variables: Value,
) -> Result<T> {
    into_data(send(client, action, query, variables).await?)
}

/// [`execute`] for a batch of aliased objects: those GitHub does not know come back as `null`
/// with a `NOT_FOUND` error each, and the found ones are kept. Any other error still fails it.
pub(crate) async fn execute_found<T: DeserializeOwned>(
    client: &GitHubClient,
    action: &str,
    query: &str,
    variables: Value,
) -> Result<T> {
    into_found(send(client, action, query, variables).await?)
}

async fn send<T: DeserializeOwned>(
    client: &GitHubClient,
    action: &str,
    query: &str,
    variables: Value,
) -> Result<Envelope<T>> {
    let envelope: Envelope<T> = http::json(post(client, action, query, &variables)?).await?;
    Ok(envelope.without_hidden_contexts())
}

/// The POST of one query or mutation; GraphQL always needs a token on GitHub.
fn post(
    client: &GitHubClient,
    action: &str,
    query: &str,
    variables: &Value,
) -> Result<RequestBuilder> {
    client.require_token(action)?;
    Ok(client
        .request(Method::POST, client.graphql_url())
        .json(&json!({ "query": query, "variables": variables })))
}

fn into_found<T>(envelope: Envelope<T>) -> Result<T> {
    match envelope {
        Envelope { data: Some(data), errors }
            if errors.iter().all(|error| error.kind.as_deref() == Some("NOT_FOUND")) =>
        {
            Ok(data)
        }
        envelope => into_data(envelope),
    }
}

/// A listing's data and what GitHub left out of it because of SAML single sign-on.
#[derive(Debug)]
pub(crate) struct Partial<T> {
    pub data: T,
    /// Organization ids from `X-GitHub-SSO: partial-results; organizations=…`.
    pub blocked_ids: Vec<u64>,
    /// GraphQL reported SAML `FORBIDDEN` errors next to the data, without naming organizations.
    pub is_saml_denied: bool,
}

/// [`execute`] for listings: organizations behind SAML SSO the token is not authorized for are
/// reported in [`Partial`] instead of failing the whole answer. Any other error still fails it.
pub(crate) async fn execute_partial<T: DeserializeOwned>(
    client: &GitHubClient,
    action: &str,
    query: &str,
    variables: Value,
) -> Result<Partial<T>> {
    let response = http::send(post(client, action, query, &variables)?).await?;
    let blocked_ids = sso::left_out_organizations(response.headers());
    let bytes = response.bytes().await?;
    let envelope: Envelope<T> = serde_json::from_slice(&bytes)?;
    into_partial(envelope.without_hidden_contexts(), blocked_ids)
}

fn into_partial<T>(envelope: Envelope<T>, blocked_ids: Vec<u64>) -> Result<Partial<T>> {
    let Envelope { data, errors } = envelope;
    let is_saml_denied = !errors.is_empty() && errors.iter().all(GraphQlError::is_saml);
    match data {
        Some(data) if is_saml_denied => Ok(Partial { data, blocked_ids, is_saml_denied }),
        data => Ok(Partial {
            data: into_data(Envelope { data, errors })?,
            blocked_ids,
            is_saml_denied: false,
        }),
    }
}

impl GraphQlError {
    fn is_saml(&self) -> bool {
        self.kind.as_deref() == Some("FORBIDDEN") && self.message.to_lowercase().contains("saml")
    }

    /// A `FORBIDDEN` on one node of a rollup's `contexts`.
    fn is_hidden_context(&self) -> bool {
        self.kind.as_deref() == Some("FORBIDDEN")
            && self.path.windows(2).any(|pair| pair[0] == "contexts" && pair[1] == "nodes")
    }
}

fn into_data<T>(envelope: Envelope<T>) -> Result<T> {
    if let Some(first) = envelope.errors.first() {
        let messages: Vec<&str> =
            envelope.errors.iter().map(|error| error.message.as_str()).collect();
        return Err(ProviderError::Status {
            status: status_for(first.kind.as_deref()),
            path: "/graphql".to_owned(),
            body: messages.join("; "),
        });
    }
    envelope.data.ok_or_else(|| ProviderError::Decode("GraphQL response without data".to_owned()))
}

fn status_for(kind: Option<&str>) -> u16 {
    match kind {
        Some("NOT_FOUND") => 404,
        Some("FORBIDDEN") => 403,
        Some("RATE_LIMITED") => 429,
        _ => 422,
    }
}

#[cfg(test)]
mod tests {
    use super::{Connection, Envelope, into_data, into_found, into_partial};
    use crate::mapping::fixtures::parse;
    use review_core::ProviderError;
    use serde_json::Value;

    #[test]
    fn errors_become_status_errors() {
        let envelope: Envelope<Value> = parse(
            r#"{"data":null,"errors":[{"type":"NOT_FOUND","message":"Could not resolve to a node"},{"message":"second"}]}"#,
        );
        match into_data(envelope) {
            Err(ProviderError::Status { status, body, .. }) => {
                assert_eq!(status, 404);
                assert_eq!(body, "Could not resolve to a node; second");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn saml_errors_next_to_data_keep_the_data() {
        let envelope: Envelope<Value> = parse(
            r#"{"data":{"x":1},"errors":[{"type":"FORBIDDEN","message":"Resource protected by organization SAML enforcement."}]}"#,
        );
        let partial = into_partial(envelope, vec![7]).expect("partial");
        assert!(partial.is_saml_denied);
        assert_eq!(partial.blocked_ids, [7]);
        assert_eq!(partial.data["x"], 1);
        let other: Envelope<Value> =
            parse(r#"{"data":{"x":1},"errors":[{"type":"FORBIDDEN","message":"Nope"}]}"#);
        assert!(matches!(
            into_partial(other, Vec::new()),
            Err(ProviderError::Status { status: 403, .. })
        ));
    }

    #[test]
    fn a_batch_keeps_its_found_objects_and_fails_on_other_errors() {
        let missing: Envelope<Value> = parse(
            r#"{"data":{"a":1,"b":null},"errors":[{"type":"NOT_FOUND","message":"Could not resolve"}]}"#,
        );
        assert_eq!(into_found(missing).expect("found")["a"], 1);
        let mixed: Envelope<Value> = parse(
            r#"{"data":{"a":1},"errors":[{"type":"NOT_FOUND","message":"gone"},{"type":"FORBIDDEN","message":"no"}]}"#,
        );
        assert!(matches!(into_found(mixed), Err(ProviderError::Status { status: 404, .. })));
        let without_data: Envelope<Value> =
            parse(r#"{"data":null,"errors":[{"type":"NOT_FOUND","message":"gone"}]}"#);
        assert!(matches!(into_found(without_data), Err(ProviderError::Status { status: 404, .. })));
    }

    #[test]
    fn a_check_hidden_from_the_token_keeps_the_listing() {
        let listing = |path: &str| -> Envelope<Value> {
            parse(&format!(
                r#"{{"data":{{"search":{{"nodes":[{{"number":1}}]}}}},"errors":[{{"type":"FORBIDDEN",
                "message":"Resource not accessible by personal access token","path":{path}}}]}}"#
            ))
        };
        let hidden = r#"["search","nodes",0,"commits","nodes",0,"commit","statusCheckRollup","contexts","nodes",0]"#;
        let partial = into_partial(listing(hidden).without_hidden_contexts(), Vec::new())
            .expect("the listing survives a hidden check");
        assert_eq!(partial.data["search"]["nodes"][0]["number"], 1);
        assert!(!partial.is_saml_denied);
        let found = into_found(listing(hidden).without_hidden_contexts()).expect("found");
        assert_eq!(found["search"]["nodes"][0]["number"], 1);
        assert!(
            matches!(
                into_partial(
                    listing(r#"["search","nodes",0]"#).without_hidden_contexts(),
                    Vec::new()
                ),
                Err(ProviderError::Status { status: 403, .. })
            ),
            "a pull request itself denied still fails"
        );
    }

    #[test]
    fn data_without_errors_passes_through() {
        let envelope: Envelope<Value> = parse(r#"{"data":{"x":1}}"#);
        assert_eq!(into_data(envelope).expect("data")["x"], 1);
    }

    #[test]
    fn connection_drops_null_nodes() {
        let connection: Connection<u32> =
            parse(r#"{"pageInfo":{"hasNextPage":true,"endCursor":"c"},"nodes":[1,null,2]}"#);
        let (page, nodes) = connection.into_nodes();
        assert!(page.has_next_page);
        assert_eq!(page.end_cursor.as_deref(), Some("c"));
        assert_eq!(nodes, vec![1, 2]);
    }
}
