//! The markers a coding agent leaves in the environment of the programs it starts. The app is
//! often started from a terminal or a script inside such a session; inherited, they would make the
//! assistant's CLIs and the terminals the app opens believe they are that session's children
//! (Claude Code then stops saving transcripts, so "Continue in terminal" finds no session to
//! resume) and would hand them the parent's private messaging socket and token.
#![expect(unsafe_code, reason = "removing environment variables is unsafe since Rust 2024")]

/// Set by Claude Code and the Codex companion for their child processes (`[observed]` in both).
/// The user's own configuration (`CLAUDE_CODE_USE_*`, `CODEX_HOME`, …) stays.
pub(super) const PARENT_SESSION_VARIABLES: [&str; 9] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_PID",
    "CODEX_COMPANION_SESSION_ID",
];

/// Removes [`PARENT_SESSION_VARIABLES`] from the app's environment, so nothing it starts inherits
/// them. Call first in `main`, before any other thread runs.
pub(super) fn forget_parent_session() {
    for variable in PARENT_SESSION_VARIABLES {
        // SAFETY: called at the start of `main`, while no other thread reads the environment.
        unsafe { std::env::remove_var(variable) };
    }
}

#[cfg(test)]
mod tests {
    use super::{PARENT_SESSION_VARIABLES, forget_parent_session};

    const TEST_NAME: &str =
        "platform::session::tests::a_parent_agent_session_does_not_reach_children";
    /// Set only in the child run below, where this test is the one thread that runs.
    const CHILD_MARKER: &str = "CODEVIEW_SESSION_TEST_CHILD";

    /// The environment is process-wide, so the test runs itself again in a child process that
    /// carries the markers, and that child checks what a program it starts would inherit.
    #[test]
    #[expect(clippy::disallowed_methods, reason = "a test, not the UI thread, waits for its child")]
    fn a_parent_agent_session_does_not_reach_children() {
        if std::env::var_os(CHILD_MARKER).is_some() {
            forget_parent_session();
            for variable in PARENT_SESSION_VARIABLES {
                assert!(std::env::var_os(variable).is_none(), "{variable} is gone");
            }
            assert!(std::env::var_os("CLAUDE_CODE_USE_BEDROCK").is_some(), "configuration stays");
            return;
        }
        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"));
        child.args(["--exact", TEST_NAME, "--test-threads=1"]).env(CHILD_MARKER, "1");
        child.env("CLAUDE_CODE_USE_BEDROCK", "1");
        for variable in PARENT_SESSION_VARIABLES {
            child.env(variable, "1");
        }
        let output = child.output().expect("the test binary runs");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{stdout}{}", String::from_utf8_lossy(&output.stderr));
        assert!(stdout.contains("1 passed"), "the child ran the check: {stdout}");
    }
}
