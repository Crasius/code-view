//! Runs `git` with credentials passed through the environment (never the command line),
//! no prompts and no console window. Commands run either on one of the app's own repositories (a
//! mirror's git dir, with settings that suit diffing) or in a working tree of the user's, where git
//! keeps the user's own settings (line endings above all) and only prompts and maintenance are off.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Stdio;

use review_core::{ProviderError, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::platform;
use crate::progress::{self, GitEvent, ProgressSink};

const MAX_STDERR_CHARS: usize = 800;

/// What every command changes from the user's configuration, in a user's working tree as well:
/// nothing that shapes the files git writes there, only what would prompt, hold the command up or
/// start work in the background.
const NON_INTERACTIVE_CONFIG: &[(&str, &str)] = &[
    ("credential.helper", ""),
    ("core.askPass", ""),
    ("core.quotePath", "false"),
    ("gc.auto", "0"),
    ("maintenance.auto", "false"),
    ("fetch.recurseSubmodules", "false"),
];

/// What a command on one of the app's own repositories adds, for diffs that compare blobs as
/// they are.
const MIRROR_CONFIG: &[(&str, &str)] = &[
    // Blobs are compared byte for byte: no line-ending conversion, whatever the user's config says.
    ("core.autocrlf", "false"),
    ("core.eol", "lf"),
    // An empty context line keeps its leading space, so the diff parser counts it.
    ("diff.suppressBlankEmpty", "false"),
    ("diff.renameLimit", "5000"),
];

/// Where a command runs.
#[derive(Clone, Copy)]
enum Location<'a> {
    /// An app-owned repository, named by its git dir (`--git-dir`).
    GitDir(&'a Path),
    /// A user's working tree (`-C`): the user's clone or one of its worktrees.
    WorkTree(&'a Path),
}

/// Where the objects of a command may come from.
#[derive(Clone, Copy)]
pub(crate) enum Objects {
    /// Missing blobs download from the remote (the mirror is a blob-less partial clone).
    FetchMissing,
    LocalOnly,
}

pub(crate) struct GitCommand<'a> {
    location: Location<'a>,
    authorization_header: Option<&'a str>,
}

impl<'a> GitCommand<'a> {
    pub(crate) fn new(git_dir: &'a Path, authorization_header: Option<&'a str>) -> Self {
        Self { location: Location::GitDir(git_dir), authorization_header }
    }

    /// Commands in the user's working tree `dir`, without credentials: what such a command
    /// fetches comes from a local mirror.
    pub(crate) fn in_tree(dir: &'a Path) -> Self {
        Self { location: Location::WorkTree(dir), authorization_header: None }
    }

    /// Runs git and returns stdout. A non-zero exit becomes [`ProviderError::Local`] with stderr.
    pub(crate) async fn run<I, S>(&self, args: I) -> Result<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        stdout_of(self.command(args, Objects::FetchMissing)).await
    }

    /// [`Self::run`] for a question git answers "none" to with exit code 1 and an empty stderr, as
    /// `rev-parse --verify --quiet` does for a name that resolves to nothing: `None` then. Every
    /// other failure (128 with `fatal:` for a folder that is no repository) is an error.
    pub(crate) async fn run_or_none<I, S>(&self, args: I) -> Result<Option<Vec<u8>>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self
            .command(args, Objects::FetchMissing)
            .output()
            .await
            .map_err(|error| spawn_error(&error))?;
        if output.status.code() == Some(1) && output.stderr.is_empty() {
            return Ok(None);
        }
        checked(output).map(Some)
    }

    /// [`Self::run`] on local objects only: a missing blob fails the command instead of being
    /// fetched lazily from the promisor remote.
    pub(crate) async fn run_offline<I, S>(&self, args: I) -> Result<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        stdout_of(self.command(args, Objects::LocalOnly)).await
    }

    /// [`Self::run_offline`] with `input` on stdin, written while git already answers.
    pub(crate) async fn run_offline_with_input<I, S>(
        &self,
        args: I,
        input: &[u8],
    ) -> Result<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(args, Objects::LocalOnly);
        command.stdin(Stdio::piped());
        let mut child = command.spawn().map_err(|error| spawn_error(&error))?;
        let mut stdin =
            child.stdin.take().ok_or_else(|| ProviderError::Local("git stdin missing".into()))?;
        let write = async move {
            let written = stdin.write_all(input).await;
            // Closing stdin ends git's input.
            drop(stdin);
            written
        };
        let (written, output) = tokio::join!(write, child.wait_with_output());
        let stdout = checked(output.map_err(|error| spawn_error(&error))?)?;
        written?;
        Ok(stdout)
    }

    /// The git process [`Self::run`] (`FetchMissing`) or [`Self::run_offline`] (`LocalOnly`)
    /// starts, for [`stdout_of`] to run apart from this borrow: on another task, several at a time.
    pub(crate) fn command<I, S>(&self, args: I, objects: Objects) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.build(args);
        if let Objects::LocalOnly = objects {
            command.env("GIT_NO_LAZY_FETCH", "1");
        }
        command
    }

    /// Runs git with `--progress` output on stderr parsed into `sink` as it arrives. The sink first
    /// gets the command line, then every progress update and every other stderr line.
    pub(crate) async fn run_with_progress<I, S>(
        &self,
        args: I,
        sink: &ProgressSink<'_>,
    ) -> Result<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args.into_iter().map(|arg| arg.as_ref().to_owned()).collect();
        sink(GitEvent::Command(display(&args)));
        let mut child = self.build(&args).spawn().map_err(|error| spawn_error(&error))?;
        let mut stdout =
            child.stdout.take().ok_or_else(|| ProviderError::Local("git stdout missing".into()))?;
        let mut stderr =
            child.stderr.take().ok_or_else(|| ProviderError::Local("git stderr missing".into()))?;

        let read_stdout = async {
            let mut output = Vec::new();
            stdout.read_to_end(&mut output).await.map(|_| output)
        };
        let read_stderr = async {
            let mut messages = String::new();
            let mut pending = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let read = stderr.read(&mut chunk).await?;
                if read == 0 {
                    break;
                }
                pending.extend_from_slice(&chunk[..read]);
                while let Some(position) =
                    pending.iter().position(|byte| *byte == b'\r' || *byte == b'\n')
                {
                    let line_bytes: Vec<u8> = pending.drain(..=position).collect();
                    let line = String::from_utf8_lossy(&line_bytes);
                    if let Some(progress) = progress::parse_line(&line) {
                        sink(GitEvent::Progress(progress));
                    } else if !line.trim().is_empty() {
                        sink(GitEvent::Message(line.trim_end().to_owned()));
                        messages.push_str(line.trim());
                        messages.push('\n');
                    }
                }
            }
            Ok::<String, std::io::Error>(messages)
        };
        let (stdout_result, stderr_result) = tokio::join!(read_stdout, read_stderr);
        let status = child.wait().await.map_err(|error| spawn_error(&error))?;
        let output = stdout_result?;
        let stderr_text = stderr_result?;
        if status.success() {
            return Ok(output);
        }
        Err(failure(&stderr_text))
    }

    fn build<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new("git");
        let location_config: &[(&str, &str)] = match self.location {
            Location::GitDir(git_dir) => {
                command.arg("--git-dir").arg(git_dir);
                MIRROR_CONFIG
            }
            Location::WorkTree(dir) => {
                command.arg("-C").arg(dir);
                &[]
            }
        };
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .kill_on_drop(true);

        let mut config: Vec<(&str, &str)> =
            NON_INTERACTIVE_CONFIG.iter().chain(location_config).copied().collect();
        if let Some(header) = self.authorization_header {
            config.push(("http.extraHeader", header));
        }
        command.env("GIT_CONFIG_COUNT", config.len().to_string());
        for (index, (key, value)) in config.iter().enumerate() {
            command.env(format!("GIT_CONFIG_KEY_{index}"), key);
            command.env(format!("GIT_CONFIG_VALUE_{index}"), value);
        }

        platform::hide_console(&mut command);
        command
    }
}

