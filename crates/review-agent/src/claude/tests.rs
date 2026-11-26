//! Claude Code's stream from trimmed probe recordings (Claude Code 2.1.304, `--model haiku`).
//! `turn-thinking.jsonl` is written after the Messages API's documented streaming shapes (thinking,
//! redacted thinking and signature blocks), not recorded: the recordings kept only the thinking
//! block's stop.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::stream::ClaudeStream;
use super::{args, models_from_initialize};
use crate::{
    AgentCli, AgentError, AgentEvent, AgentKind, ModelChoice, ModelPick, SessionConfig, StepKind,
    StepOutcome, ToolStep, Usage, resolve_model,
};

const INITIALIZE: &str = include_str!("../../tests/fixtures/claude/initialize.jsonl");
const TURN_TEXT: &str = include_str!("../../tests/fixtures/claude/turn-text.jsonl");
const TURN_TOOLS: &str = include_str!("../../tests/fixtures/claude/turn-tools.jsonl");
const TURN_INTERRUPTED: &str = include_str!("../../tests/fixtures/claude/turn-interrupted.jsonl");
const TURN_THINKING: &str = include_str!("../../tests/fixtures/claude/turn-thinking.jsonl");
const NOT_SIGNED_IN: &str = include_str!("../../tests/fixtures/claude/not-signed-in.jsonl");
const SESSION: &str = "359d5d55-e977-442a-86e3-a94c827a4672";

fn read_all(stream: &mut ClaudeStream, lines: &str) -> Vec<AgentEvent> {
    lines.lines().flat_map(|line| stream.read(line).0).collect()
}

fn answer(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn final_result(lines: &str) -> String {
    let result =
        lines.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).next_back();
    result.and_then(|result| result["result"].as_str().map(str::to_owned)).unwrap()
}

#[test]
fn a_text_turn_starts_the_session_and_shows_the_streamed_text_once() {
    let mut stream = ClaudeStream::new("C:\\work\\pr-1");
    let events = read_all(&mut stream, TURN_TEXT);
    let started = AgentEvent::Started {
        session_id: SESSION.to_owned(),
        model: "claude-haiku-5-5".to_owned(),
    };
    let usage = Usage { input: 2, output: 4, cache_read: 4557, cache_write: 2679 };
    assert_eq!(events, [started, AgentEvent::Text("pong".to_owned()), AgentEvent::TurnDone(usage)]);
}

#[test]
fn a_tool_turn_shows_each_call_and_the_answer_without_repeats() {
    let mut stream = ClaudeStream::new("C:\\work\\pr-1");
    let events = read_all(&mut stream, TURN_TOOLS);
    let tools: Vec<&ToolStep> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Tool(step) => Some(step),
            _ => None,
        })
        .collect();
    let bash = |id: &str, command: &str| ToolStep {
        id: id.to_owned(),
        tool: "Bash".to_owned(),
        kind: StepKind::Run,
        detail: command.to_owned(),
    };
    let echo = bash("toolu_01Xf9BnUKG6bHvyPMJPzCL43", "echo hi > y.txt");
    let status = bash("toolu_01GPQ6MQw21XU7uYddtNatRb", "git status");
    assert_eq!(tools, [&echo, &status]);
    let ends: Vec<&AgentEvent> =
        events.iter().filter(|event| matches!(event, AgentEvent::ToolDone { .. })).collect();
    let failed =
        |id: &str| AgentEvent::ToolDone { id: id.to_owned(), outcome: StepOutcome::Failed };
    assert_eq!(ends, [&failed(&echo.id), &failed(&status.id)], "denied, then exit code 128");
    assert_eq!(answer(&events), final_result(TURN_TOOLS), "every delta once, nothing twice");
    assert!(!events.iter().any(|event| matches!(event, AgentEvent::Failed(_))));
    assert!(
        matches!(events.last(), Some(AgentEvent::TurnDone(usage)) if usage.cache_read == 14070)
    );
}

#[test]
fn thinking_streams_as_its_own_steps_around_the_tool_and_its_end() {
    let mut stream = ClaudeStream::new("C:\\work\\pr-1");
    let events = read_all(&mut stream, TURN_THINKING);
    let thinking = |text: &str| AgentEvent::Thinking(text.to_owned());
    let read = ToolStep {
        id: "toolu_read_1".to_owned(),
        tool: "Read".to_owned(),
        kind: StepKind::Read,
        detail: "src/parse.rs".to_owned(),
    };
    let expected = [
        thinking(""),
        thinking("The parser branches on the first byte."),
        thinking(" I should read it."),
        AgentEvent::ThinkingDone,
        AgentEvent::Tool(read),
        AgentEvent::ToolDone { id: "toolu_read_1".to_owned(), outcome: StepOutcome::Ok },
        thinking(""),
        AgentEvent::ThinkingDone,
        AgentEvent::Text("It parses.".to_owned()),
    ];
    assert_eq!(events.get(1..10), Some(&expected[..]), "redacted thinking has no text");
    assert!(matches!(events.last(), Some(AgentEvent::TurnDone(_))));
}

