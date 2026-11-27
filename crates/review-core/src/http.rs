//! Shared HTTP plumbing. One pooled [`reqwest::Client`] serves every connection;
//! auth headers go on each request, never on the client.
//!
//! Every request goes through the per-host `budget`: at most six requests (`budget::MAX_IN_FLIGHT`)
//! wait for an answer per host and API kind (REST, GraphQL), a 429 (or GitHub's rate-limit 403)
//! pauses that host's REST or GraphQL requests until its `Retry-After` / reset time and the
//! request is retried once, and the [`traffic`] counters record
//! it per connection. GETs read through [`fetch`] additionally share identical in-flight requests
//! and revalidate with `If-None-Match` when the host sent an `ETag` before; a write that finished
//! since an in-flight GET started ends its sharing (see `cache`). A write is any request but a GET
//! and a GraphQL query, which GraphQL sends as a POST too ([`is_write`]).

// RS-DONT-04: headers and bodies come from the hosts; reading them (here and in every submodule)
// never indexes.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

pub(crate) mod budget;
mod cache;
pub mod image;
pub mod traffic;

use std::time::Duration;

use bytes::Bytes;
use reqwest::{Client, ClientBuilder, Method, Request, RequestBuilder, Response, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::error::Result;

pub use budget::{Api, RateLimitNotice, rate_limits, resume, without_waiting};
pub use cache::{cached_bytes, write_epoch};
pub use traffic::{ConnectionTraffic, for_connection};

const USER_AGENT: &str = concat!("CodeView/", env!("CARGO_PKG_VERSION"));

/// Builds the process-wide API client: gzip, HTTP/2, pooled keep-alive, sane timeouts.
pub fn build_client() -> Client {
    client_builder().build().expect("TLS backend must initialize")
}

/// The settings of [`build_client`], for a caller that adds its own (tests: a resolver that
/// keeps them off the real network).
pub fn client_builder() -> ClientBuilder {
    Client::builder()
        .user_agent(USER_AGENT)
        .gzip(true)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(8)
}

/// Sends and turns non-success statuses into [`ProviderError::Status`](crate::ProviderError::Status)
/// with a trimmed body.
/// The body stays a stream (job logs); use [`fetch`] for bodies read at once.
pub async fn send(request: RequestBuilder) -> Result<Response> {
    let (client, request) = request.build_split();
    let request = request?;
    let _write = write_guard(&request);
    budget::execute(&client, request).await
}

/// The guard of `request` while it runs, when it is a write ([`is_write`], [`cache::Write`]).
/// Made only for a write: a guard made and dropped at once would end the sharing of the GETs in
/// flight as a finished write does.
fn write_guard(request: &Request) -> Option<cache::Write> {
    let body = request.body().and_then(reqwest::Body::as_bytes);
    is_write(request.method(), body).then(|| cache::Cache::shared().write())
}

/// Whether a request with `method` and `body` may change what the host answers: anything but a
/// GET and a GraphQL query. A GraphQL body that cannot be read counts as a write; at worst a GET
/// then asks again. Public so that a walkthrough's mock host counts the writes it received by
/// this one definition.
pub fn is_write(method: &Method, body: Option<&[u8]>) -> bool {
    /// The part of a GraphQL request body that names its operations.
    #[derive(Deserialize)]
    struct Operation<'a> {
        #[serde(borrow)]
        query: std::borrow::Cow<'a, str>,
    }
    if method == Method::GET {
        return false;
    }
    let Some(operation) = body.and_then(|body| serde_json::from_slice::<Operation>(body).ok())
    else {
        return true;
    };
    operation
        .query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|word| word == "mutation" || word == "subscription")
}

/// A fully read answer: body, the `rel="next"` page link and the final URL after redirects.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub bytes: Bytes,
    pub next_link: Option<String>,
    pub url: Url,
}

impl Fetched {
    async fn read(response: Response) -> Result<Self> {
        let next_link = next_link(&response);
        let url = response.url().clone();
        let bytes = response.bytes().await?;
        Ok(Self { bytes, next_link, url })
    }

    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_slice(&self.bytes)?)
    }
}

/// Sends and reads the whole body. GETs share an identical request already in flight and
/// revalidate a cached answer with `If-None-Match`, so unchanged data costs a 304.
pub async fn fetch(request: RequestBuilder) -> Result<Fetched> {
    let (client, request) = request.build_split();
    let request = request?;
    if request.method() != Method::GET {
        let _write = write_guard(&request);
        return Fetched::read(budget::execute(&client, request).await?).await;
    }
    cache::Cache::shared().fetch(client, request).await
}

/// Sends, checks the status and decodes the JSON body.
pub async fn json<T: DeserializeOwned>(request: RequestBuilder) -> Result<T> {
    fetch(request).await?.json()
}

/// Sends and checks the status, discarding the body.
pub async fn unit(request: RequestBuilder) -> Result<()> {
    send(request).await?;
    Ok(())
}

