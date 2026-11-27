//! The conversation as the panel shows and stores it: each question opens a turn, the agent's
//! events fill the running one. Events with no running turn (a late answer to a stopped turn
//! after a restart) are dropped, so they never land under the wrong question.
//!
//! The agent's work becomes the turn's steps: each tool call, and each stretch of thinking, timed
//! here by the wall clock as the events arrive (the CLIs stream no timestamps of their own that
//! both share). A turn's "thought" is the summed time of its thinking blocks, not the wait from
//! the question to the first token: that wait also holds the checkout and the CLI's start, which
//! are no thinking. When the turn ends, thinking still open ends with it; a tool call whose end
//! never came keeps no duration. [`saver`] writes what is stored to disk.

pub(super) mod saver;

use std::time::{SystemTime, UNIX_EPOCH};

use review_agent::{AgentError, AgentEvent, StepKind, StepOutcome, ToolStep};
use review_store::{
    Chat, ChatQuote, ChatStep, ChatStepKind, ChatTurn, ChatUsage, TurnEnd, TurnKind,
};

/// What an event meant for the conversation.
#[derive(Debug, PartialEq)]
pub(crate) enum Applied {
    /// Text or a tool step went into the running turn (or nowhere).
    Progress,
    /// The CLI is ready and answers with `model`.
    Started { model: String },
    /// The running turn is complete.
    TurnDone,
    /// The turn or the session failed.
    Failed(AgentError),
    /// The process is gone.
    Closed,
}

/// Opens a turn for `question`, asked as `prompt`.
pub(crate) fn begin(
    chat: &mut Chat,
    kind: TurnKind,
    question: &str,
    quote: Option<ChatQuote>,
    prompt: String,
) {
    chat.turns.push(ChatTurn {
        kind,
        question: question.to_owned(),
        prompt,
        quote,
        ..ChatTurn::default()
    });
}

/// The turn the agent is answering, if any.
pub(crate) fn running(chat: &mut Chat) -> Option<&mut ChatTurn> {
    chat.turns.last_mut().filter(|turn| turn.end == TurnEnd::Running)
}

/// Takes in one event of the agent, arrived now.
pub(crate) fn apply(chat: &mut Chat, event: AgentEvent) -> Applied {
    apply_at(chat, event, now_ms())
}

/// Takes in one event of the agent, arrived at `now` (Unix milliseconds).
fn apply_at(chat: &mut Chat, event: AgentEvent, now: u64) -> Applied {
    if let AgentEvent::Started { session_id, model } = event {
        chat.session_id = Some(session_id);
        return Applied::Started { model };
    }
    let Some(turn) = running(chat) else {
        return without_turn(event);
    };
    match event {
        AgentEvent::Started { model, .. } => return Applied::Started { model },
        AgentEvent::Text(text) => turn.answer.push_str(&text),
        AgentEvent::Thinking(text) => think(turn, &text, now),
        AgentEvent::ThinkingDone => end_thinking(turn, now),
        AgentEvent::Tool(step) => turn.steps.push(tool_step(step, now)),
        AgentEvent::ToolDone { id, outcome } => {
            let open = turn.steps.iter_mut().rev().find(|step| {
                step.id == id
                    && step.duration_ms.is_none()
                    && step.kind != Some(ChatStepKind::Thinking)
            });
            if let Some(step) = open {
                step.duration_ms = step.started_ms.map(|started| now.saturating_sub(started));
                step.is_failed = outcome == StepOutcome::Failed;
            }
        }
        AgentEvent::TurnDone(usage) => {
            turn.usage = Some(ChatUsage {
                input: usage.input,
                output: usage.output,
                cache_read: usage.cache_read,
                cache_write: usage.cache_write,
            });
            end_turn(turn, TurnEnd::Done, now);
            return Applied::TurnDone;
        }
        AgentEvent::Failed(error) => {
            end_turn(turn, TurnEnd::Failed(error.to_string()), now);
            return Applied::Failed(error);
        }
        AgentEvent::Closed => {
            let end = TurnEnd::Failed("The agent stopped before it finished.".to_owned());
            end_turn(turn, end, now);
            return Applied::Closed;
        }
    }
    Applied::Progress
}

