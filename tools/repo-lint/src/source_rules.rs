//! Line-anchored rules on one lexed file: role-suffix type names and vague module names
//! (RS-DONT-08), unsafe outside platform modules (RS-DONT-03), `deny(warnings)` in source
//! (RS-DONT-07), TODO comments (AGENTS.md, Done), `block_on` in shipped app code (RS-DONT-06),
//! and `motion::install` anywhere but the app's start-up (tests pin motion). A finding of the first
//! five can be waived at its line with
//! `// repo-lint: expect(RL007) <reason>` on that line or the line above; an expectation that
//! matches nothing is itself a finding, so waivers can only shrink.

use crate::finding::{Finding, Rule};
use crate::items::attribute;
use crate::tree::Source;

const ROLE_SUFFIXES: &[&str] = &["Manager", "Service", "Helper", "Handler"];
const VAGUE_MODULES: &[&str] = &["utils", "util", "helpers", "helper", "common", "misc"];
const EXPECT_MARKER: &str = "repo-lint: expect(";

/// The app's shipped code, where `block_on` would run on the UI thread or a thread it waits on:
/// the `review-app` bin and every `cv-*` UI crate except `cv-testkit`, which only tests link.
fn is_app_source(path: &str) -> bool {
    if path.starts_with("crates/review-app/src/") {
        return true;
    }
    let Some((name, inner)) = path.strip_prefix("crates/cv-").and_then(|rest| rest.split_once('/'))
    else {
        return false;
    };
    name != "testkit" && inner.starts_with("src/")
}

/// Checks one file; `file_is_test` says the whole file is test-only code (module tree).
pub fn check_source(path: &str, source: &Source, file_is_test: bool) -> Vec<Finding> {
    let mut findings = Vec::new();
    role_suffixes(path, source, &mut findings);
    vague_modules(path, source, &mut findings);
    unsafe_outside_platform(path, source, &mut findings);
    deny_warnings(path, source, &mut findings);
    todo_comments(path, source, &mut findings);
    if !file_is_test {
        block_on(path, source, &mut findings);
    }
    motion_install(path, source, file_is_test, &mut findings);
    apply_expectations(path, source, findings)
}

/// The one file whose shipped code reads the machine's motion setting: the binary's `main`, where
/// it creates the app. `ReviewApp::new` does not, so the whole-app tests that build it never read it.
const MOTION_START_UP: &str = "crates/review-app/src/main.rs";

/// `motion::install` reads the OS's reduced-motion setting, so a test that calls it runs
/// differently on a machine with Reduce Motion on (GitHub's macOS runners). Only the app's
/// start-up calls it; tests pin motion with `motion::set_reduced`.
fn motion_install(path: &str, source: &Source, file_is_test: bool, findings: &mut Vec<Finding>) {
    for line in motion_install_lines(source) {
        let is_start_up = path == MOTION_START_UP && !file_is_test && !source.is_test_line(line);
        if !is_start_up {
            findings.push(Finding::at(
                Rule::MotionInstall,
                path,
                line,
                format!("motion::install reads the machine's reduced-motion setting; only the app's start-up ({MOTION_START_UP}) calls it, a test pins motion with motion::set_reduced(ctx, false)"),
            ));
        }
    }
}

/// Lines with the path `motion::install` in code (a call or a `use`).
fn motion_install_lines(source: &Source) -> Vec<usize> {
    source
        .tokens
        .windows(4)
        .filter_map(|window| {
            let [motion, first, second, install] = window else { return None };
            let is_path = motion.text == "motion"
                && first.text == ":"
                && second.text == ":"
                && install.text == "install";
            is_path.then_some(install.line)
        })
        .collect()
}

fn role_suffixes(path: &str, source: &Source, findings: &mut Vec<Finding>) {
    for pair in source.tokens.windows(2) {
        let [keyword, name] = pair else { continue };
        let declares = matches!(keyword.text.as_str(), "struct" | "enum" | "trait" | "type");
        let Some(suffix) = ROLE_SUFFIXES.iter().find(|s| name.text.ends_with(*s)) else { continue };
        if declares {
            findings.push(Finding::at(
                Rule::RoleSuffix,
                path,
                name.line,
                format!(
                    "type {} ends in {suffix}, a role that names no responsibility; name the work it does (RS-DONT-08)",
                    name.text
                ),
            ));
        }
    }
}

