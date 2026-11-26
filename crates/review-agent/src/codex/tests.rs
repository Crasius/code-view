//! Codex's app-server protocol from trimmed probe recordings (codex-cli 0.163.0).
//! `turn-reasoning.jsonl` is written after the schema `codex app-server generate-json-schema`
//! writes (reasoning items and their summary deltas, parsed read and search commands), not
//! recorded: the recorded turns had no reasoning. `turn-sandbox-setup-failed.jsonl` is a turn
//! whose command the elevated Windows sandbox could not start; its agent messages are translated.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::CodexProtocol;
use super::protocol::{self, ThreadSetup, TurnReader};
use crate::driver::Protocol;
use crate::{
    AgentCli, AgentError, AgentEvent, AgentKind, ModelChoice, ModelPick, SessionConfig, StepKind,
    StepOutcome, ToolStep, Usage, resolve_model,
};

const MODEL_LIST: &str = include_str!("../../tests/fixtures/codex/model-list.jsonl");
const THREAD_START: &str = include_str!("../../tests/fixtures/codex/thread-start.jsonl");
const TURN_TEXT: &str = include_str!("../../tests/fixtures/codex/turn-text.jsonl");
const TURN_TOOLS: &str = include_str!("../../tests/fixtures/codex/turn-tools.jsonl");
const TURN_INTERRUPTED: &str = include_str!("../../tests/fixtures/codex/turn-interrupted.jsonl");
const TURN_REASONING: &str = include_str!("../../tests/fixtures/codex/turn-reasoning.jsonl");
const NOT_SIGNED_IN: &str = include_str!("../../tests/fixtures/codex/turn-not-signed-in.jsonl");
const SANDBOX_SETUP_FAILED: &str =
    include_str!("../../tests/fixtures/codex/turn-sandbox-setup-failed.jsonl");
const THREAD: &str = "01a15939-1521-74d0-aba4-406184d368e9";

/// The events of the notifications in `lines`; answers to requests are not the reader's.
fn notifications(reader: &mut TurnReader, lines: &str) -> Vec<AgentEvent> {
    lines
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|message| message.get("id").is_none())
        .flat_map(|message| {
            let method = message["method"].as_str().unwrap().to_owned();
            reader.notification(&method, &message["params"])
        })
        .collect()
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

#[test]
fn a_tool_turn_shows_each_command_its_paragraphs_and_its_own_token_counts() {
    let mut reader = TurnReader::new(THREAD);
    assert_eq!(answer(&notifications(&mut reader, TURN_TEXT)), "pong");
    let events = notifications(&mut reader, TURN_TOOLS);

    let steps: Vec<&ToolStep> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Tool(step) => Some(step),
            _ => None,
        })
        .collect();
    let details: Vec<&str> = steps.iter().map(|step| step.detail.as_str()).collect();
    assert_eq!(details.get(..2), Some(&["echo hi > x.txt", "git log -1"][..]));
    assert!(steps.iter().all(|step| step.tool == "commandExecution" && step.kind == StepKind::Run));
    let outcomes: Vec<StepOutcome> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolDone { id, outcome } => {
                assert!(steps.iter().any(|step| &step.id == id), "the end names a started step");
                Some(*outcome)
            }
            _ => None,
        })
        .collect();
    let (ok, failed) = (StepOutcome::Ok, StepOutcome::Failed);
    assert_eq!(outcomes, [failed, failed, ok], "exit codes 1, 1, 0");
    assert_eq!(
        answer(&events),
        "I\u{2019}ll run the requested steps in order and report each result.\n\n\
         - `echo hi > x.txt` failed: access to `x.txt` was denied.\n\
         - `git log -1` failed: this directory is not a Git repository.\n\
         - `brief.md` exists and contains `# brief`."
    );
    let usage =
        Usage { input: 106_798 - 21_122, output: 219 - 5, cache_read: 84_608, cache_write: 0 };
    assert_eq!(events.last(), Some(&AgentEvent::TurnDone(usage)));
}

#[test]
fn reasoning_streams_as_thinking_and_parsed_commands_read_and_search() {
    let mut reader = TurnReader::new(THREAD);
    let events = notifications(&mut reader, TURN_REASONING);
    let step = |id: &str, kind: StepKind, detail: &str| {
        AgentEvent::Tool(ToolStep {
            id: id.to_owned(),
            tool: "commandExecution".to_owned(),
            kind,
            detail: detail.to_owned(),
        })
    };
    let done = |id: &str, outcome: StepOutcome| AgentEvent::ToolDone { id: id.to_owned(), outcome };
    let thinking = |text: &str| AgentEvent::Thinking(text.to_owned());
    let expected = [
        thinking(""),
        thinking("\n\n"),
        thinking("**Checking the parser**"),
        AgentEvent::ThinkingDone,
        step("exec-read-1", StepKind::Read, "src/parse.rs"),
        done("exec-read-1", StepOutcome::Ok),
        step("exec-search-1", StepKind::Search, "hunk_len"),
        done("exec-search-1", StepOutcome::Failed),
        AgentEvent::TurnDone(Usage::default()),
    ];
    assert_eq!(events, expected);
}

