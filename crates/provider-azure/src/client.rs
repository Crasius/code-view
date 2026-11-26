//! URL building per organization/project/repository, authentication and request plumbing.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::{Method, RequestBuilder, Response};
use review_core::{Connection, ProviderError, Result, http};
use serde::Serialize;
use serde::de::DeserializeOwned;

const API_VERSION: &str = "7.1";
/// `connectionData` only exists as a preview resource.
pub(crate) const API_VERSION_PREVIEW: &str = "7.1-preview";
const SERVICES_HOST: &str = "https://dev.azure.com";
const SERVICES_IDENTITY_HOST: &str = "https://vssps.dev.azure.com";

/// A parsed repository key `organization/project/repository` (raw names, not encoded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RepoKey<'a> {
    pub organization: &'a str,
    pub project: &'a str,
    pub repository: &'a str,
}

impl<'a> RepoKey<'a> {
    pub fn parse(key: &'a str) -> Result<Self> {
        let mut parts = key.split('/');
        let (Some(organization), Some(project), Some(repository), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid_key(key));
        };
        if organization.is_empty() || project.is_empty() || repository.is_empty() {
            return Err(invalid_key(key));
        }
        Ok(RepoKey { organization, project, repository })
    }

    pub fn format(organization: &str, project: &str, repository: &str) -> String {
        format!("{organization}/{project}/{repository}")
    }
}

fn invalid_key(key: &str) -> ProviderError {
    ProviderError::Local(format!(
        "'{key}' is not an Azure DevOps repository key (organization/project/repository)."
    ))
}

fn encode(segment: &str) -> String {
    urlencoding::encode(segment).into_owned()
}

/// One connection's HTTP access: every request carries `api-version` and, with a token, Basic auth.
pub(crate) struct Client {
    connection: Connection,
    http: reqwest::Client,
}

impl Client {
    pub fn new(connection: Connection, http: reqwest::Client) -> Self {
        Client { connection, http }
    }

    pub fn organizations(&self) -> Result<&[String]> {
        if self.connection.organizations.is_empty() {
            return Err(ProviderError::Unsupported(
                "This Azure DevOps connection lists no organization (or collection).".to_owned(),
            ));
        }
        Ok(&self.connection.organizations)
    }

    pub fn has_token(&self) -> bool {
        self.connection.token().is_some()
    }

    pub fn require_token(&self, action: &str) -> Result<()> {
        if self.has_token() { Ok(()) } else { Err(ProviderError::requires_token(action)) }
    }

    /// `Basic base64(":" + PAT)`, the header git and the REST API both accept.
    pub fn authorization_header(&self) -> Option<String> {
        self.connection
            .token()
            .map(|token| format!("Basic {}", STANDARD.encode(format!(":{token}"))))
    }

    /// The server root (`https://dev.azure.com` or the Azure DevOps Server address).
    pub fn base_url(&self) -> &str {
        self.connection.base_url()
    }

    /// Azure DevOps Services (dev.azure.com) as opposed to an on-premises Azure DevOps Server.
    pub fn is_services(&self) -> bool {
        self.connection.base_url().eq_ignore_ascii_case(SERVICES_HOST)
    }

    /// `{base}/{org}` — the organization (Services) or collection (Server) web root.
    pub fn organization_root(&self, organization: &str) -> String {
        format!("{}/{}", self.connection.base_url(), encode(organization))
    }

    pub fn project_root(&self, repo: RepoKey<'_>) -> String {
        format!("{}/{}", self.organization_root(repo.organization), encode(repo.project))
    }

    pub fn organization_api(&self, organization: &str, path: &str) -> String {
        format!("{}/_apis/{path}", self.organization_root(organization))
    }

    pub fn project_api(&self, repo: RepoKey<'_>, path: &str) -> String {
        format!("{}/_apis/{path}", self.project_root(repo))
    }

    /// `Repositories - Get`: the repository itself (by name).
    pub fn repository_resource_api(&self, repo: RepoKey<'_>) -> String {
        self.project_api(repo, &format!("git/repositories/{}", encode(repo.repository)))
    }

