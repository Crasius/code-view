//! One agent CLI process that speaks JSON lines on stdin and stdout. It starts without a console
//! window, in the agent's checkout, with the user's environment minus the hosts' access tokens
//! (the CLI keeps its own sign-in variables), and dies with its owner (`kill_on_drop`).
//!
//! A reader task turns stdout into lines on a channel, so a driver can `select!` on lines and
//! commands; another keeps the last lines of stderr for [`AgentError::Exited`].
//!
//! On Windows an npm-installed Codex runs as `codex.cmd`, so the child is `cmd.exe`, which starts
//! `node`, which starts the native binary. Killing `cmd.exe` would leave the others running;
//! closing stdin ends the whole chain instead, because every machine mode exits at the end of its
//! input. [`AgentProcess::finish`] therefore closes stdin first and kills only after a grace time.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::{AgentError, platform};

/// Name parts that mark a variable as one host's: `GITHUB_TOKEN`, `GL_TOKEN`, a tool's own
/// `ORCA_AZURE_DEVOPS_TOKEN` (observed in a user environment) and the like.
const HOST_NAME_PARTS: [&str; 5] = ["GITHUB", "GH", "GITLAB", "GL", "DEVOPS"];
/// Last name parts that mark a variable as a secret.
const TOKEN_NAME_ENDS: [&str; 3] = ["TOKEN", "PAT", "ACCESSTOKEN"];
/// Azure Pipelines' job token, whose name names no host.
const PIPELINE_TOKEN: &str = "SYSTEM_ACCESSTOKEN";
/// How much of stderr [`AgentError::Exited`] shows.
const STDERR_TAIL_LINES: usize = 12;
/// How long a CLI may take to exit after its input closed before it is killed.
pub(crate) const EXIT_GRACE: Duration = Duration::from_secs(5);

/// A command for an agent CLI: no window, no inherited host tokens, killed when dropped.
pub(crate) fn command<I, S>(program: &Path, args: I, workdir: &Path) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    remove_host_tokens(&mut command, std::env::vars_os().map(|(name, _)| name));
    platform::hide_console(&mut command);
    command
}

/// Keeps every variable among `names` that holds a host's access token out of `command`: tools
/// such as `gh` and `glab` read them, and the agent must not be able to post, vote or merge.
fn remove_host_tokens(command: &mut Command, names: impl IntoIterator<Item = OsString>) {
    for name in names {
        if name.to_str().is_some_and(is_host_token) {
            command.env_remove(name);
        }
    }
}

/// Whether the variable `name` holds a host's access token: a host part in its name and a token
/// as its last part, in any case (Windows names ignore it). The CLIs' own sign-in variables, such
/// as `CLAUDE_CODE_OAUTH_TOKEN` or `OPENAI_API_KEY`, name no host and stay.
fn is_host_token(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    if name == PIPELINE_TOKEN {
        return true;
    }
    let mut parts = name.split('_');
    let is_token = parts.next_back().is_some_and(|last| TOKEN_NAME_ENDS.contains(&last));
    is_token && parts.any(|part| HOST_NAME_PARTS.contains(&part))
}

pub(crate) struct AgentProcess {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::UnboundedReceiver<String>,
    stderr_tail: JoinHandle<String>,
}

