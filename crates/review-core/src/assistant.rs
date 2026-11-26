//! The pull request assistant's shared vocabulary: which coding agent CLI answers and which model
//! it is asked for. The store keeps the defaults, `review-agent` drives the CLI.

use serde::{Deserialize, Serialize};

/// The coding agent CLI the assistant drives. Both run as the unmodified, signed-in CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum AgentKind {
    #[default]
    Claude,
    Codex,
}

impl AgentKind {
    pub const ALL: [Self; 2] = [Self::Claude, Self::Codex];

    /// The product name shown in the chat and in Settings.
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }
}

/// The model a new conversation asks the CLI for.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ModelChoice {
    /// The CLI's fastest, cheapest model, picked from what the CLI offers when the chat starts.
    #[default]
    Fastest,
    /// Whatever the CLI itself is configured to use.
    CliDefault,
    /// A model id exactly as the CLI lists or accepts it.
    Named(String),
}