/// Runs `command` and returns its stdout. A non-zero exit becomes [`ProviderError::Local`] with
/// stderr.
pub(crate) async fn stdout_of(mut command: Command) -> Result<Vec<u8>> {
    checked(command.output().await.map_err(|error| spawn_error(&error))?)
}

fn checked(output: std::process::Output) -> Result<Vec<u8>> {
    if output.status.success() {
        // The read doubles its buffer as output arrives; a blob kept for the file view would hold
        // up to twice its size without this.
        let mut stdout = output.stdout;
        stdout.shrink_to_fit();
        return Ok(stdout);
    }
    Err(failure(&String::from_utf8_lossy(&output.stderr)))
}

/// The error of a failed git command: the last [`MAX_STDERR_CHARS`] characters of its `stderr`,
/// where git prints the `fatal:` line after any progress and ref lines.
fn failure(stderr: &str) -> ProviderError {
    let stderr = stderr.trim();
    let cut = stderr.chars().count().saturating_sub(MAX_STDERR_CHARS);
    if cut == 0 {
        return ProviderError::Local(format!("git failed: {stderr}"));
    }
    let kept: String = stderr.chars().skip(cut).collect();
    ProviderError::Local(format!("git failed: …{kept}"))
}

/// `git fetch --progress origin +refs/…` as the user would type it (without `--git-dir`).
fn display(args: &[OsString]) -> String {
    let mut line = String::from("git");
    for arg in args {
        line.push(' ');
        line.push_str(&arg.to_string_lossy());
    }
    line
}

fn spawn_error(error: &std::io::Error) -> ProviderError {
    ProviderError::Local(format!("cannot start git (is it installed and on PATH?): {error}"))
}

#[cfg(test)]
mod tests {
    use review_core::ProviderError;

    use super::{MAX_STDERR_CHARS, failure};

    /// Regression: the error kept the first 800 characters, so a fetch's ref lines pushed the
    /// `fatal:` line that names the cause out of it.
    #[test]
    fn a_long_error_keeps_its_last_line() {
        let refs = " * [new ref] refs/pull/1/head -> refs/review/pull/1/head\n".repeat(40);
        let stderr = format!("From file:///origin\n{refs}fatal: couldn't find remote ref x\n");
        let ProviderError::Local(message) = failure(&stderr) else {
            panic!("a local error");
        };
        assert!(message.starts_with("git failed: …"), "{message}");
        assert!(message.ends_with("fatal: couldn't find remote ref x"), "{message}");
        let kept = message.trim_start_matches("git failed: …");
        assert_eq!(kept.chars().count(), MAX_STDERR_CHARS);

        let ProviderError::Local(short) = failure("  fatal: not a git repository\n") else {
            panic!("a local error");
        };
        assert_eq!(short, "git failed: fatal: not a git repository");
    }
}
