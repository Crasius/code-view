use serde::Deserialize;

/// Label as REST and GraphQL both return it (`color` is hex without `#`).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LabelDto {
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}
