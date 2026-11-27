//! Claude Code in its SDK mode: `claude -p` with stream-json on stdin and stdout, one process per
//! session, questions as `user` messages and the interrupt as a `control_request`.
//!
//! The session is read-only and isolated: `--restricted` ignores the user's, the project's and the
//! local settings files (so no hooks and no permission rules from a pull request run) and keeps
//! the sign-in; `--strict-mcp-config` loads no MCP server; `--tools` leaves Read, Grep, Glob and
//! Bash, and `--permission-mode dontAsk` refuses every call the allow list does not cover without
//! a prompt. The allow list names the git commands that read history; Claude Code also lets its
//! own read-only commands through (observed: `git status` ran, `echo hi > y.txt` was refused).
//! The brief goes in a file because a Windows command line holds at most 32,767 characters.

mod stream;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::driver::{Launch, Protocol, Reaction};
use crate::process::{self, AgentProcess, EXIT_GRACE};
use crate::{AgentCli, AgentError, AgentEvent, ModelInfo, SessionConfig};
use stream::ClaudeStream;

/// SDK mode: one stream-json message per line on stdin and stdout.
const MACHINE_MODE: [&str; 6] =
    ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"];
/// The isolation from the module doc: no settings files and no MCP servers.
const ISOLATED: [&str; 2] = ["--restricted", "--strict-mcp-config"];
/// The tools the agent gets: reading files and running the allowed commands.
const TOOLS: &str = "Read,Grep,Glob,Bash";
/// The Bash commands the agent may run without asking: git's read-only history commands.
const ALLOWED_COMMANDS: [&str; 5] = [
    "Bash(git log:*)",
    "Bash(git show:*)",
    "Bash(git diff:*)",
    "Bash(git blame:*)",
    "Bash(git grep:*)",
];
/// How long the CLI may take to answer `initialize` with its models.
const MODELS_TIMEOUT: Duration = Duration::from_secs(30);
/// The documented `--model` aliases, for a CLI whose `initialize` lists no models.
const ALIASES: [(&str, &str); 3] = [("haiku", "Haiku"), ("sonnet", "Sonnet"), ("opus", "Opus")];

/// Makes each session's brief file name unique within this process.
static BRIEF_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) async fn launch(config: &SessionConfig) -> Result<Launch, AgentError> {
    let brief_file = write_brief(&config.brief).await?;
    let command = process::command(&config.cli.program, args(config, &brief_file), &config.workdir);
    let protocol = ClaudeProtocol {
        stream: ClaudeStream::new(&config.workdir.to_string_lossy()),
        turn_running: false,
        interrupts: 0,
    };
    Ok(Launch { command, protocol: Box::new(protocol), temporary_file: Some(brief_file) })
}

/// The command line of a session; the brief is in `brief_file`.
pub(crate) fn args(config: &SessionConfig, brief_file: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = MACHINE_MODE.map(OsString::from).to_vec();
    args.push(OsString::from("--include-partial-messages"));
    let choices = [
        ("--model", config.pick.model.as_deref()),
        ("--effort", config.pick.effort.as_deref()),
        ("--resume", config.resume.as_deref()),
    ];
    for (flag, value) in choices {
        if let Some(value) = value {
            args.extend([OsString::from(flag), OsString::from(value)]);
        }
    }
    args.extend(ISOLATED.map(OsString::from));
    args.extend(["--tools", TOOLS, "--allowedTools"].map(OsString::from));
    args.extend(ALLOWED_COMMANDS.map(OsString::from));
    args.extend(
        ["--permission-mode", "dontAsk", "--append-system-prompt-file"].map(OsString::from),
    );
    args.push(brief_file.as_os_str().to_owned());
    args
}

/// Writes the brief to a file of its own in the temp folder; the session removes it at the end.
async fn write_brief(brief: &str) -> Result<PathBuf, AgentError> {
    let dir = std::env::temp_dir().join("codeview-assistant");
    let number = BRIEF_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file = dir.join(format!("brief-{}-{number}.md", std::process::id()));
    let written = async {
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::write(&file, brief).await
    };
    written
        .await
        .map_err(|error| AgentError::Spawn(format!("could not write the brief: {error}")))?;
    Ok(file)
}

struct ClaudeProtocol {
    stream: ClaudeStream,
    turn_running: bool,
    interrupts: u64,
}

impl Protocol for ClaudeProtocol {
    fn opening(&mut self) -> Vec<Value> {
        Vec::new()
    }

