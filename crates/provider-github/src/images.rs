//! Images in descriptions and comments. The token goes to the API host only (on GitHub Enterprise
//! Server that is the web host too); github.com itself does not take tokens.
//!
//! * Attachments (`{web}/user-attachments/assets/{id}`, the older `{web}/{owner}/{repo}/assets/
//!   {user}/{id}`) of private repositories open only for a signed-in viewer. With a token they load
//!   through the address GitHub signs for that viewer: it renders the image in the repository's
//!   context (`POST /markdown`, `gfm` mode) and the rendered `<img>` points at a signed storage
//!   address valid a few minutes, which goes out without the token. It is asked for right before
//!   each load and kept nowhere; the image is cached under the address as written.
//! * A repository file linked on the web (`/blob/`, `/raw/`, `raw.githubusercontent.com`) is read
//!   through the contents API, which serves private repositories.

// RS-DONT-04: parses provider data, so indexing and narrowing casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use reqwest::{Method, RequestBuilder, Url};
use review_core::http::image::{self, Authorize, Body, Validators};
use review_core::{ProviderError, Result, http};
use serde_json::json;

use crate::client::GitHubClient;
use crate::repo_key::RepoKey;

/// The contents API answers with the file's bytes for this media type.
const RAW_MEDIA_TYPE: &str = "application/vnd.github.raw+json";
const RAW_HOST: &str = "raw.githubusercontent.com";

struct Hosts {
    /// The web root and the API root.
    own: Vec<Url>,
    api: Option<Url>,
    token: Option<String>,
}

impl Authorize for Hosts {
    fn is_own(&self, url: &Url) -> bool {
        self.own.iter().any(|origin| image::is_same_origin(url, origin))
    }

    fn authorize(&self, url: &Url, request: RequestBuilder) -> RequestBuilder {
        match (&self.token, &self.api) {
            (Some(token), Some(api)) if image::is_same_origin(url, api) => {
                request.bearer_auth(token)
            }
            _ => request,
        }
    }
}

pub(crate) async fn fetch(
    client: &GitHubClient,
    repo: &str,
    url: &str,
    limit: u64,
    known: &Validators,
) -> Result<Body> {
    let connection = client.connection();
    let web_root = connection.base_url();
    let hosts = Hosts {
        own: [web_root, client.rest_url()]
            .into_iter()
            .filter_map(|root| Url::parse(root).ok())
            .collect(),
        api: Url::parse(client.rest_url()).ok(),
        token: connection.token().map(str::to_owned),
    };
    if hosts.token.is_some() {
        if let Some(api) = contents_api(web_root, client.rest_url(), url) {
            return image::fetch(&api, RAW_MEDIA_TYPE, limit, known, &hosts).await;
        }
        if is_attachment(web_root, url) {
            return attachment(client, &hosts, repo, url, limit, known).await;
        }
    }
    image::fetch(url, image::IMAGE_ACCEPT, limit, known, &hosts).await
}

/// An attachment through the address GitHub signs for the token's user. A refused render means the
/// token does not reach the repository. When GitHub leaves the address as written (the token does
/// not reach the attachment), it loads as written, and GitHub's 404 for what the viewer may not
/// see means the image is private.
async fn attachment(
    client: &GitHubClient,
    hosts: &Hosts,
    repo: &str,
    url: &str,
    limit: u64,
    known: &Validators,
) -> Result<Body> {
    let repo = RepoKey::parse(repo)?;
    // The address is a parsed URL: quotes and angle brackets in it are percent-encoded.
    let text = format!("<img src=\"{url}\">");
    let request = client.rest(Method::POST, "/markdown").json(
        &json!({ "text": text, "mode": "gfm", "context": format!("{}/{}", repo.owner, repo.name) }),
    );
    let rendered = match http::fetch(request).await {
        Ok(rendered) => rendered,
        Err(ProviderError::Status { status: 401 | 403, .. }) => return Ok(Body::Private),
        Err(error) => return Err(error),
    };
    match first_image_source(&String::from_utf8_lossy(&rendered.bytes))
        .filter(|source| source != url)
    {
        Some(signed) => image::fetch(&signed, image::IMAGE_ACCEPT, limit, known, hosts).await,
        None => match image::fetch(url, image::IMAGE_ACCEPT, limit, known, hosts).await {
            Err(ProviderError::Status { status: 404, .. }) => Ok(Body::Private),
            other => other,
        },
    }
}

/// `{web}/user-attachments/assets/{id}` and `{web}/{owner}/{repo}/assets/{user}/{id}`; attached
/// files (`/user-attachments/files/…`) are no images and stay as they are.
fn is_attachment(web_root: &str, url: &str) -> bool {
    let (Ok(parsed), Ok(web)) = (Url::parse(url), Url::parse(web_root)) else {
        return false;
    };
    if !image::is_same_origin(&parsed, &web) {
        return false;
    }
    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
        .unwrap_or_default();
    match segments.as_slice() {
        ["user-attachments", "assets", id] => !id.is_empty(),
        [_, _, "assets", user, id] => user.chars().all(|ch| ch.is_ascii_digit()) && !id.is_empty(),
        _ => false,
    }
}

/// The `src` of the first `<img>` in rendered HTML, its entities decoded.
fn first_image_source(html: &str) -> Option<String> {
    let (_, tag) = html.split_once("<img")?;
    let (tag, _) = tag.split_once('>')?;
    let (_, value) = tag.split_once(" src=\"")?;
    let (value, _) = value.split_once('"')?;
    let decoded = value
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    Some(decoded)
}

