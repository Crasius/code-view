//! What the editor knows about where it is used, published by the pull request page once per
//! frame: the platform (mention syntax, suggestions), the people that can be mentioned and, in the
//! Files tab, the open file (code fences and suggestion blocks).

use std::ops::RangeInclusive;
use std::sync::Arc;

use egui::{Context, Id};
use review_core::ProviderKind;
use review_git::{FileDiff, LineKind};

const PULL_REQUEST_ID: &str = "review.editor.pull_request";
const FILE_ID: &str = "review.editor.file";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub id: String,
    pub display_name: String,
    pub login: Option<String>,
}

#[derive(Clone)]
pub struct PullRequestContext {
    pub kind: ProviderKind,
    /// Author, reviewers and commenters, without duplicates.
    pub people: Arc<Vec<Person>>,
}

impl PullRequestContext {
    /// Makes the context available to every editor drawn this frame.
    pub fn publish(self, ctx: &Context) {
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|data| data.insert_temp(Id::unique(PULL_REQUEST_ID), (pass, self)));
    }

    pub fn current(ctx: &Context) -> Option<Self> {
        let pass = ctx.cumulative_pass_nr();
        ctx.data(|data| data.get_temp::<(u64, Self)>(Id::unique(PULL_REQUEST_ID)))
            .filter(|(published, _)| *published == pass)
            .map(|(_, context)| context)
    }

    /// The Markdown that mentions `person` on this platform: `@login` on GitHub and GitLab,
    /// `@<id>` on Azure DevOps (its documented identity syntax).
    pub fn mention(&self, person: &Person) -> Option<String> {
        match self.kind {
            ProviderKind::AzureDevOps => {
                (!person.id.is_empty()).then(|| format!("@<{}>", person.id))
            }
            ProviderKind::GitHub | ProviderKind::GitLab => {
                person.login.as_ref().map(|login| format!("@{login}"))
            }
        }
    }

    /// People whose name or login contains `query` (case-insensitive), best matches first.
    pub fn matching(&self, query: &str) -> Vec<&Person> {
        let query = query.to_lowercase();
        let mut matches: Vec<(usize, &Person)> = self
            .people
            .iter()
            .filter_map(|person| {
                let login = person.login.as_deref().unwrap_or_default().to_lowercase();
                let name = person.display_name.to_lowercase();
                let rank = if login.starts_with(&query) {
                    0
                } else if name.starts_with(&query) {
                    1
                } else if login.contains(&query) || name.contains(&query) {
                    2
                } else {
                    return None;
                };
                Some((rank, person))
            })
            .collect();
        matches.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.display_name.cmp(&b.1.display_name)));
        matches.into_iter().map(|(_, person)| person).take(8).collect()
    }
}

/// The file shown in the Files tab.
#[derive(Clone)]
pub struct FileContext {
    pub path: String,
    pub diff: Arc<FileDiff>,
}

impl FileContext {
    pub fn publish(self, ctx: &Context) {
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|data| data.insert_temp(Id::unique(FILE_ID), (pass, self)));
    }

    pub fn current(ctx: &Context) -> Option<Self> {
        let pass = ctx.cumulative_pass_nr();
        ctx.data(|data| data.get_temp::<(u64, Self)>(Id::unique(FILE_ID)))
            .filter(|(published, _)| *published == pass)
            .map(|(_, context)| context)
    }

    /// The fence language for the file, a name all three platforms highlight.
    pub fn language(&self) -> String {
        fence_language(&self.path)
    }

    /// Head-side text of `wanted` when all of its lines are in the diff. An empty range (a reversed
    /// one included) names no lines, so it has no text either.
    pub fn new_lines(&self, wanted: RangeInclusive<u32>) -> Option<String> {
        let count = u64::from(wanted.end().checked_sub(*wanted.start())?) + 1;
        let lines: Vec<&str> = self
            .diff
            .lines()
            .iter()
            .filter(|line| line.kind != LineKind::Hunk && wanted.contains(&line.new_line))
            .map(|line| self.diff.text(line).trim_end_matches(['\n', '\r']))
            .collect();
        (lines.len() as u64 == count).then(|| lines.join("\n"))
    }

    /// True when `pasted` is one or more whole lines of this diff (copied from the diff view).
    pub fn contains_code(&self, pasted: &str) -> bool {
        let wanted: Vec<&str> =
            pasted.trim_end_matches(['\n', '\r']).lines().map(str::trim_end).collect();
        if wanted.iter().all(|line| line.trim().is_empty()) {
            return false;
        }
        let diff_lines: Vec<&str> = self
            .diff
            .lines()
            .iter()
            .filter(|line| line.kind != LineKind::Hunk)
            .map(|line| self.diff.text(line).trim_end_matches(['\n', '\r']).trim_end())
            .collect();
        if wanted.len() == 1 {
            return diff_lines
                .iter()
                .any(|line| line.trim() == wanted[0].trim() && line.trim().len() >= 8);
        }
        // Lines copied from a side-by-side view skip the other side's lines, so match in order
        // rather than as one contiguous block.
        let mut next = 0;
        for line in &diff_lines {
            if next < wanted.len() && line.trim() == wanted[next].trim() {
                next += 1;
            }
        }
        next == wanted.len()
    }
}

