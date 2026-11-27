//! Images that descriptions, comments and rendered files embed. A connection's credentials go only
//! to its own hosts and only over https: redirects are followed by hand and every hop is authorized
//! anew, so a redirect to a CDN or object storage (GitHub attachments, GitLab uploads) goes out
//! without them. Bodies are read up to a size limit and never buffered beyond it. Errors name the
//! host and path, never the query (signed addresses carry their signature there).
//!
//! A caller that keeps a copy passes its [`Validators`]: every hop asks with `If-None-Match` and
//! `If-Modified-Since`, and a `304` comes back as [`Body::NotModified`] without a body. Hosts that
//! do not support them answer 200 as before.

use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use reqwest::header::{
    ACCEPT, CACHE_CONTROL, CONTENT_TYPE, ETAG, HeaderMap, IF_MODIFIED_SINCE, IF_NONE_MATCH,
    LAST_MODIFIED, LOCATION,
};
use reqwest::{Client, RequestBuilder, Response, StatusCode, Url};

use super::{budget, traffic};
use crate::error::{ProviderError, Result};

/// What a browser sends for an `<img>`: formats the app decodes first, anything else last.
pub const IMAGE_ACCEPT: &str = "image/webp,image/png,image/svg+xml,image/*;q=0.8,*/*;q=0.5";
const MAX_REDIRECTS: usize = 5;

/// A connection's hosts and its credentials for them.
pub trait Authorize: Send + Sync {
    /// Whether `url` is on one of the platform's own hosts (web and API): a refusal or a web page
    /// from there means the image is private ([`Body::Private`]).
    fn is_own(&self, url: &Url) -> bool;
    /// `request` with the credentials when `url` is a host they are meant for, unchanged for every
    /// other host. Asked only for https addresses (and plain http on the loopback interface).
    fn authorize(&self, url: &Url, request: RequestBuilder) -> RequestBuilder;
}

/// Asks every host without credentials.
pub struct Anonymous;

impl Authorize for Anonymous {
    fn is_own(&self, _url: &Url) -> bool {
        false
    }

    fn authorize(&self, _url: &Url, request: RequestBuilder) -> RequestBuilder {
        request
    }
}

/// A complete image body.
#[derive(Debug, Clone)]
pub struct Image {
    pub bytes: Bytes,
    /// The `Content-Type` the host sent.
    pub content_type: Option<String>,
    /// How long the host lets the image be reused (`Cache-Control`); zero for `no-store` and
    /// `no-cache`, `None` when the host does not say.
    pub max_age: Option<Duration>,
    /// What to ask with when the kept copy is stale.
    pub validators: Validators,
}

/// The validators of a kept copy: `ETag` and `Last-Modified` as the host sent them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

impl Validators {
    /// No copy to ask about: the fetch is unconditional.
    pub const NONE: Validators = Validators { etag: None, last_modified: None };

    fn of(headers: &HeaderMap) -> Self {
        let header = |name| {
            headers
                .get(name)
                .and_then(|value: &reqwest::header::HeaderValue| value.to_str().ok())
                .map(str::to_owned)
        };
        Self { etag: header(ETAG), last_modified: header(LAST_MODIFIED) }
    }

    fn ask(&self, mut request: RequestBuilder) -> RequestBuilder {
        if let Some(etag) = &self.etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        if let Some(last_modified) = &self.last_modified {
            request = request.header(IF_MODIFIED_SINCE, last_modified);
        }
        request
    }
}

#[derive(Debug, Clone)]
pub enum Body {
    Image(Image),
    /// The copy the [`Validators`] name is still current (`304`); the host may name a new
    /// lifetime for it.
    NotModified {
        max_age: Option<Duration>,
    },
    /// The body is larger than the limit; its size when the host announced it.
    TooLarge(Option<u64>),
    /// One of the platform's own hosts refused the image (401, 403) or answered with a web page
    /// (its sign-in page): the image is private and the credentials sent, if any, do not reach it.
    Private,
}

