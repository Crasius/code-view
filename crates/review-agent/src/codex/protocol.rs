//! The pure half of the Codex app-server protocol: the parameters this crate sends and what the
//! server's notifications, requests and model pages mean. The shapes follow the schema that
//! `codex app-server generate-json-schema` writes (codex-cli 0.163.0).
//!
//! Text streams as `item/agentMessage/delta`; an agent message whose deltas never came counts
//! once it completes. One turn can hold several agent messages (a short "I'll look at ..." first,
//! the answer last), so each further message starts a new paragraph. Token counts arrive as
//! running thread totals (`thread/tokenUsage/updated`); a turn's usage is the difference between
//! the totals at its start and at its end.
//!
//! A tool item starts with `item/started` and ends with `item/completed`, whose `status` says
//! `failed` (a command that exited non-zero, observed) or `declined`. A command Codex could not
//! start at all (its sandbox failed) fails the same way; the turn then ends with a
//! [`AgentEvent::Failed`] that says what to do, since its other commands fail alike. Reasoning is a `reasoning`
//! item: its summary streams as `item/reasoning/summaryTextDelta`, raw reasoning (where the model
//! shares it) as `item/reasoning/textDelta` (the schema; no recording has one yet).

#![warn(clippy::string_slice, clippy::indexing_slicing)]

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use crate::driver::AnswerText;
use crate::{AgentError, AgentEvent, AgentKind, ModelInfo, StepKind, StepOutcome, ToolStep, Usage};

/// What a session's thread starts with.
#[derive(Debug, Clone)]
pub(crate) struct ThreadSetup {
    pub(crate) workdir: String,
    pub(crate) model: Option<String>,
    pub(crate) brief: String,
    /// The thread to continue instead of starting one.
    pub(crate) resume: Option<String>,
}

/// `thread/start` (or `thread/resume` with [`ThreadSetup::resume`]) parameters: read-only
/// sandbox, no approvals, the brief as developer instructions, and the config overrides that
/// switch off every MCP server in `mcp_servers` (the effective config's names), the apps and
/// plugins (which bring their own MCP servers) and hooks, the user's and the repository's.
///
/// On Windows the thread also prefers the MXC sandbox (`features.prefer_mxc`). The elevated
/// sandbox, the usual `windows.sandbox` choice, runs a setup refresh before every command that
/// re-grants read access on Codex's runtime files under `%LOCALAPPDATA%\OpenAI\Codex`; one file
/// it cannot change fails every command of the session with `setup refresh had errors`
/// (observed). MXC needs no setup and still denies writes to the checkout, the user's profile
/// and their temp folder, and network access; a command writes only to the temp folder of its
/// own app container (observed). Where MXC is unavailable Codex keeps the configured sandbox.
pub(crate) fn thread_params(setup: &ThreadSetup, mcp_servers: &[String]) -> Value {
    let mut overrides = Map::new();
    for feature in ["apps", "plugins", "hooks"] {
        overrides.insert(format!("features.{feature}"), Value::Bool(false));
    }
    if cfg!(windows) {
        overrides.insert("features.prefer_mxc".to_owned(), Value::Bool(true));
    }
    for name in mcp_servers {
        overrides.insert(format!("mcp_servers.{name}.enabled"), Value::Bool(false));
    }
    let mut params = json!({
        "cwd": setup.workdir,
        "sandbox": "read-only",
        "approvalPolicy": "never",
        "developerInstructions": setup.brief,
        "config": overrides,
    });
    with_optional(
        &mut params,
        &[("model", setup.model.as_deref()), ("threadId", setup.resume.as_deref())],
    );
    params
}

/// `turn/start` parameters for one question.
pub(crate) fn turn_params(thread_id: &str, text: &str, effort: Option<&str>) -> Value {
    let mut params = json!({"threadId": thread_id, "input": [{"type": "text", "text": text}]});
    with_optional(&mut params, &[("effort", effort)]);
    params
}