    /// The branch policies page of `branch` in the repository with GUID `repository_id`.
    pub fn branch_policies_web_url(
        &self,
        repo: RepoKey<'_>,
        repository_id: &str,
        branch: &str,
    ) -> String {
        format!(
            "{}/_settings/repositories?repo={}&_a=policiesMid&refs={}",
            self.project_root(repo),
            encode(repository_id),
            encode(&format!("refs/heads/{branch}"))
        )
    }

    /// The repository's branches page, where each branch's menu leads to its policies.
    pub fn branches_web_url(&self, repo: RepoKey<'_>) -> String {
        format!("{}/branches", self.clone_url(repo))
    }

    pub fn repository_api(&self, repo: RepoKey<'_>, path: &str) -> String {
        self.project_api(repo, &format!("git/repositories/{}/{path}", encode(repo.repository)))
    }

    pub fn pull_request_api(&self, repo: RepoKey<'_>, number: u64, path: &str) -> String {
        let suffix = if path.is_empty() { String::new() } else { format!("/{path}") };
        self.repository_api(repo, &format!("pullrequests/{number}{suffix}"))
    }

    /// Identity search root: `vssps.dev.azure.com/{org}` on Services, the collection on Server.
    pub fn identities_api(&self, organization: &str) -> String {
        if self.is_services() {
            format!("{SERVICES_IDENTITY_HOST}/{}/_apis/identities", encode(organization))
        } else {
            self.organization_api(organization, "identities")
        }
    }

    pub fn clone_url(&self, repo: RepoKey<'_>) -> String {
        format!("{}/_git/{}", self.project_root(repo), encode(repo.repository))
    }

    pub fn pull_request_web_url(&self, repo: RepoKey<'_>, number: u64) -> String {
        format!("{}/pullrequest/{number}", self.clone_url(repo))
    }

    pub fn request(&self, method: Method, url: &str) -> RequestBuilder {
        self.request_versioned(method, url, API_VERSION)
    }

    pub fn request_versioned(
        &self,
        method: Method,
        url: &str,
        api_version: &str,
    ) -> RequestBuilder {
        let request = self.http.request(method, url).query(&[("api-version", api_version)]);
        match self.authorization_header() {
            Some(header) => request.header(reqwest::header::AUTHORIZATION, header),
            None => request,
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, url: &str, query: &[(&str, String)]) -> Result<T> {
        self.json(self.request(Method::GET, url).query(query)).await
    }

    /// Like [`Client::get`], but an empty or `null` body yields `T::default()`
    /// (timelines of builds that have not started yet).
    pub async fn get_or_default<T: DeserializeOwned + Default>(&self, url: &str) -> Result<T> {
        let fetched = self.fetch(self.request(Method::GET, url)).await?;
        let bytes = &fetched.bytes;
        if bytes.iter().all(u8::is_ascii_whitespace) || bytes.as_ref() == b"null" {
            return Ok(T::default());
        }
        fetched.json()
    }

    pub async fn send_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        method: Method,
        url: &str,
        body: &B,
    ) -> Result<T> {
        self.json(self.request(method, url).json(body)).await
    }

    pub async fn send_unit<B: Serialize + ?Sized>(
        &self,
        method: Method,
        url: &str,
        body: Option<&B>,
    ) -> Result<()> {
        let request = self.request(method, url);
        let request = match body {
            Some(body) => request.json(body),
            None => request,
        };
        self.send(request).await.map(drop)
    }

    pub async fn json<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T> {
        self.fetch(request).await?.json()
    }

    /// Azure DevOps Services answers anonymous calls to non-public resources with a 302 to its
    /// sign-in page, which the shared client follows; that lands on a 200 HTML page. Report it as 401.
    pub async fn send(&self, request: RequestBuilder) -> Result<Response> {
        let (http_client, request) = request.build_split();
        let request = request?;
        let path = request.url().path().to_owned();
        let response = http::send(RequestBuilder::from_parts(http_client, request)).await?;
        check_sign_in(response.url(), path)?;
        Ok(response)
    }