/// Fetches `url` (http or https only) with `accept`, at most `limit` bytes of body; with `known`
/// validators of a kept copy, [`Body::NotModified`] when that copy is still current.
pub async fn fetch(
    url: &str,
    accept: &str,
    limit: u64,
    known: &Validators,
    auth: &dyn Authorize,
) -> Result<Body> {
    let mut url = web_url(url)?;
    for _ in 0..=MAX_REDIRECTS {
        let request = known.ask(client().get(url.clone()).header(ACCEPT, accept));
        let request =
            if may_carry_credentials(&url) { auth.authorize(&url, request) } else { request };
        let (client, request) = request.build_split();
        let response = match budget::execute_with(&client, request?, |status| {
            status.is_success() || status.is_redirection()
        })
        .await
        {
            Ok(response) => response,
            Err(ProviderError::Status { status: 401 | 403, .. }) if auth.is_own(&url) => {
                return Ok(Body::Private);
            }
            Err(error) => return Err(redact(error, &url)),
        };
        if response.status() == StatusCode::NOT_MODIFIED {
            let label = traffic::current_connection().unwrap_or_else(|| super::origin(&url));
            traffic::record_not_modified(&label);
            return Ok(Body::NotModified { max_age: max_age_of(response.headers()) });
        }
        if response.status().is_redirection() {
            url = redirect_target(&url, &response)?;
            continue;
        }
        if auth.is_own(&url) && is_web_page(&response) {
            return Ok(Body::Private);
        }
        return read(response, limit).await.map_err(|error| redact(error, &url));
    }
    Err(ProviderError::Network(format!(
        "{} redirected more than {MAX_REDIRECTS} times",
        url.host_str().unwrap_or("the host")
    )))
}

/// Credentials travel over https only; plain http only on the loopback interface (local servers).
fn may_carry_credentials(url: &Url) -> bool {
    let host = url.host_str().unwrap_or_default().trim_start_matches('[').trim_end_matches(']');
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|address| address.is_loopback());
    url.scheme() == "https" || (url.scheme() == "http" && is_loopback)
}

fn is_web_page(response: &Response) -> bool {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|kind| kind.trim_start().to_ascii_lowercase().starts_with("text/html"))
}

/// `error` without `url`'s query: transport errors spell out the whole address.
fn redact(error: ProviderError, url: &Url) -> ProviderError {
    let mut bare = url.clone();
    bare.set_query(None);
    bare.set_fragment(None);
    match error {
        ProviderError::Network(message) => {
            ProviderError::Network(message.replace(url.as_str(), bare.as_str()))
        }
        ProviderError::Decode(message) => {
            ProviderError::Decode(message.replace(url.as_str(), bare.as_str()))
        }
        other => other,
    }
}

/// `url` when it is an absolute http or https URL; other schemes (`file:`, `data:`) are refused.
fn web_url(url: &str) -> Result<Url> {
    let parsed = Url::parse(url).map_err(|error| {
        ProviderError::Local(format!("'{url}' is not a valid address: {error}"))
    })?;
    match parsed.scheme() {
        "https" | "http" if parsed.host_str().is_some() => Ok(parsed),
        scheme => Err(ProviderError::Unsupported(format!(
            "Images from {scheme}: addresses are not loaded."
        ))),
    }
}

/// Whether `url` is on `origin` (`scheme://host[:port]`), compared like a browser compares origins.
pub fn is_same_origin(url: &Url, origin: &Url) -> bool {
    url.origin() == origin.origin()
}

/// One client for every image: the API client's settings with no automatic redirects (see the
/// module docs) and a shorter timeout.
fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        super::client_builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(60))
            .build()
            .expect("TLS backend must initialize")
    })
}

fn redirect_target(from: &Url, response: &Response) -> Result<Url> {
    let location =
        response.headers().get(LOCATION).and_then(|value| value.to_str().ok()).ok_or_else(
            || ProviderError::Decode(format!("{} redirected without a location", from.path())),
        )?;
    let target = from.join(location).map_err(|error| {
        ProviderError::Decode(format!("{} redirected to an invalid location: {error}", from.path()))
    })?;
    web_url(target.as_str())
}

async fn read(mut response: Response, limit: u64) -> Result<Body> {
    if let Some(length) = response.content_length().filter(|length| *length > limit) {
        return Ok(Body::TooLarge(Some(length)));
    }
    let headers = response.headers();
    let content_type =
        headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).map(str::to_owned);
    let max_age = max_age_of(headers);
    let validators = Validators::of(headers);
    let mut bytes = BytesMut::new();
    while let Some(chunk) = response.chunk().await? {
        if (bytes.len() + chunk.len()) as u64 > limit {
            return Ok(Body::TooLarge(None));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Body::Image(Image { bytes: bytes.freeze(), content_type, max_age, validators }))
}

