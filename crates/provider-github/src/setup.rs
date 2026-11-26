//! Connection setup for GitHub and GitHub Enterprise Server: the token recipe, read-only access
//! probes, organization discovery and explanations for denied requests.
//!
//! Classic and OAuth tokens (including the GitHub CLI's) report their scopes in `X-OAuth-Scopes`
//! on every response, so one `GET /user` answers everything. Fine-grained tokens report nothing;
//! their read permissions are probed with `GET` requests on one repository the token sees, and
//! write permissions stay unverified because only a write could prove them.

use reqwest::{Method, Response};
use review_core::setup::{
    Access, AccessProblem, AccessReport, Feature, FeatureCheck, ScopeCandidate, ScopeKind,
    TOKEN_DESCRIPTION, TOKEN_NAME, TokenGuide, TokenRecipe, TokenSetting,
};
use review_core::{Connection, ProviderError, Result, http};

use crate::client::GitHubClient;
use crate::dto::repository::RepositoryDto;
use crate::dto::user::UserDto;
use crate::mapping::identity;
use crate::owners;
use crate::repo_key::RepoKey;
use crate::sso::{self, Blocked};

const PUBLIC_WEB_ROOT: &str = "https://github.com";
const CLASSIC_SCOPES: &str = "repo,read:org";
const FINE_GRAINED_PERMISSIONS: &str =
    "pull_requests=write&contents=write&issues=write&actions=read&statuses=read";
const FINE_GRAINED_EXPIRY_DAYS: u32 = 90;

/// Where to create a token on this host, and with which settings.
pub fn token_guide(base_url: &str, owner: Option<&str>) -> TokenGuide {
    let base = base_url.trim_end_matches('/');
    let is_public = base.eq_ignore_ascii_case(PUBLIC_WEB_ROOT);
    TokenGuide {
        recommended: classic_recipe(base),
        alternative: Some(fine_grained_recipe(base, owner, is_public)),
    }
}

/// The classic token page, pre-filled with the recommended scopes.
pub fn classic_token_url(base_url: &str) -> String {
    format!(
        "{}/settings/tokens/new?scopes={CLASSIC_SCOPES}&description={}",
        base_url.trim_end_matches('/'),
        urlencoding::encode(TOKEN_NAME)
    )
}

fn classic_recipe(base: &str) -> TokenRecipe {
    TokenRecipe {
        title: "Classic personal access token".to_owned(),
        create_url: classic_token_url(base),
        is_prefilled: true,
        summary: "One token for every repository and organization you can access. It covers \
                  reviews, comments, labels, merging, Actions logs and checks from other apps."
            .to_owned(),
        settings: vec![
            TokenSetting::new(
                "repo",
                "Read and review pull requests, comment, edit labels, merge, read pipelines and \
                 logs, and fetch code over git, in private and public repositories.",
            ),
            TokenSetting::new(
                "read:org",
                "List your organizations, including private memberships.",
            ),
        ],
        caveats: vec![
            "Organizations with SAML single sign-on: after creating the token, choose \
             Configure SSO next to it and authorize each organization."
                .to_owned(),
            "Pick an expiration you are comfortable with; the app tells you when the token stops working."
                .to_owned(),
        ],
    }
}

