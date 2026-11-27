//! Connection setup for Azure DevOps Services and Server: the token checklist, read-only access
//! probes, organization discovery and explanations for denied requests.
//!
//! Personal access tokens do not reveal their scopes, so reads are probed (`connectionData`,
//! repositories, pull requests, builds, identities) and write scopes stay unverified. Microsoft
//! Entra tokens (Azure CLI sign-ins, JWTs) carry the user's own permissions and need no scopes.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Method;
use review_core::setup::{
    Access, AccessProblem, AccessReport, Feature, FeatureCheck, ScopeCandidate, ScopeKind,
    TOKEN_NAME, TokenGuide, TokenRecipe, TokenSetting, generic_denial,
};
use review_core::{ProviderError, Result};
use serde::Deserialize;
use time::OffsetDateTime;
use time::macros::format_description;

use crate::client::{Client, RepoKey, is_services_host};
use crate::dto::ValueList;
use crate::dto::repository::Repository;
use crate::identity::connection_data;

/// Profile and accounts APIs of Azure DevOps Services.
const PROFILE_ROOT: &str = "https://app.vssps.visualstudio.com";

/// Where to create a token and the exact boxes to tick; Azure DevOps has no pre-fill.
pub fn token_guide(base_url: &str, organization: Option<&str>) -> TokenGuide {
    let base = base_url.trim_end_matches('/');
    let is_services = is_services_host(base);
    let create_url = token_page_url(base, organization);
    let mut settings = vec![TokenSetting::new(
        format!("Name: {TOKEN_NAME}"),
        "Any name works; this one tells you later what it is for.",
    )];
    if is_services {
        settings.push(TokenSetting::new(
            "Organization: All accessible organizations",
            "One token for every organization you belong to.",
        ));
    }
    settings.extend([
        TokenSetting::new(
            "Expiration: up to one year",
            "The app tells you when the token stops working.",
        ),
        TokenSetting::new(
            "Scopes: Custom defined, then Show all scopes",
            "Some of the scopes below are hidden until you expand the list.",
        ),
        TokenSetting::new(
            "Code: Read & write",
            "Pull requests, votes, comments, tags, completing, and fetching code over git.",
        ),
        TokenSetting::new("Build: Read", "Pipelines, timelines and logs."),
        TokenSetting::new("Identity: Read", "Find reviewers by name or e-mail."),
    ]);
    if is_services {
        settings.push(TokenSetting::new(
            "User Profile: Read",
            "List your organizations while setting up.",
        ));
    }
    let mut read_only = vec![
        TokenSetting::new("Code: Read", "Pull requests, threads and fetching code over git."),
        TokenSetting::new("Build: Read", "Pipelines, timelines and logs."),
    ];
    if is_services {
        read_only.push(TokenSetting::new(
            "User Profile: Read",
            "List your organizations while setting up.",
        ));
    }
    TokenGuide {
        recommended: TokenRecipe {
            title: "Personal access token with custom scopes".to_owned(),
            create_url: create_url.clone(),
            is_prefilled: false,
            summary: "Azure DevOps can't pre-fill the form. Choose New Token, set these values, then select Create."
                .to_owned(),
            settings,
            caveats: vec![
                "Organization policies can limit tokens to one organization or block them; ask an administrator if \
                 Create is unavailable."
                    .to_owned(),
            ],
        },
        alternative: Some(TokenRecipe {
            title: "Read-only token".to_owned(),
            create_url,
            is_prefilled: false,
            summary: "Review and read pipelines without voting, commenting or completing.".to_owned(),
            settings: read_only,
            caveats: Vec::new(),
        }),
    }
}

/// `https://dev.azure.com/{org}/_usersSettings/tokens`, or the collection's page on Server.
fn token_page_url(base_url: &str, organization: Option<&str>) -> String {
    let base = base_url.trim_end_matches('/');
    match organization.map(str::trim).filter(|organization| !organization.is_empty()) {
        Some(organization) => {
            format!("{base}/{}/_usersSettings/tokens", urlencoding::encode(organization))
        }
        None => format!("{base}/_usersSettings/tokens"),
    }
}

/// What the token itself tells before any request: Entra tokens are JWTs with an expiry claim.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TokenFacts {
    is_entra: bool,
    expires_at: Option<String>,
}

