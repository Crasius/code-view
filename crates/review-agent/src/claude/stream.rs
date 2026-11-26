//! Reads Claude Code's stream-json output (`claude -p --output-format stream-json --verbose
//! --include-partial-messages`) into [`AgentEvent`]s. Pure: one line in, events and replies out.
//!
//! Text arrives twice: as `stream_event` deltas while the model writes, then once more inside the
//! complete `assistant` message. The deltas are what the chat shows; a message's own text counts
//! only when no delta of that message came (an API error message is never streamed). Tool calls
//! come from the complete message, because only it carries the whole input; a tool's end is its
//! `tool_result` in the next `user` message, `is_error` when it failed (observed).
//!
//! Thinking streams as its own content block: `content_block_start` with a `thinking` (or
//! `redacted_thinking`) block, `thinking_delta`s with its text, `content_block_stop` with the same
//! index (the Messages API's streaming format; the probe recordings kept only the block's stop and
//! the `thinking_tokens` count). A model whose thinking is not shown sends the block without text.
//!
//! Observed with Claude Code 2.1.304: `system/init` arrives with the answer to each question (not
//! at start), so the session id is known from the first answer on; an interrupted turn ends with
//! a `result` whose `is_error` is true; a missing sign-in is an `assistant` message with
//! `error: "authentication_failed"` and a `result` with `is_error`.

#![warn(clippy::string_slice, clippy::indexing_slicing)]

use std::collections::HashSet;

use serde_json::{Value, json};

use crate::driver::AnswerText;
use crate::{AgentError, AgentEvent, AgentKind, StepKind, StepOutcome, ToolStep, Usage};

#[derive(Debug, Default)]
pub(crate) struct ClaudeStream {
    /// The checkout's path, cut from the front of tool paths.
    workdir: String,
    started: bool,
    /// The message the deltas belong to.
    current_message: Option<String>,
    /// Messages whose text already came as deltas.
    streamed: HashSet<String>,
    /// The content block index of the thinking block that is open.
    thinking_block: Option<u64>,
    text: AnswerText,
    auth_failed: bool,
    interrupting: bool,
}

impl ClaudeStream {
    pub(crate) fn new(workdir: &str) -> Self {
        Self { workdir: workdir.to_owned(), ..Self::default() }
    }

    /// The host asked to stop the turn: its failing `result` is the end of the turn, not an error.
    pub(crate) fn interrupt_requested(&mut self) {
        self.interrupting = true;
    }

