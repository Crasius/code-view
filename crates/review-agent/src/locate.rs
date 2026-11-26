//! Finds an agent CLI (or another program, [`find_program`]) on `PATH` the way a shell opened now
//! would and reads its version line: the process's own `PATH` first, then the one the system saved
//! for new processes, which already holds the folder of a CLI installed after the app started
//! (`platform::saved_paths`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::process;
use crate::{AgentCli, AgentError, AgentKind};

/// `--version` answers at once; a CLI that hangs there is broken.
const VERSION_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn program_name(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
    }
}

/// The first `name` in the folders of `paths` (`PATH` values), searched in order.
fn find_on_paths(name: &str, paths: &[OsString]) -> Option<PathBuf> {
    paths
        .iter()
        .flat_map(std::env::split_paths)
        .find_map(|dir| crate::platform::executable_in(&dir, name))
}

/// The program `name` (no extension) as a shell opened now would find it: on the process's `PATH`,
/// then on the one the system saved for new processes.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let paths: Vec<OsString> =
        std::env::var_os("PATH").into_iter().chain(crate::platform::saved_paths()).collect();
    find_on_paths(name, &paths)
}

pub(crate) async fn detect(kind: AgentKind) -> Result<AgentCli, AgentError> {
    let program = find_program(program_name(kind)).ok_or(AgentError::NotInstalled(kind))?;
    let version = version(&program).await?;
    Ok(AgentCli { kind, program, version })
}

/// The first line `program --version` prints.
async fn version(program: &Path) -> Result<String, AgentError> {
    let cwd = std::env::temp_dir();
    let mut command = process::command(program, ["--version"], &cwd);
    command.stdin(std::process::Stdio::null());
    let output = tokio::time::timeout(VERSION_TIMEOUT, command.output())
        .await
        .map_err(|_| AgentError::Spawn(format!("{} --version did not answer", program.display())))?
        .map_err(|error| AgentError::Spawn(error.to_string()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().map(str::trim).find(|line| !line.is_empty());
    match line {
        Some(line) if output.status.success() => Ok(line.to_owned()),
        _ => Err(AgentError::Exited {
            code: output.status.code(),
            stderr_tail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::find_on_paths;

    #[test]
    fn a_cli_whose_folder_only_the_saved_path_lists_is_found() {
        let root = std::env::temp_dir().join(format!("review-agent-paths-{}", std::process::id()));
        let (stale, installed) = (root.join("stale"), root.join("installed"));
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::create_dir_all(&installed).unwrap();
        let program = installed.join(if cfg!(windows) { "claude.exe" } else { "claude" });
        std::fs::write(&program, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let process_path = OsString::from(stale.as_os_str());
        let saved_path = std::env::join_paths([&stale, &installed]).unwrap();

        let from_process_only = find_on_paths("claude", std::slice::from_ref(&process_path));
        let with_saved = find_on_paths("claude", &[process_path, saved_path]);
        std::fs::remove_dir_all(&root).unwrap();

        assert_eq!(from_process_only, None, "the process's own PATH predates the install");
        assert_eq!(with_saved, Some(program));
    }
}
