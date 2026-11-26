use serde::Deserialize;

/// GraphQL `Actor` as selected by [`crate::graphql::ACTOR_FRAGMENT`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActorDto {
    pub login: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub database_id: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
}

/// GraphQL `Team` fields used for reviewer requests.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TeamNodeDto {
    #[serde(default)]
    pub database_id: Option<u64>,
    pub name: String,
    pub slug: String,
}