impl AgentProcess {
    /// Starts `command` (from [`command`]) and its reader tasks on the current runtime.
    pub(crate) fn spawn(mut command: Command) -> Result<Self, AgentError> {
        let mut child = command.spawn().map_err(|error| AgentError::Spawn(error.to_string()))?;
        let missing = || AgentError::Spawn("the agent's pipes are missing".to_owned());
        let stdin = child.stdin.take().ok_or_else(missing)?;
        let stdout = child.stdout.take().ok_or_else(missing)?;
        let stderr = child.stderr.take().ok_or_else(missing)?;
        let (line_tx, lines) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut buffer = Vec::new();
            // A read error ends the stream like EOF does; the exit code tells the rest.
            while reader.read_until(b'\n', &mut buffer).await.unwrap_or(0) > 0 {
                let line = String::from_utf8_lossy(&buffer).trim().to_owned();
                buffer.clear();
                if !line.is_empty() && line_tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr_tail = tokio::spawn(read_tail(stderr));
        Ok(Self { child, stdin, lines, stderr_tail })
    }

    /// The next stdout line; `None` once stdout closed.
    pub(crate) async fn next_line(&mut self) -> Option<String> {
        self.lines.recv().await
    }

    /// Writes `message` as one JSON line. A failed write means the CLI no longer reads its input;
    /// [`AgentProcess::finish`] then tells how it ended.
    pub(crate) async fn send(&mut self, message: &serde_json::Value) -> std::io::Result<()> {
        let mut line = message.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await
    }

    /// Closes stdin, waits up to `grace` for the exit, then kills. Returns the exit code (`None`
    /// when killed or ended by a signal) and the last lines of stderr.
    pub(crate) async fn finish(self, grace: Duration) -> (Option<i32>, String) {
        let Self { mut child, stdin, mut stderr_tail, .. } = self;
        drop(stdin);
        let code = if let Ok(Ok(status)) = tokio::time::timeout(grace, child.wait()).await {
            status.code()
        } else {
            // Already gone or unkillable: there is nothing more to do with it either way.
            let _kill_may_fail_when_gone = child.kill().await;
            None
        };
        // A grandchild that outlived a killed `cmd.exe` may hold stderr open; do not wait for it.
        let tail = if let Ok(tail) = tokio::time::timeout(grace, &mut stderr_tail).await {
            tail.unwrap_or_default()
        } else {
            stderr_tail.abort();
            String::new()
        };
        (code, tail)
    }
}

/// The last [`STDERR_TAIL_LINES`] non-empty lines of `stderr`, without terminal colour codes.
async fn read_tail(stderr: impl tokio::io::AsyncRead + Unpin) -> String {
    let mut tail: VecDeque<String> = VecDeque::with_capacity(STDERR_TAIL_LINES);
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    // A read error ends the tail like EOF does.
    while reader.read_until(b'\n', &mut line).await.unwrap_or(0) > 0 {
        push_line(&mut tail, &String::from_utf8_lossy(&line));
        line.clear();
    }
    Vec::from(tail).join("\n")
}

fn push_line(tail: &mut VecDeque<String>, line: &str) {
    let line = strip_ansi(line.trim());
    if line.is_empty() {
        return;
    }
    if tail.len() == STDERR_TAIL_LINES {
        tail.pop_front();
    }
    tail.push_back(line);
}

/// `text` without ANSI escape sequences (`ESC [ ... letter`), which Codex's log lines carry.
fn strip_ansi(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character != '\u{1b}' {
            plain.push(character);
            continue;
        }
        if chars.next() == Some('[') {
            // Parameters and intermediates run up to the final byte, a letter.
            for code in chars.by_ref() {
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    plain
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use tokio::process::Command;

    use super::{remove_host_tokens, strip_ansi};

    #[test]
    fn the_agent_starts_without_the_hosts_access_tokens() {
        let host_tokens = [
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
            "GITHUB_PERSONAL_ACCESS_TOKEN",
            "GITLAB_TOKEN",
            "GL_TOKEN",
            "GITLAB_PRIVATE_TOKEN",
            "AZURE_DEVOPS_EXT_PAT",
            "AZURE_DEVOPS_TOKEN",
            "ORCA_AZURE_DEVOPS_TOKEN",
            "System_AccessToken",
        ];
        let kept = [
            "CLAUDE_CODE_OAUTH_TOKEN",
            "OPENAI_API_KEY",
            "CODEX_HOME",
            "GITHUB_ACTIONS",
            "HIGH_TOKEN",
            "PATH",
        ];
        let mut command = Command::new("agent");
        let names = host_tokens.iter().chain(&kept).map(|name| (*name).into());
        remove_host_tokens(&mut command, names);
        let removed: Vec<&OsStr> = command
            .as_std()
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key)
            .collect();
        for variable in host_tokens {
            assert!(removed.contains(&OsStr::new(variable)), "{variable} is removed");
        }
        for variable in kept {
            assert!(!removed.contains(&OsStr::new(variable)), "{variable} stays");
        }
    }

    #[test]
    fn stderr_tail_drops_terminal_colours() {
        let line = "\u{1b}[2m2026-10-20T12:10:49Z\u{1b}[0m \u{1b}[31mERROR\u{1b}[0m worker quit";
        assert_eq!(strip_ansi(line), "2026-10-20T12:10:49Z ERROR worker quit");
    }
}
