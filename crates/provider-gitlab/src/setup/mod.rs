//! Connection setup for gitlab.com and self-managed GitLab: the token recipe, read-only access
//! probes, group discovery and explanations for denied requests.
//!
//! Legacy tokens report their scopes through `GET /personal_access_tokens/self`. Fine-grained
//! tokens report `granular_scopes` there (GitLab 19.5+); their read permissions are additionally
//! probed with `GET` requests on one project, because only reads may run during a check.

mod fine_grained;

use review_core::setup::{
    Access, AccessProblem, AccessReport, Feature, FeatureCheck, ScopeCandidate, ScopeKind,
    TOKEN_DESCRIPTION, TOKEN_NAME, TokenGuide, TokenRecipe, TokenSetting, generic_denial,
};
use review_core::{ProviderError, Result};
use serde::Deserialize;

use crate::client::ApiClient;
use crate::repositories;

const LEGACY_SCOPES: [&str; 5] =
    ["api", "read_api", "read_repository", "write_repository", "read_user"];
/// Guest access or more, the lowest role that sees merge requests.
const MIN_ACCESS_LEVEL: &str = "10";
const MAX_GROUPS: usize = 100;

/// Where to create a token on this host, and with which settings.
pub fn token_guide(base_url: &str) -> TokenGuide {
    let base = base_url.trim_end_matches('/');
    TokenGuide {
        recommended: TokenRecipe {
            title: "Personal access token with the api scope".to_owned(),
            create_url: token_url(base),
            is_prefilled: true,
            summary: "One scope covers every feature: merge requests, approvals, discussions, labels, merging, \
                      pipelines, job logs and git."
                .to_owned(),
            settings: vec![TokenSetting::new(
                "api",
                "Read and write merge requests, approve, comment, edit labels, merge, read pipelines and job logs, \
                 and fetch code over git.",
            )],
            caveats: vec![
                "Self-managed GitLab older than 16 keeps tokens under Preferences > Access Tokens; use that page if \
                 the link shows a 404."
                    .to_owned(),
            ],
        },
        alternative: Some(TokenRecipe {
            title: "Fine-grained personal access token".to_owned(),
            create_url: format!("{base}/-/user_settings/personal_access_tokens"),
            is_prefilled: false,
            summary: "Least privilege: only the groups, projects and actions you pick. Needs GitLab 18.10 or later."
                .to_owned(),
            settings: fine_grained::PERMISSIONS
                .iter()
                .map(|(permission, reason)| TokenSetting::new(*permission, *reason))
                .collect(),
            caveats: vec![
                "GitLab can't pre-fill fine-grained tokens: choose Generate token > Fine-grained token and add each \
                 permission."
                    .to_owned(),
            ],
        }),
    }
}

/// The token page, pre-filled with name, description and the `api` scope.
fn token_url(base: &str) -> String {
    format!(
        "{base}/-/user_settings/personal_access_tokens?name={}&description={}&scopes=api",
        urlencoding::encode(TOKEN_NAME),
        urlencoding::encode(TOKEN_DESCRIPTION)
    )
}

