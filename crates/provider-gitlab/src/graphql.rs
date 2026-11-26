//! What a host's GraphQL schema lacks.
//!
//! GitLab's schema differs by edition and release: the Community Edition has no Premium fields
//! (`MergeRequest.approvalsLeft` lives in `ee/app/graphql/ee/types/merge_request_type.rb`), and a
//! self-managed host on an older release lacks newer fields and arguments (`Project.pipeline(id:)`
//! came with 17.2). GraphQL rejects a whole query for one unknown field, so a query is built from
//! what the host is known to lack and leaves those parts out.
//!
//! The host says what it lacks in GraphQL's validation errors. GitLab runs graphql-ruby, whose
//! static validation puts the gap into `extensions`: `undefinedField` with `typeName` and
//! `fieldName`, `argumentNotAccepted` with the field as `name` and `argumentName`
//! (graphql-ruby `lib/graphql/static_validation/rules/fields_are_defined_on_type_error.rb` and
//! `arguments_are_defined_error.rb`). Only those two codes count; any other error stays an error.
//! The gaps are kept for the connection, so each is learned once.

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use serde::Deserialize;

/// The answer of `POST /api/graphql`; errors come with status 200.
#[derive(Deserialize)]
pub(crate) struct Envelope<T> {
    pub(crate) data: Option<T>,
    #[serde(default)]
    pub(crate) errors: Vec<GraphqlError>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GraphqlError {
    pub(crate) message: String,
    #[serde(default)]
    extensions: Option<Extensions>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Extensions {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    type_name: Option<String>,
    #[serde(default)]
    field_name: Option<String>,
    /// For `argumentNotAccepted`: the field (or its alias) the argument was given to.
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    argument_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Gap {
    /// `{type_name}.{field}` does not exist.
    Field { type_name: String, field: String },
    /// The field `field` takes no argument `argument`. graphql-ruby names no parent type here.
    Argument { field: String, argument: String },
}

impl GraphqlError {
    fn gap(&self) -> Option<Gap> {
        let extensions = self.extensions.as_ref()?;
        match extensions.code.as_deref()? {
            "undefinedField" => Some(Gap::Field {
                type_name: extensions.type_name.clone()?,
                field: extensions.field_name.clone()?,
            }),
            "argumentNotAccepted" => Some(Gap::Argument {
                field: extensions.name.clone()?,
                argument: extensions.argument_name.clone()?,
            }),
            _ => None,
        }
    }
}

/// The fields and arguments one host reported unknown.
#[derive(Default)]
pub(crate) struct SchemaGaps(Mutex<HashSet<Gap>>);

impl SchemaGaps {
    /// False once the host reported `{type_name}.{field}` unknown.
    pub(crate) fn has_field(&self, type_name: &str, field: &str) -> bool {
        let gap = Gap::Field { type_name: type_name.to_owned(), field: field.to_owned() };
        !self.lock().contains(&gap)
    }

    /// False once the host reported that `field` takes no `argument`.
    pub(crate) fn accepts(&self, field: &str, argument: &str) -> bool {
        let gap = Gap::Argument { field: field.to_owned(), argument: argument.to_owned() };
        !self.lock().contains(&gap)
    }

    /// Keeps the gaps `errors` report; true when one of them was new.
    pub(crate) fn learn(&self, errors: &[GraphqlError]) -> bool {
        let mut gaps = self.lock();
        let mut learned = false;
        for gap in errors.iter().filter_map(GraphqlError::gap) {
            learned |= gaps.insert(gap);
        }
        learned
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<Gap>> {
        // A panic while inserting leaves a valid set; the gaps stay usable.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(json: &str) -> Vec<GraphqlError> {
        serde_json::from_str(json).expect("fixture")
    }

    /// The error shapes graphql-ruby's static validation writes, as GitLab serves them.
    #[test]
    fn learns_unknown_fields_and_arguments_once_and_nothing_else() {
        let gaps = SchemaGaps::default();
        let answer = errors(
            r#"[{"message": "Field 'approvalsLeft' doesn't exist on type 'MergeRequest'",
                 "locations": [{"line": 3, "column": 24}],
                 "path": ["query", "project", "mergeRequests", "nodes", "approvalsLeft"],
                 "extensions": {"code": "undefinedField", "typeName": "MergeRequest", "fieldName": "approvalsLeft"}},
                {"message": "Field 'pipeline' doesn't accept argument 'id'",
                 "extensions": {"code": "argumentNotAccepted", "name": "pipeline", "typeName": "Field", "argumentName": "id"}}]"#,
        );
        assert!(gaps.learn(&answer));
        assert!(!gaps.has_field("MergeRequest", "approvalsLeft"));
        assert!(gaps.has_field("MergeRequest", "approvalsRequired"));
        assert!(!gaps.accepts("pipeline", "id"));
        assert!(gaps.accepts("pipeline", "iid"));
        assert!(!gaps.learn(&answer), "known already");
        let other = errors(
            r#"[{"message": "Internal server error"},
                {"message": "Variable $project of type ID! was provided invalid value", "extensions": {"code": "variableMismatch"}}]"#,
        );
        assert!(!gaps.learn(&other), "only the two schema codes are gaps");
    }
}
