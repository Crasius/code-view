//! repo-lint: the repository rules that clippy cannot check (file length, type and module names,
//! crate dependency direction, unsafe placement, folder budgets, manifest hygiene). Each rule has
//! a stable id (RL001..), documented with Rule, Why and Enforced by in the README.
//!
//! Usage: `cargo run -p repo-lint -- [--base <rev>] [PATH...]`. PATHs limit the checks to files
//! and crates under them. `--base` names the revision the length allowlist may not grow against
//! (default `HEAD`, which sees only uncommitted raises; a branch passes its fork branch); a file
//! git reports as renamed since then keeps the entry of its old path.
//! `--lengths` prints shipped line counts over 400; `--shrink-allowlist` lowers allowlist entries
//! to the current lengths. Exit 0 = clean, 1 = findings, 2 = error.
#![forbid(unsafe_code)]

mod finding;
mod folders;
mod items;
mod length;
mod lexer;
mod manifest;
mod repo;
mod source_rules;
mod tree;

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::process::ExitCode;

use finding::{Finding, Rule};
use repo::{Repo, RepoError};
use tree::Source;

const USAGE: &str = "usage: repo-lint [--base <rev>] [--lengths | --shrink-allowlist] [PATH...]";

enum Mode {
    Help,
    Check,
    Lengths,
    ShrinkAllowlist,
}

struct Options {
    mode: Mode,
    base: String,
    paths: Vec<String>,
}

fn main() -> ExitCode {
    let result = parse_args(std::env::args().skip(1)).and_then(|options| run(&options));
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(RepoError(message)) => {
            let _written = writeln!(io::stderr(), "repo-lint: {message}");
            ExitCode::from(2)
        }
    }
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Options, RepoError> {
    let mut options = Options { mode: Mode::Check, base: "HEAD".to_owned(), paths: Vec::new() };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--base" => {
                options.base =
                    args.next().ok_or_else(|| RepoError("--base needs a revision".into()))?;
            }
            "--lengths" => options.mode = Mode::Lengths,
            "--shrink-allowlist" => options.mode = Mode::ShrinkAllowlist,
            "--help" | "-h" => options.mode = Mode::Help,
            flag if flag.starts_with('-') => {
                return Err(RepoError(format!("unknown option {flag}\n{USAGE}")));
            }
            path => options.paths.push(path.to_owned()),
        }
    }
    Ok(options)
}

/// Runs the chosen mode; returns whether the repository is clean.
fn run(options: &Options) -> Result<bool, RepoError> {
    match options.mode {
        Mode::Help => {
            writeln!(io::stdout(), "{USAGE}")?;
            Ok(true)
        }
        Mode::Lengths => {
            let parsed = Parsed::read()?;
            print_lengths(&parsed.lengths, &Scope::new(&parsed.repo, &options.paths))
        }
        Mode::ShrinkAllowlist => {
            let parsed = Parsed::read()?;
            shrink_allowlist(&parsed.repo, &parsed.lengths)
        }
        Mode::Check => {
            let parsed = Parsed::read()?;
            let scope = Scope::new(&parsed.repo, &options.paths);
            report(&check(&parsed, &scope, &options.base)?)
        }
    }
}

/// The repository with every Rust file parsed, classified as shipped or test-only and counted.
struct Parsed {
    repo: Repo,
    sources: BTreeMap<String, Source>,
    is_test: BTreeMap<String, bool>,
    /// Shipped lines per file; 0 for a test-only file.
    lengths: BTreeMap<String, usize>,
}

impl Parsed {
    fn read() -> Result<Parsed, RepoError> {
        let repo = Repo::open()?;
        let mut sources = BTreeMap::new();
        for path in &repo.rust_files {
            sources.insert(path.clone(), Source::parse(repo.read(path)?));
        }
        let roots = repo.crates.iter().flat_map(|krate| krate.roots.iter().cloned());
        let mut is_test = tree::classify(
            roots,
            |p| sources.get(p).map(|s| &s.items),
            |p| sources.contains_key(p),
        );
        for path in sources.keys() {
            is_test.entry(path.clone()).or_insert(false);
        }
        let lengths = sources
            .iter()
            .map(|(path, source)| {
                let test_only = is_test.get(path) == Some(&true);
                (path.clone(), if test_only { 0 } else { source.shipped_lines() })
            })
            .collect();
        Ok(Parsed { repo, sources, is_test, lengths })
    }
}

/// The paths a run is limited to; empty means the whole repository.
struct Scope {
    prefixes: Vec<String>,
}

impl Scope {
    fn new(repo: &Repo, paths: &[String]) -> Scope {
        let root = repo::slashed(&repo.root);
        let prefixes = paths
            .iter()
            .map(|path| {
                let path = path.replace('\\', "/");
                let path = repo::relative(&path, &root).unwrap_or(path);
                path.trim_start_matches("./").trim_end_matches('/').to_owned()
            })
            .filter(|path| !path.is_empty() && path != ".")
            .collect();
        Scope { prefixes }
    }

