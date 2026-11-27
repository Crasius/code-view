//! The authenticated user per organization and identity search for reviewer names.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use reqwest::Method;
use review_core::{ProviderError, Result, UserIdentity};
use tokio::sync::OnceCell;

use crate::client::{API_VERSION_PREVIEW, Client};
use crate::dto::ValueList;
use crate::dto::identity::{ConnectionData, Identity};

/// Identity ids are per organization (tenant), so the current user is cached per organization.
///
/// One cell per organization: the map's lock is held only to find the cell, never across the
/// request, so several organizations ask at the same time, while callers of one organization
/// share its single `connectionData` request. A failed request leaves the cell empty for the next
/// caller to try again.
#[derive(Default)]
pub(crate) struct CurrentUsers {
    by_organization: Mutex<HashMap<String, Arc<OnceCell<UserIdentity>>>>,
}

impl CurrentUsers {
    pub(crate) async fn get(&self, client: &Client, organization: &str) -> Result<UserIdentity> {
        let cell = Arc::clone(
            self.by_organization
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(organization.to_owned())
                .or_default(),
        );
        let user = cell
            .get_or_try_init(|| async {
                let data = connection_data(client, organization).await?;
                Ok::<_, ProviderError>(data.authenticated_user.to_user())
            })
            .await?;
        Ok(user.clone())
    }

    /// The user of `organization` when an earlier [`Self::get`] already read it; never asks.
    pub(crate) fn cached(&self, organization: &str) -> Option<UserIdentity> {
        let cache = self.by_organization.lock().unwrap_or_else(PoisonError::into_inner);
        cache.get(organization).and_then(|cell| cell.get().cloned())
    }
}

/// `connectionData` of `organization`: the user the token signs in as.
pub(crate) async fn connection_data(client: &Client, organization: &str) -> Result<ConnectionData> {
    let url = client.organization_api(organization, "connectionData");
    client.json(client.request_versioned(Method::GET, &url, API_VERSION_PREVIEW)).await
}

/// Resolves what the user typed (e-mail, account name or display name) to an identity id.
pub(crate) async fn find_identity(
    client: &Client,
    organization: &str,
    query: &str,
) -> Result<UserIdentity> {
    let query = query.trim();
    let url = client.identities_api(organization);
    let params = [
        ("searchFilter", "General".to_owned()),
        ("filterValue", query.to_owned()),
        ("queryMembership", "None".to_owned()),
    ];
    let identities: ValueList<Identity> = client.get(&url, &params).await?;
    pick_identity(&identities.value, query).map(Identity::to_user).ok_or_else(|| {
        ProviderError::Local(format!(
            "No Azure DevOps identity in '{organization}' matches '{query}'."
        ))
    })
}

/// A "General" search also matches display names; an active identity whose account or mail equals
/// the query wins over the first active hit.
pub(crate) fn pick_identity<'a>(identities: &'a [Identity], query: &str) -> Option<&'a Identity> {
    let exact = |identity: &&Identity| {
        ["Account", "Mail"].iter().any(|name| {
            identity
                .properties
                .get(*name)
                .and_then(|value| value.as_text())
                .is_some_and(|text| text.eq_ignore_ascii_case(query))
        })
    };
    let active = || identities.iter().filter(|identity| identity.is_active);
    active().find(exact).or_else(|| active().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_exact_account_match() {
        let identities: ValueList<Identity> = serde_json::from_str(
            r#"{"value":[
              {"id":"inactive","providerDisplayName":"Jo","isActive":false,"properties":{"Account":{"$value":"jo@x.com"}}},
              {"id":"group","providerDisplayName":"[p]\\Jo fans","isActive":true,"isContainer":true,"properties":{"Account":{"$value":"Jo fans"}}},
              {"id":"user","providerDisplayName":"Jo","isActive":true,"properties":{"Mail":{"$value":"JO@x.com"}}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            pick_identity(&identities.value, "jo@x.com").map(|identity| identity.id.as_str()),
            Some("user")
        );
        assert_eq!(
            pick_identity(&identities.value, "Jo").map(|identity| identity.id.as_str()),
            Some("group")
        );
        assert!(pick_identity(&[], "x").is_none());
    }
}
