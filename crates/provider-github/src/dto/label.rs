use serde::Deserialize;

/// Label as REST and GraphQL both return it (`color` is hex without `#`).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LabelDto {
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}