/// What `event` means while no turn runs: only the ends of the session count.
fn without_turn(event: AgentEvent) -> Applied {
    match event {
        AgentEvent::Started { model, .. } => Applied::Started { model },
        AgentEvent::TurnDone(_) => Applied::TurnDone,
        AgentEvent::Failed(error) => Applied::Failed(error),
        AgentEvent::Closed => Applied::Closed,
        AgentEvent::Text(_)
        | AgentEvent::Thinking(_)
        | AgentEvent::ThinkingDone
        | AgentEvent::Tool(_)
        | AgentEvent::ToolDone { .. } => Applied::Progress,
    }
}

/// More thinking: the open stretch grows, or a new one starts.
fn think(turn: &mut ChatTurn, text: &str, now: u64) {
    match turn.steps.last_mut().filter(|step| is_open_thinking(step)) {
        Some(step) => step.thinking.push_str(text),
        None => turn.steps.push(ChatStep {
            kind: Some(ChatStepKind::Thinking),
            started_ms: Some(now),
            thinking: text.to_owned(),
            ..ChatStep::default()
        }),
    }
}

fn end_thinking(turn: &mut ChatTurn, now: u64) {
    for step in turn.steps.iter_mut().filter(|step| is_open_thinking(step)) {
        step.duration_ms = step.started_ms.map(|started| now.saturating_sub(started));
    }
}

fn is_open_thinking(step: &ChatStep) -> bool {
    step.kind == Some(ChatStepKind::Thinking) && step.duration_ms.is_none()
}

fn tool_step(step: ToolStep, now: u64) -> ChatStep {
    ChatStep {
        tool: step.tool,
        detail: step.detail,
        kind: Some(chat_kind(step.kind)),
        id: step.id,
        started_ms: Some(now),
        ..ChatStep::default()
    }
}

/// The stored kind of a tool step.
fn chat_kind(kind: StepKind) -> ChatStepKind {
    match kind {
        StepKind::Read => ChatStepKind::Read,
        StepKind::Search => ChatStepKind::Search,
        StepKind::Run => ChatStepKind::Run,
        StepKind::Edit => ChatStepKind::Edit,
        StepKind::Other => ChatStepKind::Other,
    }
}

/// Ends the turn as `end`; thinking still open ends with it.
fn end_turn(turn: &mut ChatTurn, end: TurnEnd, now: u64) {
    end_thinking(turn, now);
    turn.end = end;
}

/// Ends the running turn as stopped by the user.
pub(crate) fn stop(chat: &mut Chat) {
    if let Some(turn) = running(chat) {
        end_turn(turn, TurnEnd::Stopped, now_ms());
    }
}

/// The wall clock in Unix milliseconds.
pub(crate) fn now_ms() -> u64 {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    u64::try_from(since_epoch.as_millis()).unwrap_or(u64::MAX)
}

/// A conversation read from disk or shown again: a turn the app closed during counts as stopped.
/// Its open thinking keeps no length, since the clock ran on while nothing was thinking.
pub(crate) fn settle_loaded(chat: &mut Chat) {
    if let Some(turn) = running(chat) {
        turn.end = TurnEnd::Stopped;
    }
}

#[cfg(test)]
mod tests {
    use review_agent::{AgentKind, Usage};

    use super::*;

    fn read(id: &str, path: &str) -> AgentEvent {
        let (id, tool, detail) = (id.into(), "Read".into(), path.into());
        AgentEvent::Tool(ToolStep { id, tool, kind: StepKind::Read, detail })
    }

    fn done(id: &str, outcome: StepOutcome) -> AgentEvent {
        AgentEvent::ToolDone { id: id.into(), outcome }
    }

