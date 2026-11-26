//! Pull request tags. Azure DevOps tags are free text, so there is no catalog to offer.

use reqwest::Method;
use review_core::{Label, Result};
use serde_json::json;

use crate::client::{Client, RepoKey};
use crate::dto::ValueList;
use crate::dto::pull_request::Tag;

/// Always empty: tags are free text and the API has no cheap per-repository tag list
/// (`Capabilities::has_label_catalog` is false).
pub(crate) fn available() -> Vec<Label> {
    Vec::new()
}

pub(crate) async fn set(client: &Client, repo: &str, number: u64, wanted: &[String]) -> Result<()> {
    client.require_token("Changing tags")?;
    let key = RepoKey::parse(repo)?;
    let url = client.pull_request_api(key, number, "labels");
    let current: ValueList<Tag> = client.get(&url, &[]).await?;
    let current: Vec<&str> =
        current.value.iter().filter(|tag| tag.active).map(|tag| tag.name.as_str()).collect();
    let (add, remove) = diff(&current, wanted);
    for name in remove {
        client
            .send_unit::<()>(Method::DELETE, &format!("{url}/{}", urlencoding::encode(name)), None)
            .await?;
    }
    for name in add {
        client.send_unit(Method::POST, &url, Some(&json!({ "name": name }))).await?;
    }
    Ok(())
}

/// Tags compare case-insensitively, as Azure DevOps does. Returns (to add, to remove).
pub(crate) fn diff<'a>(current: &[&'a str], wanted: &'a [String]) -> (Vec<&'a str>, Vec<&'a str>) {
    let contains =
        |list: &[&str], name: &str| list.iter().any(|item| item.eq_ignore_ascii_case(name));
    let mut wanted_unique: Vec<&str> = Vec::new();
    for name in wanted.iter().map(|name| name.trim()).filter(|name| !name.is_empty()) {
        if !contains(&wanted_unique, name) {
            wanted_unique.push(name);
        }
    }
    let add = wanted_unique.iter().copied().filter(|name| !contains(current, name)).collect();
    let remove = current.iter().copied().filter(|name| !contains(&wanted_unique, name)).collect();
    (add, remove)
}

#[cfg(test)]
mod tests {
    use super::diff;

    #[test]
    fn diffs_case_insensitively() {
        let wanted = vec!["Bug".to_owned(), "ui".to_owned(), "UI".to_owned(), " ".to_owned()];
        let (add, remove) = diff(&["bug", "old"], &wanted);
        assert_eq!(add, ["ui"]);
        assert_eq!(remove, ["old"]);
    }
}
