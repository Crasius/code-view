//! What a tool step is, for the chat's activity line ("read 3 files · 1 search · ran 1 command"):
//! the one mapping from both CLIs' tool names, and from the command actions Codex parses out of a
//! shell command, to a few kinds.

/// What a step did, as the activity line counts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepKind {
    Read,
    Search,
    Run,
    Edit,
    /// Any other tool; the chat shows its own name.
    Other,
}

/// How a finished step went, where the CLI says: Claude's `tool_result` with `is_error`, Codex's
/// item status (a command that exits non-zero is `failed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    Ok,
    Failed,
}

impl StepKind {
    /// The kind of a tool by its name: Claude Code's tools (`Read`, `Grep`, `Bash`, ...) and
    /// Codex's item types (`commandExecution`, `fileChange`, ...). Codex's commands are
    /// `StepKind::of_command` when Codex parsed what they do.
    pub fn of_tool(tool: &str) -> Self {
        match tool {
            "Read" | "NotebookRead" | "LS" | "imageView" => Self::Read,
            "Grep" | "Glob" | "WebSearch" | "webSearch" => Self::Search,
            "Bash" | "BashOutput" | "PowerShell" | "commandExecution" => Self::Run,
            "Edit" | "MultiEdit" | "Write" | "NotebookEdit" | "fileChange" => Self::Edit,
            _ => Self::Other,
        }
    }

    /// The kind of a Codex command by the type of the first action Codex parsed out of it
    /// (`read`, `listFiles`, `search`, `unknown`): `cat a.rs` reads, `rg greet` searches.
    pub(crate) fn of_command(action: Option<&str>) -> Self {
        match action {
            Some("read" | "listFiles") => Self::Read,
            Some("search") => Self::Search,
            _ => Self::Run,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StepKind;
    use super::StepKind::{Edit, Other, Read, Run, Search};

    #[test]
    fn both_clis_tools_map_to_their_kinds() {
        let claude = ["Read", "Grep", "Glob", "Bash", "Edit", "Write", "WebFetch"];
        let kinds: Vec<StepKind> = claude.into_iter().map(StepKind::of_tool).collect();
        assert_eq!(kinds, [Read, Search, Search, Run, Edit, Edit, Other]);
        let codex = ["commandExecution", "fileChange", "webSearch", "imageView", "mcpToolCall"];
        let kinds: Vec<StepKind> = codex.into_iter().map(StepKind::of_tool).collect();
        assert_eq!(kinds, [Run, Edit, Search, Read, Other]);
        let commands = [Some("read"), Some("listFiles"), Some("search"), Some("unknown"), None];
        let kinds: Vec<StepKind> = commands.into_iter().map(StepKind::of_command).collect();
        assert_eq!(kinds, [Read, Read, Search, Run, Run]);
    }
}
