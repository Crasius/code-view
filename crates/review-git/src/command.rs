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
use crate::progress::{self, GitEvent};

const MAX_STDERR_CHARS: usize = 800;

/// Settings that keep every invocation non-interactive and fast on large repositories.
const FIXED_CONFIG: &[(&str, &str)] = &[
    ("credential.helper", ""),
    ("core.askPass", ""),
    ("core.quotePath", "false"),
    // Blobs are compared byte for byte: no line-ending conversion, whatever the user's config says.
    ("core.autocrlf", "false"),
    ("core.eol", "lf"),
    // An empty context line keeps its leading space, so the diff parser counts it.
    ("diff.suppressBlankEmpty", "false"),
    ("diff.renameLimit", "5000"),
    ("gc.auto", "0"),
    ("maintenance.auto", "false"),
    ("fetch.recurseSubmodules", "false"),
];

/// What a command in a user's working tree changes from the user's configuration: nothing that
/// shapes the files git writes there, only what would prompt, hold the command up or start work
/// in the background.
const TREE_CONFIG: &[(&str, &str)] = &[
    ("credential.helper", ""),
    ("core.askPass", ""),
    ("core.quotePath", "false"),
    ("gc.auto", "0"),
    ("maintenance.auto", "false"),
    ("fetch.recurseSubmodules", "false"),
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
        sink: &(dyn Fn(GitEvent) + Send + Sync + '_),
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
            let mut tail = String::new();
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
                        tail.push_str(line.trim());
                        tail.push('\n');
                    }
                }
            }
            Ok::<String, std::io::Error>(tail)
        };
        let (stdout_result, stderr_result) = tokio::join!(read_stdout, read_stderr);
        let status = child.wait().await.map_err(|error| spawn_error(&error))?;
        let output = stdout_result?;
        let stderr_text = stderr_result?;
        if status.success() {
            return Ok(output);
        }
        let trimmed: String = stderr_text.trim().chars().take(MAX_STDERR_CHARS).collect();
        Err(ProviderError::Local(format!("git failed: {trimmed}")))
    }

    fn build<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new("git");
        let fixed = match self.location {
            Location::GitDir(git_dir) => {
                command.arg("--git-dir").arg(git_dir);
                FIXED_CONFIG
            }
            Location::WorkTree(dir) => {
                command.arg("-C").arg(dir);
                TREE_CONFIG
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

        let mut config: Vec<(&str, &str)> = fixed.to_vec();
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
    let stderr = String::from_utf8_lossy(&output.stderr);
    let trimmed: String = stderr.trim().chars().take(MAX_STDERR_CHARS).collect();
    Err(ProviderError::Local(format!("git failed: {trimmed}")))
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
