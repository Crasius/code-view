//! "Use my GitHub CLI / Azure CLI sign-in": asks an installed command-line tool for the token it
//! already holds. Runs only when the user clicks the option. The token goes straight from the
//! tool's stdout into the connection (and from there into the DPAPI-protected store); it is never
//! printed or logged, and error messages only carry the tool's stderr.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use cv_base::platform;
use review_core::ProviderKind;

/// The Azure DevOps resource id that Microsoft Entra tokens for Azure DevOps are issued for.
const AZURE_DEVOPS_RESOURCE: &str = "499b84ac-1321-427f-aa17-267ca6975798";
const TIMEOUT: Duration = Duration::from_secs(45);

/// Which sign-in tools are installed (found on `PATH`); nothing is executed to find out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CliTools {
    pub github: bool,
    pub azure: bool,
}

impl CliTools {
    pub fn detect() -> Self {
        Self { github: on_path(&["gh.exe", "gh"]), azure: on_path(&["az.cmd", "az"]) }
    }

    /// The sign-in this platform and host can use, if its tool is installed.
    pub fn login_for(
        self,
        kind: ProviderKind,
        host_name: &str,
        is_cloud: bool,
    ) -> Option<CliLogin> {
        match kind {
            ProviderKind::GitHub if self.github && is_host_name(host_name) => {
                Some(CliLogin::GitHub { host: host_name.to_owned() })
            }
            // Entra tokens exist for Azure DevOps Services only, not for on-premises servers.
            ProviderKind::AzureDevOps if self.azure && is_cloud => Some(CliLogin::Azure),
            _ => None,
        }
    }
}

fn on_path(file_names: &[&str]) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path)
        .any(|directory| file_names.iter().any(|name| is_file(&directory.join(name))))
}

fn is_file(path: &Path) -> bool {
    path.metadata().is_ok_and(|metadata| metadata.is_file())
}

/// Host names only reach the command line after this check, so no argument can smuggle in options.
fn is_host_name(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('-')
        && host.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | ':')
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliLogin {
    GitHub { host: String },
    Azure,
}

impl CliLogin {
    pub fn tool_name(&self) -> &'static str {
        match self {
            CliLogin::GitHub { .. } => "GitHub CLI",
            CliLogin::Azure => "Azure CLI",
        }
    }

    /// The command that signs in, for the user to run in a terminal when the tool has no sign-in yet.
    pub fn sign_in_command(&self) -> String {
        match self {
            CliLogin::GitHub { host } => format!("gh auth login --hostname {host}"),
            CliLogin::Azure => "az login".to_owned(),
        }
    }

    pub fn explanation(&self) -> &'static str {
        match self {
            CliLogin::GitHub { .. } => {
                "Code View asks gh for the token you already use in the terminal. Nothing to create or copy."
            }
            CliLogin::Azure => {
                "Code View asks az for a Microsoft Entra token with your own permissions. It lasts up to an hour; \
                 Review renews it from the Azure CLI while the app runs."
            }
        }
    }

    fn program_and_arguments(&self) -> (&'static str, Vec<String>) {
        match self {
            CliLogin::GitHub { host } => {
                ("gh", vec!["auth".into(), "token".into(), "--hostname".into(), host.clone()])
            }
            CliLogin::Azure => (
                platform::AZURE_CLI,
                vec![
                    "account".into(),
                    "get-access-token".into(),
                    "--resource".into(),
                    AZURE_DEVOPS_RESOURCE.into(),
                    "--query".into(),
                    "accessToken".into(),
                    "--output".into(),
                    "tsv".into(),
                ],
            ),
        }
    }

    /// Runs the tool and returns the token from its stdout.
    pub async fn fetch_token(self) -> Result<String, String> {
        let (program, arguments) = self.program_and_arguments();
        let mut command = tokio::process::Command::new(program);
        command
            .args(&arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Keeps `gh`/`az` from flashing a console window in the GUI build.
        platform::hide_console(&mut command);
        let tool = self.tool_name();
        let output = match tokio::time::timeout(TIMEOUT, command.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => return Err(format!("Could not start the {tool}: {error}")),
            Err(_) => {
                return Err(format!(
                    "The {tool} did not answer within {} seconds.",
                    TIMEOUT.as_secs()
                ));
            }
        };
        let token = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if output.status.success() && !token.is_empty() && !token.contains(char::is_whitespace) {
            return Ok(token);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("no sign-in found");
        Err(format!("The {tool} has no token for this host ({reason})."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_only_matching_installed_tools() {
        let both = CliTools { github: true, azure: true };
        assert_eq!(
            both.login_for(ProviderKind::GitHub, "github.com", true),
            Some(CliLogin::GitHub { host: "github.com".into() })
        );
        assert_eq!(
            both.login_for(ProviderKind::AzureDevOps, "dev.azure.com", true),
            Some(CliLogin::Azure)
        );
        assert_eq!(both.login_for(ProviderKind::AzureDevOps, "tfs.example.com", false), None);
        assert_eq!(both.login_for(ProviderKind::GitLab, "gitlab.com", true), None);
        assert_eq!(CliTools::default().login_for(ProviderKind::GitHub, "github.com", true), None);
        assert_eq!(both.login_for(ProviderKind::GitHub, "--evil", false), None);
    }

    #[test]
    fn sign_in_commands_name_the_host() {
        assert_eq!(
            CliLogin::GitHub { host: "git.corp".into() }.sign_in_command(),
            "gh auth login --hostname git.corp"
        );
        assert_eq!(CliLogin::Azure.sign_in_command(), "az login");
    }
}
