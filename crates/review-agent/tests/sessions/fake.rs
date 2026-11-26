//! The fake CLIs: this test binary, started by a session with Claude's or Codex's arguments,
//! plays the recorded fixtures back. A `fake.json` in its working directory (the session's
//! checkout) names the file where it records its arguments and every line it reads.

use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

const CLAUDE_INITIALIZE: &str = include_str!("../fixtures/claude/initialize.jsonl");
const CLAUDE_TOOLS: &str = include_str!("../fixtures/claude/turn-tools.jsonl");
const CLAUDE_INTERRUPTED: &str = include_str!("../fixtures/claude/turn-interrupted.jsonl");
const CODEX_MODELS: &str = include_str!("../fixtures/codex/model-list.jsonl");
const CODEX_THREAD: &str = include_str!("../fixtures/codex/thread-start.jsonl");
const CODEX_TOOLS: &str = include_str!("../fixtures/codex/turn-tools.jsonl");
const CODEX_INTERRUPTED: &str = include_str!("../fixtures/codex/turn-interrupted.jsonl");
/// The thread of the Codex fixtures.
const THREAD: &str = "01a15939-1521-74d0-aba4-406184d368e9";

/// Where this run records what it got; `None` outside a session's checkout (listing models).
fn record_file() -> Option<PathBuf> {
    let setup = std::fs::read_to_string("fake.json").ok()?;
    let setup: Value = serde_json::from_str(&setup).ok()?;
    setup.get("record").and_then(Value::as_str).map(PathBuf::from)
}

fn record(entry: &Value) {
    let Some(path) = record_file() else { return };
    let mut file =
        OpenOptions::new().create(true).append(true).open(path).expect("open the record");
    writeln!(file, "{entry}").expect("write the record");
}

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}").expect("write to the session");
    stdout.flush().expect("flush to the session");
}

fn parse(line: &str) -> Value {
    serde_json::from_str(line).expect("the session sends JSON lines")
}

/// `claude -p --input-format stream-json ...`.
pub fn claude(args: &[String]) {
    record(&json!({"args": args}));
    if let Some(brief) = args.iter().position(|arg| arg == "--append-system-prompt-file") {
        let file = args.get(brief + 1).expect("a brief file follows the flag");
        let text = std::fs::read_to_string(file).expect("the brief file exists while the CLI runs");
        record(&json!({"brief": text}));
    }
    // Up to the interrupt's answer, the slow turn's own lines; after it, the turn's end.
    let interrupted: Vec<&str> = CLAUDE_INTERRUPTED.lines().collect();
    let split = interrupted
        .iter()
        .position(|line| line.contains("\"control_response\""))
        .expect("the recording holds the interrupt");
    let ended = interrupted
        .iter()
        .position(|line| line.contains("\"type\":\"result\""))
        .expect("the interrupted turn ends");
    let (before, after) = interrupted.split_at(split);
    for line in std::io::stdin().lock().lines() {
        let line = line.expect("read from the session");
        record(&json!({"stdin": line}));
        let message = parse(&line);
        let request_id = message.get("request_id").cloned().unwrap_or(Value::Null);
        match message.pointer("/request/subtype").and_then(Value::as_str) {
            Some("initialize") => {
                let mut answer = parse(CLAUDE_INITIALIZE.trim());
                answer["response"]["request_id"] = request_id;
                emit(&answer.to_string());
            }
            Some("interrupt") => {
                let mut answer = parse(after.first().expect("the interrupt's answer"));
                answer["response"]["request_id"] = request_id;
                emit(&answer.to_string());
                after.iter().take(ended - split + 1).skip(1).for_each(|line| emit(line));
            }
            _ => match message.pointer("/message/content").and_then(Value::as_str) {
                Some("crash") => {
                    std::io::stderr().write_all(b"fatal: boom\n").expect("write to stderr");
                    std::process::exit(3);
                }
                Some("slow") => before.iter().for_each(|line| emit(line)),
                _ => CLAUDE_TOOLS.lines().for_each(emit),
            },
        }
    }
}

/// `codex app-server`.
pub fn codex(args: &[String]) {
    record(&json!({"args": args}));
    let mut lines = std::io::stdin().lock().lines();
    while let Some(line) = lines.next() {
        let line = line.expect("read from the session");
        record(&json!({"stdin": line}));
        let message = parse(&line);
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let answer = |result: Value| emit(&json!({"id": id, "result": result}).to_string());
        match message.get("method").and_then(Value::as_str) {
            Some("initialize" | "turn/interrupt") => answer(json!({})),
            Some("account/read") => {
                answer(json!({"account": {"type": "apiKey"}, "requiresOpenaiAuth": true}));
            }
            Some("config/read") => {
                answer(json!({"config": {"mcp_servers": {"playwright": {"enabled": true}}}}));
            }
            Some("thread/start" | "thread/resume") => {
                answer(parse(CODEX_THREAD.trim())["result"].clone());
            }
            Some("model/list") => answer(model_page(message.pointer("/params/cursor"))),
            Some("turn/start") => {
                answer(json!({"turn": {"id": "turn-1", "status": "inProgress"}}));
                let text = message.pointer("/params/input/0/text").and_then(Value::as_str);
                if text == Some("slow") {
                    play_until_interrupted(&mut lines);
                } else {
                    ask_approval(&mut lines);
                    notifications(CODEX_TOOLS).for_each(|line| emit(&line));
                }
            }
            _ => {}
        }
    }
}

/// The recorded notifications of `lines`, without the recorded answers to requests.
fn notifications(lines: &str) -> impl Iterator<Item = String> + '_ {
    lines.lines().filter(|line| parse(line).get("id").is_none()).map(str::to_owned)
}

/// Asks the session to approve a command and records its answer.
fn ask_approval(lines: &mut impl Iterator<Item = std::io::Result<String>>) {
    let request = json!({"id": "approve-1", "method": "item/commandExecution/requestApproval", "params": {"threadId": THREAD}});
    emit(&request.to_string());
    let answer = lines.next().expect("the session answers").expect("read the answer");
    record(&json!({"stdin": answer}));
}

/// The slow turn up to its first text, then its end once the interrupt came.
fn play_until_interrupted(lines: &mut impl Iterator<Item = std::io::Result<String>>) {
    let recorded: Vec<String> = notifications(CODEX_INTERRUPTED).collect();
    let first_text = recorded
        .iter()
        .position(|line| line.contains("agentMessage/delta"))
        .expect("the recording streams text");
    let (before, after) = recorded.split_at(first_text + 1);
    for line in before {
        emit(line);
    }
    let interrupt = lines.next().expect("the session interrupts").expect("read the interrupt");
    record(&json!({"stdin": interrupt}));
    let id = parse(&interrupt).get("id").cloned().unwrap_or(Value::Null);
    emit(&json!({"id": id, "result": {}}).to_string());
    for line in after {
        emit(line);
    }
}

/// The recorded model list in two pages, to make the session follow the cursor.
fn model_page(cursor: Option<&Value>) -> Value {
    let models = parse(CODEX_MODELS.trim())["result"]["data"]
        .as_array()
        .cloned()
        .expect("the recording lists models");
    let (first, second) = models.split_at(4);
    match cursor.and_then(Value::as_str) {
        Some("page-2") => json!({"data": second, "nextCursor": null}),
        _ => json!({"data": first, "nextCursor": "page-2"}),
    }
}
