//! One session's loop, shared by both CLIs: start the process, hand questions to the protocol one
//! turn at a time, turn its stdout lines into events and its replies into stdin lines, and close.
//! What differs per CLI (the launch, the messages, the stream format) sits behind [`Protocol`].

use std::collections::VecDeque;
use std::path::PathBuf;

use serde_json::Value;
use tokio::process::Command as ProcessCommand;
use tokio::sync::mpsc;

use crate::process::{AgentProcess, EXIT_GRACE};
use crate::session::Command;
use crate::{AgentError, AgentEvent, AgentKind, SessionConfig, claude, codex};

/// What one stdout line of the CLI leads to.
#[derive(Debug, Default)]
pub(crate) struct Reaction {
    pub(crate) events: Vec<AgentEvent>,
    /// Lines for the CLI's stdin: answers to its requests, the next step of a handshake.
    pub(crate) send: Vec<Value>,
    /// The session cannot go on (for example the CLI is not signed in); end the process.
    pub(crate) close: bool,
}

/// One CLI's machine protocol over JSON lines.
pub(crate) trait Protocol: Send {
    /// What to send right after the process started.
    fn opening(&mut self) -> Vec<Value>;
    /// True when a question may go now: the session is set up and no turn runs.
    fn is_ready(&self) -> bool;
    /// The lines that ask `text` as the next turn.
    fn prompt(&mut self, text: &str) -> Vec<Value>;
    /// The lines that stop the running turn; none when no turn runs.
    fn interrupt(&mut self) -> Vec<Value>;
    fn read(&mut self, line: &str) -> Reaction;
}

/// Joins a turn's separate text blocks (Claude's text between tool calls, Codex's agent messages)
/// into one answer, with a blank line between two blocks.
#[derive(Debug, Default)]
pub(crate) struct AnswerText {
    flow: TextFlow,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum TextFlow {
    #[default]
    Empty,
    /// Text was shown; more text of the same block continues it.
    InBlock,
    /// Text was shown and its block ended; the next text starts a paragraph.
    BlockEnded,
}

impl AnswerText {
    /// A new text block begins.
    pub(crate) fn new_block(&mut self) {
        if self.flow == TextFlow::InBlock {
            self.flow = TextFlow::BlockEnded;
        }
    }

    pub(crate) fn push(&mut self, text: &str, events: &mut Vec<AgentEvent>) {
        if text.is_empty() {
            return;
        }
        if self.flow == TextFlow::BlockEnded {
            events.push(AgentEvent::Text("\n\n".to_owned()));
        }
        self.flow = TextFlow::InBlock;
        events.push(AgentEvent::Text(text.to_owned()));
    }
}

/// A CLI process to start and the protocol to speak with it.
pub(crate) struct Launch {
    pub(crate) command: ProcessCommand,
    pub(crate) protocol: Box<dyn Protocol>,
    /// A file that only lives as long as the session (Claude's brief).
    pub(crate) temporary_file: Option<PathBuf>,
}

/// Why the conversation loop ended.
#[derive(PartialEq, Eq)]
enum End {
    /// The [`crate::Session`] was dropped.
    Owner,
    /// The protocol gave up; it reported why.
    Protocol,
    /// The process closed its output or its input on its own.
    Process,
}

pub(crate) async fn run(
    config: SessionConfig,
    commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<AgentEvent>,
) {
    let launch = match config.cli.kind {
        AgentKind::Claude => claude::launch(&config).await,
        AgentKind::Codex => Ok(codex::launch(&config)),
    };
    let emit = |event: AgentEvent| {
        // The owner may have dropped the receiver; the session then just runs down.
        let _receiver_may_be_gone = events.send(event);
    };
    match launch {
        Ok(Launch { command, mut protocol, temporary_file }) => {
            match AgentProcess::spawn(command) {
                Ok(process) => converse(process, protocol.as_mut(), commands, &emit).await,
                Err(error) => emit(AgentEvent::Failed(error)),
            }
            if let Some(file) = temporary_file {
                // A leftover brief in the temp folder harms nothing; the next start writes anew.
                let _removal_is_best_effort = tokio::fs::remove_file(file).await;
            }
        }
        Err(error) => emit(AgentEvent::Failed(error)),
    }
    emit(AgentEvent::Closed);
}

async fn converse(
    mut process: AgentProcess,
    protocol: &mut dyn Protocol,
    mut commands: mpsc::UnboundedReceiver<Command>,
    emit: &impl Fn(AgentEvent),
) {
    let mut queued: VecDeque<String> = VecDeque::new();
    let mut end = send_all(&mut process, protocol.opening()).await;
    while end.is_none() {
        if protocol.is_ready()
            && let Some(prompt) = queued.pop_front()
        {
            end = send_all(&mut process, protocol.prompt(&prompt)).await;
            continue;
        }
        tokio::select! {
            command = commands.recv() => match command {
                None => end = Some(End::Owner),
                Some(Command::Prompt(prompt)) => queued.push_back(prompt),
                Some(Command::Interrupt) => end = send_all(&mut process, protocol.interrupt()).await,
            },
            line = process.next_line() => {
                let Some(line) = line else {
                    end = Some(End::Process);
                    continue;
                };
                let reaction = protocol.read(&line);
                reaction.events.into_iter().for_each(emit);
                end = send_all(&mut process, reaction.send).await;
                if reaction.close && end.is_none() {
                    end = Some(End::Protocol);
                }
            }
        }
    }
    let (code, stderr_tail) = process.finish(EXIT_GRACE).await;
    if end == Some(End::Process) {
        emit(AgentEvent::Failed(AgentError::Exited { code, stderr_tail }));
    }
}

/// Sends `messages` in order; a write that fails means the process is gone.
async fn send_all(process: &mut AgentProcess, messages: Vec<Value>) -> Option<End> {
    for message in messages {
        if process.send(&message).await.is_err() {
            return Some(End::Process);
        }
    }
    None
}
