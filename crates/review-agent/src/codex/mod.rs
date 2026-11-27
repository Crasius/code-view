//! Codex in its app-server mode: `codex app-server`, JSON-RPC over stdio, one process per session.
//!
//! A session sets itself up before the first question: `initialize` and `initialized`, then
//! `account/read` (a missing sign-in ends the session at once instead of a turn that retries for
//! seconds against a 401), `config/read` for the MCP server names to switch off, and
//! `thread/start` or `thread/resume`. Each question is a `turn/start`; `turn/interrupt` stops one.
//! Every approval the server asks for is declined; the sandbox is read-only anyway.

mod protocol;

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::driver::{Launch, Protocol, Reaction};
use crate::process::{self, AgentProcess, EXIT_GRACE};
use crate::{AgentCli, AgentError, AgentEvent, AgentKind, ModelInfo, SessionConfig};
use protocol::{ThreadSetup, TurnReader};

/// How long the server may take to list its models.
const MODELS_TIMEOUT: Duration = Duration::from_secs(30);
/// Not a JSON-RPC method the server knows how to be answered for.
const METHOD_NOT_FOUND: i64 = -32601;

pub(crate) fn launch(config: &SessionConfig) -> Launch {
    let command = process::command(&config.cli.program, ["app-server"], &config.workdir);
    Launch { command, protocol: Box::new(CodexProtocol::new(config)), temporary_file: None }
}

fn initialize_params() -> Value {
    let version = env!("CARGO_PKG_VERSION");
    json!({"clientInfo": {"name": "codeview", "title": "CodeView", "version": version}})
}

fn interrupt_params(thread_id: &str, turn_id: &str) -> Value {
    json!({"threadId": thread_id, "turnId": turn_id})
}

/// What an answer with this id belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
    Initialize,
    Account,
    Config,
    Thread,
    Turn,
    Interrupt,
}

#[derive(Debug, PartialEq, Eq)]
enum Turn {
    Idle,
    /// `turn/start` is out; the turn's id comes with its answer.
    Starting {
        interrupt_due: bool,
    },
    Running {
        id: String,
    },
}

struct CodexProtocol {
    setup: ThreadSetup,
    effort: Option<String>,
    next_id: u64,
    waiting: HashMap<u64, Request>,
    /// The thread's reader, once `thread/start` answered.
    thread: Option<TurnReader>,
    turn: Turn,
}

impl CodexProtocol {
    fn new(config: &SessionConfig) -> Self {
        let setup = ThreadSetup {
            workdir: config.workdir.to_string_lossy().into_owned(),
            model: config.pick.model.clone(),
            brief: config.brief.clone(),
            resume: config.resume.clone(),
        };
        Self {
            setup,
            effort: config.pick.effort.clone(),
            next_id: 0,
            waiting: HashMap::new(),
            thread: None,
            turn: Turn::Idle,
        }
    }

    fn request(&mut self, kind: Request, method: &str, params: Value) -> Value {
        self.next_id += 1;
        self.waiting.insert(self.next_id, kind);
        let message: Map<String, Value> = [
            ("id".to_owned(), json!(self.next_id)),
            ("method".to_owned(), json!(method)),
            ("params".to_owned(), params),
        ]
        .into_iter()
        .collect();
        Value::Object(message)
    }

    /// The next step after the answer to `kind`.
    fn answer(&mut self, kind: Request, message: &Value) -> Reaction {
        let error = message.get("error").map(|error| {
            let words = error.get("message").and_then(Value::as_str).unwrap_or("request failed");
            words.to_owned()
        });
        let result = message.get("result").unwrap_or(&Value::Null);
        let mut reaction = Reaction::default();
        match (kind, error) {
            (Request::Interrupt, _) => {}
            (Request::Turn, Some(words)) => {
                self.turn = Turn::Idle;
                reaction.events = vec![
                    AgentEvent::Failed(AgentError::Turn(words)),
                    AgentEvent::TurnDone(crate::Usage::default()),
                ];
            }
            (Request::Turn, None) => {
                let id =
                    result.pointer("/turn/id").and_then(Value::as_str).unwrap_or("").to_owned();
                if self.turn == (Turn::Starting { interrupt_due: true })
                    && let Some(reader) = &self.thread
                {
                    let params = interrupt_params(reader.thread_id(), &id);
                    reaction.send.push(self.request(Request::Interrupt, "turn/interrupt", params));
                }
                self.turn = Turn::Running { id };
            }
            // A config that cannot be read leaves only the user's MCP servers on; go on without.
            (Request::Config, _) => {
                let params =
                    protocol::thread_params(&self.setup, &protocol::mcp_server_names(result));
                let method =
                    if self.setup.resume.is_some() { "thread/resume" } else { "thread/start" };
                reaction.send.push(self.request(Request::Thread, method, params));
            }
            (Request::Initialize | Request::Account | Request::Thread, Some(words)) => {
                reaction.events.push(AgentEvent::Failed(AgentError::Turn(words)));
                reaction.close = true;
            }
            (Request::Initialize, None) => {
                reaction.send.push(json!({"method": "initialized"}));
                reaction.send.push(self.request(Request::Account, "account/read", json!({})));
            }
            (Request::Account, None) if protocol::needs_sign_in(result) => {
                reaction.events.push(AgentEvent::Failed(AgentError::NotSignedIn(AgentKind::Codex)));
                reaction.close = true;
            }
            (Request::Account, None) => {
                let params = json!({"cwd": self.setup.workdir});
                reaction.send.push(self.request(Request::Config, "config/read", params));
            }
            (Request::Thread, None) => {
                let thread_id = result.pointer("/thread/id").and_then(Value::as_str).unwrap_or("");
                let model = result.get("model").and_then(Value::as_str).unwrap_or("");
                reaction.events.push(AgentEvent::Started {
                    session_id: thread_id.to_owned(),
                    model: model.to_owned(),
                });
                self.thread = Some(TurnReader::new(thread_id));
            }
        }
        reaction
    }
}

