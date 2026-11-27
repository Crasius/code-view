//! The module tree: which Rust files are test code. A file is test code when every way to reach
//! it from a target root goes through a test target (`tests/`, benches) or a `#[cfg(test)]`
//! module declaration. Inline `#[cfg(test)]` items of shipped files are excluded line by line.

use std::collections::{BTreeMap, VecDeque};

use crate::items::{Items, Token, items, tokens};
use crate::lexer::{Scanned, scan};

/// One lexed Rust file.
pub struct Source {
    pub text: String,
    pub scanned: Scanned,
    pub tokens: Vec<Token>,
    pub items: Items,
}

impl Source {
    pub fn parse(text: String) -> Source {
        let scanned = scan(&text);
        let tokens = tokens(&scanned);
        let lines: Vec<&str> = text.lines().collect();
        let items = items(&tokens, &lines);
        Source { text, scanned, tokens, items }
    }

    /// Lines of shipped code: all lines minus those of `#[cfg(test)]` items.
    pub fn shipped_lines(&self) -> usize {
        if self.items.file_is_test {
            return 0;
        }
        let total = self.text.lines().count();
        let mut covered = vec![false; total + 1];
        for &(start, end) in &self.items.test_ranges {
            for line in start..=end.min(total) {
                if let Some(slot) = covered.get_mut(line) {
                    *slot = true;
                }
            }
        }
        total - covered.iter().filter(|&&c| c).count()
    }

    /// True when the 1-based `line` belongs to a `#[cfg(test)]` item.
    pub fn is_test_line(&self, line: usize) -> bool {
        self.items.file_is_test
            || self.items.test_ranges.iter().any(|&(start, end)| (start..=end).contains(&line))
    }
}

/// Every module file reached from `roots`, with whether it is test-only code.
/// `decls_of` returns the module declarations of a file (`None` when it does not exist).
pub fn classify<'a>(
    roots: impl IntoIterator<Item = (String, bool)>,
    mut decls_of: impl FnMut(&str) -> Option<&'a Items>,
    exists: impl Fn(&str) -> bool,
) -> BTreeMap<String, bool> {
    let mut is_test: BTreeMap<String, bool> = BTreeMap::new();
    let mut queue: VecDeque<(String, bool, bool)> =
        roots.into_iter().map(|(path, test)| (path, test, true)).collect();
    while let Some((path, test, is_root)) = queue.pop_front() {
        // A file reached once as shipped stays shipped; a test-only mark is upgraded when a
        // shipped path reaches it.
        let settled = is_test.get(&path).is_some_and(|&known| !known || test);
        if settled {
            continue;
        }
        is_test.insert(path.clone(), test);
        let Some(items) = decls_of(&path) else {
            continue;
        };
        let child_test = test || items.file_is_test;
        for decl in &items.decls {
            if let Some(child) =
                child_path(&path, is_root, &decl.name, decl.path.as_deref(), &exists)
            {
                queue.push_back((child, child_test || decl.is_test, false));
            }
        }
    }
    is_test
}

/// Resolves `mod name;` declared in `parent`. Crate roots and `mod.rs` files own their directory;
/// any other `dir/stem.rs` owns `dir/stem/`. A `#[path]` is relative to the parent's directory.
pub fn child_path(
    parent: &str,
    parent_is_root: bool,
    name: &str,
    path_attr: Option<&str>,
    exists: impl Fn(&str) -> bool,
) -> Option<String> {
    let (dir, file) = parent.rsplit_once('/').unwrap_or(("", parent));
    if let Some(path) = path_attr {
        return Some(normalize(&join(dir, path)));
    }
    let owns_dir = parent_is_root || file == "mod.rs";
    let base = if owns_dir { dir.to_owned() } else { join(dir, file.trim_end_matches(".rs")) };
    [join(&base, &format!("{name}.rs")), join(&base, &format!("{name}/mod.rs"))]
        .into_iter()
        .find(|p| exists(p))
}

fn join(dir: &str, rest: &str) -> String {
    if dir.is_empty() { rest.to_owned() } else { format!("{dir}/{rest}") }
}

/// Resolves `.` and `..` components of a forward-slash path.
pub fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_children_of_roots_mod_files_and_plain_files() {
        let exists = |p: &str| {
            matches!(p, "src/app.rs" | "src/app/view.rs" | "src/net/mod.rs" | "src/net/http.rs")
        };
        assert_eq!(
            child_path("src/main.rs", true, "app", None, exists).as_deref(),
            Some("src/app.rs")
        );
        assert_eq!(
            child_path("src/app.rs", false, "view", None, exists).as_deref(),
            Some("src/app/view.rs")
        );
        assert_eq!(
            child_path("src/lib.rs", true, "net", None, exists).as_deref(),
            Some("src/net/mod.rs")
        );
        assert_eq!(
            child_path("src/net/mod.rs", false, "http", None, exists).as_deref(),
            Some("src/net/http.rs")
        );
        assert_eq!(child_path("src/app.rs", false, "gone", None, exists), None);
    }

    #[test]
    fn path_attributes_resolve_against_the_parent_directory() {
        let never = |_: &str| false;
        let child = child_path(
            "crates/a/examples/probe.rs",
            true,
            "x",
            Some("../src/platform/x.rs"),
            never,
        );
        assert_eq!(child.as_deref(), Some("crates/a/src/platform/x.rs"));
    }

    #[test]
    fn a_file_is_test_only_when_every_path_to_it_is() {
        let sources: BTreeMap<&str, Source> = [
            ("src/main.rs", "mod shared;\n#[cfg(test)]\nmod walkthrough;\n"),
            ("src/shared.rs", "fn f() {}\n"),
            ("src/walkthrough.rs", "mod steps;\n"),
            ("src/walkthrough/steps.rs", ""),
            ("tests/flow.rs", "#[path = \"../src/shared.rs\"]\nmod shared;\n"),
        ]
        .into_iter()
        .map(|(path, text)| (path, Source::parse(text.to_owned())))
        .collect();
        let roots = [("src/main.rs".to_owned(), false), ("tests/flow.rs".to_owned(), true)];
        let classes =
            classify(roots, |p| sources.get(p).map(|s| &s.items), |p| sources.contains_key(p));
        let tests: Vec<(&str, bool)> = classes.iter().map(|(p, t)| (p.as_str(), *t)).collect();
        assert_eq!(
            tests,
            [
                ("src/main.rs", false),
                ("src/shared.rs", false),
                ("src/walkthrough.rs", true),
                ("src/walkthrough/steps.rs", true),
                ("tests/flow.rs", true),
            ]
        );
    }

    #[test]
    fn shipped_lines_leave_out_test_items() {
        let source =
            Source::parse("fn a() {}\n\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n".to_owned());
        assert_eq!(source.shipped_lines(), 2);
        assert!(source.is_test_line(5));
        assert!(!source.is_test_line(1));
    }
}