fn fine_grained_recipe(base: &str, owner: Option<&str>, is_public: bool) -> TokenRecipe {
    let target = owner
        .map(str::trim)
        .filter(|owner| !owner.is_empty())
        .map_or_else(String::new, |owner| format!("&target_name={}", urlencoding::encode(owner)));
    let url = format!(
        "{base}/settings/personal-access-tokens/new?name={}&description={}&expires_in={FINE_GRAINED_EXPIRY_DAYS}&{FINE_GRAINED_PERMISSIONS}{target}",
        urlencoding::encode(TOKEN_NAME),
        urlencoding::encode(TOKEN_DESCRIPTION),
    );
    TokenRecipe {
        title: "Fine-grained personal access token".to_owned(),
        create_url: url,
        // URL templates are documented for github.com; older Enterprise Server versions ignore them.
        is_prefilled: is_public,
        summary: "Least privilege: limited to one owner and the repositories you select."
            .to_owned(),
        settings: vec![
            TokenSetting::new(
                "Repository access: All repositories",
                "Or select the repositories you review.",
            ),
            TokenSetting::new(
                "Pull requests: Read and write",
                "Read pull requests, approve, request changes, comment and resolve threads.",
            ),
            TokenSetting::new(
                "Contents: Read and write",
                "Fetch code over git and merge pull requests.",
            ),
            TokenSetting::new("Issues: Read and write", "Edit labels."),
            TokenSetting::new("Actions: Read", "Workflow runs, jobs and logs."),
            TokenSetting::new("Commit statuses: Read", "Status checks of pull requests."),
        ],
        caveats: vec![
            "One owner per token: create one token, and one connection, per organization."
                .to_owned(),
            "Fine-grained tokens can't read check runs, so checks from apps other than Actions are missing."
                .to_owned(),
            "If the organization requires approval, the token reads only public repositories until an owner approves it."
                .to_owned(),
        ],
    }
}

/// Signs in with `GET /user` and works out what the token can do. Only `GET` requests.
pub(crate) async fn inspect(client: &GitHubClient) -> Result<AccessReport> {
    client.require_token("Checking the token")?;
    let response = http::send(client.rest(Method::GET, "/user")).await?;
    let scopes = header(&response, "x-oauth-scopes").map(|value| parse_scopes(&value));
    let expires_at = header(&response, "github-authentication-token-expiration")
        .map(|value| value.chars().take(10).collect());
    let user: UserDto = decode(response).await?;
    let token = client.connection().token().unwrap_or_default();
    let report = if let Some(scopes) = scopes {
        AccessReport {
            user: identity::user(user),
            token_kind: scoped_token_kind(token).to_owned(),
            checks: scoped_checks(&scopes),
            notes: scoped_notes(&scopes),
            scopes,
            expires_at,
        }
    } else {
        // Five probes run at once; boxed, so their state does not inflate every caller's future.
        let (checks, notes) = Box::pin(probe_fine_grained(client)).await?;
        AccessReport {
            user: identity::user(user),
            token_kind: "Fine-grained personal access token".to_owned(),
            scopes: Vec::new(),
            expires_at,
            checks,
            notes,
        }
    };
    Ok(report)
}

fn scoped_token_kind(token: &str) -> &'static str {
    if token.starts_with("gho_") {
        "GitHub CLI or OAuth app token"
    } else {
        "Classic personal access token"
    }
}

fn parse_scopes(header: &str) -> Vec<String> {
    header.split(',').map(str::trim).filter(|scope| !scope.is_empty()).map(str::to_owned).collect()
}

/// Classic and OAuth tokens: the scopes decide everything.
fn scoped_checks(scopes: &[String]) -> Vec<FeatureCheck> {
    let has = |scope: &str| scopes.iter().any(|granted| granted == scope);
    let has_repo = has("repo");
    let has_public_repo = has("public_repo");
    Feature::ALL
        .iter()
        .map(|feature| {
            let (access, detail) = match (feature, has_repo, has_public_repo) {
                (_, true, _) => (Access::Granted, "Granted by the repo scope."),
                (
                    Feature::Vote | Feature::Comment | Feature::Labels | Feature::Merge,
                    false,
                    false,
                ) => (
                    Access::Missing,
                    "Needs the repo scope (or public_repo for public repositories).",
                ),
                (_, false, _) => (
                    Access::Limited,
                    "Public repositories only. Add the repo scope for private repositories.",
                ),
            };
            FeatureCheck::new(*feature, access, detail)
        })
        .collect()
}

