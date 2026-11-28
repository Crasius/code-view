//! The repository as the rules see it: its root, its Rust files (tracked plus untracked and not
//! ignored, so new files count before they are committed), the workspace crates from
//! `cargo metadata`, and files at a base revision for the shrink-only checks. Third-party code
//! vendored under a `vendor/` folder keeps its upstream style, so the rules skip it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// A failure to read the repository; the tool exits with 2.
#[derive(Debug)]
pub(crate) struct RepoError(pub String);

impl From<std::io::Error> for RepoError {
    fn from(error: std::io::Error) -> RepoError {
        RepoError(format!("cannot write: {error}"))
    }
}

/// One workspace member from `cargo metadata`.
pub(crate) struct Crate {
    pub name: String,
    /// Repo-relative directory, forward slashes.
    pub dir: String,
    /// Direct dependencies by package name, any kind (normal, dev, build) and any target.
    pub dependencies: Vec<String>,
    /// Target root files (repo-relative) and whether they are test or bench targets.
    pub roots: Vec<(String, bool)>,
}

pub(crate) struct Repo {
    pub root: PathBuf,
    /// Repo-relative paths of every `.rs` file, forward slashes, sorted.
    pub rust_files: Vec<String>,
    /// Repo-relative folders that hold a `.folder-budget` file.
    pub budget_folders: Vec<String>,
    pub crates: Vec<Crate>,
}

impl Repo {
    pub(crate) fn open() -> Result<Repo, RepoError> {
        let top = run("git", &["rev-parse", "--show-toplevel"], None)?;
        let root = PathBuf::from(top.trim());
        let listing = run(
            "git",
            &["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
            Some(&root),
        )?;
        let present: Vec<&str> = listing
            .split('\0')
            .filter(|path| !is_vendored(path) && root.join(path).is_file())
            .collect();
        let mut rust_files: Vec<String> = present
            .iter()
            .filter(|path| Path::new(path).extension().is_some_and(|ext| ext == "rs"))
            .map(|path| (*path).to_owned())
            .collect();
        rust_files.sort();
        rust_files.dedup();
        let mut budget_folders: Vec<String> = present
            .iter()
            .filter_map(|path| path.strip_suffix("/.folder-budget"))
            .map(str::to_owned)
            .collect();
        budget_folders.sort();
        budget_folders.dedup();
        let metadata =
            run("cargo", &["metadata", "--format-version", "1", "--no-deps"], Some(&root))?;
        let crates = parse_metadata(&metadata, &slashed(&root))?;
        Ok(Repo { root, rust_files, budget_folders, crates })
    }

    pub(crate) fn read(&self, path: &str) -> Result<String, RepoError> {
        std::fs::read_to_string(self.root.join(path))
            .map_err(|error| RepoError(format!("{path}: {error}")))
    }

    /// The working-tree file, or `None` when it does not exist.
    pub(crate) fn read_optional(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(path)).ok()
    }

    /// Fails unless `revision` names a commit, so a mistyped or unfetched base is an error and
    /// not a base without an allowlist.
    pub(crate) fn verify_revision(&self, revision: &str) -> Result<(), RepoError> {
        let commit = format!("{revision}^{{commit}}");
        run("git", &["rev-parse", "--verify", "--quiet", &commit], Some(&self.root))
            .map(drop)
            .map_err(|_| RepoError(format!("--base {revision}: no such commit")))
    }

    /// The file at `revision`, or `None` when it does not exist there.
    pub(crate) fn read_at(&self, revision: &str, path: &str) -> Option<String> {
        run("git", &["show", &format!("{revision}:{path}")], Some(&self.root)).ok()
    }

    /// The files among `paths` that git sees as renamed between `revision` and the working tree
    /// (`git diff -M`), new path to old path. Limiting the diff to `paths` keeps the similarity
    /// search to those files; both the old and the new path must be among them.
    pub(crate) fn renames_since(
        &self,
        revision: &str,
        paths: &[&str],
    ) -> Result<BTreeMap<String, String>, RepoError> {
        let mut args = vec!["diff", "-M", "-z", "--name-status", revision, "--"];
        args.extend_from_slice(paths);
        run("git", &args, Some(&self.root)).map(|listing| parse_renames(&listing))
    }
}

/// Reads `git diff -z --name-status` output: a status, then one path, or two for a rename
/// (`R<score>`) or copy (`C<score>`), each NUL-terminated. Returns renames, new path to old.
fn parse_renames(listing: &str) -> BTreeMap<String, String> {
    let mut renames = BTreeMap::new();
    let mut fields = listing.split('\0').filter(|field| !field.is_empty());
    while let Some(status) = fields.next() {
        let paired = status.starts_with('R') || status.starts_with('C');
        let Some(first) = fields.next() else { break };
        if !paired {
            continue;
        }
        let Some(second) = fields.next() else { break };
        if status.starts_with('R') {
            renames.insert(second.to_owned(), first.to_owned());
        }
    }
    renames
}

