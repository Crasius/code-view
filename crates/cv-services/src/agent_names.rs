//! The names and install guides of the agent CLIs the assistant runs (Claude Code, Codex).
//!
//! They live beside the agent detection in `cv-services` because both the pull request
//! assistant and Settings → Assistant show them, and Settings must not depend on the assistant.

use review_agent::AgentKind;

/// Install guides, linked where a CLI is missing.
pub const CLAUDE_INSTALL_URL: &str = "https://docs.anthropic.com/en/docs/claude-code/setup";
pub const CODEX_INSTALL_URL: &str = "https://github.com/openai/codex";

/// The product name of `kind`'s CLI.
pub fn cli_name(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "Claude Code",
        AgentKind::Codex => "Codex",
    }
}

/// The install guide of `kind`'s CLI.
pub fn install_url(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => CLAUDE_INSTALL_URL,
        AgentKind::Codex => CODEX_INSTALL_URL,
    }
}