/// Maps a file name to a fence tag that GitHub (Linguist), GitLab (Rouge) and Azure DevOps
/// (highlight.js) all know; unknown extensions are used as they are.
pub fn fence_language(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let lower = name.to_lowercase();
    if lower == "dockerfile" {
        return "dockerfile".into();
    }
    if lower == "makefile" {
        return "makefile".into();
    }
    let Some((_, extension)) = lower.rsplit_once('.') else {
        return String::new();
    };
    let language = match extension {
        "rs" => "rust",
        "cs" => "csharp",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "jsx",
        "py" => "python",
        "rb" => "ruby",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "php" => "php",
        "sh" | "bash" => "bash",
        "ps1" | "psm1" => "powershell",
        "sql" => "sql",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" => "scss",
        "json" => "json",
        "yml" | "yaml" => "yaml",
        "toml" => "toml",
        "xml" | "csproj" | "props" | "targets" | "xaml" => "xml",
        "md" | "markdown" => "markdown",
        "razor" | "cshtml" => "razor",
        "fs" | "fsx" => "fsharp",
        "vb" => "vbnet",
        "dart" => "dart",
        "lua" => "lua",
        "r" => "r",
        "scala" => "scala",
        other => other,
    };
    language.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_file_names_to_fence_languages() {
        assert_eq!(fence_language("src/main.rs"), "rust");
        assert_eq!(fence_language("App/Program.CS"), "csharp");
        assert_eq!(fence_language("Dockerfile"), "dockerfile");
        assert_eq!(fence_language("notes"), "");
        assert_eq!(fence_language("a.zig"), "zig");
    }

    fn people() -> PullRequestContext {
        let person = |id: &str, name: &str, login: &str| Person {
            id: id.into(),
            display_name: name.into(),
            login: Some(login.into()),
        };
        PullRequestContext {
            kind: ProviderKind::GitHub,
            people: Arc::new(vec![
                person("1", "Ada Lovelace", "ada"),
                person("2", "Grace Hopper", "ghopper"),
            ]),
        }
    }

    #[test]
    fn mentions_follow_the_platform() {
        let mut context = people();
        assert_eq!(context.mention(&context.people[0]).as_deref(), Some("@ada"));
        context.kind = ProviderKind::AzureDevOps;
        assert_eq!(context.mention(&context.people[0]).as_deref(), Some("@<1>"));
    }

    #[test]
    fn matches_people_by_login_and_name() {
        let context = people();
        let names: Vec<&str> =
            context.matching("g").iter().map(|person| person.display_name.as_str()).collect();
        assert_eq!(names, ["Grace Hopper"]);
        assert_eq!(context.matching("").len(), 2);
        assert_eq!(context.matching("zzz"), Vec::<&Person>::new());
    }

    #[test]
    fn new_lines_of_an_empty_range_are_none() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,3 @@\n one\n+two\n three\n";
        let file = FileContext {
            path: "a.rs".into(),
            diff: Arc::new(FileDiff::parse_unified(diff.as_bytes())),
        };
        assert_eq!(file.new_lines(2..=3).as_deref(), Some("two\nthree"));
        // A range anchor a push moved past its end line, as `suggestion_lines` would pass it.
        let (start, end) = (3, 2);
        assert_eq!(file.new_lines(start..=end), None, "a reversed range has no lines");
    }
}