/// Adds the present values of `fields` to the object `params`.
fn with_optional(params: &mut Value, fields: &[(&str, Option<&str>)]) {
    let Some(object) = params.as_object_mut() else { return };
    for (key, value) in fields {
        if let Some(value) = value {
            object.insert((*key).to_owned(), Value::String((*value).to_owned()));
        }
    }
}

/// The MCP server names in a `config/read` result.
pub(crate) fn mcp_server_names(config_read: &Value) -> Vec<String> {
    config_read
        .pointer("/config/mcp_servers")
        .and_then(Value::as_object)
        .map(|servers| servers.keys().cloned().collect())
        .unwrap_or_default()
}

/// True when an `account/read` result says the CLI needs a sign-in it does not have.
pub(crate) fn needs_sign_in(account_read: &Value) -> bool {
    let has_account = account_read.get("account").is_some_and(|account| !account.is_null());
    let requires = account_read.get("requiresOpenaiAuth").and_then(Value::as_bool).unwrap_or(false);
    requires && !has_account
}

/// The visible models of one `model/list` page and the cursor of the next page.
pub(crate) fn model_page(result: &Value) -> (Vec<ModelInfo>, Option<String>) {
    let models = result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|model| !model.get("hidden").and_then(Value::as_bool).unwrap_or(false))
        .map(|model| {
            let field = |key: &str| model.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
            let efforts = model.get("supportedReasoningEfforts").and_then(Value::as_array);
            ModelInfo {
                id: field("model"),
                display_name: field("displayName"),
                description: field("description"),
                is_default: model.get("isDefault").and_then(Value::as_bool).unwrap_or(false),
                efforts: efforts
                    .into_iter()
                    .flatten()
                    .filter_map(|effort| effort.get("reasoningEffort").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect(),
            }
        })
        .filter(|model| !model.id.is_empty())
        .collect();
    let cursor = result.get("nextCursor").and_then(Value::as_str).map(str::to_owned);
    (models, cursor)
}

/// The answer to a server request: every approval is declined, a question gets no answer, and
/// anything else is not supported. `None` for a request this crate does not know.
pub(crate) fn decline(method: &str) -> Option<Value> {
    let result = match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            json!({"decision": "decline"})
        }
        "execCommandApproval" | "applyPatchApproval" => json!({"decision": "denied"}),
        "item/permissions/requestApproval" => json!({"permissions": {}}),
        "mcpServer/elicitation/request" => json!({"action": "decline"}),
        "item/tool/requestUserInput" => json!({"answers": {}}),
        _ => return None,
    };
    Some(result)
}

/// A turn's error (`TurnError`) as the chat's next step: a missing or rejected sign-in, or the
/// CLI's own words.
pub(crate) fn turn_error(error: &Value) -> AgentError {
    let info = error.get("codexErrorInfo").unwrap_or(&Value::Null);
    let status = [
        "httpConnectionFailed",
        "responseStreamConnectionFailed",
        "responseStreamDisconnected",
        "responseTooManyFailedAttempts",
    ]
    .iter()
    .find_map(|key| info.pointer(&format!("/{key}/httpStatusCode")).and_then(Value::as_u64));
    if info.as_str() == Some("unauthorized") || status == Some(401) {
        return AgentError::NotSignedIn(AgentKind::Codex);
    }
    let message = error.get("message").and_then(Value::as_str).unwrap_or("the turn failed");
    AgentError::Turn(message.to_owned())
}

/// How the output of a command Codex could not start at all begins, as in `Failed to create
/// unified exec process: helper_unknown_error: setup refresh had errors` (observed on Windows,
/// where the sandbox's setup failed).
const NOT_STARTED: &str = "Failed to create unified exec process: ";

