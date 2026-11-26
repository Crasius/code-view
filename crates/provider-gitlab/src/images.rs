//! Images in descriptions and comments. `PRIVATE-TOKEN` goes to the GitLab host only. With a token,
//! uploads (`/group/project/uploads/<secret>/<file>`, `/-/project/<id>/uploads/...`) and raw
//! repository files (`/group/project/-/raw/<ref>/<path>`) are read through the API
//! (`GET /projects/:id/uploads/:secret/:filename`, `GET /projects/:id/repository/files/:path/raw`),
//! which accepts the token where the web routes want a browser session; GitLab before 17.4 has no
//! uploads API, so an API request that is refused or not found falls back to the web address
//! (which public projects serve to anyone; private ones answer with the sign-in page).

// RS-DONT-04: parses provider data, so indexing and narrowing casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use reqwest::{RequestBuilder, Url};
use review_core::http::image::{self, Authorize, Body, Validators};
use review_core::{ProviderError, Result};

use crate::client::ApiClient;

struct OwnHost {
    origin: Option<Url>,
    token: Option<String>,
}

impl Authorize for OwnHost {
    fn is_own(&self, url: &Url) -> bool {
        self.origin.as_ref().is_some_and(|origin| image::is_same_origin(url, origin))
    }

    fn authorize(&self, url: &Url, request: RequestBuilder) -> RequestBuilder {
        match &self.token {
            Some(token) if self.is_own(url) => request.header("PRIVATE-TOKEN", token),
            _ => request,
        }
    }
}

pub(crate) async fn fetch(
    client: &ApiClient,
    url: &str,
    limit: u64,
    known: &Validators,
) -> Result<Body> {
    let auth = OwnHost {
        origin: Url::parse(client.web_root()).ok(),
        token: client.token().map(str::to_owned),
    };
    let candidates =
        if auth.token.is_some() { api_urls(client.web_root(), url) } else { Vec::new() };
    for api in candidates {
        match image::fetch(&api, image::IMAGE_ACCEPT, limit, known, &auth).await {
            Ok(Body::Private) | Err(ProviderError::Status { status: 404, .. }) => {}
            other => return other,
        }
    }
    image::fetch(url, image::IMAGE_ACCEPT, limit, known, &auth).await
}

/// The API addresses that may serve an upload or a raw repository file on `web_root`, to try in
/// order; empty for anything else. A raw file's web path does not mark where a ref with slashes
/// (`feature/docs`) ends and the file path begins, so each split is one address, shortest ref first.
fn api_urls(web_root: &str, url: &str) -> Vec<String> {
    if let Some(upload) = upload_api_url(web_root, url) {
        return vec![upload];
    }
    raw_api_urls(web_root, url).unwrap_or_default()
}

/// The path segments of `url` below `web_root`, and the API root; `None` for another host.
fn segments_below<'a>(web_root: &str, url: &'a Url) -> Option<(Vec<&'a str>, String)> {
    let web = Url::parse(web_root).ok()?;
    if !image::is_same_origin(url, &web) {
        return None;
    }
    let base_path = web.path().trim_end_matches('/');
    let path = url.path().strip_prefix(base_path)?;
    let segments = path.split('/').filter(|segment| !segment.is_empty()).collect();
    Some((segments, format!("{}/api/v4", web_root.trim_end_matches('/'))))
}

fn upload_api_url(web_root: &str, url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let (segments, api) = segments_below(web_root, &parsed)?;
    let is_secret =
        |segment: &str| segment.len() == 32 && segment.chars().all(|ch| ch.is_ascii_hexdigit());
    let at = segments
        .windows(2)
        .position(|pair| matches!(pair, ["uploads", secret] if is_secret(secret)))?;
    let (before, [_, secret, file @ ..]) = segments.split_at_checked(at)? else {
        return None;
    };
    if file.is_empty() {
        return None;
    }
    let project = match before {
        ["-", "project", id] => (*id).to_owned(),
        [project @ .., "-"] | project => urlencoding::encode(&project.join("/")).into_owned(),
    };
    if project.is_empty() {
        return None;
    }
    Some(format!("{api}/projects/{project}/uploads/{secret}/{}", file.join("/")))
}

fn raw_api_urls(web_root: &str, url: &str) -> Option<Vec<String>> {
    let parsed = Url::parse(url).ok()?;
    let (segments, api) = segments_below(web_root, &parsed)?;
    let raw = segments.windows(2).position(|pair| pair == ["-", "raw"])?;
    let (project, [_, _, rest @ ..]) = segments.split_at_checked(raw)? else {
        return None;
    };
    if project.is_empty() {
        return None;
    }
    let project = urlencoding::encode(&project.join("/")).into_owned();
    let decode = |parts: &[&str]| Some(urlencoding::decode(&parts.join("/")).ok()?.into_owned());
    (1..rest.len())
        .map(|ref_length| {
            let (reference, file) = rest.split_at_checked(ref_length)?;
            Some(format!(
                "{api}/projects/{project}/repository/files/{}/raw?ref={}",
                urlencoding::encode(&decode(file)?),
                urlencoding::encode(&decode(reference)?)
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::api_urls;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn uploads_and_raw_files_read_through_the_api() {
        assert_eq!(
            api_urls(
                "https://gitlab.com",
                &format!("https://gitlab.com/group/sub/app/uploads/{SECRET}/shot.png")
            ),
            [format!(
                "https://gitlab.com/api/v4/projects/group%2Fsub%2Fapp/uploads/{SECRET}/shot.png"
            )]
        );
        assert_eq!(
            api_urls(
                "https://gitlab.com",
                &format!("https://gitlab.com/-/project/42/uploads/{SECRET}/a.png")
            ),
            [format!("https://gitlab.com/api/v4/projects/42/uploads/{SECRET}/a.png")]
        );
        assert_eq!(
            api_urls(
                "https://git.example.com/gitlab",
                "https://git.example.com/gitlab/g/app/-/raw/main/a%20b.png"
            ),
            [
                "https://git.example.com/gitlab/api/v4/projects/g%2Fapp/repository/files/a%20b.png/raw?ref=main"
            ]
        );
        assert_eq!(
            api_urls("https://gitlab.com", "https://img.shields.io/badge/x"),
            Vec::<String>::new(),
            "other hosts stay as they are"
        );
        assert_eq!(
            api_urls("https://gitlab.com", "https://gitlab.com/g/app/uploads/short/a.png"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_raw_file_on_a_branch_with_slashes_tries_each_split_of_ref_and_path() {
        assert_eq!(
            api_urls("https://gitlab.com", "https://gitlab.com/g/app/-/raw/feature/docs/img/a.png"),
            [
                "https://gitlab.com/api/v4/projects/g%2Fapp/repository/files/docs%2Fimg%2Fa.png/raw?ref=feature",
                "https://gitlab.com/api/v4/projects/g%2Fapp/repository/files/img%2Fa.png/raw?ref=feature%2Fdocs",
                "https://gitlab.com/api/v4/projects/g%2Fapp/repository/files/a.png/raw?ref=feature%2Fdocs%2Fimg",
            ]
        );
    }

    #[test]
    fn an_upload_in_a_group_named_uploads_reads_through_the_api() {
        assert_eq!(
            api_urls(
                "https://gitlab.com",
                &format!("https://gitlab.com/uploads/app/uploads/{SECRET}/shot.png")
            ),
            [format!("https://gitlab.com/api/v4/projects/uploads%2Fapp/uploads/{SECRET}/shot.png")]
        );
    }
}
