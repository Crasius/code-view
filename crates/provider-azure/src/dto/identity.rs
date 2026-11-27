use std::collections::HashMap;

use review_core::{ReviewVote, Reviewer, UserIdentity};
use serde::Deserialize;

use super::{Href, PropertyValue};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct AvatarLinks {
    pub avatar: Option<Href>,
}

/// `IdentityRef` as embedded in pull requests, reviewers and comments.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IdentityRef {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    pub unique_name: Option<String>,
    pub image_url: Option<String>,
    #[serde(rename = "_links")]
    pub links: Option<AvatarLinks>,
    #[serde(default)]
    pub is_container: bool,
}

impl IdentityRef {
    pub(crate) fn to_user(&self) -> UserIdentity {
        let avatar = self
            .links
            .as_ref()
            .and_then(|links| links.avatar.as_ref())
            .and_then(|avatar| avatar.href.clone());
        UserIdentity {
            id: self.id.clone(),
            display_name: self.display_name.clone(),
            login: self.unique_name.clone().filter(|name| !name.is_empty()),
            avatar_url: avatar.or_else(|| self.image_url.clone()),
        }
    }
}

/// `IdentityRefWithVote`: a pull request reviewer.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewerRef {
    #[serde(flatten)]
    pub identity: IdentityRef,
    #[serde(default)]
    pub vote: i32,
    #[serde(default)]
    pub is_required: bool,
}

impl ReviewerRef {
    pub(crate) fn to_reviewer(&self) -> Reviewer {
        Reviewer {
            identity: self.identity.to_user(),
            vote: vote_from_ado(self.vote),
            is_required: self.is_required,
            is_group: self.identity.is_container,
        }
    }
}

/// 10 / 5 / 0 / -5 / -10; values in between round towards the nearer meaning.
pub(crate) fn vote_from_ado(vote: i32) -> ReviewVote {
    match vote {
        10.. => ReviewVote::Approved,
        1..=9 => ReviewVote::ApprovedWithSuggestions,
        0 => ReviewVote::None,
        -9..=-1 => ReviewVote::WaitingForAuthor,
        _ => ReviewVote::ChangesRequested,
    }
}

pub(crate) fn vote_to_ado(vote: ReviewVote) -> i32 {
    match vote {
        ReviewVote::Approved => 10,
        ReviewVote::ApprovedWithSuggestions => 5,
        ReviewVote::None => 0,
        ReviewVote::WaitingForAuthor => -5,
        ReviewVote::ChangesRequested => -10,
    }
}

/// An identity from `connectionData` or the identities search API.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Identity {
    pub id: String,
    #[serde(default)]
    pub provider_display_name: String,
    pub custom_display_name: Option<String>,
    #[serde(default)]
    pub properties: HashMap<String, PropertyValue>,
    #[serde(default = "crate::dto::default_true")]
    pub is_active: bool,
}

impl Identity {
    pub(crate) fn to_user(&self) -> UserIdentity {
        let property = |name: &str| {
            self.properties.get(name).and_then(PropertyValue::as_text).map(str::to_owned)
        };
        let display_name =
            self.custom_display_name.clone().unwrap_or_else(|| self.provider_display_name.clone());
        UserIdentity {
            id: self.id.clone(),
            display_name,
            login: property("Account")
                .or_else(|| property("Mail"))
                .filter(|login| !login.is_empty()),
            avatar_url: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionData {
    pub authenticated_user: Identity,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_reviewer_with_vote_and_group_flag() {
        let json = r#"{"vote":-10,"isRequired":true,"isContainer":true,"displayName":"[web]\\Reviewers",
            "id":"g1","uniqueName":"vstfs:///Classification/TeamProject/x","imageUrl":"https://img",
            "_links":{"avatar":{"href":"https://avatar"}}}"#;
        let reviewer: ReviewerRef = serde_json::from_str(json).unwrap();
        let mapped = reviewer.to_reviewer();
        assert_eq!(mapped.vote, ReviewVote::ChangesRequested);
        assert!(mapped.is_required && mapped.is_group);
        assert_eq!(mapped.identity.avatar_url.as_deref(), Some("https://avatar"));
        assert_eq!(mapped.identity.display_name, "[web]\\Reviewers");
    }

    #[test]
    fn vote_mapping_round_trips() {
        for vote in [
            ReviewVote::Approved,
            ReviewVote::ApprovedWithSuggestions,
            ReviewVote::None,
            ReviewVote::WaitingForAuthor,
            ReviewVote::ChangesRequested,
        ] {
            assert_eq!(vote_from_ado(vote_to_ado(vote)), vote);
        }
    }

    #[test]
    fn maps_connection_data_user() {
        let json = r#"{"authenticatedUser":{"id":"u1","providerDisplayName":"Jamal Hartnett",
            "properties":{"Account":{"$type":"System.String","$value":"jamal@fabrikam.com"}}}}"#;
        let data: ConnectionData = serde_json::from_str(json).unwrap();
        let user = data.authenticated_user.to_user();
        assert_eq!(user.id, "u1");
        assert_eq!(user.display_name, "Jamal Hartnett");
        assert_eq!(user.login.as_deref(), Some("jamal@fabrikam.com"));
    }
}