/// The chat's next step after Codex could not start a command, with the CLI's `reason`.
fn not_started_error(reason: &str) -> AgentError {
    AgentError::Turn(format!(
        "Codex could not start a command in its sandbox ({reason}). Run `codex doctor` in a \
         terminal: its sandbox section names what failed and how to repair it. Then ask again."
    ))
}

/// Reads one thread's notifications into events, turn by turn.
#[derive(Debug, Default)]
pub(crate) struct TurnReader {
    thread_id: String,
    /// The thread's token totals now and when the running turn started.
    total: Usage,
    at_turn_start: Usage,
    /// Agent messages whose text already came as deltas.
    streamed: HashSet<String>,
    text: AnswerText,
    /// Why Codex could not start a command of the running turn, the first time it could not.
    not_started: Option<String>,
}

impl TurnReader {
    pub(crate) fn new(thread_id: &str) -> Self {
        Self { thread_id: thread_id.to_owned(), ..Self::default() }
    }

    pub(crate) fn notification(&mut self, method: &str, params: &Value) -> Vec<AgentEvent> {
        let other_thread = params
            .get("threadId")
            .and_then(Value::as_str)
            .is_some_and(|thread| thread != self.thread_id);
        if other_thread {
            return Vec::new();
        }
        let mut events = Vec::new();
        let item = params.get("item").unwrap_or(&Value::Null);
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match method {
            "turn/started" => {
                self.at_turn_start = self.total;
                self.streamed.clear();
                self.text = AnswerText::default();
                self.not_started = None;
            }
            "item/started" if item_type == "agentMessage" => self.text.new_block(),
            "item/started" if item_type == "reasoning" => {
                events.push(AgentEvent::Thinking(String::new()));
            }
            "item/started" => events.extend(tool_step(item).map(AgentEvent::Tool)),
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                let delta = params.get("delta").and_then(Value::as_str).unwrap_or("");
                if !delta.is_empty() {
                    events.push(AgentEvent::Thinking(delta.to_owned()));
                }
            }
            "item/reasoning/summaryPartAdded" => {
                events.push(AgentEvent::Thinking(
                    "

"
                    .to_owned(),
                ));
            }
            "item/completed" if item_type == "reasoning" => events.push(AgentEvent::ThinkingDone),
            "item/agentMessage/delta" => {
                let delta = params.get("delta").and_then(Value::as_str).unwrap_or("");
                if let Some(id) = params.get("itemId").and_then(Value::as_str) {
                    self.streamed.insert(id.to_owned());
                }
                self.text.push(delta, &mut events);
            }
            "item/completed" if item_type == "agentMessage" => {
                let id = item.get("id").and_then(Value::as_str).unwrap_or("");
                if !self.streamed.contains(id) {
                    let text = item.get("text").and_then(Value::as_str).unwrap_or("");
                    self.text.push(text, &mut events);
                }
            }
            "item/completed" => {
                if self.not_started.is_none() {
                    self.not_started = not_started_reason(item).map(str::to_owned);
                }
                events.extend(tool_done(item));
            }
            "thread/tokenUsage/updated" => {
                if let Some(total) = params.pointer("/tokenUsage/total") {
                    self.total = usage(total);
                }
            }
            "turn/completed" => {
                let turn = params.get("turn").unwrap_or(&Value::Null);
                let not_started = self.not_started.take();
                if turn.get("status").and_then(Value::as_str) == Some("failed") {
                    let error = turn.get("error").unwrap_or(&Value::Null);
                    events.push(AgentEvent::Failed(turn_error(error)));
                } else if let Some(reason) = not_started {
                    events.push(AgentEvent::Failed(not_started_error(&reason)));
                }
                events.push(AgentEvent::TurnDone(difference(self.total, self.at_turn_start)));
            }
            _ => {}
        }
        events
    }
}