    #[test]
    fn events_fill_the_running_turn_until_it_is_done() {
        let mut chat = Chat::default();
        begin(&mut chat, TurnKind::Question, "What changed?", None, String::new());
        let started = AgentEvent::Started { session_id: "s-9".into(), model: "haiku".into() };
        assert_eq!(apply(&mut chat, started), Applied::Started { model: "haiku".into() });
        assert_eq!(apply_at(&mut chat, read("t1", "src/a.rs"), 1_000), Applied::Progress);
        apply_at(&mut chat, AgentEvent::Text("It renames ".into()), 1_100);
        apply_at(&mut chat, done("t1", StepOutcome::Ok), 1_400);
        apply_at(&mut chat, AgentEvent::Text("`greet`.".into()), 1_500);
        let usage = Usage { input: 3, output: 40, cache_read: 12_000, cache_write: 0 };
        assert_eq!(apply_at(&mut chat, AgentEvent::TurnDone(usage), 1_600), Applied::TurnDone);

        assert_eq!(chat.session_id.as_deref(), Some("s-9"));
        let turn = &chat.turns[0];
        assert_eq!(turn.answer, "It renames `greet`.");
        let step = &turn.steps[0];
        assert_eq!((step.tool.as_str(), step.detail.as_str()), ("Read", "src/a.rs"));
        assert_eq!(
            (step.kind, step.duration_ms, step.is_failed),
            (Some(ChatStepKind::Read), Some(400), false)
        );
        assert_eq!(turn.usage.map(|usage| usage.cache_read), Some(12_000));
        assert_eq!(turn.end, TurnEnd::Done);

        apply(&mut chat, AgentEvent::Text("late".into()));
        assert_eq!(chat.turns[0].answer, "It renames `greet`.", "no running turn, no text");
    }

    /// Thinking is timed from its first event to its end, its text kept with it; a tool's end
    /// finds its step by id and says whether it failed; the turn's end ends open thinking, and a
    /// tool whose end never came keeps no duration.
    #[test]
    fn steps_are_timed_from_their_start_to_their_end() {
        let mut chat = Chat::default();
        begin(&mut chat, TurnKind::Question, "Why?", None, String::new());
        let thinking = |text: &str| AgentEvent::Thinking(text.into());
        apply_at(&mut chat, thinking(""), 0);
        apply_at(&mut chat, thinking("The parser branches"), 500);
        apply_at(&mut chat, thinking(" on the first byte."), 900);
        apply_at(&mut chat, AgentEvent::ThinkingDone, 6_000);
        apply_at(&mut chat, read("t1", "src/parse.rs"), 6_100);
        apply_at(&mut chat, read("t2", "src/lib.rs"), 6_200);
        apply_at(&mut chat, done("t2", StepOutcome::Failed), 6_700);
        apply_at(&mut chat, done("t9", StepOutcome::Ok), 6_800);
        apply_at(&mut chat, thinking("Enough."), 7_000);
        apply_at(&mut chat, AgentEvent::TurnDone(Usage::default()), 9_000);

        let steps = &chat.turns[0].steps;
        let shape: Vec<(Option<ChatStepKind>, Option<u64>, bool)> =
            steps.iter().map(|step| (step.kind, step.duration_ms, step.is_failed)).collect();
        let (thought, read) = (Some(ChatStepKind::Thinking), Some(ChatStepKind::Read));
        assert_eq!(
            shape,
            [
                (thought, Some(6_000), false),
                (read, None, false),
                (read, Some(500), true),
                (thought, Some(2_000), false),
            ]
        );
        assert_eq!(steps[0].thinking, "The parser branches on the first byte.");
    }

    #[test]
    fn failures_stops_and_restarts_end_the_running_turn() {
        let mut chat = Chat::default();
        begin(&mut chat, TurnKind::Question, "One", None, String::new());
        let error = AgentError::Turn("rate limited".into());
        assert_eq!(apply(&mut chat, AgentEvent::Failed(error.clone())), Applied::Failed(error));
        assert_eq!(chat.turns[0].end, TurnEnd::Failed("rate limited".into()));

        begin(&mut chat, TurnKind::Question, "Two", None, String::new());
        stop(&mut chat);
        assert_eq!(chat.turns[1].end, TurnEnd::Stopped);

        begin(&mut chat, TurnKind::Question, "Three", None, String::new());
        assert_eq!(apply(&mut chat, AgentEvent::Closed), Applied::Closed);
        assert!(matches!(chat.turns[2].end, TurnEnd::Failed(_)));

        let mut loaded = Chat { agent: AgentKind::Codex, ..Chat::default() };
        begin(&mut loaded, TurnKind::Question, "Cut off", None, String::new());
        apply_at(&mut loaded, AgentEvent::Thinking("x".into()), 0);
        settle_loaded(&mut loaded);
        assert_eq!(loaded.turns[0].end, TurnEnd::Stopped, "the app closed during the turn");
        assert_eq!(
            loaded.turns[0].steps[0].duration_ms, None,
            "thinking cut off by the close has no known length"
        );
    }
}