#[test]
fn an_interrupted_turn_ends_without_an_error_and_the_session_goes_on() {
    let mut stream = ClaudeStream::new("C:\\work\\pr-1");
    stream.interrupt_requested();
    let events = read_all(&mut stream, TURN_INTERRUPTED);
    let done = events.iter().filter(|event| matches!(event, AgentEvent::TurnDone(_))).count();
    let started = events.iter().filter(|event| matches!(event, AgentEvent::Started { .. })).count();
    assert_eq!((done, started), (2, 1), "two turns, one start");
    assert!(!events.iter().any(|event| matches!(event, AgentEvent::Failed(_))));
    assert!(
        answer(&events).ends_with(
            "The first thing you asked was for me to reply with the single word \"pong.\""
        )
    );
}

#[test]
fn a_missing_sign_in_is_not_signed_in_and_shows_no_text() {
    let mut stream = ClaudeStream::new("C:\\work\\pr-1");
    let events = read_all(&mut stream, NOT_SIGNED_IN);
    let failed = AgentEvent::Failed(AgentError::NotSignedIn(AgentKind::Claude));
    assert_eq!(events.get(1..), Some(&[failed, AgentEvent::TurnDone(Usage::default())][..]));
    assert_eq!(answer(&events), "");
}

#[test]
fn a_failed_turn_reports_the_clis_own_words() {
    let mut stream = ClaudeStream::new("C:\\work\\pr-1");
    let limit = json!({"type": "result", "subtype": "success", "is_error": true, "result": "Claude AI usage limit reached|1792498800"});
    let events = stream.read(&limit.to_string()).0;
    assert_eq!(
        events.first(),
        Some(&AgentEvent::Failed(AgentError::Turn(
            "Claude AI usage limit reached|1792498800".to_owned()
        )))
    );
}

#[test]
fn tool_paths_show_inside_the_checkout_and_permission_requests_are_denied() {
    let mut stream = ClaudeStream::new("C:\\work\\pr-1");
    let call = json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": "m1", "content": [
        {"type": "tool_use", "name": "Read", "input": {"file_path": "C:\\work\\pr-1\\src\\lib.rs"}},
        {"type": "tool_use", "name": "Grep", "input": {"pattern": "fn main", "path": "C:\\work\\pr-1\\src"}},
    ]}});
    let steps = stream.read(&call.to_string()).0;
    let details: Vec<(&str, StepKind, &str)> = steps
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Tool(step) => Some((step.tool.as_str(), step.kind, step.detail.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        details,
        [("Read", StepKind::Read, "src/lib.rs"), ("Grep", StepKind::Search, "fn main in src")]
    );

    let ask = json!({"type": "control_request", "request_id": "r7", "request": {"subtype": "can_use_tool", "tool_name": "Write"}});
    let replies = stream.read(&ask.to_string()).1;
    assert_eq!(replies[0]["response"]["request_id"], "r7");
    assert_eq!(replies[0]["response"]["response"]["behavior"], "deny");
}

#[test]
fn the_cli_lists_its_models_with_their_efforts_and_the_fastest_is_haiku_at_low() {
    let answer =
        INITIALIZE.lines().find_map(|line| serde_json::from_str::<Value>(line).ok()).unwrap();
    let models = models_from_initialize(&answer["response"]["response"]);
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(ids.get(..4), Some(&["opus", "fable", "sonnet", "haiku"][..]));
    assert!(!ids.contains(&"default"));
    let defaults: Vec<&str> =
        models.iter().filter(|model| model.is_default).map(|model| model.id.as_str()).collect();
    assert_eq!(defaults, ["opus"], "the entry the CLI's default resolves to");
    let pick = resolve_model(AgentKind::Claude, &ModelChoice::Fastest, &models);
    assert_eq!(pick, ModelPick { model: Some("haiku".to_owned()), effort: Some("low".to_owned()) });
}

#[test]
fn without_a_model_list_the_documented_aliases_stand_in() {
    let ids: Vec<String> =
        models_from_initialize(&json!({})).into_iter().map(|model| model.id).collect();
    assert_eq!(ids, ["haiku", "sonnet", "opus"]);
}

#[test]
fn the_session_runs_restricted_with_read_tools_and_the_brief_in_a_file() {
    let config = SessionConfig {
        cli: AgentCli {
            kind: AgentKind::Claude,
            program: PathBuf::from("claude"),
            version: String::new(),
        },
        pick: ModelPick { model: Some("haiku".to_owned()), effort: Some("low".to_owned()) },
        workdir: PathBuf::from("C:\\work\\pr-1"),
        brief: "brief".to_owned(),
        resume: Some(SESSION.to_owned()),
    };
    let args: Vec<String> = args(&config, Path::new("brief.md"))
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let line = args.join(" ");
    assert!(line.starts_with("-p --input-format stream-json --output-format stream-json --verbose --include-partial-messages"));
    assert!(line.contains(&format!("--model haiku --effort low --resume {SESSION}")));
    assert!(line.contains("--restricted --strict-mcp-config --tools Read,Grep,Glob,Bash --allowedTools Bash(git log:*)"));
    assert!(line.ends_with("--permission-mode dontAsk --append-system-prompt-file brief.md"));
}