    /// Events and replies for one stdout line. Lines that are not JSON or not for this reader
    /// (hook and status notices, rate-limit updates) give nothing.
    pub(crate) fn read(&mut self, line: &str) -> (Vec<AgentEvent>, Vec<Value>) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return (Vec::new(), Vec::new());
        };
        // Messages of a subagent carry their parent's tool id; only the main thread answers.
        if message.get("parent_tool_use_id").and_then(Value::as_str).is_some() {
            return (Vec::new(), Vec::new());
        }
        let mut events = Vec::new();
        let mut replies = Vec::new();
        match message.get("type").and_then(Value::as_str) {
            Some("system") => self.system(&message, &mut events),
            Some("stream_event") => self.stream_event(&message, &mut events),
            Some("assistant") => self.assistant(&message, &mut events),
            Some("user") => tool_results(&message, &mut events),
            Some("result") => self.result(&message, &mut events),
            Some("control_request") => replies.push(refuse(&message)),
            _ => {}
        }
        (events, replies)
    }

    fn system(&mut self, message: &Value, events: &mut Vec<AgentEvent>) {
        let is_init = message.get("subtype").and_then(Value::as_str) == Some("init");
        if !is_init || self.started {
            return;
        }
        self.started = true;
        events.push(AgentEvent::Started {
            session_id: text_field(message, "session_id"),
            model: text_field(message, "model"),
        });
    }

    fn stream_event(&mut self, message: &Value, events: &mut Vec<AgentEvent>) {
        let Some(event) = message.get("event") else { return };
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                self.current_message =
                    event.pointer("/message/id").and_then(Value::as_str).map(str::to_owned);
            }
            Some("content_block_start") => {
                match event.pointer("/content_block/type").and_then(Value::as_str) {
                    Some("text") => self.text.new_block(),
                    Some("thinking" | "redacted_thinking") => {
                        self.thinking_block = event.get("index").and_then(Value::as_u64);
                        events.push(AgentEvent::Thinking(String::new()));
                    }
                    _ => {}
                }
            }
            Some("content_block_delta") => {
                match event.pointer("/delta/type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        let text =
                            event.pointer("/delta/text").and_then(Value::as_str).unwrap_or("");
                        if text.is_empty() {
                            return;
                        }
                        if let Some(id) = &self.current_message {
                            self.streamed.insert(id.clone());
                        }
                        self.text.push(text, events);
                    }
                    Some("thinking_delta") => {
                        let text = event.pointer("/delta/thinking").and_then(Value::as_str);
                        if let Some(text) = text.filter(|text| !text.is_empty()) {
                            events.push(AgentEvent::Thinking(text.to_owned()));
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                let index = event.get("index").and_then(Value::as_u64);
                if index.is_some() && index == self.thinking_block {
                    self.thinking_block = None;
                    events.push(AgentEvent::ThinkingDone);
                }
            }
            _ => {}
        }
    }

    fn assistant(&mut self, message: &Value, events: &mut Vec<AgentEvent>) {
        if let Some(error) = message.get("error").and_then(Value::as_str) {
            self.auth_failed |= error == "authentication_failed";
            return;
        }
        let id = message.pointer("/message/id").and_then(Value::as_str).unwrap_or("");
        let was_streamed = self.streamed.contains(id);
        let blocks = message.pointer("/message/content").and_then(Value::as_array);
        for block in blocks.into_iter().flatten() {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => events.push(AgentEvent::Tool(self.tool_step(block))),
                Some("text") if !was_streamed => {
                    let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                    self.text.new_block();
                    self.text.push(text, events);
                }
                _ => {}
            }
        }
    }

    fn result(&mut self, message: &Value, events: &mut Vec<AgentEvent>) {
        let failed = message.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        if failed && !self.interrupting {
            events.push(AgentEvent::Failed(self.turn_error(message)));
        }
        events.push(AgentEvent::TurnDone(usage(message)));
        let started = self.started;
        *self = Self { workdir: std::mem::take(&mut self.workdir), started, ..Self::default() };
    }

    fn turn_error(&self, message: &Value) -> AgentError {
        let result = message.get("result").and_then(Value::as_str).unwrap_or("").trim();
        if self.auth_failed || result.contains("/login") {
            return AgentError::NotSignedIn(AgentKind::Claude);
        }
        let errors: Vec<&str> = message
            .get("errors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let words = if !result.is_empty() {
            result.to_owned()
        } else if !errors.is_empty() {
            errors.join("; ")
        } else {
            text_field(message, "subtype")
        };
        AgentError::Turn(words)
    }

    fn tool_step(&self, block: &Value) -> ToolStep {
        let tool = text_field(block, "name");
        let kind = StepKind::of_tool(&tool);
        let input = block.get("input").unwrap_or(&Value::Null);
        let field = |key: &str| input.get(key).and_then(Value::as_str);
        let detail = match tool.as_str() {
            "Grep" => match (field("pattern"), field("path")) {
                (Some(pattern), Some(path)) => format!("{pattern} in {}", self.relative(path)),
                (pattern, _) => pattern.unwrap_or("").to_owned(),
            },
            _ => ["command", "file_path", "path", "pattern", "url", "query"]
                .iter()
                .find_map(|key| field(key))
                .map(|value| self.relative(value))
                .unwrap_or_default(),
        };
        ToolStep::new(text_field(block, "id"), tool, kind, &detail)
    }

    /// `path` without the checkout's folder in front.
    fn relative(&self, path: &str) -> String {
        let normalized = path.replace('\\', "/");
        let root = self.workdir.replace('\\', "/");
        let inside = normalized
            .strip_prefix(root.trim_end_matches('/'))
            .filter(|rest| rest.starts_with('/'))
            .map(|rest| rest.trim_start_matches('/'));
        match inside {
            Some("") => ".".to_owned(),
            Some(rest) => rest.to_owned(),
            None => path.to_owned(),
        }
    }
}

/// The end of each tool call a `user` message reports: its `tool_result` blocks.
fn tool_results(message: &Value, events: &mut Vec<AgentEvent>) {
    let blocks = message.pointer("/message/content").and_then(Value::as_array);
    let results = blocks
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"));
    for result in results {
        let failed = result.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        let outcome = if failed { StepOutcome::Failed } else { StepOutcome::Ok };
        events.push(AgentEvent::ToolDone { id: text_field(result, "tool_use_id"), outcome });
    }
}

/// Claude Code asks the host only when it would prompt (`can_use_tool`); the session is
/// read-only, so the answer is no. Any other request is not supported here.
fn refuse(request: &Value) -> Value {
    let request_id = request.get("request_id").cloned().unwrap_or(Value::Null);
    let subtype = request.pointer("/request/subtype").and_then(Value::as_str).unwrap_or("");
    let response = if subtype == "can_use_tool" {
        json!({
            "subtype": "success",
            "request_id": request_id,
            "response": {"behavior": "deny", "message": "The pull request assistant is read-only."},
        })
    } else {
        json!({
            "subtype": "error",
            "request_id": request_id,
            "error": format!("CodeView does not support {subtype}"),
        })
    };
    json!({"type": "control_response", "response": response})
}

fn usage(result: &Value) -> Usage {
    let count = |key: &str| result.pointer(&format!("/usage/{key}")).and_then(Value::as_u64);
    Usage {
        input: count("input_tokens").unwrap_or(0),
        output: count("output_tokens").unwrap_or(0),
        cache_read: count("cache_read_input_tokens").unwrap_or(0),
        cache_write: count("cache_creation_input_tokens").unwrap_or(0),
    }
}

fn text_field(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
}
