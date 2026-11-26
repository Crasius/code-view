//! Users, actors, teams, labels and repositories.

use review_core::{Label, RepositoryInfo, UserIdentity};

use crate::dto::actor::{ActorDto, TeamNodeDto};
use crate::dto::label::LabelDto;
use crate::dto::repository::RepositoryDto;
use crate::dto::user::{TeamDto, UserDto};

/// Prefix of team reviewer ids, so `remove_reviewer` can tell teams from users.
pub(crate) const TEAM_ID_PREFIX: &str = "team:";

/// GitHub shows deleted accounts as `ghost`.
const GHOST_LOGIN: &str = "ghost";

pub(crate) fn user(dto: UserDto) -> UserIdentity {
    UserIdentity {
        id: dto.id.to_string(),
        display_name: dto.name.filter(|name| !name.is_empty()).unwrap_or_else(|| dto.login.clone()),
        login: Some(dto.login),
        avatar_url: dto.avatar_url,
    }
}

pub(crate) fn optional_user(dto: Option<UserDto>) -> UserIdentity {
    dto.map_or_else(ghost, user)
}

/// GraphQL actor; bots and mannequins without a database id use their login as id.
pub(crate) fn actor(dto: Option<ActorDto>) -> UserIdentity {
    let Some(dto) = dto else {
        return ghost();
    };
    UserIdentity {
        id: dto.database_id.map_or_else(|| dto.login.clone(), |id| id.to_string()),
        display_name: dto.name.filter(|name| !name.is_empty()).unwrap_or_else(|| dto.login.clone()),
        login: Some(dto.login),
        avatar_url: dto.avatar_url,
    }
}

pub(crate) fn team(dto: TeamDto) -> UserIdentity {
    UserIdentity {
        id: format!("{TEAM_ID_PREFIX}{}", dto.id),
        display_name: dto.name,
        login: Some(dto.slug),
        avatar_url: None,
    }
}

pub(crate) fn team_node(dto: TeamNodeDto) -> UserIdentity {
    let id = dto.database_id.map_or_else(|| dto.slug.clone(), |id| id.to_string());
    UserIdentity {
        id: format!("{TEAM_ID_PREFIX}{id}"),
        display_name: dto.name,
        login: Some(dto.slug),
        avatar_url: None,
    }
}

fn ghost() -> UserIdentity {
    UserIdentity {
        id: GHOST_LOGIN.to_owned(),
        display_name: GHOST_LOGIN.to_owned(),
        login: Some(GHOST_LOGIN.to_owned()),
        avatar_url: None,
    }
}

pub(crate) fn label(dto: LabelDto) -> Label {
    Label {
        name: dto.name,
        color: dto.color.filter(|color| !color.is_empty()),
        description: dto.description.filter(|description| !description.is_empty()),
    }
}

pub(crate) fn repository(dto: RepositoryDto) -> RepositoryInfo {
    RepositoryInfo {
        key: dto.full_name,
        name: dto.name,
        description: dto.description.filter(|description| !description.is_empty()),
        web_url: dto.html_url,
        default_branch: dto.default_branch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::fixtures::parse;

    #[test]
    fn user_falls_back_to_login_for_display_name() {
        let identity = user(parse(r#"{"id":42,"login":"octocat","avatar_url":"https://a/1"}"#));
        assert_eq!(identity.id, "42");
        assert_eq!(identity.display_name, "octocat");
        assert_eq!(identity.login.as_deref(), Some("octocat"));
        assert_eq!(identity.avatar_url.as_deref(), Some("https://a/1"));
    }

    #[test]
    fn missing_user_is_ghost() {
        assert_eq!(optional_user(None).login.as_deref(), Some("ghost"));
        assert_eq!(actor(None).id, "ghost");
    }

    #[test]
    fn bot_actor_without_database_id_uses_login() {
        let identity = actor(Some(parse(r#"{"login":"dependabot","avatarUrl":null}"#)));
        assert_eq!(identity.id, "dependabot");
        let identity = actor(Some(parse(r#"{"login":"mona","databaseId":7,"name":"Mona"}"#)));
        assert_eq!((identity.id.as_str(), identity.display_name.as_str()), ("7", "Mona"));
    }

    #[test]
    fn empty_label_fields_become_none() {
        let mapped = label(parse(r#"{"name":"bug","color":"d73a4a","description":""}"#));
        assert_eq!(mapped.color.as_deref(), Some("d73a4a"));
        assert_eq!(mapped.description, None);
    }
}
