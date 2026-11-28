//! Pure DTO → domain mapping. No I/O, so every rule here is unit tested with JSON fixtures.

// RS-DONT-04: parses provider data, so indexing and narrowing casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

pub(crate) mod checks;
pub(crate) mod identity;
pub(crate) mod pipelines;
pub(crate) mod pull_request;
pub(crate) mod reviewers;
pub(crate) mod run_status;
pub(crate) mod search;
pub(crate) mod thread_id;
pub(crate) mod threads;
pub(crate) mod workflow;

#[cfg(test)]
pub(crate) mod fixtures {
    use serde::de::DeserializeOwned;

    pub(crate) fn parse<T: DeserializeOwned>(json: &str) -> T {
        serde_json::from_str(json).unwrap_or_else(|error| panic!("fixture does not parse: {error}"))
    }
}
