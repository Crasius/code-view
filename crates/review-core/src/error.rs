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

    /// The request never got an answer (DNS, TLS, timeout, connection reset).
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
fn readable_body(body: &str) -> String {
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
        if error.is_decode() {
            return ProviderError::Decode(error.to_string());
        }
        ProviderError::Network(error.to_string())
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

    #[test]
    fn shortens_long_plain_bodies() {
        let error = ProviderError::Status { status: 500, path: "/x".into(), body: "e".repeat(400) };
        assert!(error.to_string().starts_with(&"e".repeat(300)));
        assert!(error.to_string().contains("… (500 from /x)"));
    }
}
