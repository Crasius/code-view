use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::{AgentCli, AgentError, StepKind, StepOutcome};

/// Longest tool detail the chat shows.
const DETAIL_CHARS: usize = 160;

/// What a session starts with.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub cli: AgentCli,
    /// Model and effort; [`crate::resolve_model`] picks them.
    pub pick: crate::ModelPick,
    /// The checkout the agent reads: the pull request's head commit, detached.
    pub workdir: PathBuf,
    /// The pull request brief (title, description, changed files, threads, checks), appended to
    /// the CLI's own system prompt.
    pub brief: String,
    /// A session id from an earlier run of the same conversation, to continue it.
    pub resume: Option<String>,
}

/// One tool call the agent started, as a step of the chat's activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStep {
    /// The CLI's id of the call, which its [`AgentEvent::ToolDone`] names.
    pub id: String,
    /// The tool's name as the CLI reports it ("Read", "Grep", "commandExecution", ...).
    pub tool: String,
    pub kind: StepKind,
    /// What it worked on: a path, a pattern or a command line.
    pub detail: String,
}

impl ToolStep {
    /// The step with the first line of `detail`, at most [`DETAIL_CHARS`] characters of it.
    pub(crate) fn new(id: String, tool: String, kind: StepKind, detail: &str) -> Self {
        let first = detail.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
        let mut short: String = first.chars().take(DETAIL_CHARS).collect();
        if first.chars().count() > DETAIL_CHARS {
            short.push('…');
        }
        Self { id, tool, kind, detail: short }
    }
}

/// Token counts of one turn, as the CLI reports them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// What the agent did, in order.
///
/// A turn ends with [`AgentEvent::TurnDone`], also after it failed ([`AgentEvent::Failed`] comes
/// first) or was interrupted. A failure of the whole session (the CLI is not signed in at the
/// start, the process could not start or died) ends with [`AgentEvent::Closed`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// The CLI is ready: its session id (to continue it later) and the model that answers. Once
    /// per session; Claude reports it with its answer to the first question.
    Started { session_id: String, model: String },
    /// More answer text, to append to the current answer.
    Text(String),
    /// The model thinks: a thinking block began (empty text) or more of its text came. A CLI or
    /// model that keeps its thinking to itself sends the block without text, or nothing.
    Thinking(String),
    /// The thinking block ended.
    ThinkingDone,
    /// A tool call started.
    Tool(ToolStep),
    /// The tool call `id` ([`ToolStep::id`]) finished.
    ToolDone { id: String, outcome: StepOutcome },
    /// The turn is complete; the next prompt may go.
    TurnDone(Usage),
    /// The turn or the session failed.
    Failed(AgentError),
    /// The process is gone; the session takes no more prompts.
    Closed,
}

pub(crate) enum Command {
    Prompt(String),
    Interrupt,
}

/// A running conversation with one CLI process. Dropping it ends the process.
pub struct Session {
    commands: mpsc::UnboundedSender<Command>,
}

impl Session {
    /// Starts the CLI for `config` on the current tokio runtime. Events arrive on the receiver
    /// until [`AgentEvent::Closed`]; a process that cannot start reports
    /// [`AgentError::Spawn`] there. Errors here only without a tokio runtime.
    ///
    /// Questions asked while a turn runs wait for it; [`Session::interrupt`] stops the running
    /// turn and leaves the waiting questions queued.
    pub fn start(
        config: SessionConfig,
    ) -> Result<(Self, mpsc::UnboundedReceiver<AgentEvent>), AgentError> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| AgentError::Spawn(format!("no async runtime: {error}")))?;
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (events, event_rx) = mpsc::unbounded_channel();
        runtime.spawn(crate::driver::run(config, command_rx, events));
        Ok((Self { commands }, event_rx))
    }

    /// Sends the user's next question. Ignored once the session is closed.
    pub fn ask(&self, prompt: impl Into<String>) {
        // A closed session reports Closed on its event stream; nothing more to do here.
        let _sent = self.commands.send(Command::Prompt(prompt.into()));
    }

    /// Stops the current turn; the session stays open for the next question.
    pub fn interrupt(&self) {
        let _sent = self.commands.send(Command::Interrupt);
    }

    /// A session without a process for the app's walkthroughs: the returned [`FakeAgent`] sees
    /// every prompt and plays events back.
    #[cfg(feature = "fake")]
    pub fn fake() -> (Self, mpsc::UnboundedReceiver<AgentEvent>, FakeAgent) {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (events, event_rx) = mpsc::unbounded_channel();
        (Self { commands }, event_rx, FakeAgent { commands: command_rx, events })
    }
}

/// The other end of [`Session::fake`].
#[cfg(feature = "fake")]
pub struct FakeAgent {
    commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<AgentEvent>,
}

#[cfg(feature = "fake")]
impl FakeAgent {
    /// The prompts asked so far and not yet taken; interrupts show as `None`.
    pub fn take_prompts(&mut self) -> Vec<Option<String>> {
        let mut taken = Vec::new();
        while let Ok(command) = self.commands.try_recv() {
            taken.push(match command {
                Command::Prompt(prompt) => Some(prompt),
                Command::Interrupt => None,
            });
        }
        taken
    }

    pub fn emit(&self, event: AgentEvent) {
        let _sent = self.events.send(event);
    }
}
