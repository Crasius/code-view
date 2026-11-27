//! The one error type of every provider call, and how a platform's error body reads to the user.

// RS-DONT-04: error bodies come from the platforms; reading them never indexes.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use thiserror::Error;

pub type Result<T> = std::result::Result<T, ProviderError>;

#[derive(Debug, Clone, Error)]
pub enum ProviderError {
    /// The platform answered with a non-success status. Display shows the platform's own
    /// message instead of the raw body when the body is a JSON error document.
    #[error("{} ({status} from {path})", readable_body(.body))]
    Status { status: u16, path: String, body: String },

    /// The request never got an answer (DNS, TLS, timeout, connection reset); the message names
    /// the cause after reqwest's own text.
    #[error("network error: {0}")]
    Network(String),

    /// The answer did not match the expected shape.
    #[error("unexpected response: {0}")]
    Decode(String),

    /// The call needs a token or a feature the connection does not have.
    #[error("{0}")]
    Unsupported(String),

    /// Local failures: git, file system, invalid input.
    #[error("{0}")]
    Local(String),

    /// The host still answered 429 (or GitHub's rate-limit 403) after the app waited and retried once.
    #[error(
        "{host} is rate-limiting this connection; it accepts requests again in about {retry_after_secs} s."
    )]
    RateLimited { host: String, retry_after_secs: u64 },

    /// The platform refused a merge because the source branch no longer ends at
    /// [`crate::MergeOptions::head_sha`], the commit the user reviewed. Reloading the pull request,
    /// not a retry, is the way out.
    #[error(
        "New commits arrived since you reviewed this pull request. Reload it and review them first."
    )]
    HeadMoved,
}