    fn is_ready(&self) -> bool {
        !self.turn_running
    }

    fn prompt(&mut self, text: &str) -> Vec<Value> {
        self.turn_running = true;
        vec![json!({"type": "user", "message": {"role": "user", "content": text}})]
    }

    fn interrupt(&mut self) -> Vec<Value> {
        if !self.turn_running {
            return Vec::new();
        }
        self.interrupts += 1;
        self.stream.interrupt_requested();
        let request_id = format!("interrupt-{}", self.interrupts);
        vec![
            json!({"type": "control_request", "request_id": request_id, "request": {"subtype": "interrupt"}}),
        ]
    }

    fn read(&mut self, line: &str) -> Reaction {
        let (events, send) = self.stream.read(line);
        if events.iter().any(|event| matches!(event, AgentEvent::TurnDone(_))) {
            self.turn_running = false;
        }
        Reaction { events, send, close: false }
    }
}

/// The models from the CLI's answer to the SDK's `initialize` control request; the documented
/// aliases when it lists none.
pub(crate) async fn list_models(cli: &AgentCli) -> Result<Vec<ModelInfo>, AgentError> {
    let args = MACHINE_MODE.iter().chain(&ISOLATED).chain(&["--no-session-persistence"]);
    let command = process::command(&cli.program, args, &std::env::temp_dir());
    let mut agent = AgentProcess::spawn(command)?;
    let request = json!({"type": "control_request", "request_id": "models", "request": {"subtype": "initialize"}});
    let answer = tokio::time::timeout(MODELS_TIMEOUT, async {
        agent.send(&request).await.map_err(|_| None)?;
        initialize_answer(&mut agent).await.map_err(Some)
    })
    .await;
    let (code, stderr_tail) = agent.finish(EXIT_GRACE).await;
    let response = answer.unwrap_or(Err(Some(AgentError::Timeout("the model list"))));
    response
        .map(|response| models_from_initialize(&response))
        .map_err(|error| error.unwrap_or(AgentError::Exited { code, stderr_tail }))
}

async fn initialize_answer(agent: &mut AgentProcess) -> Result<Value, AgentError> {
    while let Some(line) = agent.next_line().await {
        let Ok(mut message) = serde_json::from_str::<Value>(&line) else { continue };
        let is_answer = message.get("type").and_then(Value::as_str) == Some("control_response")
            && message.pointer("/response/request_id").and_then(Value::as_str) == Some("models");
        if !is_answer {
            continue;
        }
        let mut response = message.get_mut("response").map(Value::take).unwrap_or_default();
        if response.get("subtype").and_then(Value::as_str) == Some("error") {
            let words =
                response.get("error").and_then(Value::as_str).unwrap_or("initialize failed");
            return Err(AgentError::Protocol(words.to_owned()));
        }
        return Ok(response.get_mut("response").map(Value::take).unwrap_or_default());
    }
    Err(AgentError::Protocol("the CLI ended before it listed its models".to_owned()))
}

/// `models` of an `initialize` answer, in order, without the "default" entry (the CLI's own
/// choice; the entry with the same resolved model becomes the default instead).
pub(crate) fn models_from_initialize(response: &Value) -> Vec<ModelInfo> {
    let entries: &[Value] =
        response.get("models").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let field =
        |entry: &Value, key: &str| entry.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
    let default_model = entries
        .iter()
        .find(|entry| entry.get("value").and_then(Value::as_str) == Some("default"))
        .map(|entry| field(entry, "resolvedModel"));
    let mut default_taken = false;
    let mut models = Vec::new();
    for entry in entries {
        let id = field(entry, "value");
        if id.is_empty() || id == "default" {
            continue;
        }
        let is_default = !default_taken
            && default_model.as_deref().is_some_and(|model| field(entry, "resolvedModel") == model);
        default_taken |= is_default;
        let efforts = entry.get("supportedEffortLevels").and_then(Value::as_array);
        models.push(ModelInfo {
            display_name: field(entry, "displayName"),
            description: field(entry, "description"),
            is_default,
            efforts: efforts
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            id,
        });
    }
    if models.is_empty() {
        return ALIASES
            .iter()
            .map(|(id, name)| ModelInfo {
                id: (*id).to_owned(),
                display_name: (*name).to_owned(),
                description: String::new(),
                is_default: false,
                efforts: Vec::new(),
            })
            .collect();
    }
    models
}

#[cfg(test)]
mod tests;
