//! Wire types of the Azure DevOps REST API (7.1) and their pure mapping onto the domain model.

// RS-DONT-04: parses provider data, so indexing and narrowing casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

pub(crate) mod identity;
pub(crate) mod iteration;
pub(crate) mod pipeline;
pub(crate) mod pull_request;
pub(crate) mod repository;
pub(crate) mod thread;
pub(crate) mod timeline;

use serde::Deserialize;

/// The `{ "count": n, "value": [...] }` envelope of every Azure DevOps collection.
#[derive(Debug, Deserialize)]
pub(crate) struct ValueList<T> {
    /// `default = "Vec::new"`, not `default`: the plain attribute would ask `T: Default`.
    #[serde(default = "Vec::new")]
    pub value: Vec<T>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommitRef {
    pub commit_id: String,
}

/// `#[serde(default = "...")]` of flags that are on unless the record says otherwise.
pub(crate) fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Href {
    pub href: Option<String>,
}

/// Typed property bag entry: `{ "$type": "System.String", "$value": "..." }`.
#[derive(Debug, Deserialize)]
pub(crate) struct PropertyValue {
    #[serde(rename = "$value")]
    pub value: serde_json::Value,
}

impl PropertyValue {
    pub fn as_text(&self) -> Option<&str> {
        self.value.as_str()
    }
}

/// RFC 3339 timestamps. Azure DevOps writes up to seven fractional digits and uses
/// `0001-01-01T00:00:00` (no offset) for "not set"; optional fields map that to `None`.
pub(crate) mod date {
    use serde::{Deserialize, Deserializer};
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;

    pub fn required<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<OffsetDateTime, D::Error> {
        let text = String::deserialize(deserializer)?;
        OffsetDateTime::parse(&text, &Rfc3339).map_err(serde::de::Error::custom)
    }

    pub fn optional<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<OffsetDateTime>, D::Error> {
        let text = Option::<String>::deserialize(deserializer)?;
        Ok(text.and_then(|text| OffsetDateTime::parse(&text, &Rfc3339).ok()))
    }
}

/// Strips `refs/heads/` for display; other refs stay as they are.
pub(crate) fn branch_name(ref_name: &str) -> String {
    ref_name.strip_prefix("refs/heads/").unwrap_or(ref_name).to_owned()
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use time::macros::datetime;

    use super::*;

    #[derive(Deserialize)]
    struct Dates {
        #[serde(deserialize_with = "date::required")]
        created: time::OffsetDateTime,
        #[serde(default, deserialize_with = "date::optional")]
        unset: Option<time::OffsetDateTime>,
        #[serde(default, deserialize_with = "date::optional")]
        missing: Option<time::OffsetDateTime>,
    }

    #[test]
    fn parses_seven_fraction_digits_and_unset_sentinel() {
        let dates: Dates = serde_json::from_str(
            r#"{"created":"2026-02-18T04:35:01.9643061Z","unset":"0001-01-01T00:00:00"}"#,
        )
        .unwrap();
        assert_eq!(dates.created, datetime!(2026-02-18 04:35:01.9643061 UTC));
        assert_eq!(dates.unset, None);
        assert_eq!(dates.missing, None);
    }

    #[test]
    fn strips_heads_prefix_only() {
        assert_eq!(branch_name("refs/heads/feature/x"), "feature/x");
        assert_eq!(branch_name("refs/pull/5/merge"), "refs/pull/5/merge");
    }
}