#[test]
fn an_interrupted_turn_ends_without_an_error() {
    let mut reader = TurnReader::new(THREAD);
    let events = notifications(&mut reader, TURN_INTERRUPTED);
    assert!(!events.iter().any(|event| matches!(event, AgentEvent::Failed(_))));
    assert!(matches!(events.last(), Some(AgentEvent::TurnDone(_))));
}

#[test]
fn a_turn_rejected_with_401_is_not_signed_in() {
    let mut reader = TurnReader::new("01a1593a-dd97-7892-aa9b-dee62855a7bc");
    let events = notifications(&mut reader, NOT_SIGNED_IN);
    let failed = AgentEvent::Failed(AgentError::NotSignedIn(AgentKind::Codex));
    assert_eq!(events, [failed, AgentEvent::TurnDone(Usage::default())], "retries say nothing");
}

#[test]
fn a_command_codex_cannot_start_ends_the_turn_with_a_next_step() {
    let mut reader = TurnReader::new("01a16050-4580-7aa0-98ac-dfe14db3d296");
    let events = notifications(&mut reader, SANDBOX_SETUP_FAILED);
    let id = "exec-8a4025fa-4bea-4c49-b73a-b4854be6ffdb";
    assert!(
        events.contains(&AgentEvent::ToolDone { id: id.to_owned(), outcome: StepOutcome::Failed })
    );
    let failed = AgentEvent::Failed(AgentError::Turn(
        "Codex could not start a command in its sandbox (helper_unknown_error: setup refresh had \
         errors). Run `codex doctor` in a terminal: its sandbox section names what failed and how \
         to repair it. Then ask again."
            .to_owned(),
    ));
    assert_eq!(
        events.get(events.len() - 2..),
        Some(&[failed, AgentEvent::TurnDone(Usage::default())][..])
    );
    let words = "The command failed before it started: `setup refresh had errors`.";
    assert!(answer(&events).ends_with(words), "Codex's own answer stays");
}

#[test]
fn notifications_of_another_thread_are_ignored() {
    let mut reader = TurnReader::new("main-thread");
    let delta = json!({"threadId": "sub-agent", "itemId": "m", "delta": "hidden"});
    assert_eq!(reader.notification("item/agentMessage/delta", &delta), []);
}

#[test]
fn the_model_list_keeps_its_order_skips_hidden_models_and_the_fastest_is_the_fast_one() {
    let page: Value = serde_json::from_str(MODEL_LIST.trim()).unwrap();
    let (models, cursor) = protocol::model_page(&page["result"]);
    assert_eq!(cursor, None);
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(ids.first(), Some(&"gpt-6.1-sol"));
    assert!(models.first().unwrap().is_default);
    let pick = resolve_model(AgentKind::Codex, &ModelChoice::Fastest, &models);
    assert_eq!(
        pick,
        ModelPick { model: Some("gpt-6-luna".to_owned()), effort: Some("low".to_owned()) }
    );

    let hidden = json!({"data": [{"model": "secret", "hidden": true}, {"model": "shown"}], "nextCursor": "c2"});
    let (models, cursor) = protocol::model_page(&hidden);
    assert_eq!(models.iter().map(|model| model.id.as_str()).collect::<Vec<_>>(), ["shown"]);
    assert_eq!(cursor.as_deref(), Some("c2"));
}

#[test]
fn approvals_are_declined_and_unknown_requests_refused() {
    assert_eq!(
        protocol::decline("item/commandExecution/requestApproval"),
        Some(json!({"decision": "decline"}))
    );
    assert_eq!(protocol::decline("execCommandApproval"), Some(json!({"decision": "denied"})));
    assert_eq!(protocol::decline("item/tool/call"), None);
}

fn config(resume: Option<&str>) -> SessionConfig {
    SessionConfig {
        cli: AgentCli {
            kind: AgentKind::Codex,
            program: PathBuf::from("codex"),
            version: String::new(),
        },
        pick: ModelPick { model: Some("gpt-6-luna".to_owned()), effort: Some("low".to_owned()) },
        workdir: PathBuf::from("C:\\work\\pr-1"),
        brief: "The brief.".to_owned(),
        resume: resume.map(str::to_owned),
    }
}

/// Feeds `message` to `codex` and returns what it sends back.
fn answer_with(codex: &mut CodexProtocol, message: &Value) -> crate::driver::Reaction {
    codex.read(&message.to_string())
}