    /// Like [`Client::send`] with the body read: identical GETs in flight share one request and
    /// repeated ones revalidate with `If-None-Match` where the server sent an `ETag`.
    async fn fetch(&self, request: RequestBuilder) -> Result<http::Fetched> {
        let (http_client, request) = request.build_split();
        let request = request?;
        let path = request.url().path().to_owned();
        let fetched = http::fetch(RequestBuilder::from_parts(http_client, request)).await?;
        check_sign_in(&fetched.url, path)?;
        Ok(fetched)
    }
}

fn check_sign_in(url: &reqwest::Url, path: String) -> Result<()> {
    if is_sign_in_page(url) {
        return Err(ProviderError::Status {
            status: 401,
            path,
            body: "Azure DevOps requires sign-in for this resource.".to_owned(),
        });
    }
    Ok(())
}

fn is_sign_in_page(url: &reqwest::Url) -> bool {
    url.path().to_ascii_lowercase().contains("/_signin")
}

#[cfg(test)]
mod tests {
    use review_core::ProviderKind;

    use super::*;

    fn client(base: &str, token: Option<&str>) -> Client {
        let connection = Connection {
            id: "c".to_owned(),
            name: "c".to_owned(),
            kind: ProviderKind::AzureDevOps,
            base_url: base.to_owned(),
            organizations: vec!["fabrikam".to_owned()],
            token: token.map(str::to_owned),
        };
        Client::new(connection, reqwest::Client::new())
    }

    #[test]
    fn parses_repository_key() {
        let key = RepoKey::parse("fabrikam/My Project/web").unwrap();
        assert_eq!(
            key,
            RepoKey { organization: "fabrikam", project: "My Project", repository: "web" }
        );
        assert!(RepoKey::parse("fabrikam/web").is_err());
        assert!(RepoKey::parse("a/b/c/d").is_err());
        assert!(RepoKey::parse("a//c").is_err());
    }

    #[test]
    fn encodes_each_segment() {
        let client = client("https://dev.azure.com/", None);
        let key = RepoKey::parse("fabrikam/My Project/web app").unwrap();
        assert_eq!(
            client.pull_request_api(key, 7, "threads"),
            "https://dev.azure.com/fabrikam/My%20Project/_apis/git/repositories/web%20app/pullrequests/7/threads"
        );
        assert_eq!(
            client.pull_request_api(key, 7, ""),
            client.repository_api(key, "pullrequests/7")
        );
        assert_eq!(
            client.clone_url(key),
            "https://dev.azure.com/fabrikam/My%20Project/_git/web%20app"
        );
    }

    #[test]
    fn identities_host_depends_on_deployment() {
        assert_eq!(
            client("https://dev.azure.com", None).identities_api("fabrikam"),
            "https://vssps.dev.azure.com/fabrikam/_apis/identities"
        );
        assert_eq!(
            client("https://tfs.example.com/tfs", None).identities_api("DefaultCollection"),
            "https://tfs.example.com/tfs/DefaultCollection/_apis/identities"
        );
    }

    #[test]
    fn basic_auth_uses_empty_user() {
        assert_eq!(
            client("https://dev.azure.com", Some("pat")).authorization_header().as_deref(),
            Some("Basic OnBhdA==")
        );
        assert_eq!(client("https://dev.azure.com", Some("")).authorization_header(), None);
        assert!(client("https://dev.azure.com", None).require_token("Voting").is_err());
    }

    #[test]
    fn detects_sign_in_redirect() {
        let url = reqwest::Url::parse(
            "https://spsprodcus4.vssps.visualstudio.com/_signin?realm=dev.azure.com",
        )
        .unwrap();
        assert!(is_sign_in_page(&url));
        let url = reqwest::Url::parse("https://dev.azure.com/o/p/_apis/git/repositories").unwrap();
        assert!(!is_sign_in_page(&url));
    }
}