#[derive(Debug, Deserialize)]
struct TokenSelfDto {
    #[serde(default)]
    scopes: Vec<String>,
    granular_scopes: Option<Vec<GranularScopeDto>>,
    expires_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GranularScopeDto {
    #[serde(default)]
    permissions: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GroupDto {
    full_path: String,
    full_name: Option<String>,
}

/// Signs in and works out what the token can do. Only `GET` requests.
pub(crate) async fn inspect(client: &ApiClient) -> Result<AccessReport> {
    client.require_token("Checking the token")?;
    let token = match client.get::<TokenSelfDto>("/personal_access_tokens/self", &[]).await {
        Ok(token) => Some(token),
        // Fine-grained tokens without "Personal Access Token: Read", or GitLab before 15.5.
        Err(ProviderError::Status { status: 403 | 404, .. }) => None,
        Err(error) => return Err(error),
    };
    let user = repositories::current_user(client).await?;
    let expires_at = token.as_ref().and_then(|token| token.expires_at.clone());
    let legacy: Vec<String> = token
        .as_ref()
        .map(|token| {
            token
                .scopes
                .iter()
                .filter(|scope| LEGACY_SCOPES.contains(&scope.as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let granular = token.as_ref().and_then(|token| token.granular_scopes.as_ref());
    if granular.is_none() && !legacy.is_empty() {
        return Ok(AccessReport {
            user,
            token_kind: "Personal access token".to_owned(),
            checks: legacy_checks(&legacy),
            notes: Vec::new(),
            scopes: legacy,
            expires_at,
        });
    }
    let permissions: Vec<String> = granular
        .into_iter()
        .flatten()
        .flat_map(|scope| scope.permissions.iter().cloned())
        .collect();
    let checks = fine_grained::checks(client, &permissions).await?;
    Ok(AccessReport {
        user,
        token_kind: "Fine-grained personal access token".to_owned(),
        scopes: permissions,
        expires_at,
        checks,
        notes: Vec::new(),
    })
}

fn legacy_checks(scopes: &[String]) -> Vec<FeatureCheck> {
    let has = |scope: &str| scopes.iter().any(|granted| granted == scope);
    let can_read = has("api") || has("read_api");
    let can_fetch = has("api") || has("read_repository") || has("write_repository");
    Feature::ALL
        .iter()
        .map(|feature| {
            let (access, detail) = match feature {
                _ if has("api") => (Access::Granted, "Granted by the api scope."),
                Feature::ReadPullRequests | Feature::Pipelines if can_read => {
                    (Access::Granted, "Granted by read_api.")
                }
                Feature::ReadPullRequests | Feature::Pipelines => {
                    (Access::Missing, "Needs the api scope.")
                }
                Feature::GitClone if can_fetch => (Access::Granted, "Granted by read_repository."),
                Feature::GitClone => (Access::Missing, "Needs the api or read_repository scope."),
                _ => (Access::Missing, "Needs the api scope; read_api only reads."),
            };
            FeatureCheck::new(*feature, access, detail)
        })
        .collect()
}

/// Top-level groups the user is a member of.
pub(crate) async fn discover(client: &ApiClient) -> Result<Vec<ScopeCandidate>> {
    client.require_token("Listing your groups")?;
    let query = [
        ("min_access_level", MIN_ACCESS_LEVEL.to_owned()),
        ("top_level_only", "true".to_owned()),
        ("order_by", "path".to_owned()),
    ];
    let groups: Vec<GroupDto> = client.get_pages("/groups", &query, MAX_GROUPS).await?;
    Ok(groups
        .into_iter()
        .map(|group| ScopeCandidate {
            display_name: group
                .full_name
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| group.full_path.clone()),
            key: group.full_path,
            kind: ScopeKind::Group,
        })
        .collect())
}

/// GitLab's 403 answers: legacy `insufficient_scope` and fine-grained "requires … [Permission]".
pub(crate) fn explain(error: &ProviderError) -> Option<AccessProblem> {
    let ProviderError::Status { status: 403, body, .. } = error else {
        return generic_denial(error);
    };
    if body.contains("insufficient_scope") {
        return Some(AccessProblem {
            status: 403,
            title: "The token's scopes don't allow this request; it needs the api scope."
                .to_owned(),
            fix: "Create a token with the api scope and paste it into this connection.".to_owned(),
            actions: Vec::new(),
        });
    }
    let message = error.server_message().unwrap_or_default();
    if message.contains("fine-grained personal access token") {
        let permissions = bracketed(&message).unwrap_or_else(|| "a permission".to_owned());
        return Some(AccessProblem {
            status: 403,
            title: format!("The fine-grained token lacks {permissions}."),
            fix: format!(
                "Edit the token and add {permissions}, or create a token with the api scope."
            ),
            actions: Vec::new(),
        });
    }
    generic_denial(error)
}

/// `"… permissions: [Merge Request: Read, Job: Read]."` → `"Merge Request: Read, Job: Read"`.
fn bracketed(message: &str) -> Option<String> {
    let (_, rest) = message.split_once('[')?;
    let (inner, _) = rest.split_once(']')?;
    Some(inner.trim().to_owned()).filter(|inner| !inner.is_empty())
}

#[cfg(test)]
mod tests {
    use review_core::{Connection, Provider, ProviderKind};
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

    use super::fine_grained::permission_key;
    use super::*;
    use crate::GitLabProvider;

    async fn provider() -> (MockServer, GitLabProvider) {
        let server = MockServer::start().await;
        let connection = Connection {
            id: "mock".into(),
            name: "Mock".into(),
            kind: ProviderKind::GitLab,
            base_url: server.uri(),
            organizations: Vec::new(),
            token: Some("test-token".into()),
        };
        (server, GitLabProvider::new(connection, reqwest::Client::new()))
    }

    fn get(route: &str) -> MockBuilder {
        Mock::given(method("GET"))
            .and(path(format!("/api/v4{route}")))
            .and(header("private-token", "test-token"))
    }

    async fn mount_user(server: &MockServer) {
        get("/user")
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id": 7, "username": "ada", "name": "Ada"})),
            )
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn api_scope_grants_everything() {
        let (server, provider) = provider().await;
        get("/personal_access_tokens/self")
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"scopes": ["api"], "expires_at": "2026-12-01"})),
            )
            .mount(&server)
            .await;
        mount_user(&server).await;
        let report = provider.inspect_access().await.expect("report");
        assert_eq!(report.user.login.as_deref(), Some("ada"));
        assert_eq!(report.expires_at.as_deref(), Some("2026-12-01"));
        assert!(report.checks.iter().all(|check| check.access == Access::Granted));
    }

    #[tokio::test]
    async fn read_api_token_reads_but_cannot_write() {
        let (server, provider) = provider().await;
        get("/personal_access_tokens/self")
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"scopes": ["read_api"]})))
            .mount(&server)
            .await;
        mount_user(&server).await;
        let report = provider.inspect_access().await.expect("report");
        let access = |feature| report.check(feature).map(|check| check.access);
        assert_eq!(access(Feature::ReadPullRequests), Some(Access::Granted));
        assert_eq!(access(Feature::Vote), Some(Access::Missing));
        assert_eq!(access(Feature::GitClone), Some(Access::Missing));
    }

    #[tokio::test]
    async fn fine_grained_token_missing_merge_request_read() {
        let (server, provider) = provider().await;
        get("/personal_access_tokens/self")
            .respond_with(
                ResponseTemplate::new(403).set_body_json(json!({"message": "403 Forbidden"})),
            )
            .mount(&server)
            .await;
        mount_user(&server).await;
        get("/projects")
            .and(query_param("membership", "true"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([{"id": 42, "path_with_namespace": "acme/app"}])),
            )
            .mount(&server)
            .await;
        let denied = ResponseTemplate::new(403).set_body_json(json!({
            "message": "Access denied: This operation requires a fine-grained personal access token with the following project permissions: [Merge Request: Read]."
        }));
        get("/projects/42/merge_requests").respond_with(denied).mount(&server).await;
        let empty = || ResponseTemplate::new(200).set_body_json(json!([]));
        get("/projects/42/labels").respond_with(empty()).mount(&server).await;
        get("/projects/42/pipelines").respond_with(empty()).mount(&server).await;
        get("/projects/42/jobs").respond_with(empty()).mount(&server).await;

        let report = provider.inspect_access().await.expect("report");
        let access = |feature| report.check(feature).map(|check| check.access);
        assert_eq!(report.token_kind, "Fine-grained personal access token");
        assert_eq!(access(Feature::ReadPullRequests), Some(Access::Missing));
        assert_eq!(access(Feature::Pipelines), Some(Access::Granted));
        assert_eq!(access(Feature::Vote), Some(Access::Unverified));
        assert!(!report.is_usable());
        let requests = server.received_requests().await.expect("recorded");
        assert!(requests.iter().all(|request| request.method.as_str() == "GET"));
    }

    #[tokio::test]
    async fn granular_scopes_decide_write_permissions() {
        let (server, provider) = provider().await;
        get("/personal_access_tokens/self")
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "scopes": [],
                "granular_scopes": [{"access": "all_memberships", "permissions": ["read_merge_request", "approve_merge_request", "read_job"], "project_id": null, "group_id": null}]
            })))
            .mount(&server)
            .await;
        mount_user(&server).await;
        get("/projects")
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([{"id": 1, "path_with_namespace": "a/b"}])),
            )
            .mount(&server)
            .await;
        let empty = || ResponseTemplate::new(200).set_body_json(json!([]));
        for route in [
            "/projects/1/merge_requests",
            "/projects/1/labels",
            "/projects/1/pipelines",
            "/projects/1/jobs",
        ] {
            get(route).respond_with(empty()).mount(&server).await;
        }
        let report = provider.inspect_access().await.expect("report");
        let access = |feature| report.check(feature).map(|check| check.access);
        assert_eq!(access(Feature::Vote), Some(Access::Granted));
        assert_eq!(access(Feature::Merge), Some(Access::Missing));
        assert_eq!(report.scopes, ["read_merge_request", "approve_merge_request", "read_job"]);
    }

    #[tokio::test]
    async fn discovers_top_level_groups() {
        let (server, provider) = provider().await;
        get("/groups")
            .and(query_param("min_access_level", "10"))
            .and(query_param("top_level_only", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id": 1, "full_path": "acme", "full_name": "Acme"},
                {"id": 2, "full_path": "tools", "full_name": ""}
            ])))
            .mount(&server)
            .await;
        let scopes = provider.discover_scopes().await.expect("groups");
        let found: Vec<(&str, &str)> =
            scopes.iter().map(|scope| (scope.key.as_str(), scope.display_name.as_str())).collect();
        assert_eq!(found, [("acme", "Acme"), ("tools", "tools")]);
    }

    #[test]
    fn explains_the_reported_fine_grained_denial() {
        let error = ProviderError::Status {
            status: 403,
            path: "/api/v4/projects/acme%2Fapp/merge_requests/3/approvals".into(),
            body: json!({"message": "Access denied: This operation requires a fine-grained personal access token with the following project permissions: [Merge Request: Read]."}).to_string(),
        };
        let problem = explain(&error).expect("explained");
        assert_eq!(problem.title, "The fine-grained token lacks Merge Request: Read.");
        let legacy = ProviderError::Status {
            status: 403,
            path: "/x".into(),
            body: json!({"error": "insufficient_scope", "error_description": "The request requires higher privileges than provided by the access token.", "scope": "api"}).to_string(),
        };
        assert!(explain(&legacy).expect("explained").title.contains("api scope"));
    }

    #[test]
    fn guide_prefills_the_api_scope() {
        let guide = token_guide("https://gitlab.example.com/");
        assert_eq!(
            guide.recommended.create_url,
            "https://gitlab.example.com/-/user_settings/personal_access_tokens?name=Code%20View&description=Pull%20request%20review%20desktop%20app&scopes=api"
        );
    }

    #[test]
    fn maps_permission_names_to_granular_keys() {
        assert_eq!(permission_key("Merge Request: Read"), "read_merge_request");
        assert_eq!(permission_key("Code: Download"), "download_code");
    }
}
