//! SAML single sign-on. When the token is not authorized for an organization that enforces SSO,
//! GitHub leaves that organization out of listings and searches without failing them: the
//! `X-GitHub-SSO: partial-results; organizations=21955855,20582480` header names the organization
//! ids, and GraphQL may add `FORBIDDEN` errors next to the data. Single requests into such an
//! organization fail with 403 instead (see [`crate::setup::explain`]).

// RS-DONT-04: parses provider data, so indexing and narrowing casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use reqwest::header::HeaderMap;
use review_core::{AccessProblem, Connection, ProblemAction, Result};
use serde::Deserialize;

use crate::client::GitHubClient;

const HEADER: &str = "x-github-sso";

/// An organization whose results GitHub left out; `login` is `None` when it could not be named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Blocked {
    pub login: Option<String>,
}

/// The organizations whose results GitHub left out of this response, from its `x-github-sso`
/// header; empty when the response is complete.
pub(crate) fn left_out_organizations(headers: &HeaderMap) -> Vec<u64> {
    headers
        .get(HEADER)
        .and_then(|value| value.to_str().ok())
        .map(partial_organization_ids)
        .unwrap_or_default()
}

/// The organization ids of a `partial-results; organizations=…` header value.
fn partial_organization_ids(value: &str) -> Vec<u64> {
    let mut parts = value.split(';').map(str::trim);
    if parts.next() != Some("partial-results") {
        return Vec::new();
    }
    parts
        .filter_map(|part| part.strip_prefix("organizations="))
        .flat_map(|ids| ids.split(','))
        .filter_map(|id| id.trim().parse().ok())
        .collect()
}

#[derive(Deserialize)]
struct OrganizationDto {
    login: String,
}

/// Names the organizations behind `ids` (`GET /organizations/{id}`, public data), once per id and
/// provider. An organization that cannot be looked up stays unnamed rather than failing the listing.
pub(crate) async fn name_organizations(client: &GitHubClient, ids: &[u64]) -> Vec<Blocked> {
    let mut blocked = Vec::new();
    for id in ids {
        let login = match client.cached_organization_login(*id) {
            Some(login) => Some(login),
            None => look_up_login(client, *id).await.ok(),
        };
        let entry = Blocked { login };
        if !blocked.contains(&entry) {
            blocked.push(entry);
        }
    }
    blocked
}

async fn look_up_login(client: &GitHubClient, id: u64) -> Result<String> {
    let organization: OrganizationDto = client.get_json(&format!("/organizations/{id}")).await?;
    client.remember_organization_login(id, &organization.login);
    Ok(organization.login)
}

/// GitHub CLI and other OAuth app tokens are authorized by signing in to the organization's
/// single sign-on; personal access tokens through "Configure SSO" on the token page.
fn is_oauth_token(connection: &Connection) -> bool {
    connection.token().is_some_and(|token| token.starts_with("gho_"))
}

/// The page that authorizes the connection's token for `organization`.
pub(crate) fn authorize_url(connection: &Connection, organization: Option<&str>) -> String {
    let web = connection.base_url();
    match (is_oauth_token(connection), organization) {
        (true, Some(organization)) => {
            format!("{web}/orgs/{}/sso", urlencoding::encode(organization))
        }
        (true, None) => format!("{web}/settings/applications"),
        (false, _) => format!("{web}/settings/tokens"),
    }
}

/// "Authorize for acme", one per left-out organization.
pub(crate) fn actions(connection: &Connection, blocked: &[Blocked]) -> Vec<ProblemAction> {
    blocked
        .iter()
        .map(|entry| ProblemAction {
            label: entry.login.as_deref().map_or_else(
                || "Authorize the sign-in".to_owned(),
                |login| format!("Authorize for {login}"),
            ),
            url: authorize_url(connection, entry.login.as_deref()),
        })
        .collect()
}

/// What the latest listing left out, or `None` when it was complete.
pub(crate) fn problem(connection: &Connection, blocked: &[Blocked]) -> Option<AccessProblem> {
    if blocked.is_empty() {
        return None;
    }
    let names: Vec<&str> = blocked.iter().filter_map(|entry| entry.login.as_deref()).collect();
    let (which, its) = match names.as_slice() {
        [] => ("an organization".to_owned(), "its"),
        [name] => ((*name).to_owned(), "its"),
        _ => (names.join(", "), "their"),
    };
    Some(AccessProblem {
        status: 403,
        title: format!(
            "Pull requests from {which} are missing: the sign-in is not authorized for {its} SAML single sign-on."
        ),
        fix: fix(connection).to_owned(),
        actions: actions(connection, blocked),
    })
}

/// How the connection's kind of token gets authorized for an organization's SSO.
pub(crate) fn fix(connection: &Connection) -> &'static str {
    if is_oauth_token(connection) {
        "Sign in to the organization's single sign-on with the button, run gh auth refresh in a terminal, \
         then choose Fix token and Use my GitHub CLI sign-in again."
    } else {
        "Open the token page with the button, choose Configure SSO next to the token and authorize the organization."
    }
}

/// The owner a REST path addresses (`/repos/{owner}/…`, `/orgs/{owner}/…`), behind any API prefix.
pub(crate) fn owner_of_path(path: &str) -> Option<&str> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    segments.by_ref().find(|segment| matches!(*segment, "repos" | "orgs"))?;
    segments.next()
}

#[cfg(test)]
mod tests {
    use review_core::ProviderKind;

    use super::*;

    fn connection(token: &str) -> Connection {
        Connection {
            id: "gh".to_owned(),
            name: "GitHub".to_owned(),
            kind: ProviderKind::GitHub,
            base_url: "https://github.com/".to_owned(),
            organizations: Vec::new(),
            token: Some(token.to_owned()),
        }
    }

    #[test]
    fn parses_partial_results() {
        assert_eq!(
            partial_organization_ids("partial-results; organizations=21955855,20582480"),
            [21_955_855, 20_582_480]
        );
        assert_eq!(
            partial_organization_ids("required; url=https://github.com/orgs/acme/sso?x=1"),
            Vec::<u64>::new()
        );
        assert_eq!(partial_organization_ids(""), Vec::<u64>::new());
    }

    #[test]
    fn cli_tokens_sign_in_to_the_organization_and_tokens_configure_sso() {
        let blocked = [Blocked { login: Some("acme".to_owned()) }, Blocked { login: None }];
        let cli = actions(&connection("gho_x"), &blocked);
        assert_eq!(cli[0].label, "Authorize for acme");
        assert_eq!(cli[0].url, "https://github.com/orgs/acme/sso");
        assert_eq!(cli[1].url, "https://github.com/settings/applications");
        let classic = actions(&connection("ghp_x"), &blocked);
        assert_eq!(classic[0].url, "https://github.com/settings/tokens");
        let problem = problem(&connection("gho_x"), &blocked).expect("problem");
        assert!(problem.title.contains("acme"), "{}", problem.title);
        assert!(super::problem(&connection("gho_x"), &[]).is_none());
    }

    #[test]
    fn finds_the_owner_of_a_path() {
        assert_eq!(owner_of_path("/api/v3/repos/acme/app/pulls"), Some("acme"));
        assert_eq!(owner_of_path("/orgs/acme/repos"), Some("acme"));
        assert_eq!(owner_of_path("/graphql"), None);
    }
}
