use crate::AgentKind;

/// Why the assistant cannot answer. Every variant maps to one next step in the chat.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    /// The CLI is not on `PATH`: the chat links its install guide.
    #[error("{} is not installed", .0.label())]
    NotInstalled(AgentKind),
    /// The CLI runs but has no sign-in: the chat offers a terminal to sign in.
    #[error("{} is not signed in", .0.label())]
    NotSignedIn(AgentKind),
    /// The process could not start.
    #[error("could not start the agent: {0}")]
    Spawn(String),
    /// The CLI answered something this version cannot read.
    #[error("unexpected answer from the agent: {0}")]
    Protocol(String),
    /// The CLI reported an error for the turn (rate limit, model not available, ...), its own words.
    #[error("{0}")]
    Turn(String),
    /// The process ended: its exit code and the last lines it wrote to stderr.
    #[error("the agent stopped (exit code {code:?}): {stderr_tail}")]
    Exited { code: Option<i32>, stderr_tail: String },
    /// The CLI did not give the named answer within our time limit.
    #[error("{0} did not answer in time")]
    Timeout(&'static str),
}