#[expect(
    clippy::disallowed_methods,
    reason = "CLI tool: its only thread waits for git and cargo by design; there is no UI to block"
)]
fn run(program: &str, args: &[&str], dir: Option<&Path>) -> Result<String, RepoError> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let output =
        command.output().map_err(|error| RepoError(format!("cannot run {program}: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RepoError(format!("{program} {} failed: {}", args.join(" "), stderr.trim())));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| RepoError(format!("{program} printed non-UTF-8 output")))
}

/// Whether `path` (repo-relative, forward slashes) lies under a `vendor/` folder.
fn is_vendored(path: &str) -> bool {
    path.split('/').rev().skip(1).any(|folder| folder == "vendor")
}

pub(crate) fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Turns an absolute path from cargo into a repo-relative one with forward slashes.
pub(crate) fn relative(path: &str, root: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let root = root.trim_end_matches('/');
    let head = path.get(..root.len())?;
    let rest = path.get(root.len()..)?;
    (head.eq_ignore_ascii_case(root) && rest.starts_with('/'))
        .then(|| rest.trim_start_matches('/').to_owned())
}

pub(crate) fn parse_metadata(json: &str, root: &str) -> Result<Vec<Crate>, RepoError> {
    let value: Value = serde_json::from_str(json)
        .map_err(|error| RepoError(format!("cargo metadata: bad JSON: {error}")))?;
    let packages = value["packages"]
        .as_array()
        .ok_or_else(|| RepoError("cargo metadata: no packages".into()))?;
    let mut crates = Vec::new();
    for package in packages {
        let name = package["name"].as_str().unwrap_or_default().to_owned();
        let manifest = package["manifest_path"].as_str().unwrap_or_default();
        let manifest = relative(manifest, root).ok_or_else(|| {
            RepoError(format!("cargo metadata: {name} lives outside the repository"))
        })?;
        let dir = manifest.trim_end_matches("Cargo.toml").trim_end_matches('/').to_owned();
        let mut dependencies: Vec<String> = package["dependencies"]
            .as_array()
            .map(|deps| {
                deps.iter().filter_map(|dep| dep["name"].as_str().map(str::to_owned)).collect()
            })
            .unwrap_or_default();
        dependencies.sort();
        dependencies.dedup();
        let roots = package["targets"]
            .as_array()
            .map(|targets| target_roots(targets, root))
            .unwrap_or_default();
        crates.push(Crate { name, dir, dependencies, roots });
    }
    crates.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(crates)
}

fn target_roots(targets: &[Value], root: &str) -> Vec<(String, bool)> {
    targets
        .iter()
        .filter_map(|target| {
            let path = relative(target["src_path"].as_str()?, root)?;
            let kinds = target["kind"].as_array()?;
            let is_test = kinds.iter().any(|kind| matches!(kind.as_str(), Some("test" | "bench")));
            Some((path, is_test))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_ignore_slash_style_and_drive_case() {
        assert_eq!(
            relative("D:\\Dev\\Repo\\crates\\a\\Cargo.toml", "d:/Dev/Repo").as_deref(),
            Some("crates/a/Cargo.toml")
        );
        assert_eq!(relative("/home/me/repo-other/x.rs", "/home/me/repo"), None);
    }

    #[test]
    fn skips_vendored_paths_only() {
        assert!(is_vendored("crates/review-grammars/vendor/syntect/src/lib.rs"));
        assert!(is_vendored("vendor/x/lib.rs"));
        assert!(!is_vendored("crates/review-app/src/vendor.rs"));
        assert!(!is_vendored("crates/vendors/src/lib.rs"));
    }

    #[test]
    fn reads_renames_from_name_status_output() {
        let listing = "M\0tools/repo-lint/file-lengths.allow\0\
            R087\0crates/review-app/src/features/pull_request/files/diff_view.rs\0crates/cv-pr-diff/src/diff_view.rs\0\
            C100\0a.rs\0b.rs\0D\0gone.rs\0R100\0x/mod.rs\0y/lib.rs\0";
        let renames: Vec<(String, String)> = parse_renames(listing).into_iter().collect();
        assert_eq!(
            renames,
            [
                (
                    "crates/cv-pr-diff/src/diff_view.rs".to_owned(),
                    "crates/review-app/src/features/pull_request/files/diff_view.rs".to_owned()
                ),
                ("y/lib.rs".to_owned(), "x/mod.rs".to_owned()),
            ]
        );
    }

    #[test]
    fn reads_members_dependencies_and_target_roots() {
        let json = r#"{"packages":[{"name":"b","manifest_path":"/r/crates/b/Cargo.toml",
            "dependencies":[{"name":"serde","kind":null},{"name":"a","kind":"dev"},{"name":"serde","kind":"build"}],
            "targets":[{"kind":["lib"],"src_path":"/r/crates/b/src/lib.rs"},
                       {"kind":["test"],"src_path":"/r/crates/b/tests/flow.rs"}]}]}"#;
        let crates = parse_metadata(json, "/r").expect("valid metadata");
        let only = crates.first().expect("one crate");
        assert_eq!((only.name.as_str(), only.dir.as_str()), ("b", "crates/b"));
        assert_eq!(only.dependencies, ["a", "serde"]);
        assert_eq!(
            only.roots,
            [
                ("crates/b/src/lib.rs".to_owned(), false),
                ("crates/b/tests/flow.rs".to_owned(), true)
            ]
        );
    }
}