fn vague_modules(path: &str, source: &Source, findings: &mut Vec<Finding>) {
    let declared = source.items.decls.iter().map(|d| (d.name.as_str(), d.line));
    let inline = source.items.inline_mods.iter().map(|(name, line)| (name.as_str(), *line));
    for (name, line) in declared.chain(inline) {
        if VAGUE_MODULES.contains(&name) {
            findings.push(Finding::at(
                Rule::VagueModule,
                path,
                line,
                format!("module `{name}` names no topic; move each item to the module of its topic (RS-DONT-08)"),
            ));
        }
    }
}

pub fn is_platform_path(path: &str) -> bool {
    path.split('/').any(|part| part == "platform" || part == "platform.rs")
}

fn unsafe_outside_platform(path: &str, source: &Source, findings: &mut Vec<Finding>) {
    if is_platform_path(path) {
        return;
    }
    for token in source.tokens.iter().filter(|t| t.text == "unsafe") {
        findings.push(Finding::at(
            Rule::UnsafeOutsidePlatform,
            path,
            token.line,
            "unsafe outside a platform/ module: wrap the FFI call in a safe function there (RS-DONT-03)".to_owned(),
        ));
    }
}

fn deny_warnings(path: &str, source: &Source, findings: &mut Vec<Finding>) {
    let tokens = &source.tokens;
    let lines: Vec<&str> = source.text.lines().collect();
    for (index, token) in tokens.iter().enumerate() {
        let Some((attr, _)) = attribute(tokens, index, &lines) else { continue };
        let body = tokens.get(attr.body).unwrap_or_default();
        let denies = body.iter().any(|t| matches!(t.text.as_str(), "deny" | "forbid"));
        if denies && body.iter().any(|t| t.text == "warnings") {
            findings.push(Finding::at(
                Rule::DenyWarnings,
                path,
                token.line,
                "deny(warnings) in source breaks the build on every new compiler; -D warnings comes from the command line (RS-DONT-07)".to_owned(),
            ));
        }
    }
}

fn todo_comments(path: &str, source: &Source, findings: &mut Vec<Finding>) {
    for (line, text) in &source.scanned.comments {
        if let Some(marker) = todo_marker(text) {
            findings.push(Finding::at(
                Rule::TodoComment,
                path,
                *line,
                format!("{marker} comment: finish the work in this change or drop the note (AGENTS.md, Done)"),
            ));
        }
    }
}

/// `TODO` or `FIXME` that opens the comment or is followed by `:` or `(`. A prose mention
/// ("no TODO is left") is not a marker.
pub fn todo_marker(comment: &str) -> Option<&'static str> {
    let text =
        comment.trim_start_matches(|c: char| c == '/' || c == '!' || c == '*' || c.is_whitespace());
    ["TODO", "FIXME"].into_iter().find(|marker| {
        let opens = text
            .strip_prefix(marker)
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_alphanumeric()));
        opens || text.contains(&format!("{marker}:")) || text.contains(&format!("{marker}("))
    })
}

fn block_on(path: &str, source: &Source, findings: &mut Vec<Finding>) {
    if !is_app_source(path) {
        return;
    }
    for token in
        source.tokens.iter().filter(|t| t.text == "block_on" && !source.is_test_line(t.line))
    {
        findings.push(Finding::at(
            Rule::BlockOn,
            path,
            token.line,
            "block_on in shipped app code blocks its thread; spawn on cv_runtime::background and wake the UI once (RS-DONT-06)".to_owned(),
        ));
    }
}

/// An expectation comment: the rules it waives, its reason, and the line it covers.
#[derive(Debug, PartialEq, Eq)]
pub struct Expectation {
    pub line: usize,
    pub rules: Vec<String>,
    pub reason: String,
}

/// Parses `repo-lint: expect(RL001, RL007) reason` from a comment's text.
/// The comment must start with the marker; doc comments and prose that mention it do not count.
pub fn parse_expectation(line: usize, comment: &str) -> Option<Expectation> {
    let rest = comment.trim_start().strip_prefix(EXPECT_MARKER)?;
    let (ids, reason) = rest.split_once(')')?;
    let rules = ids.split(',').map(|id| id.trim().to_owned()).filter(|id| !id.is_empty()).collect();
    Some(Expectation { line, rules, reason: reason.trim().to_owned() })
}