fn max_age_of(headers: &HeaderMap) -> Option<Duration> {
    headers.get(CACHE_CONTROL).and_then(|value| value.to_str().ok()).and_then(parse_max_age)
}

/// `max-age` of a `Cache-Control` value; zero when the answer must not be reused.
fn parse_max_age(value: &str) -> Option<Duration> {
    let mut max_age = None;
    for directive in value.split(',').map(|part| part.trim().to_ascii_lowercase()) {
        if directive == "no-store" || directive == "no-cache" {
            return Some(Duration::ZERO);
        }
        if let Some(seconds) = directive
            .strip_prefix("max-age=")
            .and_then(|seconds| seconds.trim_matches('"').parse::<u64>().ok())
        {
            max_age = Some(Duration::from_secs(seconds));
        }
    }
    max_age
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;

    /// Credentials for one origin, like a provider's.
    struct Token(Url);

    impl Authorize for Token {
        fn is_own(&self, url: &Url) -> bool {
            is_same_origin(url, &self.0)
        }

        fn authorize(&self, url: &Url, request: RequestBuilder) -> RequestBuilder {
            if self.is_own(url) { request.header("PRIVATE-TOKEN", "secret") } else { request }
        }
    }

    async fn get(url: String, limit: u64, auth: &dyn Authorize) -> Result<Body> {
        fetch(&url, IMAGE_ACCEPT, limit, &Validators::NONE, auth).await
    }

    #[test]
    fn credentials_travel_over_https_or_loopback_only() {
        let may = |url: &str| may_carry_credentials(&Url::parse(url).unwrap());
        assert!(may("https://gitlab.example.com/a.png"));
        assert!(
            may("http://127.0.0.1:8080/a.png")
                && may("http://localhost/a.png")
                && may("http://[::1]/a.png")
        );
        assert!(
            !may("http://gitlab.example.com/a.png"),
            "never over plain http to another machine"
        );
        assert!(!may("http://10.0.0.5/a.png"));
    }

    #[test]
    fn errors_leave_the_query_out() {
        let url =
            Url::parse("https://private-user-images.githubusercontent.com/1/2-a.png?jwt=signature")
                .unwrap();
        let error =
            redact(ProviderError::Network(format!("error sending request for url ({url})")), &url);
        assert!(!error.to_string().contains("signature"), "{error}");
        assert!(error.to_string().contains("private-user-images.githubusercontent.com/1/2-a.png"));
    }

    /// The cause a failed request now carries never spells the address again, so the redaction
    /// of reqwest's text leaves the signature out of the whole message.
    #[tokio::test]
    async fn the_cause_of_a_failed_request_leaves_the_query_out() {
        let port = crate::error::closed_port();
        let url = format!("http://127.0.0.1:{port}/1/2-a.png?jwt=signature");
        let error = get(url, 100, &Anonymous).await.unwrap_err();
        let ProviderError::Network(message) = &error else { panic!("{error:?}") };
        assert!(!message.contains("signature") && !message.contains("jwt"), "{message}");
        assert!(message.contains(&format!("127.0.0.1:{port}/1/2-a.png)")), "{message}");
        assert!(message.to_lowercase().contains("connection refused"), "{message}");
    }

    /// The own host's sign-in page or refusal means the image is private; elsewhere a refusal is just that.
    #[tokio::test]
    async fn sign_in_pages_and_refusals_of_the_own_host_mean_private() {
        let own = MockServer::start().await;
        let other = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/uploads/a.png"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "/users/sign_in"))
            .mount(&own)
            .await;
        Mock::given(method("GET"))
            .and(path("/users/sign_in"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("<!DOCTYPE html>", "text/html; charset=utf-8"),
            )
            .mount(&own)
            .await;
        Mock::given(method("GET"))
            .and(path("/denied.png"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&own)
            .await;
        Mock::given(method("GET"))
            .and(path("/denied.png"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&other)
            .await;
        let auth = Token(Url::parse(&own.uri()).unwrap());
        let signed_out = get(format!("{}/uploads/a.png", own.uri()), 100, &auth).await.unwrap();
        assert!(matches!(signed_out, Body::Private), "{signed_out:?}");
        let refused = get(format!("{}/denied.png", own.uri()), 100, &auth).await.unwrap();
        assert!(matches!(refused, Body::Private), "{refused:?}");
        let elsewhere = get(format!("{}/denied.png", other.uri()), 100, &auth).await.unwrap_err();
        assert_eq!(elsewhere.status(), Some(403));
    }

    #[test]
    fn reads_cache_control() {
        assert_eq!(parse_max_age("public, max-age=300"), Some(Duration::from_secs(300)));
        assert_eq!(parse_max_age("max-age=31536000, no-cache"), Some(Duration::ZERO));
        assert_eq!(parse_max_age("private"), None);
    }

    #[test]
    fn refuses_other_schemes() {
        assert!(matches!(
            web_url("file:///C:/Windows/win.ini"),
            Err(ProviderError::Unsupported(_))
        ));
        assert!(matches!(
            web_url("data:image/png;base64,AAAA"),
            Err(ProviderError::Unsupported(_))
        ));
        assert!(web_url("https://img.shields.io/badge/a-b-c").is_ok());
    }

    /// A redirect to another host goes out without the token; the image arrives from there.
    #[tokio::test]
    async fn credentials_stay_on_their_own_host() {
        let own = MockServer::start().await;
        let storage = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/uploads/a.png"))
            .and(header("PRIVATE-TOKEN", "secret"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}/bucket/a.png", storage.uri())),
            )
            .mount(&own)
            .await;
        Mock::given(method("GET"))
            .and(path("/bucket/a.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/png")
                    .set_body_bytes(vec![1, 2, 3]),
            )
            .mount(&storage)
            .await;
        let auth = Token(Url::parse(&own.uri()).unwrap());
        let body = get(format!("{}/uploads/a.png", own.uri()), 100, &auth).await.unwrap();
        let Body::Image(image) = body else { panic!("expected an image, got {body:?}") };
        assert_eq!(&image.bytes[..], &[1, 2, 3]);
        assert_eq!(image.content_type.as_deref(), Some("image/png"));
        let storage_requests: Vec<Request> = storage.received_requests().await.unwrap();
        assert!(
            storage_requests.iter().all(|request| !request.headers.contains_key("PRIVATE-TOKEN")),
            "the token left its host"
        );
    }

    /// A kept copy is asked about with its validators; the host's 304 brings no body.
    #[tokio::test]
    async fn a_current_copy_comes_back_as_not_modified() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/a.png"))
            .and(header("If-None-Match", "\"v1\""))
            .respond_with(ResponseTemplate::new(304).insert_header("Cache-Control", "max-age=60"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/a.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ETag", "\"v1\"")
                    .insert_header("Last-Modified", "Wed, 21 Oct 2026 07:28:00 GMT")
                    .set_body_bytes(vec![1, 2, 3]),
            )
            .mount(&server)
            .await;
        let url = format!("{}/a.png", server.uri());
        let body = get(url.clone(), 100, &Anonymous).await.unwrap();
        let Body::Image(first) = body else { panic!("expected an image, got {body:?}") };
        assert_eq!(first.validators.etag.as_deref(), Some("\"v1\""));
        assert_eq!(
            first.validators.last_modified.as_deref(),
            Some("Wed, 21 Oct 2026 07:28:00 GMT")
        );
        let again = fetch(&url, IMAGE_ACCEPT, 100, &first.validators, &Anonymous).await.unwrap();
        assert!(
            matches!(again, Body::NotModified { max_age: Some(age) } if age == Duration::from_secs(60)),
            "{again:?}"
        );
    }

    #[tokio::test]
    async fn stops_at_the_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; 64]))
            .mount(&server)
            .await;
        let body = get(format!("{}/big.png", server.uri()), 10, &Anonymous).await.unwrap();
        assert!(matches!(body, Body::TooLarge(Some(64))), "{body:?}");
        let missing = MockServer::start().await;
        let error = get(format!("{}/gone.png", missing.uri()), 10, &Anonymous).await.unwrap_err();
        assert_eq!(error.status(), Some(404));
    }
}