/// Copies the body of `response` into `sink`, chunk by chunk.
pub async fn copy_body(
    mut response: Response,
    sink: &mut (dyn AsyncWrite + Unpin + Send),
) -> Result<()> {
    while let Some(chunk) = response.chunk().await? {
        sink.write_all(&chunk).await?;
    }
    sink.flush().await?;
    Ok(())
}

/// Parses the `rel="next"` target out of an RFC 8288 `Link` header (GitHub, GitLab).
fn next_link(response: &Response) -> Option<String> {
    let header = response.headers().get(reqwest::header::LINK)?.to_str().ok()?;
    parse_next_link(header)
}

fn parse_next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let (target, params) = part.split_once(';')?;
        let is_next = params.split(';').any(|param| param.trim() == "rel=\"next\"");
        if !is_next {
            return None;
        }
        let url = target.trim().trim_start_matches('<').trim_end_matches('>');
        Some(url.to_owned())
    })
}

/// `scheme://host[:port]`, the unit the budget pauses and caps.
pub(crate) fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use reqwest::Method;
    use serde_json::json;
    use tokio::io::AsyncWrite;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{copy_body, is_write, parse_next_link, write_guard};
    use crate::ProviderError;

    /// A sink whose every write fails, as a full disk does.
    struct FullDisk;

    impl AsyncWrite for FullDisk {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Err(io::Error::other("disk full")))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    /// A job log larger than one chunk arrives whole; a sink that cannot take it ends the copy
    /// with a local error, not a network one.
    #[tokio::test]
    async fn copies_a_body_of_many_chunks_and_reports_a_failing_sink() {
        let server = MockServer::start().await;
        let log: Vec<u8> = (0..=u8::MAX).cycle().take(4 * 1024 * 1024).collect();
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(log.clone()))
            .mount(&server)
            .await;
        let client = reqwest::Client::new();

        let mut copied = Vec::new();
        let response = client.get(server.uri()).send().await.expect("answer");
        copy_body(response, &mut copied).await.expect("copied");
        assert_eq!(copied.len(), log.len());
        assert!(copied == log, "the copy has every byte in order");

        let response = client.get(server.uri()).send().await.expect("answer");
        let failed = copy_body(response, &mut FullDisk).await;
        assert!(
            matches!(&failed, Err(ProviderError::Local(message)) if message.contains("disk full")),
            "{failed:?}"
        );
    }

    #[test]
    fn graphql_queries_are_reads_and_mutations_writes() {
        let client = reqwest::Client::new();
        let post = |body| {
            client.request(Method::POST, "http://127.0.0.1/graphql").json(&body).build().unwrap()
        };
        let is_write = |request: &reqwest::Request| {
            is_write(request.method(), request.body().and_then(reqwest::Body::as_bytes))
        };
        let query = json!({ "query": "query Mine { search(type: ISSUE) { nodes { id } } }" });
        assert!(!is_write(&post(query)), "a GraphQL query shares nothing it should not");
        let shorthand = json!({ "query": "{ viewer { login } }", "variables": {} });
        assert!(!is_write(&post(shorthand)));
        let mutation = json!({ "query": "mutation { addLabelsToLabelable(input: {}) { id } }" });
        assert!(is_write(&post(mutation)));
        assert!(is_write(&post(json!({ "status": "completed" }))), "a REST write");
        let get = client.get("http://127.0.0.1/pulls/7").build().unwrap();
        assert!(!is_write(&get));
        let delete = client.delete("http://127.0.0.1/labels/bug").build().unwrap();
        assert!(is_write(&delete));
    }

    /// Regression: every request but a GET made a guard and dropped it at once, so each GraphQL
    /// query (a list's search, a page's threads) ended the sharing of the GETs in flight, and the
    /// page a click opened asked again for the detail the list was reading ahead.
    #[test]
    fn only_a_write_ends_a_write() {
        let client = reqwest::Client::new();
        let post = |body| {
            client.request(Method::POST, "http://127.0.0.1/graphql").json(&body).build().unwrap()
        };
        assert!(
            write_guard(&post(json!({ "query": "query { viewer { login } }" }))).is_none(),
            "a GraphQL query is no write"
        );
        assert!(
            write_guard(&post(json!({ "query": "mutation { addStar(input: {}) { id } }" })))
                .is_some(),
            "a mutation is one"
        );
    }

    #[test]
    fn parses_next_from_link_header() {
        let header = r#"<https://api.github.com/x?page=2>; rel="next", <https://api.github.com/x?page=5>; rel="last""#;
        assert_eq!(parse_next_link(header).as_deref(), Some("https://api.github.com/x?page=2"));
    }

    #[test]
    fn returns_none_without_next() {
        let header = r#"<https://api.github.com/x?page=1>; rel="prev""#;
        assert_eq!(parse_next_link(header), None);
    }
}
