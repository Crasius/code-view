//! Platform-neutral model of pull requests, reviews and pipelines plus the [`Provider`] trait
//! that GitHub, GitLab and Azure DevOps implement.

// RS-DONT-03: no FFI in this crate.
#![forbid(unsafe_code)]

pub mod assistant;
pub mod connection;
pub mod error;
pub mod http;
pub mod model;
pub mod provider;
pub mod setup;

/// The product name the app shows in its window, breadcrumbs and on platform tokens.
pub const APP_NAME: &str = "Code View";

pub use assistant::{AgentKind, ModelChoice};
pub use connection::{Connection, ProviderKind};
pub use error::{ProviderError, Result};
// The model is the crate's surface: re-exported whole, like a prelude (RS-READ-08 exempts it).
pub use model::*;
pub use provider::{Capabilities, Provider};
pub use setup::{AccessProblem, AccessReport, Feature, ProblemAction, ScopeCandidate, TokenGuide};