fn apply_expectations(path: &str, source: &Source, mut findings: Vec<Finding>) -> Vec<Finding> {
    let mut extra = Vec::new();
    for (line, text) in &source.scanned.comments {
        let Some(expectation) = parse_expectation(*line, text) else { continue };
        let target = target_line(source, expectation.line);
        if expectation.reason.len() < 10 {
            extra.push(Finding::at(
                Rule::Expectation,
                path,
                *line,
                "an expectation needs a reason that names the cause".to_owned(),
            ));
        }
        for id in &expectation.rules {
            let Some(rule) = Rule::from_id(id).filter(|rule| rule.is_waivable()) else {
                extra.push(Finding::at(
                    Rule::Expectation,
                    path,
                    *line,
                    format!("{id} is not a rule that a comment can waive"),
                ));
                continue;
            };
            let before = findings.len();
            findings
                .retain(|f| !(f.rule == rule && f.line.is_some_and(|l| l == *line || l == target)));
            if findings.len() == before {
                extra.push(Finding::at(
                    Rule::Expectation,
                    path,
                    *line,
                    format!("expectation for {id} matches no finding: remove the comment"),
                ));
            }
        }
    }
    findings.extend(extra);
    findings
}

/// The line an expectation on `line` covers: the next line with code, or `line` itself when it
/// carries code (a trailing comment).
fn target_line(source: &Source, line: usize) -> usize {
    let has_code =
        |l: usize| source.scanned.code.get(l - 1).is_some_and(|code| !code.trim().is_empty());
    if has_code(line) {
        return line;
    }
    (line + 1..=source.scanned.code.len()).find(|&l| has_code(l)).unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_expectations_with_several_ids() {
        let parsed =
            parse_expectation(4, " repo-lint: expect(RL007, RL013) GL query of the driver name");
        assert_eq!(
            parsed,
            Some(Expectation {
                line: 4,
                rules: vec!["RL007".to_owned(), "RL013".to_owned()],
                reason: "GL query of the driver name".to_owned()
            })
        );
        assert_eq!(parse_expectation(1, " plain comment"), None);
        assert_eq!(parse_expectation(1, "/ Waive with `repo-lint: expect(RL007) reason`."), None);
    }

    #[test]
    fn todo_markers_open_the_comment_or_carry_a_colon() {
        assert_eq!(todo_marker(" TODO handle the error"), Some("TODO"));
        assert_eq!(todo_marker("/ FIXME(owner): wrong offset"), Some("FIXME"));
        assert_eq!(todo_marker(" see the note. TODO: later"), Some("TODO"));
        assert_eq!(todo_marker(" Leave no TODO in a commit."), None);
        assert_eq!(todo_marker(" TODOS are tracked elsewhere"), None);
    }

    #[test]
    fn platform_paths_are_platform_directories_or_files() {
        assert!(is_platform_path("crates/cv-base/src/platform/windows/caption.rs"));
        assert!(is_platform_path("crates/cv-shell/src/browser/platform/windows.rs"));
        assert!(is_platform_path("crates/review-store/src/platform.rs"));
        assert!(!is_platform_path("crates/cv-shell/src/platform_bar.rs"));
    }

    #[test]
    fn only_the_apps_shipped_start_up_reads_the_motion_setting() {
        let text = "fn start(ctx: &Context) {\n    motion::install(ctx);\n}\n\n#[cfg(test)]\n\
            mod tests {\n    fn harness() { theme::motion::install(ctx); }\n}\n";
        let source = Source::parse(text.to_owned());
        assert_eq!(motion_install_lines(&source), [2, 7]);
        let lines = |path: &str, file_is_test: bool| {
            let mut findings = Vec::new();
            motion_install(path, &source, file_is_test, &mut findings);
            findings.iter().filter_map(|finding| finding.line).collect::<Vec<_>>()
        };
        assert_eq!(lines(MOTION_START_UP, false), [7], "a test in the start-up file is flagged");
        assert_eq!(lines(MOTION_START_UP, true), [2, 7], "a test-only file is all test");
        assert_eq!(lines("crates/cv-pr-diff/src/testing.rs", false), [2, 7]);

        let pinned =
            Source::parse("fn setup() { theme::motion::set_reduced(ctx, false); }\n".into());
        assert_eq!(motion_install_lines(&pinned), [0_usize; 0]);
        let defined =
            Source::parse("/// See motion::install.\npub fn install(ctx: &Context) {}\n".into());
        assert_eq!(
            motion_install_lines(&defined),
            [0_usize; 0],
            "the definition and docs are no call"
        );
    }

    #[test]
    fn app_source_is_the_bin_and_the_ui_crates_without_the_test_kit() {
        assert!(is_app_source("crates/review-app/src/app.rs"));
        assert!(is_app_source("crates/cv-pr-diff/src/diff_view.rs"));
        assert!(is_app_source("crates/cv-base/src/platform/heap.rs"));
        assert!(!is_app_source("crates/cv-testkit/src/lib.rs"));
        assert!(!is_app_source("crates/cv-base/examples/frame_pacing.rs"));
        assert!(!is_app_source("crates/review-core/src/lib.rs"));
        assert!(!is_app_source("crates/cv-runtime"));
    }
}