impl ProviderError {
    pub fn status(&self) -> Option<u16> {
        match self {
            ProviderError::Status { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// The platform's own message of a [`ProviderError::Status`] (the `message` field of a JSON body).
    pub fn server_message(&self) -> Option<String> {
        match self {
            ProviderError::Status { body, .. } => Some(readable_body(body)),
            _ => None,
        }
    }

    pub fn requires_token(action: &str) -> Self {
        ProviderError::Unsupported(format!(
            "{action} needs a personal access token on this connection."
        ))
    }
}

const MAX_BODY_CHARS: usize = 300;

/// The `message` / `error_description` / `error` field of a JSON error body, the `<Message>` of an
/// XML one (blob storage behind a redirect), the `<title>` of an HTML page (a gateway's
/// `502 Bad Gateway`), else the trimmed body.
pub(crate) fn readable_body(body: &str) -> String {
    let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
    let message = parsed.as_ref().and_then(|value| {
        ["message", "error_description", "error"]
            .iter()
            .find_map(|field| value.get(field).and_then(serde_json::Value::as_str))
    });
    let text =
        message.or_else(|| xml_message(body)).or_else(|| html_title(body)).unwrap_or(body).trim();
    if text.is_empty() {
        return "No details from the server".to_owned();
    }
    if text.chars().count() <= MAX_BODY_CHARS {
        return text.to_owned();
    }
    let shortened: String = text.chars().take(MAX_BODY_CHARS).collect();
    format!("{shortened}…")
}

/// The first line of `<Message>` in an XML error document (`<Error><Code>BlobNotFound</Code>
/// <Message>The specified blob does not exist.\nRequestId:…</Message></Error>`).
fn xml_message(body: &str) -> Option<&str> {
    let body = body.trim_start();
    if !body.starts_with('<') {
        return None;
    }
    let (_, rest) = body.split_once("<Message>")?;
    let (message, _) = rest.split_once("</Message>")?;
    message.lines().map(str::trim).find(|line| !line.is_empty())
}

/// The `<title>` of an HTML error page; an HTML page without one reads as no details, never as
/// markup.
fn html_title(body: &str) -> Option<&str> {
    let body = body.trim_start();
    let head: String = body.chars().take(16).collect::<String>().to_ascii_lowercase();
    if !(head.starts_with("<html") || head.starts_with("<!doctype html")) {
        return None;
    }
    let title = body
        .split_once("<title>")
        .and_then(|(_, rest)| rest.split_once("</title>"))
        .map_or("", |(title, _)| title);
    Some(title)
}

impl From<reqwest::Error> for ProviderError {
    fn from(error: reqwest::Error) -> Self {
        let message = with_cause(&error);
        if error.is_decode() {
            return ProviderError::Decode(message);
        }
        ProviderError::Network(message)
    }
}

/// reqwest's own text, then why the request failed, on one line:
/// `error sending request for url (http://127.0.0.1:8080/x): tcp connect error: connection refused: …`.
///
/// reqwest's `Display` names only its own layer; the reason sits at the bottom of the
/// [`source`](std::error::Error::source) chain. The innermost level is the reason (the OS error,
/// the resolver's answer, `operation timed out`); the level just above it names the step that
/// failed where hyper adds one (`tcp connect error`, `error resolving DNS`, `connection error`).
/// The levels in between repeat those two (`client error (Connect)`) and are left out. No level
/// below reqwest's carries the URL, so the message holds no more of it than reqwest's own text.
fn with_cause(error: &reqwest::Error) -> String {
    let mut levels = Vec::new();
    let mut level = std::error::Error::source(error);
    while let Some(current) = level {
        levels.push(current);
        level = current.source();
    }
    let Some((innermost, above)) = levels.split_last() else {
        return error.to_string();
    };
    let mut cause = reason(*innermost);
    if let Some(step) = above.last().map(ToString::to_string)
        && !cause.to_lowercase().contains(&step.to_lowercase())
    {
        cause = format!("{step}: {cause}");
    }
    let cause = cause.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{error}: {cause}")
}

/// The innermost level of a request error. An OS error reads in the system's language (Windows
/// localizes socket errors), so a network [`std::io::ErrorKind`] goes in front in English unless
/// the text already says it (`Connection refused (os error 111)` on Linux).
fn reason(level: &(dyn std::error::Error + 'static)) -> String {
    use std::io::ErrorKind;
    let text = level.to_string();
    let kind = level.downcast_ref::<std::io::Error>().map(std::io::Error::kind).filter(|kind| {
        matches!(
            kind,
            ErrorKind::ConnectionRefused
                | ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted
                | ErrorKind::NotConnected
                | ErrorKind::TimedOut
                | ErrorKind::HostUnreachable
                | ErrorKind::NetworkUnreachable
                | ErrorKind::NetworkDown
                | ErrorKind::AddrNotAvailable
                | ErrorKind::BrokenPipe
                | ErrorKind::UnexpectedEof
        )
    });
    match kind {
        Some(kind) if !text.to_lowercase().contains(&kind.to_string()) => format!("{kind}: {text}"),
        _ => text,
    }
}

impl From<serde_json::Error> for ProviderError {
    fn from(error: serde_json::Error) -> Self {
        ProviderError::Decode(error.to_string())
    }
}

impl From<std::io::Error> for ProviderError {
    fn from(error: std::io::Error) -> Self {
        ProviderError::Local(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderError;

    #[test]
    fn shows_the_platform_message_instead_of_raw_json() {
        let error = ProviderError::Status {
            status: 404,
            path: "/org/_apis/git/pullrequests/14".into(),
            body: r#"{"$id":"1","message":"TF401180: The requested pull request was not found.","errorCode":0}"#.into(),
        };
        assert_eq!(
            error.to_string(),
            "TF401180: The requested pull request was not found. (404 from /org/_apis/git/pullrequests/14)"
        );
    }

    #[test]
    fn shows_the_message_of_an_xml_error_instead_of_the_document() {
        let error = ProviderError::Status {
            status: 404,
            path: "/actions-results/7f/workflow-job-run-1/logs/job/job-logs.txt".into(),
            body: "<?xml version=\"1.0\" encoding=\"utf-8\"?><Error><Code>BlobNotFound</Code><Message>The specified blob does not exist.\nRequestId:5f1c\nTime:2026-10-19T18:00:00Z</Message></Error>".into(),
        };
        assert_eq!(
            error.to_string(),
            "The specified blob does not exist. (404 from /actions-results/7f/workflow-job-run-1/logs/job/job-logs.txt)"
        );
    }

    /// GitHub's gateway answered a timed-out GraphQL search with nginx's HTML page.
    #[test]
    fn shows_the_title_of_an_html_error_page_instead_of_the_markup() {
        let page = "<html>\r\n<head><title>502 Bad Gateway</title></head>\r\n<body>\r\n<center><h1>502 Bad Gateway</h1></center>\r\n<hr><center>nginx</center>\r\n</body>\r\n</html>\r\n";
        let error =
            ProviderError::Status { status: 502, path: "/graphql".into(), body: page.into() };
        assert_eq!(error.to_string(), "502 Bad Gateway (502 from /graphql)");
        let untitled = ProviderError::Status {
            status: 503,
            path: "/x".into(),
            body: "<!DOCTYPE html><html><body>down</body></html>".into(),
        };
        assert_eq!(untitled.to_string(), "No details from the server (503 from /x)");
    }

    /// A port nobody listens on: the listener is bound and dropped, so the port stays free.
    fn closed_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        listener.local_addr().expect("bound address").port()
    }

    /// The message names the refusal next to reqwest's own text. `connection refused` is the
    /// English name of `io::ErrorKind::ConnectionRefused`, the same on every system; the OS text
    /// after it is the system's own (German on a German Windows).
    #[tokio::test]
    async fn a_refused_connection_says_it_was_refused() {
        let url = format!("http://127.0.0.1:{}/repos/acme/site?page=2", closed_port());
        let client = crate::http::build_client();
        let error = ProviderError::from(client.get(&url).send().await.unwrap_err());
        let ProviderError::Network(message) = &error else { panic!("{error:?}") };
        assert!(
            message.starts_with(&format!("error sending request for url ({url}): ")),
            "reqwest's own text stays first: {message}"
        );
        assert!(message.to_lowercase().contains("connection refused"), "{message}");
        assert!(!message.contains('\n'), "one line: {message}");
        assert_eq!(message.matches(url.as_str()).count(), 1, "the URL once: {message}");
    }

    /// A request that outlives the client's timeout keeps saying that it timed out.
    #[tokio::test]
    async fn a_timeout_says_it_timed_out() {
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = silent.local_addr().expect("bound address");
        let _held = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = silent.accept().await {
                held.push(stream);
            }
        });
        let client = crate::http::client_builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .expect("client");
        let error = client.get(format!("http://{address}/x")).send().await.unwrap_err();
        assert!(error.is_timeout(), "{error:?}");
        let error = ProviderError::from(error);
        assert!(matches!(&error, ProviderError::Network(_)), "{error:?}");
        assert!(error.to_string().contains("timed out"), "{error}");
    }

    #[test]
    fn shortens_long_plain_bodies() {
        let error = ProviderError::Status { status: 500, path: "/x".into(), body: "e".repeat(400) };
        assert!(error.to_string().starts_with(&"e".repeat(300)));
        assert!(error.to_string().contains("… (500 from /x)"));
    }
}
