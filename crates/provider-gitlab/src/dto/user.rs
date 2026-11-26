use review_core::UserIdentity;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct UserDto {
    pub id: u64,
    pub username: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub public_email: Option<String>,
}

impl UserDto {
    pub(crate) fn to_identity(&self) -> UserIdentity {
        let display_name = self
            .name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| self.username.clone());
        UserIdentity {
            id: self.id.to_string(),
            display_name,
            login: Some(self.username.clone()),
            avatar_url: self.avatar_url.clone(),
        }
    }
}
