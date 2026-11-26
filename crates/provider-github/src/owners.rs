//! Connection owners are organizations or users; the kind decides REST paths and search qualifiers.

use reqwest::Method;
use review_core::{ProviderError, Result, http};
use serde::Deserialize;
use urlencoding::encode;

use crate::client::GitHubClient;
use crate::dto::user::UserDto;
use crate::sso::{self, Blocked};

const MAX_ORGANIZATIONS: &str = "100";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerKind {
    Organization,
    User,
}

#[derive(Debug, Deserialize)]
struct OwnerDto {
    #[serde(rename = "type")]
    kind: String,
}

impl OwnerKind {
    fn from_type(kind: &str) -> Self {
        if kind.eq_ignore_ascii_case("Organization") {
            OwnerKind::Organization
        } else {
            OwnerKind::User
        }
    }

    /// `org:` or `user:` qualifier for issue search.
    pub fn search_qualifier(self, owner: &str) -> String {
        match self {
            OwnerKind::Organization => format!("org:{owner}"),
            OwnerKind::User => format!("user:{owner}"),
        }
    }

    pub fn repositories_path(self, owner: &str) -> String {
        match self {
            OwnerKind::Organization => format!("/orgs/{}/repos", encode(owner)),
            OwnerKind::User => format!("/users/{}/repos", encode(owner)),
        }
    }
}

/// Looks the owner up once per connection (`GET /users/{owner}` answers for organizations too).
pub(crate) async fn owner_kind(client: &GitHubClient, owner: &str) -> Result<OwnerKind> {
    if let Some(kind) = client.cached_owner_kind(owner) {
        return Ok(kind);
    }
    let dto: OwnerDto = client.get_json(&format!("/users/{}", encode(owner))).await?;
    let kind = OwnerKind::from_type(&dto.kind);
    client.remember_owner_kind(owner, kind);
    Ok(kind)
}

/// The signed-in user and the organizations the token sees: the owners of a connection that shows
/// everything ("Everything this token can see" leaves [`review_core::Connection::organizations`] empty).
#[derive(Debug, Clone)]
pub(crate) struct Reach {
    pub login: String,
    pub organizations: Vec<String>,
    /// Organizations GitHub left out of `/user/orgs` because the token lacks their SAML SSO.
    pub blocked: Vec<Blocked>,
}

#[derive(Deserialize)]
struct OrganizationDto {
    login: String,
}

/// [`Reach`], looked up once per provider (`GET /user`, `GET /user/orgs`). While an organization
/// is blocked by SSO it is looked up again on every call, so authorizing it shows up without a restart.
pub(crate) async fn reach(client: &GitHubClient) -> Result<Reach> {
    let mut cached = client.reach_cache().lock().await;
    if let Some(reach) = cached.as_ref().filter(|reach| reach.blocked.is_empty()) {
        return Ok(reach.clone());
    }
    let fresh = look_up_reach(client).await?;
    *cached = Some(fresh.clone());
    Ok(fresh)
}

async fn look_up_reach(client: &GitHubClient) -> Result<Reach> {
    let user: UserDto = client.get_json("/user").await?;
    client.remember_owner_kind(&user.login, OwnerKind::User);
    let request = client.rest(Method::GET, "/user/orgs").query(&[("per_page", MAX_ORGANIZATIONS)]);
    let (organizations, blocked_ids) = match http::send(request).await {
        Ok(response) => {
            let blocked_ids = sso::left_out_organizations(response.headers());
            let bytes = response.bytes().await?;
            let organizations: Vec<OrganizationDto> = serde_json::from_slice(&bytes)?;
            (organizations, blocked_ids)
        }
        // Fine-grained tokens may not list organizations; the account alone is still useful.
        Err(ProviderError::Status { status: 403, .. }) => (Vec::new(), Vec::new()),
        Err(error) => return Err(error),
    };
    let organizations: Vec<String> =
        organizations.into_iter().map(|organization| organization.login).collect();
    for organization in &organizations {
        client.remember_owner_kind(organization, OwnerKind::Organization);
    }
    Ok(Reach {
        login: user.login,
        organizations,
        blocked: sso::name_organizations(client, &blocked_ids).await,
    })
}

/// Whose repositories a pull request search covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchOwners {
    /// `user:`/`org:` qualifiers.
    pub qualifiers: Vec<String>,
    /// The connection shows everything the token sees: the owners are the user and all their
    /// organizations, and pull requests elsewhere that involve the user belong in as well.
    pub is_everything: bool,
}

/// The connection's chosen owners, or with "everything" the signed-in user plus every organization
/// the token sees, and the organizations SSO kept out of that list.
pub(crate) async fn search_owners(client: &GitHubClient) -> Result<(SearchOwners, Vec<Blocked>)> {
    let chosen: Vec<&str> = client
        .connection()
        .organizations
        .iter()
        .map(|owner| owner.trim())
        .filter(|owner| !owner.is_empty())
        .collect();
    if chosen.is_empty() {
        let reach = reach(client).await?;
        let qualifiers = std::iter::once(OwnerKind::User.search_qualifier(&reach.login))
            .chain(
                reach
                    .organizations
                    .iter()
                    .map(|organization| OwnerKind::Organization.search_qualifier(organization)),
            )
            .collect();
        let owners = SearchOwners { qualifiers, is_everything: true };
        return Ok((owners, reach.blocked));
    }
    let mut qualifiers = Vec::new();
    for owner in chosen {
        qualifiers.push(owner_kind(client, owner).await?.search_qualifier(owner));
    }
    let owners = SearchOwners { qualifiers, is_everything: false };
    Ok((owners, Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::OwnerKind;

    #[test]
    fn owner_type_picks_the_repository_listing_endpoint() {
        assert_eq!(OwnerKind::from_type("Organization"), OwnerKind::Organization);
        assert_eq!(OwnerKind::from_type("organization"), OwnerKind::Organization);
        assert_eq!(OwnerKind::from_type("User"), OwnerKind::User);
        assert_eq!(OwnerKind::from_type("Bot"), OwnerKind::User, "unknown types list as a user");
        assert_eq!(OwnerKind::Organization.repositories_path("cli"), "/orgs/cli/repos");
        assert_eq!(OwnerKind::User.repositories_path("octo cat"), "/users/octo%20cat/repos");
    }
}