fn scoped_notes(scopes: &[String]) -> Vec<String> {
    let mut notes = Vec::new();
    let has_org_read =
        scopes.iter().any(|scope| matches!(scope.as_str(), "read:org" | "write:org" | "admin:org"));
    if !has_org_read {
        notes.push(
            "Without read:org, organizations where your membership is private are not listed."
                .to_owned(),
        );
    }
    notes
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Probe {
    Allowed,
    Denied,
    /// The request failed for another reason (empty repository, no such ref).
    Inconclusive,
}

async fn probe(client: &GitHubClient, path: String) -> Result<Probe> {
    match http::send(client.rest(Method::GET, &path).query(&[("per_page", "1")])).await {
        Ok(_) => Ok(Probe::Allowed),
        Err(ProviderError::Status { status: 403, .. }) => Ok(Probe::Denied),
        Err(ProviderError::Status { status: 401, path, body }) => {
            Err(ProviderError::Status { status: 401, path, body })
        }
        Err(ProviderError::Status { .. }) => Ok(Probe::Inconclusive),
        Err(error) => Err(error),
    }
}

/// Read probes on the most recently pushed repository the token sees.
async fn probe_fine_grained(client: &GitHubClient) -> Result<(Vec<FeatureCheck>, Vec<String>)> {
    let repositories: Vec<RepositoryDto> = http::json(
        client.rest(Method::GET, "/user/repos").query(&[("per_page", "1"), ("sort", "pushed")]),
    )
    .await?;
    let Some(sample) = repositories.into_iter().next() else {
        return Ok((no_repository_checks(), Vec::new()));
    };
    let repo = RepoKey::parse(&sample.full_name)?;
    let base = repo.api_path();
    let branch = sample.default_branch.unwrap_or_else(|| "HEAD".to_owned());
    let commit = format!("{base}/commits/{}", urlencoding::encode(&branch));
    let (pulls, contents, actions, statuses, check_runs) = tokio::try_join!(
        probe(client, format!("{base}/pulls")),
        probe(client, format!("{base}/commits")),
        probe(client, format!("{base}/actions/runs")),
        probe(client, format!("{commit}/status")),
        probe(client, format!("{commit}/check-runs")),
    )?;
    let on = &sample.full_name;
    let checks = vec![
        read_check(Feature::ReadPullRequests, pulls, "Pull requests: Read", on),
        unverified(Feature::Vote, "Pull requests: Read and write"),
        unverified(Feature::Comment, "Pull requests: Read and write"),
        unverified(Feature::Labels, "Issues: Read and write"),
        unverified(Feature::Merge, "Contents: Read and write and Pull requests: Read and write"),
        pipelines_check(actions, statuses, check_runs, on),
        read_check(Feature::GitClone, contents, "Contents: Read", on),
    ];
    let notes = vec![
        "Fine-grained tokens reach one owner. Add another connection for each further organization."
            .to_owned(),
    ];
    Ok((checks, notes))
}

fn no_repository_checks() -> Vec<FeatureCheck> {
    Feature::ALL
        .iter()
        .map(|feature| {
            FeatureCheck::new(
                *feature,
                Access::Missing,
                "The token can't see any repository. Under Repository access, choose All \
                 repositories or select the ones you review.",
            )
        })
        .collect()
}

/// A permission that only a write request could confirm.
fn unverified(feature: Feature, permission: &str) -> FeatureCheck {
    FeatureCheck::new(
        feature,
        Access::Unverified,
        format!("Needs {permission}. Only a write could confirm it, so make sure it is selected."),
    )
}

/// A permission that a read probe on the repository `on` confirmed, denied or left open.
fn read_check(feature: Feature, result: Probe, permission: &str, on: &str) -> FeatureCheck {
    match result {
        Probe::Allowed => {
            FeatureCheck::new(feature, Access::Granted, format!("{permission} confirmed on {on}."))
        }
        Probe::Denied => {
            FeatureCheck::new(feature, Access::Missing, format!("Needs {permission}."))
        }
        Probe::Inconclusive => FeatureCheck::new(
            feature,
            Access::Unverified,
            format!("Needs {permission}. {on} could not confirm it (empty repository?)."),
        ),
    }
}

/// Pipelines need Actions and Commit statuses; check runs of other apps need a classic token.
fn pipelines_check(actions: Probe, statuses: Probe, check_runs: Probe, on: &str) -> FeatureCheck {
    match (actions, statuses, check_runs) {
        (Probe::Denied, _, _) => {
            FeatureCheck::new(Feature::Pipelines, Access::Missing, "Needs Actions: Read.")
        }
        (_, Probe::Denied, _) => {
            FeatureCheck::new(Feature::Pipelines, Access::Missing, "Needs Commit statuses: Read.")
        }
        (Probe::Allowed, Probe::Allowed, Probe::Denied) => FeatureCheck::new(
            Feature::Pipelines,
            Access::Limited,
            "Actions runs, statuses and logs work. Checks from other apps need a classic token.",
        ),
        (Probe::Allowed, Probe::Allowed, Probe::Allowed) => FeatureCheck::new(
            Feature::Pipelines,
            Access::Granted,
            format!("Actions: Read and Commit statuses: Read confirmed on {on}."),
        ),
        _ => unverified(Feature::Pipelines, "Actions: Read and Commit statuses: Read"),
    }
}

/// The signed-in account first, then its organizations: the same owners "everything" lists.
pub(crate) async fn discover(client: &GitHubClient) -> Result<Vec<ScopeCandidate>> {
    client.require_token("Listing your organizations")?;
    let reach = owners::reach(client).await?;
    let account = ScopeCandidate {
        key: reach.login.clone(),
        display_name: reach.login,
        kind: ScopeKind::Account,
    };
    let organizations = reach.organizations.into_iter().map(|organization| ScopeCandidate {
        display_name: organization.clone(),
        key: organization,
        kind: ScopeKind::Organization,
    });
    Ok(std::iter::once(account).chain(organizations).collect())
}

/// GitHub's 403 messages, mapped onto the permission the request needed.
pub(crate) fn explain(connection: &Connection, error: &ProviderError) -> Option<AccessProblem> {
    let ProviderError::Status { status: 403, path, .. } = error else {
        return review_core::setup::generic_denial(error);
    };
    let message = error.server_message().unwrap_or_default().to_lowercase();
    if message.contains("rate limit") {
        return None;
    }
    if message.contains("saml") || message.contains("single sign-on") {
        let blocked = [Blocked { login: sso::owner_of_path(path).map(str::to_owned) }];
        return Some(AccessProblem {
            status: 403,
            title: "The organization enforces SAML single sign-on and the token is not authorized for it."
                .to_owned(),
            fix: sso::fix(connection).to_owned(),
            actions: sso::actions(connection, &blocked),
        });
    }
    if message.contains("personal access token") || message.contains("not accessible") {
        let permission = permission_for_path(path);
        return Some(AccessProblem {
            status: 403,
            title: format!("The token lacks {permission} for this request."),
            fix: format!(
                "Edit the fine-grained token and grant {permission}, or use a classic token with the repo scope."
            ),
            actions: Vec::new(),
        });
    }
    review_core::setup::generic_denial(error)
}

/// The fine-grained permission a REST path needs.
fn permission_for_path(path: &str) -> &'static str {
    let rules: [(&str, &str); 7] = [
        ("/check-runs", "check run access (classic tokens only)"),
        ("/actions/", "Actions: Read"),
        ("/status", "Commit statuses: Read"),
        ("/labels", "Issues: Read and write"),
        ("/merge", "Contents: Read and write"),
        ("/graphql", "Pull requests: Read and write"),
        ("/pulls", "Pull requests: Read and write"),
    ];
    rules
        .iter()
        .find(|(fragment, _)| path.contains(fragment))
        .map_or("the permission for this action", |(_, permission)| permission)
}