/// The step of a tool item; `None` for items that are not tool calls.
fn tool_step(item: &Value) -> Option<ToolStep> {
    let item_type = item.get("type").and_then(Value::as_str)?;
    let field = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("");
    let kind = if item_type == "commandExecution" {
        StepKind::of_command(first_action(item).and_then(|action| action.get("type")?.as_str()))
    } else {
        StepKind::of_tool(item_type)
    };
    let detail = match item_type {
        "commandExecution" => command_detail(item),
        "fileChange" => {
            let changes = item.get("changes").and_then(Value::as_array);
            let paths: Vec<&str> = changes
                .into_iter()
                .flatten()
                .filter_map(|change| change.get("path").and_then(Value::as_str))
                .collect();
            paths.join(", ")
        }
        "mcpToolCall" => format!("{}/{}", field("server"), field("tool")),
        "dynamicToolCall" => field("tool").to_owned(),
        "webSearch" => field("query").to_owned(),
        "imageView" => field("path").to_owned(),
        _ => return None,
    };
    Some(ToolStep::new(field("id").to_owned(), item_type.to_owned(), kind, &detail))
}

/// The end of a tool item, with how it went; `None` for items that are not tool calls.
fn tool_done(item: &Value) -> Option<AgentEvent> {
    let step = tool_step(item)?;
    let status = item.get("status").and_then(Value::as_str);
    let succeeded = item.get("success").and_then(Value::as_bool);
    let failed = matches!(status, Some("failed" | "declined")) || succeeded == Some(false);
    let outcome = if failed { StepOutcome::Failed } else { StepOutcome::Ok };
    Some(AgentEvent::ToolDone { id: step.id, outcome })
}

/// Why Codex could not start the command `item` at all; `None` for any other item, also for a
/// command that started and failed.
fn not_started_reason(item: &Value) -> Option<&str> {
    if item.get("type").and_then(Value::as_str) != Some("commandExecution") {
        return None;
    }
    let output = item.get("aggregatedOutput").and_then(Value::as_str)?;
    output.strip_prefix(NOT_STARTED).map(str::trim)
}

/// The first action Codex parsed out of a command.
fn first_action(item: &Value) -> Option<&Value> {
    item.get("commandActions").and_then(Value::as_array).and_then(|actions| actions.first())
}

/// What a command does: the path or pattern Codex parsed out of it, else the command as Codex
/// parsed it, else the command line without the shell that runs it.
fn command_detail(item: &Value) -> String {
    let action = first_action(item);
    let action_field = |key: &str| {
        action
            .and_then(|action| action.get(key))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let parsed = match action.and_then(|action| action.get("type")).and_then(Value::as_str) {
        Some("read" | "listFiles") => action_field("path"),
        Some("search") => action_field("query").or_else(|| action_field("path")),
        _ => action_field("command"),
    };
    let command = item.get("command").and_then(Value::as_str).unwrap_or("");
    parsed.map_or_else(|| without_shell(command).to_owned(), str::to_owned)
}

/// `"...\pwsh.exe" -Command 'git log -1'` or `bash -lc 'git log -1'` → `git log -1`.
fn without_shell(command: &str) -> &str {
    let script = [" -Command ", " -lc ", " -c "]
        .iter()
        .find_map(|flag| command.split_once(flag).map(|(_, script)| script))
        .unwrap_or(command)
        .trim();
    ['\'', '"']
        .iter()
        .find_map(|quote| script.strip_prefix(*quote).and_then(|inner| inner.strip_suffix(*quote)))
        .unwrap_or(script)
}

fn usage(breakdown: &Value) -> Usage {
    let count = |key: &str| breakdown.get(key).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        input: count("inputTokens"),
        output: count("outputTokens"),
        cache_read: count("cachedInputTokens"),
        cache_write: count("cacheWriteInputTokens"),
    }
}

fn difference(now: Usage, before: Usage) -> Usage {
    Usage {
        input: now.input.saturating_sub(before.input),
        output: now.output.saturating_sub(before.output),
        cache_read: now.cache_read.saturating_sub(before.cache_read),
        cache_write: now.cache_write.saturating_sub(before.cache_write),
    }
}