impl Protocol for CodexProtocol {
    fn opening(&mut self) -> Vec<Value> {
        vec![self.request(Request::Initialize, "initialize", initialize_params())]
    }

    fn is_ready(&self) -> bool {
        self.thread.is_some() && self.turn == Turn::Idle
    }

    fn prompt(&mut self, text: &str) -> Vec<Value> {
        let Some(reader) = &self.thread else { return Vec::new() };
        let params = protocol::turn_params(reader.thread_id(), text, self.effort.as_deref());
        self.turn = Turn::Starting { interrupt_due: false };
        vec![self.request(Request::Turn, "turn/start", params)]
    }

    fn interrupt(&mut self) -> Vec<Value> {
        match &self.turn {
            Turn::Idle => Vec::new(),
            Turn::Starting { .. } => {
                self.turn = Turn::Starting { interrupt_due: true };
                Vec::new()
            }
            Turn::Running { id } => {
                let Some(reader) = &self.thread else { return Vec::new() };
                let params = interrupt_params(reader.thread_id(), id);
                vec![self.request(Request::Interrupt, "turn/interrupt", params)]
            }
        }
    }

    fn read(&mut self, line: &str) -> Reaction {
        let Ok(message) = serde_json::from_str::<Value>(line) else { return Reaction::default() };
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id");
        match (method, id) {
            (Some(method), Some(id)) => {
                let reply = match protocol::decline(method) {
                    Some(result) => json!({"id": id, "result": result}),
                    None => {
                        json!({"id": id, "error": {"code": METHOD_NOT_FOUND, "message": format!("CodeView does not support {method}")}})
                    }
                };
                Reaction { send: vec![reply], ..Reaction::default() }
            }
            (Some(method), None) => {
                let params = message.get("params").unwrap_or(&Value::Null);
                let events = match &mut self.thread {
                    Some(reader) => reader.notification(method, params),
                    None => Vec::new(),
                };
                if events.iter().any(|event| matches!(event, AgentEvent::TurnDone(_))) {
                    self.turn = Turn::Idle;
                }
                Reaction { events, ..Reaction::default() }
            }
            (None, Some(id)) => {
                let kind = id.as_u64().and_then(|id| self.waiting.remove(&id));
                kind.map(|kind| self.answer(kind, &message)).unwrap_or_default()
            }
            (None, None) => Reaction::default(),
        }
    }
}

/// The visible models from `model/list`, all pages, in the server's order (newest first).
pub(crate) async fn list_models(cli: &AgentCli) -> Result<Vec<ModelInfo>, AgentError> {
    let command = process::command(&cli.program, ["app-server"], &std::env::temp_dir());
    let mut agent = AgentProcess::spawn(command)?;
    let listed = tokio::time::timeout(MODELS_TIMEOUT, all_models(&mut agent)).await;
    let (code, stderr_tail) = agent.finish(EXIT_GRACE).await;
    let listed = listed.unwrap_or(Err(Some(AgentError::Timeout("the model list"))));
    listed.map_err(|error| error.unwrap_or(AgentError::Exited { code, stderr_tail }))
}

/// Every model page; the error is `None` when the CLI stopped reading its input, whose exit
/// then tells why.
async fn all_models(agent: &mut AgentProcess) -> Result<Vec<ModelInfo>, Option<AgentError>> {
    let initialize = json!({"id": 0, "method": "initialize", "params": initialize_params()});
    agent.send(&initialize).await.map_err(|_| None)?;
    let mut models = Vec::new();
    let mut page = 0;
    while let Some(line) = agent.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        if message.get("method").is_some() {
            continue;
        }
        if let Some(error) = message.get("error") {
            let words = error.get("message").and_then(Value::as_str).unwrap_or("model/list failed");
            return Err(Some(AgentError::Protocol(words.to_owned())));
        }
        let result = message.get("result").unwrap_or(&Value::Null);
        let cursor = if message.get("id").and_then(Value::as_u64) == Some(0) {
            agent.send(&json!({"method": "initialized"})).await.map_err(|_| None)?;
            None
        } else {
            let (listed, cursor) = protocol::model_page(result);
            models.extend(listed);
            match cursor {
                Some(cursor) => Some(cursor),
                None => return Ok(models),
            }
        };
        page += 1;
        agent
            .send(&json!({"id": page, "method": "model/list", "params": {"cursor": cursor}}))
            .await
            .map_err(|_| None)?;
    }
    Err(Some(AgentError::Protocol("the CLI ended before it listed its models".to_owned())))
}

#[cfg(test)]
mod tests;