fn header(response: &Response, name: &str) -> Option<String> {
    response.headers().get(name)?.to_str().ok().map(str::to_owned)
}

async fn decode<T: serde::de::DeserializeOwned>(response: Response) -> Result<T> {
    let bytes = response.bytes().await?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use review_core::{Connection, Provider, ProviderKind};
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::GitHubProvider;

    const TOKEN: &str = "test-token";

    async fn provider() -> (MockServer, GitHubProvider) {
        let server = MockServer::start().await;
        let connection = Connection {
            id: "mock".to_owned(),
            name: "Mock".to_owned(),
            kind: ProviderKind::GitHub,
            base_url: server.uri(),
            organizations: Vec::new(),
            token: Some(TOKEN.to_owned()),
        };
        (server, GitHubProvider::new(connection, reqwest::Client::new()))
    }

    fn user_body() -> serde_json::Value {
        json!({"id": 1, "login": "octocat", "name": "The Octocat", "avatar_url": null})
    }

    /// Only `GET` is mounted; any other method would answer 404 and fail the check.
    fn get(route: &str) -> wiremock::MockBuilder {
        Mock::given(method("GET"))
            .and(path(format!("/api/v3{route}")))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
    }

    #[tokio::test]
    async fn classic_repo_scope_grants_everything() {
        let (server, provider) = provider().await;
        get("/user")
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-oauth-scopes", "repo, read:org, gist")
                    .insert_header(
                        "github-authentication-token-expiration",
                        "2026-12-31 10:00:00 UTC",
                    )
                    .set_body_json(user_body()),
            )
            .expect(1)
            .mount(&server)
            .await;
        let report = provider.inspect_access().await.expect("report");
        assert_eq!(report.user.login.as_deref(), Some("octocat"));
        assert_eq!(report.token_kind, "Classic personal access token");
        assert_eq!(report.expires_at.as_deref(), Some("2026-12-31"));
        assert_eq!(report.scopes, ["repo", "read:org", "gist"]);
        assert!(report.checks.iter().all(|check| check.access == Access::Granted));
        assert!(report.notes.is_empty(), "{:?}", report.notes);
    }

    #[tokio::test]
    async fn classic_token_without_repo_scope_cannot_write() {
        let (server, provider) = provider().await;
        get("/user")
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-oauth-scopes", "")
                    .set_body_json(user_body()),
            )
            .mount(&server)
            .await;
        let report = provider.inspect_access().await.expect("report");
        let access = |feature| report.check(feature).map(|check| check.access);
        assert_eq!(access(Feature::ReadPullRequests), Some(Access::Limited));
        assert_eq!(access(Feature::Vote), Some(Access::Missing));
        assert_eq!(access(Feature::Merge), Some(Access::Missing));
        assert_eq!(report.notes.len(), 1, "read:org hint");
    }

    #[tokio::test]
    async fn fine_grained_token_is_probed_with_reads_only() {
        let (server, provider) = provider().await;
        get("/user")
            .respond_with(ResponseTemplate::new(200).set_body_json(user_body()))
            .mount(&server)
            .await;
        get("/user/repos")
            .and(query_param("per_page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "full_name": "octo/app", "name": "app", "html_url": "https://x/octo/app", "default_branch": "main"
            }])))
            .mount(&server)
            .await;
        let ok = || ResponseTemplate::new(200).set_body_json(json!([]));
        let denied = || {
            ResponseTemplate::new(403).set_body_json(
                json!({"message": "Resource not accessible by personal access token"}),
            )
        };
        get("/repos/octo/app/pulls").respond_with(ok()).mount(&server).await;
        get("/repos/octo/app/commits").respond_with(ok()).mount(&server).await;
        get("/repos/octo/app/actions/runs").respond_with(ok()).mount(&server).await;
        get("/repos/octo/app/commits/main/status").respond_with(ok()).mount(&server).await;
        get("/repos/octo/app/commits/main/check-runs").respond_with(denied()).mount(&server).await;

        let report = provider.inspect_access().await.expect("report");
        let access = |feature| report.check(feature).map(|check| check.access);
        assert_eq!(report.token_kind, "Fine-grained personal access token");
        assert_eq!(access(Feature::ReadPullRequests), Some(Access::Granted));
        assert_eq!(access(Feature::GitClone), Some(Access::Granted));
        assert_eq!(access(Feature::Pipelines), Some(Access::Limited));
        assert_eq!(access(Feature::Vote), Some(Access::Unverified));
        let requests = server.received_requests().await.expect("recorded");
        assert!(requests.iter().all(|request| request.method.as_str() == "GET"));
    }

    #[tokio::test]
    async fn fine_grained_token_without_repositories_is_all_missing() {
        let (server, provider) = provider().await;
        get("/user")
            .respond_with(ResponseTemplate::new(200).set_body_json(user_body()))
            .mount(&server)
            .await;
        get("/user/repos")
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        let report = provider.inspect_access().await.expect("report");
        assert!(report.checks.iter().all(|check| check.access == Access::Missing));
        assert!(!report.is_usable());
    }

    #[tokio::test]
    async fn rejected_token_surfaces_as_401() {
        let (server, provider) = provider().await;
        get("/user")
            .respond_with(
                ResponseTemplate::new(401).set_body_json(json!({"message": "Bad credentials"})),
            )
            .mount(&server)
            .await;
        let error = provider.inspect_access().await.expect_err("rejected");
        assert_eq!(error.status(), Some(401));
        assert_eq!(provider.explain_denial(&error).map(|problem| problem.status), Some(401));
    }

    #[tokio::test]
    async fn discovers_account_and_organizations() {
        let (server, provider) = provider().await;
        get("/user")
            .respond_with(ResponseTemplate::new(200).set_body_json(user_body()))
            .mount(&server)
            .await;
        get("/user/orgs")
            .and(query_param("per_page", "100"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([{"login": "github"}, {"login": "cli"}])),
            )
            .mount(&server)
            .await;
        let scopes = provider.discover_scopes().await.expect("scopes");
        let keys: Vec<(&str, ScopeKind)> =
            scopes.iter().map(|scope| (scope.key.as_str(), scope.kind)).collect();
        assert_eq!(
            keys,
            [
                ("octocat", ScopeKind::Account),
                ("github", ScopeKind::Organization),
                ("cli", ScopeKind::Organization)
            ]
        );
    }

    #[test]
    fn guide_prefills_both_token_pages() {
        let guide = token_guide("https://github.com/", Some("acme"));
        assert_eq!(
            guide.recommended.create_url,
            "https://github.com/settings/tokens/new?scopes=repo,read:org&description=Code%20View"
        );
        let alternative = guide.alternative.expect("fine-grained");
        assert!(alternative.is_prefilled);
        assert!(alternative.create_url.starts_with(
            "https://github.com/settings/personal-access-tokens/new?name=Code%20View&description="
        ));
        assert!(alternative.create_url.contains(
            "&pull_requests=write&contents=write&issues=write&actions=read&statuses=read"
        ));
        assert!(alternative.create_url.ends_with("&target_name=acme"));
        let enterprise = token_guide("https://git.example.com", None);
        assert!(
            enterprise
                .recommended
                .create_url
                .starts_with("https://git.example.com/settings/tokens/new?")
        );
        assert!(!enterprise.alternative.expect("fine-grained").is_prefilled);
    }

    #[test]
    fn explains_fine_grained_and_sso_denials() {
        let denied = |path: &str, message: &str| ProviderError::Status {
            status: 403,
            path: path.to_owned(),
            body: json!({ "message": message }).to_string(),
        };
        let connection = Connection {
            id: "gh".to_owned(),
            name: "GitHub".to_owned(),
            kind: ProviderKind::GitHub,
            base_url: "https://github.com".to_owned(),
            organizations: Vec::new(),
            token: Some("ghp_classic".to_owned()),
        };
        let explain = |error: &ProviderError| explain(&connection, error);
        let problem = explain(&denied(
            "/repos/o/r/actions/jobs/1/logs",
            "Resource not accessible by personal access token",
        ))
        .expect("explained");
        assert_eq!(problem.title, "The token lacks Actions: Read for this request.");
        let sso = explain(&denied(
            "/repos/o/r/pulls",
            "Resource protected by organization SAML enforcement. You must grant your Personal Access token access to this organization.",
        ))
        .expect("explained");
        assert!(sso.fix.contains("Configure SSO"));
        assert_eq!(sso.actions[0].label, "Authorize for o");
        assert_eq!(sso.actions[0].url, "https://github.com/settings/tokens");
        assert!(explain(&denied("/repos/o/r", "API rate limit exceeded for user")).is_none());
    }
}