impl TokenFacts {
    pub fn of(token: Option<&str>) -> Self {
        match token {
            Some(token) if is_entra_token(token) => {
                Self { is_entra: true, expires_at: jwt_expiry(token) }
            }
            _ => Self::default(),
        }
    }
}

/// Microsoft Entra access tokens are JWTs; personal access tokens are not.
fn is_entra_token(token: &str) -> bool {
    token.starts_with("eyJ") && token.split('.').count() == 3
}

/// The `exp` claim of a JWT as `2026-10-18 21:40 UTC`. The signature is not checked; this is display only.
fn jwt_expiry(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let expiry = OffsetDateTime::from_unix_timestamp(claims.get("exp")?.as_i64()?).ok()?;
    expiry.format(format_description!("[year]-[month]-[day] [hour]:[minute] UTC")).ok()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Probe {
    Allowed,
    Denied,
}

async fn probe(client: &Client, url: &str, query: &[(&str, &str)]) -> Result<Probe> {
    match client.send(client.request(Method::GET, url).query(query)).await {
        Ok(_) => Ok(Probe::Allowed),
        // `connectionData` already accepted the token, so a refusal here is a missing scope.
        Err(ProviderError::Status { status: 401 | 403, .. }) => Ok(Probe::Denied),
        Err(error) => Err(error),
    }
}

/// Signs in per organization and probes what the token reads. Only `GET` requests.
pub(crate) async fn inspect(client: &Client, facts: &TokenFacts) -> Result<AccessReport> {
    client.require_token("Checking the token")?;
    let (organization, others) =
        client.organizations()?.split_first().expect("organizations() is never empty");
    let user = connection_data(client, organization).await?.authenticated_user.to_user();
    let mut notes = Vec::new();
    for other in others {
        if let Err(error) = connection_data(client, other).await {
            notes.push(format!("The token does not work for '{other}': {error}"));
        }
    }
    if facts.is_entra {
        return Ok(AccessReport {
            user,
            token_kind: "Microsoft Entra sign-in".to_owned(),
            scopes: Vec::new(),
            expires_at: facts.expires_at.clone(),
            checks: entra_checks(),
            notes,
        });
    }
    let login = user.login.clone().unwrap_or_else(|| user.display_name.clone());
    let probes = probe_token(client, organization, &login).await?;
    if probes.identities == Probe::Denied {
        notes.push("Adding reviewers by name needs Identity: Read.".to_owned());
    }
    Ok(AccessReport {
        user,
        token_kind: "Personal access token".to_owned(),
        scopes: Vec::new(),
        expires_at: None,
        checks: token_checks(organization, &probes),
        notes,
    })
}

/// Entra tokens carry the user's own permissions: every feature works as far as the user may.
fn entra_checks() -> Vec<FeatureCheck> {
    Feature::ALL
        .iter()
        .map(|feature| {
            FeatureCheck::new(*feature, Access::Granted, "Uses your own Azure DevOps permissions.")
        })
        .collect()
}

/// What the read probes of a personal access token found.
struct TokenProbes {
    /// Listing repositories worked (Code: Read).
    code_read: bool,
    pull_requests: Probe,
    identities: Probe,
    /// `None` when no repository names a project to probe builds in.
    builds: Option<Probe>,
}

async fn probe_token(client: &Client, organization: &str, login: &str) -> Result<TokenProbes> {
    let repositories_url = client.organization_api(organization, "git/repositories");
    let repositories = match client.get::<ValueList<Repository>>(&repositories_url, &[]).await {
        Ok(list) => Some(list.value),
        Err(ProviderError::Status { status: 401 | 403, .. }) => None,
        Err(error) => return Err(error),
    };
    let pull_requests_url = client.organization_api(organization, "git/pullrequests");
    let identities_url = client.identities_api(organization);
    let identity_query =
        [("searchFilter", "General"), ("filterValue", login), ("queryMembership", "None")];
    let (pull_requests, identities) = tokio::try_join!(
        probe(client, &pull_requests_url, &[("$top", "1")]),
        probe(client, &identities_url, &identity_query),
    )?;
    let sample_project = repositories
        .as_ref()
        .and_then(|list| list.first())
        .map(|repository| &repository.project.name);
    let builds = match sample_project {
        Some(project) => {
            let key = RepoKey { organization, project, repository: "" };
            Some(probe(client, &client.project_api(key, "build/builds"), &[("$top", "1")]).await?)
        }
        None => None,
    };
    Ok(TokenProbes { code_read: repositories.is_some(), pull_requests, identities, builds })
}

fn token_checks(organization: &str, probes: &TokenProbes) -> Vec<FeatureCheck> {
    let read_check = match (probes.code_read, probes.pull_requests) {
        (true, Probe::Allowed) => FeatureCheck::new(
            Feature::ReadPullRequests,
            Access::Granted,
            format!("Code: Read confirmed in '{organization}'."),
        ),
        _ => FeatureCheck::new(
            Feature::ReadPullRequests,
            Access::Missing,
            "Needs Code: Read (or Read & write).",
        ),
    };
    let write_check = |feature| {
        if probes.code_read {
            FeatureCheck::new(
                feature,
                Access::Unverified,
                "Needs Code: Read & write. Only a write could confirm it, so make sure it is ticked.",
            )
        } else {
            FeatureCheck::new(feature, Access::Missing, "Needs Code: Read & write.")
        }
    };
    let pipelines = match probes.builds {
        Some(Probe::Allowed) => {
            FeatureCheck::new(Feature::Pipelines, Access::Granted, "Build: Read confirmed.")
        }
        Some(Probe::Denied) => {
            FeatureCheck::new(Feature::Pipelines, Access::Missing, "Needs Build: Read.")
        }
        None => FeatureCheck::new(
            Feature::Pipelines,
            Access::Unverified,
            "Needs Build: Read. No repository to confirm it on.",
        ),
    };
    let git = if probes.code_read {
        FeatureCheck::new(Feature::GitClone, Access::Granted, "Code: Read confirmed.")
    } else {
        FeatureCheck::new(Feature::GitClone, Access::Missing, "Needs Code: Read.")
    };
    vec![
        read_check,
        write_check(Feature::Vote),
        write_check(Feature::Comment),
        write_check(Feature::Labels),
        write_check(Feature::Merge),
        pipelines,
        git,
    ]
}

#[derive(Deserialize)]
struct ProfileDto {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountDto {
    account_name: String,
}

#[derive(Deserialize)]
struct CollectionDto {
    name: String,
}

/// Organizations of the signed-in user (Services) or the server's project collections (Server).
pub(crate) async fn discover(client: &Client) -> Result<Vec<ScopeCandidate>> {
    client.require_token("Listing your organizations")?;
    if client.is_services() {
        return discover_organizations(client, PROFILE_ROOT).await;
    }
    let url = format!("{}/_apis/projectCollections", client.base_url());
    let collections: ValueList<CollectionDto> = client.get(&url, &[]).await?;
    Ok(collections
        .value
        .into_iter()
        .map(|collection| ScopeCandidate {
            display_name: collection.name.clone(),
            key: collection.name,
            kind: ScopeKind::Collection,
        })
        .collect())
}

async fn discover_organizations(
    client: &Client,
    profile_root: &str,
) -> Result<Vec<ScopeCandidate>> {
    let profile_url = format!("{profile_root}/_apis/profile/profiles/me");
    let profile: ProfileDto = match client.get(&profile_url, &[]).await {
        Ok(profile) => profile,
        Err(ProviderError::Status { status: 401 | 403, .. }) => {
            return Err(ProviderError::Unsupported(
                "Listing your organizations needs the User Profile: Read scope. Add it to the token, or type the \
                 organization names."
                    .to_owned(),
            ));
        }
        Err(error) => return Err(error),
    };
    let accounts_url = format!("{profile_root}/_apis/accounts");
    let accounts: ValueList<AccountDto> =
        client.get(&accounts_url, &[("memberId", profile.id)]).await?;
    let mut candidates: Vec<ScopeCandidate> = accounts
        .value
        .into_iter()
        .map(|account| ScopeCandidate {
            display_name: account.account_name.clone(),
            key: account.account_name,
            kind: ScopeKind::Organization,
        })
        .collect();
    candidates.sort_by_cached_key(|candidate| candidate.key.to_lowercase());
    Ok(candidates)
}

/// Azure DevOps answers a missing scope with 401 or 403; the path names the scope area.
pub(crate) fn explain(error: &ProviderError) -> Option<AccessProblem> {
    let ProviderError::Status { status: status @ (401 | 403), path, .. } = error else {
        return generic_denial(error);
    };
    let scope = scope_for_path(path);
    Some(AccessProblem {
        status: *status,
        title: format!(
            "Azure DevOps refused the token for this request. It may have expired, belong to another organization, \
             or lack {scope}."
        ),
        fix: format!(
            "Create a token with {scope} for this organization (or All accessible organizations)."
        ),
        actions: Vec::new(),
    })
}

fn scope_for_path(path: &str) -> &'static str {
    let path = path.to_ascii_lowercase();
    if path.contains("/build/") {
        "Build: Read"
    } else if path.contains("/identities") {
        "Identity: Read"
    } else if path.contains("/profile/") || path.contains("/accounts") {
        "User Profile: Read"
    } else {
        "Code: Read & write"
    }
}

#[cfg(test)]
mod tests {
    use review_core::{Connection, ProviderKind};
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

    use super::*;

    const PAT: &str = "test-pat";

    fn client(base_url: &str, token: &str) -> Client {
        let connection = Connection {
            id: "mock".into(),
            name: "Mock".into(),
            kind: ProviderKind::AzureDevOps,
            base_url: base_url.to_owned(),
            organizations: vec!["fabrikam".into()],
            token: Some(token.to_owned()),
        };
        Client::new(connection, reqwest::Client::new())
    }

    fn get(route: &str) -> MockBuilder {
        // base64(":test-pat")
        Mock::given(method("GET"))
            .and(path(route))
            .and(header("authorization", "Basic OnRlc3QtcGF0"))
    }

    async fn mount_connection_data(server: &MockServer) {
        get("/fabrikam/_apis/connectionData")
            .and(query_param("api-version", "7.1-preview"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "authenticatedUser": {"id": "u1", "providerDisplayName": "Jamal", "properties": {"Account": {"$value": "jamal@fabrikam.com"}}}
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn pat_reads_are_probed_and_writes_stay_unverified() {
        let server = MockServer::start().await;
        mount_connection_data(&server).await;
        get("/fabrikam/_apis/git/repositories")
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"value": [{"name": "web", "project": {"name": "Shop"}}]}),
                ),
            )
            .mount(&server)
            .await;
        get("/fabrikam/_apis/git/pullrequests")
            .and(query_param("$top", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
            .mount(&server)
            .await;
        get("/fabrikam/_apis/identities")
            .and(query_param("filterValue", "jamal@fabrikam.com"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({"message": "denied"})))
            .mount(&server)
            .await;
        get("/fabrikam/Shop/_apis/build/builds")
            .respond_with(ResponseTemplate::new(401).set_body_string(""))
            .mount(&server)
            .await;

        let report =
            inspect(&client(&server.uri(), PAT), &TokenFacts::of(Some(PAT))).await.expect("report");
        let access = |feature| report.check(feature).map(|check| check.access);
        assert_eq!(report.user.display_name, "Jamal");
        assert_eq!(access(Feature::ReadPullRequests), Some(Access::Granted));
        assert_eq!(access(Feature::Vote), Some(Access::Unverified));
        assert_eq!(access(Feature::Pipelines), Some(Access::Missing));
        assert_eq!(access(Feature::GitClone), Some(Access::Granted));
        assert_eq!(report.notes, ["Adding reviewers by name needs Identity: Read."]);
        let requests = server.received_requests().await.expect("recorded");
        assert!(requests.iter().all(|request| request.method.as_str() == "GET"));
    }

    #[tokio::test]
    async fn pat_without_code_scope_misses_pull_requests() {
        let server = MockServer::start().await;
        mount_connection_data(&server).await;
        let denied = || ResponseTemplate::new(401).set_body_string("");
        get("/fabrikam/_apis/git/repositories").respond_with(denied()).mount(&server).await;
        get("/fabrikam/_apis/git/pullrequests").respond_with(denied()).mount(&server).await;
        get("/fabrikam/_apis/identities")
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
            .mount(&server)
            .await;
        let report =
            inspect(&client(&server.uri(), PAT), &TokenFacts::of(Some(PAT))).await.expect("report");
        assert!(!report.is_usable());
        assert_eq!(report.check(Feature::Merge).map(|check| check.access), Some(Access::Missing));
    }

    #[tokio::test]
    async fn rejected_token_fails_the_sign_in() {
        let server = MockServer::start().await;
        get("/fabrikam/_apis/connectionData")
            .respond_with(ResponseTemplate::new(401).set_body_string(""))
            .mount(&server)
            .await;
        let error = inspect(&client(&server.uri(), PAT), &TokenFacts::of(Some(PAT)))
            .await
            .expect_err("rejected");
        assert_eq!(error.status(), Some(401));
    }

    #[tokio::test]
    async fn discovers_organizations_through_profile_and_accounts() {
        let server = MockServer::start().await;
        get("/_apis/profile/profiles/me")
            .and(query_param("api-version", "7.1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id": "d6245f20", "displayName": "Jamal"})),
            )
            .mount(&server)
            .await;
        get("/_apis/accounts")
            .and(query_param("memberId", "d6245f20"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"count": 2, "value": [
                {"accountId": "1", "accountName": "fabrikam"}, {"accountId": "2", "accountName": "Contoso"}
            ]})))
            .mount(&server)
            .await;
        let found = discover_organizations(&client("https://dev.azure.com", PAT), &server.uri())
            .await
            .expect("organizations");
        let keys: Vec<&str> = found.iter().map(|candidate| candidate.key.as_str()).collect();
        assert_eq!(keys, ["Contoso", "fabrikam"]);
    }

    #[tokio::test]
    async fn missing_profile_scope_explains_itself() {
        let server = MockServer::start().await;
        get("/_apis/profile/profiles/me")
            .respond_with(ResponseTemplate::new(401).set_body_string(""))
            .mount(&server)
            .await;
        let error = discover_organizations(&client("https://dev.azure.com", PAT), &server.uri())
            .await
            .expect_err("needs scope");
        assert!(matches!(error, ProviderError::Unsupported(_)), "{error:?}");
        assert!(error.to_string().contains("User Profile: Read"));
    }

    #[tokio::test]
    async fn server_lists_project_collections() {
        let server = MockServer::start().await;
        get("/_apis/projectCollections")
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"value": [{"id": "c1", "name": "DefaultCollection"}]})),
            )
            .mount(&server)
            .await;
        let found = discover(&client(&server.uri(), PAT)).await.expect("collections");
        assert_eq!(found[0].key, "DefaultCollection");
        assert_eq!(found[0].kind, ScopeKind::Collection);
    }

    #[test]
    fn entra_tokens_are_recognized_and_dated() {
        let payload = URL_SAFE_NO_PAD.encode(r#"{"exp":1792359600}"#);
        let token = format!("eyJhbGciOiJSUzI1NiJ9.{payload}.signature");
        assert!(is_entra_token(&token));
        assert!(!is_entra_token("plain-personal-access-token"));
        assert_eq!(jwt_expiry(&token).as_deref(), Some("2026-10-18 21:40 UTC"));
    }

    #[test]
    fn guide_links_the_organization_token_page() {
        let guide = token_guide("https://dev.azure.com", Some("Fabrikam Industries"));
        assert_eq!(
            guide.recommended.create_url,
            "https://dev.azure.com/Fabrikam%20Industries/_usersSettings/tokens"
        );
        assert!(!guide.recommended.is_prefilled);
        let names: Vec<&str> =
            guide.recommended.settings.iter().map(|setting| setting.name.as_str()).collect();
        assert!(names.contains(&"Code: Read & write") && names.contains(&"User Profile: Read"));
        let server = token_guide("https://tfs.example.com/tfs", Some("DefaultCollection"));
        assert_eq!(
            server.recommended.create_url,
            "https://tfs.example.com/tfs/DefaultCollection/_usersSettings/tokens"
        );
        assert!(
            server.recommended.settings.iter().all(|setting| setting.name != "User Profile: Read")
        );
    }

    #[test]
    fn explains_denials_by_scope_area() {
        let denied = |path: &str| ProviderError::Status {
            status: 401,
            path: path.into(),
            body: String::new(),
        };
        assert!(
            explain(&denied("/o/p/_apis/build/builds/3/logs/1"))
                .expect("explained")
                .fix
                .contains("Build: Read")
        );
        assert!(
            explain(&denied("/o/p/_apis/git/repositories/r/pullrequests/1/threads"))
                .expect("explained")
                .fix
                .contains("Code: Read & write")
        );
    }
}
