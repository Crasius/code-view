//! RS-READ-09 folder budget (adapted from platform-dotnet ELY0010/ELY0011): a module folder with
//! more than 12 shipped `.rs` files (test-only files and `mod.rs`, `lib.rs`, `main.rs` excluded)
//! is split into sub-modules or carries a `.folder-budget` file whose first line says why the
//! files belong together. A budget file in a folder within the limit is stale and goes away.

use std::collections::BTreeMap;

use crate::finding::{Finding, Rule};

pub const BUDGET: usize = 12;
pub const BUDGET_FILE: &str = ".folder-budget";

/// Shipped module files per folder under a `src/` tree. `files` maps a repo-relative path to
/// whether it is test-only code.
pub fn shipped_per_folder(files: &BTreeMap<String, bool>) -> BTreeMap<String, usize> {
    let mut folders: BTreeMap<String, usize> = BTreeMap::new();
    for (path, &is_test) in files {
        let Some((dir, file)) = path.rsplit_once('/') else { continue };
        let in_source_tree = dir.split('/').any(|part| part == "src");
        let is_module_root = matches!(file, "mod.rs" | "lib.rs" | "main.rs");
        let count = folders.entry(dir.to_owned()).or_default();
        if in_source_tree && !is_test && !is_module_root {
            *count += 1;
        }
    }
    folders.retain(|_, count| *count > 0);
    folders
}

/// The finding for one folder, given its count and the first line of its budget file, if any.
pub fn check(dir: &str, shipped: usize, budget_reason: Option<&str>) -> Option<Finding> {
    let budget_path = format!("{dir}/{BUDGET_FILE}");
    match budget_reason {
        None if shipped > BUDGET => Some(Finding::file(
            Rule::FolderBudget,
            dir,
            format!(
                "{shipped} module files, over the budget of {BUDGET}: split it into sub-modules by topic, \
                 or add {BUDGET_FILE} whose first line says why they belong together (RS-READ-09)"
            ),
        )),
        Some(reason) if shipped > BUDGET && reason.trim().is_empty() => Some(Finding::file(
            Rule::FolderBudget,
            &budget_path,
            "the first line must give the reason the files stay in one folder".to_owned(),
        )),
        Some(_) if shipped <= BUDGET => Some(Finding::file(
            Rule::FolderBudget,
            &budget_path,
            format!(
                "the folder has {shipped} module files, within {BUDGET}: delete the budget file"
            ),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_shipped_module_files_under_src_only() {
        let files: BTreeMap<String, bool> = [
            ("crates/a/src/lib.rs", false),
            ("crates/a/src/one.rs", false),
            ("crates/a/src/one_tests.rs", true),
            ("crates/a/src/net/mod.rs", false),
            ("crates/a/src/net/http.rs", false),
            ("crates/a/tests/flow.rs", true),
            ("crates/a/examples/probe.rs", false),
        ]
        .into_iter()
        .map(|(p, t)| (p.to_owned(), t))
        .collect();
        let counts: Vec<(String, usize)> = shipped_per_folder(&files).into_iter().collect();
        assert_eq!(counts, [("crates/a/src".to_owned(), 1), ("crates/a/src/net".to_owned(), 1)]);
    }

    #[test]
    fn over_budget_needs_a_reason_and_a_budget_within_limit_is_stale() {
        assert_eq!(check("d", 12, None), None);
        assert!(check("d", 13, None).is_some());
        assert_eq!(check("d", 13, Some("One file per provider API area.")), None);
        assert!(check("d", 13, Some("  ")).is_some());
        assert!(check("d", 12, Some("reason")).is_some());
    }
}
