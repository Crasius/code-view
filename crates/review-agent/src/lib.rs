//! The pull request assistant's engine. It starts the user's own, unmodified `claude` or `codex`
//! CLI in its official machine mode (`claude -p` with stream-json in and out, `codex app-server`
//! over JSON-RPC) and turns either stream into one [`AgentEvent`] stream. Sessions, prompt caching,
//! sign-in and model routing stay with the CLI; this crate only starts it, talks to it and reads
//! its answers.
//!
//! The agent works read-only in a detached checkout of the pull request's head commit, without the
//! repository's own hooks, settings or MCP servers (a pull request can change those files), and
//! without the hosts' access tokens in its environment, so it cannot post, vote or merge.
//!
//! Layout: `locate` finds the CLI, `process` runs it, `driver` runs one session over a
//! `driver::Protocol`, which `claude` and `codex` implement on top of their pure stream readers;
//! `models` picks the model, `steps` says what kind of work a tool call is. `platform` holds the
//! only FFI (the Windows registry's `PATH`).

mod claude;
mod codex;
mod driver;
mod error;
mod locate;
mod models;
mod platform;
mod process;
mod session;
mod steps;

pub use error::AgentError;
pub use locate::find_program;
pub use review_core::{AgentKind, ModelChoice};
#[cfg(feature = "fake")]
pub use session::FakeAgent;
pub use session::{AgentEvent, Session, SessionConfig, ToolStep, Usage};
pub use steps::{StepKind, StepOutcome};

use std::path::PathBuf;

/// An installed agent CLI, found on `PATH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCli {
    pub kind: AgentKind,
    pub program: PathBuf,
    /// The CLI's own version line, for Settings.
    pub version: String,
}

/// A model the CLI offers, as the CLI names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// What the CLI accepts as `--model` (Claude) or `model` (Codex).
    pub id: String,
    pub display_name: String,
    pub description: String,
    /// The CLI's own default.
    pub is_default: bool,
    /// The reasoning efforts the model takes ("low", "medium", ...), as the CLI names them;
    /// empty when the CLI does not say.
    pub efforts: Vec<String>,
}

/// Finds the CLI on `PATH` (on Windows through `PATHEXT`, never a `.ps1`) and reads its version.
/// [`AgentError::NotInstalled`] when it is missing.
pub async fn detect(kind: AgentKind) -> Result<AgentCli, AgentError> {
    locate::detect(kind).await
}

/// The models the CLI offers, asked from the CLI itself, in the CLI's order. Starts the CLI in
/// its machine mode for a moment; no model is called.
pub async fn list_models(cli: &AgentCli) -> Result<Vec<ModelInfo>, AgentError> {
    match cli.kind {
        AgentKind::Claude => claude::list_models(cli).await,
        AgentKind::Codex => codex::list_models(cli).await,
    }
}

/// What a session asks the CLI for; `None` leaves that choice to the CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelPick {
    pub model: Option<String>,
    /// The reasoning effort ("low", "medium", ...), as the CLI names it.
    pub effort: Option<String>,
}

/// The model and effort to start a session with, from the user's choice and the CLI's models
/// ([`list_models`]; an empty list works too).
pub fn resolve_model(kind: AgentKind, choice: &ModelChoice, models: &[ModelInfo]) -> ModelPick {
    models::resolve(kind, choice, models)
}

/// The command that continues `session_id` in the full, interactive CLI in a terminal, or starts
/// a fresh session without one, run in the session's working directory: program and arguments.
pub fn terminal_command(kind: AgentKind, session_id: Option<&str>) -> (String, Vec<String>) {
    let program = locate::program_name(kind).to_owned();
    let Some(session_id) = session_id else {
        return (program, Vec::new());
    };
    match kind {
        AgentKind::Claude => (program, vec!["--resume".to_owned(), session_id.to_owned()]),
        AgentKind::Codex => (program, vec!["resume".to_owned(), session_id.to_owned()]),
    }
}
