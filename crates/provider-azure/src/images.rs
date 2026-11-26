//! Images in descriptions and comments: pasted images are attachments behind the REST API
//! (`_apis/git/repositories/{id}/pullRequests/{n}/attachments/{name}`, `_apis/wit/attachments/{id}`),
//! which take the PAT like every other call. The PAT goes to the connection's server only; on
//! Azure DevOps Services also to the legacy addresses of its own organizations
//! (`{organization}.visualstudio.com`). Without access the server answers with its sign-in page.

use reqwest::header::AUTHORIZATION;
use reqwest::{RequestBuilder, Url};
use review_core::Result;
use review_core::http::image::{self, Authorize, Body, Validators};

use crate::client::Client;

const LEGACY_SERVICES_SUFFIX: &str = ".visualstudio.com";

struct OwnHosts {
    origin: Option<Url>,
    /// Azure DevOps Services: the organizations whose legacy addresses count as the server's.
    legacy_organizations: Vec<String>,
    authorization: Option<String>,
}

impl Authorize for OwnHosts {
    fn is_own(&self, url: &Url) -> bool {
        let is_server =
            self.origin.as_ref().is_some_and(|origin| image::is_same_origin(url, origin));
        let is_legacy = url.scheme() == "https"
            && url.port().is_none()
            && url
                .host_str()
                .and_then(|host| host.strip_suffix(LEGACY_SERVICES_SUFFIX))
                .is_some_and(|organization| {
                    self.legacy_organizations
                        .iter()
                        .any(|own| own.eq_ignore_ascii_case(organization))
                });
        is_server || is_legacy
    }

    fn authorize(&self, url: &Url, request: RequestBuilder) -> RequestBuilder {
        match &self.authorization {
            Some(authorization) if self.is_own(url) => request.header(AUTHORIZATION, authorization),
            _ => request,
        }
    }
}

pub(crate) async fn fetch(
    client: &Client,
    url: &str,
    limit: u64,
    known: &Validators,
) -> Result<Body> {
    let legacy_organizations = if client.is_services() {
        client.organizations().map(<[String]>::to_vec).unwrap_or_default()
    } else {
        Vec::new()
    };
    let auth = OwnHosts {
        origin: Url::parse(client.base_url()).ok(),
        legacy_organizations,
        authorization: client.authorization_header(),
    };
    image::fetch(url, image::IMAGE_ACCEPT, limit, known, &auth).await
}

#[cfg(test)]
mod tests {
    use reqwest::Url;
    use review_core::http::image::Authorize as _;

    use super::OwnHosts;

    #[test]
    fn the_token_stays_on_the_organizations_hosts() {
        let hosts = OwnHosts {
            origin: Url::parse("https://dev.azure.com").ok(),
            legacy_organizations: vec!["Org".into()],
            authorization: Some("Basic x".into()),
        };
        let own = |url: &str| hosts.is_own(&Url::parse(url).unwrap());
        assert!(own(
            "https://dev.azure.com/org/p/_apis/git/repositories/r/pullRequests/1/attachments/a.png"
        ));
        assert!(own("https://org.visualstudio.com/_apis/wit/attachments/1?fileName=a.png"));
        assert!(
            !own("https://other.visualstudio.com/_apis/wit/attachments/1"),
            "another organization"
        );
        assert!(!own("https://org.vsblob.visualstudio.com/a.png"), "storage behind a redirect");
        assert!(!own("https://evil.example.com/dev.azure.com/a.png"));
        assert!(!own("http://org.visualstudio.com/a.png"), "never over plain http");
        assert!(!own("https://img.shields.io/badge/a"));
    }
}
