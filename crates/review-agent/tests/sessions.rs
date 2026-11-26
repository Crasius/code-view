//! Whole sessions against fake CLIs, through real processes and pipes: the questions queue, the
//! events arrive in order, interrupts end a turn, a crash reports its exit, and models list.
//!
//! The fake is this test binary itself (`harness = false`): started with Claude's arguments
//! (`-p ...`) or Codex's (`app-server`) it plays recorded fixtures back (`sessions/fake.rs`);
//! started by `cargo test` it runs the checks below. No network, no real CLI.

#[path = "sessions/fake.rs"]
mod fake;

use std::path::{Path, PathBuf};
use std::time::Duration;

use review_agent::{
    AgentCli, AgentError, AgentEvent, AgentKind, ModelChoice, ModelPick, Session, SessionConfig,
    ToolStep, list_models, resolve_model,
};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

/// Far above what a fake needs; a hang fails instead of blocking the run.
const WAIT: Duration = Duration::from_secs(30);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-p") => fake::claude(&args),
        Some("app-server") => fake::codex(&args),
        _ => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime must start");
            runtime.block_on(checks());
        }
    }
}

async fn checks() {
    claude_answers_queued_questions_with_tools_and_cleans_up().await;
    claude_interrupt_ends_the_turn_and_the_session_goes_on().await;
    claude_crash_reports_the_exit_code_and_stderr().await;
    codex_session_declines_approvals_and_streams_the_turn().await;
    codex_interrupt_ends_the_turn().await;
    both_clis_list_their_models().await;
}

/// A checkout folder for one check, with the fake's setup in it.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("review-agent-{name}-{}", std::process::id()));
        // A folder an earlier run with the same process id left behind; usually there is none.
        let _stale_folder_may_be_missing = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the scratch folder");
        let setup = json!({"record": path.join("record.jsonl")});
        std::fs::write(path.join("fake.json"), setup.to_string()).expect("write the fake's setup");
        Self(path)
    }

    /// What the fake recorded: its arguments, the brief and every line it read.
    fn record(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.0.join("record.jsonl")).unwrap_or_default();
        text.lines().map(|line| serde_json::from_str(line).expect("the record is JSON")).collect()
    }

    fn stdin(&self) -> Vec<Value> {
        let lines = self.record().into_iter().filter_map(|entry| entry.get("stdin").cloned());
        lines
            .map(|line| serde_json::from_str(line.as_str().expect("a line")).expect("JSON"))
            .collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a folder left in the temp directory harms nothing.
        let _cleanup_is_best_effort = std::fs::remove_dir_all(&self.0);
    }
}

fn cli(kind: AgentKind) -> AgentCli {
    let program = std::env::current_exe().expect("the test binary has a path");
    AgentCli { kind, program, version: "fake".to_owned() }
}

fn start(
    kind: AgentKind,
    workdir: &Path,
    pick: ModelPick,
) -> (Session, UnboundedReceiver<AgentEvent>) {
    let config = SessionConfig {
        cli: cli(kind),
        pick,
        workdir: workdir.to_path_buf(),
        brief: "The pull request brief.".to_owned(),
        resume: None,
    };
    Session::start(config).expect("a runtime is running")
}

async fn next(events: &mut UnboundedReceiver<AgentEvent>) -> AgentEvent {
    let event = tokio::time::timeout(WAIT, events.recv()).await.expect("an event in time");
    event.expect("the event stream ends only after Closed")
}

/// Events up to and with the next `TurnDone` or `Closed`.
async fn turn(events: &mut UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut seen = Vec::new();
    loop {
        let event = next(events).await;
        let ends = matches!(event, AgentEvent::TurnDone(_) | AgentEvent::Closed);
        seen.push(event);
        if ends {
            return seen;
        }
    }
}

fn text(events: &[AgentEvent]) -> String {
    let texts = events.iter().filter_map(|event| match event {
        AgentEvent::Text(text) => Some(text.as_str()),
        _ => None,
    });
    texts.collect()
}

fn tools(events: &[AgentEvent]) -> Vec<ToolStep> {
    let steps = events.iter().filter_map(|event| match event {
        AgentEvent::Tool(step) => Some(step.clone()),
        _ => None,
    });
    steps.collect()
}

fn pick(model: &str, effort: &str) -> ModelPick {
    ModelPick { model: Some(model.to_owned()), effort: Some(effort.to_owned()) }
}

async fn claude_answers_queued_questions_with_tools_and_cleans_up() {
    let scratch = Scratch::new("claude-turns");
    let (session, mut events) = start(AgentKind::Claude, &scratch.0, pick("haiku", "low"));
    session.ask("What changed?");
    session.ask("And why?");
    let first = turn(&mut events).await;
    let second = turn(&mut events).await;

    let started = first.iter().filter(|event| matches!(event, AgentEvent::Started { .. })).count();
    assert_eq!(started, 1, "{first:?}");
    let commands: Vec<(String, String)> =
        tools(&first).into_iter().map(|step| (step.tool, step.detail)).collect();
    let bash = |command: &str| ("Bash".to_owned(), command.to_owned());
    assert_eq!(commands, [bash("echo hi > y.txt"), bash("git status")]);
    assert!(text(&first).starts_with("- **Write tool (x.txt):** Not run."));
    assert!(!second.iter().any(|event| matches!(event, AgentEvent::Started { .. })));
    assert_eq!(tools(&second).len(), 2, "the queued question got its own turn");

    drop(session);
    assert_eq!(turn(&mut events).await, [AgentEvent::Closed], "a dropped session just closes");
    let record = scratch.record();
    let args: Vec<&str> = record[0]["args"]
        .as_array()
        .expect("arguments")
        .iter()
        .map(|arg| arg.as_str().expect("text"))
        .collect();
    let line = args.join(" ");
    assert!(line.contains("--model haiku --effort low --restricted"), "{line}");
    assert_eq!(record[1]["brief"], "The pull request brief.");
    let brief_file = args.last().expect("the brief file is last");
    assert!(!Path::new(brief_file).exists(), "the brief file goes with the session");
    let questions: Vec<Value> =
        scratch.stdin().into_iter().map(|line| line["message"]["content"].clone()).collect();
    assert_eq!(questions, ["What changed?", "And why?"]);
}

