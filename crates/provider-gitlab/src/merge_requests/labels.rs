//! Project label catalog (including ancestor group labels) and label assignment.

use reqwest::Method;
use review_core::{Label, Result};
use serde_json::json;

use crate::client::{ApiClient, merge_request_path, project_path};
use crate::dto::{LabelDto, LabelRef};

const MAX_LABELS: usize = 2_000;

pub(crate) async fn available(client: &ApiClient, repo: &str) -> Result<Vec<Label>> {
    let path = format!("{}/labels", project_path(repo));
    let labels: Vec<LabelDto> = client
        .get_pages(&path, &[("include_ancestor_groups", "true".to_owned())], MAX_LABELS)
        .await?;
    Ok(labels.into_iter().map(|label| to_label(LabelRef::Detail(label))).collect())
}

/// Replaces all labels; an empty list clears them. GitLab creates unknown names as project labels.
pub(crate) async fn set(client: &ApiClient, repo: &str, iid: u64, labels: &[String]) -> Result<()> {
    client.require_token("Changing labels")?;
    let request = client
        .request(Method::PUT, &merge_request_path(repo, iid))
        .json(&json!({ "labels": labels.join(",") }));
    client.unit(request).await
}

pub(crate) fn to_label(label: LabelRef) -> Label {
    match label {
        LabelRef::Name(name) => Label { name, color: None, description: None },
        LabelRef::Detail(detail) => Label {
            name: detail.name,
            color: detail.color.as_deref().and_then(hex_color),
            description: detail.description.filter(|description| !description.is_empty()),
        },
    }
}

/// `#RRGGBB` → `RRGGBB`; named CSS colors (old GitLab hosts) are dropped.
fn hex_color(color: &str) -> Option<String> {
    let hex = color.trim().trim_start_matches('#');
    let is_hex = hex.len() == 6 && hex.chars().all(|character| character.is_ascii_hexdigit());
    is_hex.then(|| hex.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_detailed_and_plain_labels() {
        let labels: Vec<LabelRef> = serde_json::from_str(
            r##"["bug", {"name":"type::feature","color":"#330066","description":""}]"##,
        )
        .expect("fixture");
        let mapped: Vec<Label> = labels.into_iter().map(to_label).collect();
        assert_eq!(mapped[0], Label { name: "bug".into(), color: None, description: None });
        assert_eq!(
            mapped[1],
            Label { name: "type::feature".into(), color: Some("330066".into()), description: None }
        );
    }

    #[test]
    fn rejects_non_hex_colors() {
        assert_eq!(hex_color("#a1b2c3").as_deref(), Some("A1B2C3"));
        assert_eq!(hex_color("red"), None);
        assert_eq!(hex_color("#fff"), None);
    }
}