#[test]
fn a_session_signs_in_switches_off_mcp_servers_and_starts_a_read_only_thread() {
    let mut codex = CodexProtocol::new(&config(None));
    let opening = codex.opening();
    assert_eq!(opening[0]["method"], "initialize");

    let account = answer_with(&mut codex, &json!({"id": 1, "result": {}})).send;
    assert_eq!(account[0], json!({"method": "initialized"}));
    assert_eq!(account[1]["method"], "account/read");
    let config_read = answer_with(
        &mut codex,
        &json!({"id": 2, "result": {"account": {"type": "chatgpt"}, "requiresOpenaiAuth": true}}),
    )
    .send;
    assert_eq!(config_read[0]["params"]["cwd"], "C:\\work\\pr-1");
    let mcp =
        json!({"id": 3, "result": {"config": {"mcp_servers": {"playwright": {}, "rider": {}}}}});
    let thread = answer_with(&mut codex, &mcp).send;
    let params = &thread[0]["params"];
    assert_eq!(thread[0]["method"], "thread/start");
    assert_eq!(
        (params["sandbox"].as_str(), params["approvalPolicy"].as_str()),
        (Some("read-only"), Some("never"))
    );
    assert_eq!(params["developerInstructions"], "The brief.");
    assert_eq!(params["model"], "gpt-6-luna");
    for key in [
        "mcp_servers.playwright.enabled",
        "mcp_servers.rider.enabled",
        "features.apps",
        "features.plugins",
        "features.hooks",
    ] {
        assert_eq!(params["config"][key], false, "{key}");
    }
    assert!(!codex.is_ready());

    let started = codex.read(THREAD_START.trim()).events;
    assert_eq!(
        started,
        [AgentEvent::Started { session_id: THREAD.to_owned(), model: "gpt-6-luna".to_owned() }]
    );
    assert!(codex.is_ready());
}

#[test]
fn an_interrupt_before_the_turn_has_an_id_goes_out_with_the_id() {
    let mut codex = CodexProtocol::new(&config(Some(THREAD)));
    codex.opening();
    answer_with(&mut codex, &json!({"id": 1, "result": {}}));
    answer_with(
        &mut codex,
        &json!({"id": 2, "result": {"account": null, "requiresOpenaiAuth": false}}),
    );
    let thread =
        answer_with(&mut codex, &json!({"id": 3, "error": {"code": -1, "message": "no config"}}))
            .send;
    assert_eq!(
        (thread[0]["method"].as_str(), thread[0]["params"]["threadId"].as_str()),
        (Some("thread/resume"), Some(THREAD))
    );
    codex.read(THREAD_START.trim());

    let turn = codex.prompt("Why?");
    assert_eq!(turn[0]["params"]["effort"], "low");
    assert_eq!(turn[0]["params"]["input"][0]["text"], "Why?");
    assert!(codex.interrupt().is_empty(), "no turn id yet");
    let interrupt =
        answer_with(&mut codex, &json!({"id": turn[0]["id"], "result": {"turn": {"id": "t1"}}}))
            .send;
    assert_eq!(interrupt[0]["method"], "turn/interrupt");
    assert_eq!(interrupt[0]["params"]["turnId"], "t1");
}

#[test]
fn a_missing_sign_in_ends_the_session_before_any_turn() {
    let mut codex = CodexProtocol::new(&config(None));
    codex.opening();
    answer_with(&mut codex, &json!({"id": 1, "result": {}}));
    let reaction = answer_with(
        &mut codex,
        &json!({"id": 2, "result": {"account": null, "requiresOpenaiAuth": true}}),
    );
    assert_eq!(reaction.events, [AgentEvent::Failed(AgentError::NotSignedIn(AgentKind::Codex))]);
    assert!(reaction.close);
}

#[test]
fn approval_requests_get_an_answer_with_their_id() {
    let mut codex = CodexProtocol::new(&config(None));
    let request = json!({"id": 41, "method": "item/fileChange/requestApproval", "params": {}});
    let reply = answer_with(&mut codex, &request).send;
    assert_eq!(reply, [json!({"id": 41, "result": {"decision": "decline"}})]);
    let unknown = json!({"id": "x", "method": "item/tool/call", "params": {}});
    assert_eq!(answer_with(&mut codex, &unknown).send[0]["error"]["code"], -32601);
}

#[test]
fn thread_setup_without_model_or_resume_leaves_both_to_the_cli() {
    let setup =
        ThreadSetup { workdir: "w".to_owned(), model: None, brief: String::new(), resume: None };
    let params = protocol::thread_params(&setup, &[]);
    assert!(params.get("model").is_none() && params.get("threadId").is_none());
}

#[test]
fn on_windows_the_read_only_thread_prefers_the_mxc_sandbox() {
    let setup =
        ThreadSetup { workdir: "w".to_owned(), model: None, brief: String::new(), resume: None };
    let params = protocol::thread_params(&setup, &[]);
    assert_eq!(params["sandbox"], "read-only");
    let prefer_mxc = params["config"].get("features.prefer_mxc");
    assert_eq!(prefer_mxc, cfg!(windows).then_some(&Value::Bool(true)));
    assert!(params["config"].get("windows.sandbox").is_none(), "the user's fallback stays");
}