    fn covers(&self, path: &str) -> bool {
        self.prefixes.is_empty()
            || self.prefixes.iter().any(|prefix| {
                path == prefix
                    || path.strip_prefix(prefix.as_str()).is_some_and(|rest| rest.starts_with('/'))
            })
    }
}

fn check(parsed: &Parsed, scope: &Scope, base: &str) -> Result<Vec<Finding>, RepoError> {
    let Parsed { repo, sources, is_test, lengths } = parsed;
    let mut findings = Vec::new();
    for (path, source) in sources.iter().filter(|(path, _)| scope.covers(path)) {
        let file_is_test = is_test.get(path) == Some(&true);
        findings.extend(source_rules::check_source(path, source, file_is_test));
    }
    findings.extend(length_findings(repo, scope, base, lengths)?);
    findings.extend(folder_findings(repo, scope, is_test)?);
    findings.extend(manifest::check_crates(&repo.crates));
    for krate in &repo.crates {
        let manifest_path = format!("{}/Cargo.toml", krate.dir);
        findings
            .extend(manifest::check_member_manifest(&manifest_path, &repo.read(&manifest_path)?));
    }
    findings.retain(|finding| scope.covers(&finding.path));
    findings.sort();
    findings.dedup();
    Ok(findings)
}

fn length_findings(
    repo: &Repo,
    scope: &Scope,
    base: &str,
    lengths: &BTreeMap<String, usize>,
) -> Result<Vec<Finding>, RepoError> {
    repo.verify_revision(base)?;
    let allowlist = read_allowlist(repo.read_optional(length::ALLOWLIST).as_deref())?;
    let mut findings: Vec<Finding> = lengths
        .iter()
        .filter(|(path, _)| scope.covers(path))
        .filter_map(|(path, &lines)| length::check(path, lines, allowlist.get(path).copied()))
        .collect();
    for path in allowlist.keys().filter(|path| !lengths.contains_key(*path)) {
        findings.push(Finding::file(
            Rule::LengthAllowlist,
            length::ALLOWLIST,
            format!("{path} no longer exists: remove its entry"),
        ));
    }
    if let Some(base_text) = repo.read_at(base, length::ALLOWLIST) {
        let base_list = read_allowlist(Some(&base_text))?;
        let listed: Vec<&str> =
            base_list.keys().chain(allowlist.keys()).map(String::as_str).collect();
        let renamed_from = repo.renames_since(base, &listed)?;
        findings.extend(length::check_growth(&allowlist, &base_list, &renamed_from));
    }
    Ok(findings)
}

fn read_allowlist(text: Option<&str>) -> Result<length::Allowlist, RepoError> {
    length::parse_allowlist(text.unwrap_or_default()).map_err(RepoError)
}

fn folder_findings(
    repo: &Repo,
    scope: &Scope,
    is_test: &BTreeMap<String, bool>,
) -> Result<Vec<Finding>, RepoError> {
    let mut counts = folders::shipped_per_folder(is_test);
    for folder in &repo.budget_folders {
        counts.entry(folder.clone()).or_insert(0);
    }
    let mut findings = Vec::new();
    for (dir, &count) in counts.iter().filter(|(dir, _)| scope.covers(dir)) {
        let reason = if repo.budget_folders.contains(dir) {
            let text = repo.read(&format!("{dir}/{}", folders::BUDGET_FILE))?;
            Some(text.lines().next().unwrap_or_default().to_owned())
        } else {
            None
        };
        findings.extend(folders::check(dir, count, reason.as_deref()));
    }
    Ok(findings)
}

fn print_lengths(lengths: &BTreeMap<String, usize>, scope: &Scope) -> Result<bool, RepoError> {
    let mut rows: Vec<(&String, &usize)> = lengths
        .iter()
        .filter(|(path, lines)| **lines > length::SPLIT_BY_TOPIC && scope.covers(path))
        .collect();
    rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let mut out = io::stdout().lock();
    for (path, lines) in rows {
        writeln!(out, "{lines} {path}")?;
    }
    Ok(true)
}

fn shrink_allowlist(repo: &Repo, lengths: &BTreeMap<String, usize>) -> Result<bool, RepoError> {
    let text = repo.read_optional(length::ALLOWLIST).unwrap_or_default();
    let shrunk = length::shrink(&text, lengths);
    if shrunk != text {
        std::fs::write(repo.root.join(length::ALLOWLIST), &shrunk)?;
    }
    writeln!(io::stdout(), "repo-lint: {} is up to date", length::ALLOWLIST)?;
    Ok(true)
}

fn report(findings: &[Finding]) -> Result<bool, RepoError> {
    let mut out = io::stdout().lock();
    for finding in findings {
        writeln!(out, "{finding}")?;
    }
    if !findings.is_empty() {
        writeln!(out, "repo-lint: {} findings (rules: tools/repo-lint/README.md)", findings.len())?;
    }
    Ok(findings.is_empty())
}
