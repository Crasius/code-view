//! Serde shapes of GitHub responses. REST DTOs use `snake_case`, GraphQL DTOs `camelCase`.
//! Only fields the mapping reads are declared.

// RS-DONT-04: parses provider data, so indexing and narrowing casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

pub(crate) mod actor;
pub(crate) mod comment;
pub(crate) mod label;
pub(crate) mod pipeline;
pub(crate) mod pull_request;
pub(crate) mod repository;
pub(crate) mod review;
pub(crate) mod review_thread;
pub(crate) mod search;
pub(crate) mod user;