/// `{web}/{owner}/{repo}/blob|raw/{ref}/{path}` (and on github.com `raw.githubusercontent.com/
/// {owner}/{repo}/{ref}/{path}`) as `{api}/repos/{owner}/{repo}/contents/{path}?ref={ref}`. A
/// branch name with a slash is ambiguous in these URLs; the first segment counts as the ref.
fn contents_api(web_root: &str, rest_root: &str, url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let web = Url::parse(web_root).ok()?;
    let segments: Vec<&str> =
        parsed.path_segments()?.filter(|segment| !segment.is_empty()).collect();
    let (owner, repo, reference, path) = if image::is_same_origin(&parsed, &web) {
        match segments.as_slice() {
            [owner, repo, "blob" | "raw", reference, path @ ..] if !path.is_empty() => {
                (*owner, *repo, *reference, path.join("/"))
            }
            _ => return None,
        }
    } else if parsed.host_str() == Some(RAW_HOST) && rest_root == "https://api.github.com" {
        match segments.as_slice() {
            [owner, repo, reference, path @ ..] if !path.is_empty() => {
                (*owner, *repo, *reference, path.join("/"))
            }
            _ => return None,
        }
    } else {
        return None;
    };
    Some(format!("{rest_root}/repos/{owner}/{repo}/contents/{path}?ref={reference}"))
}

#[cfg(test)]
mod tests {
    use reqwest::Url;
    use review_core::http::image::Authorize as _;

    use super::{Hosts, contents_api, first_image_source, is_attachment};

    #[test]
    fn repository_files_on_the_web_read_through_the_contents_api() {
        let api = |url: &str| contents_api("https://github.com", "https://api.github.com", url);
        assert_eq!(
            api("https://github.com/cli/cli/blob/abc123/docs/img/a%20b.png?raw=true").as_deref(),
            Some("https://api.github.com/repos/cli/cli/contents/docs/img/a%20b.png?ref=abc123")
        );
        assert_eq!(
            api("https://raw.githubusercontent.com/cli/cli/main/logo.svg").as_deref(),
            Some("https://api.github.com/repos/cli/cli/contents/logo.svg?ref=main")
        );
        assert_eq!(
            api("https://github.com/user-attachments/assets/0d6b2a3c-1111"),
            None,
            "attachments stay as they are"
        );
        assert_eq!(api("https://camo.githubusercontent.com/abc/def"), None);
        let enterprise = contents_api(
            "https://git.example.com",
            "https://git.example.com/api/v3",
            "https://git.example.com/o/r/raw/main/a.png",
        );
        assert_eq!(
            enterprise.as_deref(),
            Some("https://git.example.com/api/v3/repos/o/r/contents/a.png?ref=main")
        );
    }

    #[test]
    fn attachments_are_recognized_on_the_web_host_only() {
        let web = "https://github.com";
        assert!(is_attachment(
            web,
            "https://github.com/user-attachments/assets/e7d69315-2274-4873-a343-7c087c9f8543"
        ));
        assert!(is_attachment(
            web,
            "https://github.com/o/r/assets/64602043/0b3ca59e-f183-4ef6-b83f-9c63f550e50d"
        ));
        assert!(!is_attachment(web, "https://github.com/user-attachments/files/123/report.pdf"));
        assert!(!is_attachment(web, "https://github.com/o/r/blob/main/assets/a.png"));
        assert!(!is_attachment(web, "https://evil.example.com/user-attachments/assets/e7d69315"));
        assert!(is_attachment(
            "https://git.example.com",
            "https://git.example.com/user-attachments/assets/1b2c"
        ));
    }

    /// The shape `POST /markdown` renders an attachment in (github.com, October 2026).
    #[test]
    fn reads_the_signed_source_of_the_rendered_image() {
        let html = r#"<p dir="auto"><a target="_blank" rel="noopener noreferrer" href="https://private-user-images.githubusercontent.com/1/2-e7d6.png?jwt=a.b.c"><img src="https://private-user-images.githubusercontent.com/1/2-e7d6.png?jwt=a.b.c&amp;x=1" alt="shot" style="max-width: 100%;"></a></p>"#;
        assert_eq!(
            first_image_source(html).as_deref(),
            Some("https://private-user-images.githubusercontent.com/1/2-e7d6.png?jwt=a.b.c&x=1")
        );
        assert_eq!(first_image_source("<p>no image</p>"), None);
    }

    #[test]
    fn the_token_goes_to_the_api_host_only() {
        let hosts = Hosts {
            own: vec![
                Url::parse("https://github.com").unwrap(),
                Url::parse("https://api.github.com").unwrap(),
            ],
            api: Url::parse("https://api.github.com").ok(),
            token: Some("token".into()),
        };
        let carries = |url: &str| {
            let request = hosts
                .authorize(&Url::parse(url).unwrap(), reqwest::Client::new().get(url))
                .build()
                .unwrap();
            request.headers().contains_key(reqwest::header::AUTHORIZATION)
        };
        assert!(carries("https://api.github.com/repos/o/r/contents/a.png"));
        assert!(
            !carries("https://github.com/user-attachments/assets/1b2c"),
            "github.com takes no tokens"
        );
        assert!(
            !carries("https://private-user-images.githubusercontent.com/1/2-a.png?jwt=x"),
            "signed addresses go out without it"
        );
        assert!(hosts.is_own(&Url::parse("https://github.com/login").unwrap()));
        assert!(
            !hosts.is_own(
                &Url::parse("https://private-user-images.githubusercontent.com/1").unwrap()
            )
        );
    }
}