async fn claude_interrupt_ends_the_turn_and_the_session_goes_on() {
    let scratch = Scratch::new("claude-interrupt");
    let (session, mut events) = start(AgentKind::Claude, &scratch.0, ModelPick::default());
    session.ask("slow");
    while !matches!(next(&mut events).await, AgentEvent::Text(_)) {}
    session.interrupt();
    let rest = turn(&mut events).await;
    assert!(matches!(rest.last(), Some(AgentEvent::TurnDone(_))), "{rest:?}");
    assert!(!rest.iter().any(|event| matches!(event, AgentEvent::Failed(_))), "{rest:?}");

    session.ask("go on");
    let after = turn(&mut events).await;
    assert!(matches!(after.last(), Some(AgentEvent::TurnDone(_))));
    let sent = scratch.stdin();
    assert_eq!(sent[1]["request"]["subtype"], "interrupt");
}

async fn claude_crash_reports_the_exit_code_and_stderr() {
    let scratch = Scratch::new("claude-crash");
    let (session, mut events) = start(AgentKind::Claude, &scratch.0, ModelPick::default());
    session.ask("crash");
    let failed = next(&mut events).await;
    let AgentEvent::Failed(AgentError::Exited { code, stderr_tail }) = failed else {
        unreachable!("expected an exit, got {failed:?}");
    };
    assert_eq!((code, stderr_tail.as_str()), (Some(3), "fatal: boom"));
    assert_eq!(next(&mut events).await, AgentEvent::Closed);
}

async fn codex_session_declines_approvals_and_streams_the_turn() {
    let scratch = Scratch::new("codex-turn");
    let (session, mut events) = start(AgentKind::Codex, &scratch.0, pick("gpt-6-luna", "low"));
    session.ask("What changed?");
    let events = turn(&mut events).await;
    let started = AgentEvent::Started {
        session_id: "01a15939-1521-74d0-aba4-406184d368e9".to_owned(),
        model: "gpt-6-luna".to_owned(),
    };
    assert_eq!(events.first(), Some(&started));
    let details: Vec<String> = tools(&events).into_iter().map(|step| step.detail).collect();
    assert_eq!(
        details.get(..2),
        Some(&["echo hi > x.txt".to_owned(), "git log -1".to_owned()][..])
    );
    assert!(text(&events).ends_with("- `brief.md` exists and contains `# brief`."));

    let sent = scratch.stdin();
    let method = |line: &Value| line["method"].as_str().unwrap_or("").to_owned();
    let thread = sent.iter().find(|line| method(line) == "thread/start").expect("a thread starts");
    assert_eq!(thread["params"]["config"]["mcp_servers.playwright.enabled"], false);
    assert_eq!(thread["params"]["developerInstructions"], "The pull request brief.");
    let turn_start = sent.iter().find(|line| method(line) == "turn/start").expect("a turn starts");
    assert_eq!(turn_start["params"]["effort"], "low");
    let approval =
        sent.iter().find(|line| line["id"] == "approve-1").expect("the approval is answered");
    assert_eq!(approval["result"]["decision"], "decline");
}

async fn codex_interrupt_ends_the_turn() {
    let scratch = Scratch::new("codex-interrupt");
    let (session, mut events) = start(AgentKind::Codex, &scratch.0, ModelPick::default());
    session.ask("slow");
    while !matches!(next(&mut events).await, AgentEvent::Text(_)) {}
    session.interrupt();
    let rest = turn(&mut events).await;
    assert!(matches!(rest.last(), Some(AgentEvent::TurnDone(_))), "{rest:?}");
    let sent = scratch.stdin();
    let interrupt =
        sent.iter().find(|line| line["method"] == "turn/interrupt").expect("interrupted");
    assert_eq!(interrupt["params"]["turnId"], "turn-1");
}

async fn both_clis_list_their_models() {
    let claude = list_models(&cli(AgentKind::Claude)).await.expect("Claude lists models");
    let fastest = resolve_model(AgentKind::Claude, &ModelChoice::Fastest, &claude);
    assert_eq!(fastest, pick("haiku", "low"));

    let codex = list_models(&cli(AgentKind::Codex)).await.expect("Codex lists models");
    assert_eq!(codex.len(), 7, "both pages");
    let fastest = resolve_model(AgentKind::Codex, &ModelChoice::Fastest, &codex);
    assert_eq!(fastest, pick("gpt-6-luna", "low"));
}
