//! Crate-level rules from `cargo metadata` and the member manifests: the dependency direction
//! (RS-DO-03), UI crates only in the app's crates, profiles only in the root manifest
//! (RS-PERF-05), every dependency declared once in `[workspace.dependencies]` (rule `rust`,
//! Dependencies), and the `test-support` feature only for tests (rule `testing`).

use crate::finding::{Finding, Rule};
use crate::items::TEST_FEATURE;
use crate::repo::Crate;

const DOMAIN: &[&str] = &[
    "review-core",
    "review-agent",
    "review-git",
    "review-store",
    "review-grammars",
    "provider-azure",
    "provider-github",
    "provider-gitlab",
];

/// The direction table: each member with its layer and the domain crates it may depend on. It
/// is the layer table of .agents/rules/rust.md (Workspace); keep both in step. A crate with a
/// layer may also depend on every crate of a strictly lower layer, as a normal, optional, dev or
/// build dependency. Rows without a layer are the domain crates and the tools: none of the app's
/// crates may serve them. A new member needs a row here before it builds clean.
const DIRECTION: &[(&str, Option<u8>, &[&str])] = &[
    ("review-core", None, &[]),
    ("review-git", None, &["review-core"]),
    ("review-store", None, &["review-core"]),
    ("review-agent", None, &["review-core"]),
    ("review-grammars", None, &[]),
    ("provider-azure", None, &["review-core"]),
    ("provider-github", None, &["review-core"]),
    ("provider-gitlab", None, &["review-core"]),
    ("repo-lint", None, &[]),
    ("cv-base", Some(0), &["review-store"]),
    ("cv-widgets", Some(1), &["review-core", "review-git", "review-store"]),
    ("cv-testkit", Some(1), &["review-core", "review-store"]),
    (
        "cv-providers",
        Some(1),
        &["review-core", "provider-azure", "provider-github", "provider-gitlab"],
    ),
    ("cv-runtime", Some(2), &["review-core", "review-git", "review-store"]),
    ("cv-media", Some(3), &["review-core", "review-git", "review-agent"]),
    ("cv-syntax", Some(3), &["review-grammars"]),
    ("cv-markdown", Some(4), &["review-core", "review-store"]),
    (
        "cv-services",
        Some(4),
        &["review-core", "review-git", "review-store", "review-agent", "provider-github"],
    ),
    ("cv-editor", Some(5), &["review-core", "review-git", "review-store"]),
    ("cv-code", Some(5), &["review-core", "review-git"]),
    ("cv-shell", Some(5), &["review-core", "review-store"]),
    ("cv-pr-session", Some(6), &["review-core", "review-git", "review-store"]),
    ("cv-pr-tree", Some(6), &["review-core", "review-git", "review-store"]),
    ("cv-pr-threads", Some(7), &["review-core", "review-git", "review-store", "provider-github"]),
    ("cv-pr-assistant", Some(7), &["review-core", "review-git", "review-store", "review-agent"]),
    ("cv-pr-pipelines", Some(7), &["review-core", "review-store"]),
    ("cv-lists", Some(7), &["review-core", "review-git", "review-store"]),
    (
        "cv-settings",
        Some(7),
        &[
            "review-core",
            "review-git",
            "review-store",
            "review-agent",
            "provider-azure",
            "provider-github",
            "provider-gitlab",
        ],
    ),
    ("cv-pr-diff", Some(8), &["review-core", "review-git", "review-store"]),
    ("cv-pr-rendered", Some(9), &["review-core", "review-git", "review-store"]),
    ("cv-pr-summary", Some(10), &["review-core", "review-git", "review-store"]),
    ("cv-pr-files", Some(11), &["review-core", "review-git", "review-store"]),
    ("cv-pr-page", Some(12), &["review-core", "review-git", "review-store"]),
    ("review-app", Some(13), DOMAIN),
];

/// The workspace crates `name` may depend on, or `None` when it has no row.
fn allowed_dependencies(name: &str) -> Option<Vec<&'static str>> {
    let &(_, layer, domain) = DIRECTION.iter().find(|(row, _, _)| *row == name)?;
    let lower = DIRECTION
        .iter()
        .filter(|(_, other, _)| other.zip(layer).is_some_and(|(other, own)| other < own))
        .map(|(row, _, _)| *row);
    Some(domain.iter().copied().chain(lower).collect())
}

/// UI crates; only the app's crates may depend on them, so domain crates stay testable without a
/// GPU.
const UI_CRATES: &[&str] = &["egui", "eframe", "egui_extras", "egui_kittest", "resvg"];

/// The app's crates: the `review-app` bin and the `cv-*` UI crates below it.
fn is_ui_owner(name: &str) -> bool {
    name == "review-app" || name.starts_with("cv-")
}

pub fn check_crates(crates: &[Crate]) -> Vec<Finding> {
    let members: Vec<&str> = crates.iter().map(|c| c.name.as_str()).collect();
    let mut findings = Vec::new();
    for krate in crates {
        let manifest = format!("{}/Cargo.toml", krate.dir);
        let Some(allowed) = allowed_dependencies(&krate.name) else {
            findings.push(Finding::file(
                Rule::CrateDirection,
                &manifest,
                format!(
                    "crate {} has no row in the dependency direction table (tools/repo-lint/src/manifest.rs); \
                     add it with its layer and domain crates (.agents/rules/rust.md, Workspace)",
                    krate.name
                ),
            ));
            continue;
        };
        for dependency in &krate.dependencies {
            let is_member = members.contains(&dependency.as_str());
            if is_member && !allowed.contains(&dependency.as_str()) {
                let allowed = if allowed.is_empty() {
                    "no workspace crate".to_owned()
                } else {
                    allowed.join(", ")
                };
                findings.push(Finding::file(
                    Rule::CrateDirection,
                    &manifest,
                    format!(
                        "{} depends on {dependency}; it may use {allowed}. Move the shared item \
                         down to a crate both may use: review-core for domain types, a lower \
                         cv-* layer for UI code (RS-DO-03)",
                        krate.name
                    ),
                ));
            }
            if UI_CRATES.contains(&dependency.as_str()) && !is_ui_owner(&krate.name) {
                findings.push(Finding::file(
                    Rule::UiDependency,
                    &manifest,
                    format!(
                        "{} depends on the UI crate {dependency}; only review-app and the cv-* \
                         crates may (RS-DO-03)",
                        krate.name
                    ),
                ));
            }
        }
    }
    findings
}

/// Profile tables and dependencies that bypass `[workspace.dependencies]` in a member manifest.
pub fn check_member_manifest(path: &str, text: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("[profile") {
            findings.push(Finding::at(
                Rule::MemberProfile,
                path,
                index + 1,
                "Cargo ignores profiles outside the root manifest: move it to the root Cargo.toml (RS-PERF-05)".to_owned(),
            ));
        }
    }
    for dependency in dependencies(text).into_iter().filter(|d| !d.inherits) {
        findings.push(Finding::at(
            Rule::WorkspaceDependency,
            path,
            dependency.line,
            format!(
                "{} is declared here; declare it once in [workspace.dependencies] of the root Cargo.toml \
                 and use `{}.workspace = true` (rule rust, Dependencies)",
                dependency.name, dependency.name
            ),
        ));
    }
    for line in test_feature_lines(text) {
        findings.push(Finding::at(
            Rule::TestSupportFeature,
            path,
            line,
            format!(
                "a shipped dependency turns on `{TEST_FEATURE}`, which links test fakes into the \
                 app; enable it under [dev-dependencies] or forward it from [features] (rule testing)"
            ),
        ));
    }
    findings
}

/// Lines that turn on a dependency's `test-support` feature outside `[dev-dependencies]`: in a
/// normal or build dependency table, per target or dotted. A `[features]` list may forward the
/// feature (`test-support = ["cv-base/test-support"]`), since a feature only adds test items.
fn test_feature_lines(text: &str) -> Vec<usize> {
    let quoted = format!("\"{TEST_FEATURE}\"");
    let mut in_shipped_table = false;
    let mut lines = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.split(" #").next().unwrap_or_default().trim();
        if line.starts_with('[') {
            let header = line.trim_matches(|c| c == '[' || c == ']');
            let lists_dependencies =
                header.ends_with("dependencies") || header.contains("dependencies.");
            in_shipped_table = lists_dependencies && !header.contains("dev-dependencies");
            continue;
        }
        if in_shipped_table && line.contains(&quoted) {
            lines.push(index + 1);
        }
    }
    lines
}

#[derive(Debug, PartialEq, Eq)]
pub struct Dependency {
    pub name: String,
    pub line: usize,
    /// Declared with `workspace = true`.
    pub inherits: bool,
}

/// The dependencies of a member manifest, from every `[*dependencies]` table (normal, dev, build,
/// per target) and every `[dependencies.<name>]` table. Not a TOML parser: it follows the layout
/// Cargo manifests use (one key per line, values that may span lines).
pub fn dependencies(text: &str) -> Vec<Dependency> {
    let mut out: Vec<Dependency> = Vec::new();
    let mut in_table = false;
    let mut in_dotted = false;
    let mut open: Option<(String, usize, String)> = None;
    for (index, raw) in text.lines().enumerate() {
        let line = raw.split(" #").next().unwrap_or_default().trim();
        if let Some((name, start, mut value)) = open.take() {
            value.push_str(line);
            finish_or_keep(&mut out, &mut open, name, start, value);
            continue;
        }
        if line.starts_with('[') {
            let header = line.trim_matches(|c| c == '[' || c == ']');
            in_table = header.ends_with("dependencies") && !header.starts_with("workspace");
            let dotted =
                header.rsplit_once("dependencies.").filter(|_| !header.starts_with("workspace"));
            in_dotted = dotted.is_some();
            if let Some((_, name)) = dotted {
                out.push(Dependency { name: name.to_owned(), line: index + 1, inherits: false });
            }
            continue;
        }
        if in_dotted
            && line.replace(' ', "") == "workspace=true"
            && let Some(last) = out.last_mut()
        {
            last.inherits = true;
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| in_table) else {
            continue;
        };
        finish_or_keep(
            &mut out,
            &mut open,
            key.trim().to_owned(),
            index + 1,
            value.trim().to_owned(),
        );
    }
    out
}

fn finish_or_keep(
    out: &mut Vec<Dependency>,
    open: &mut Option<(String, usize, String)>,
    key: String,
    line: usize,
    value: String,
) {
    let depth = value.chars().fold(0i32, |depth, c| match c {
        '{' | '[' => depth + 1,
        '}' | ']' => depth - 1,
        _ => depth,
    });
    if depth > 0 {
        *open = Some((key, line, value));
        return;
    }
    let (name, inherits) = match key.strip_suffix(".workspace") {
        Some(name) => (name.to_owned(), value == "true"),
        None => (key, value.replace(' ', "").contains("workspace=true")),
    };
    out.push(Dependency { name, line, inherits });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_dependencies_from_every_kind_of_table() {
        let manifest = "[package]\nname = \"x\"\n\n[dependencies]\nserde.workspace = true\n\
            arboard = { version = \"3.7\", default-features = false }\n\
            windows-sys = { workspace = true, features = [\n    \"A\",\n] }\n\
            [target.'cfg(windows)'.build-dependencies]\nwinresource = \"0.1\"\n\
            [dev-dependencies.wiremock]\nworkspace = true\n\
            [package.metadata.bundle]\nname = \"Code View\"\n";
        let summary: Vec<(String, usize, bool)> =
            dependencies(manifest).into_iter().map(|d| (d.name, d.line, d.inherits)).collect();
        assert_eq!(
            summary,
            [
                ("serde".to_owned(), 5, true),
                ("arboard".to_owned(), 6, false),
                ("windows-sys".to_owned(), 7, true),
                ("winresource".to_owned(), 11, false),
                ("wiremock".to_owned(), 12, true),
            ]
        );
    }

    #[test]
    fn workspace_table_and_package_keys_are_not_dependencies() {
        let root = "[workspace.dependencies]\nserde = \"1\"\n[package]\nversion = \"1\"\n";
        assert_eq!(dependencies(root), []);
    }

    fn may_use(name: &str, dependency: &str) -> bool {
        allowed_dependencies(name).expect("a row").contains(&dependency)
    }

    #[test]
    fn edges_must_point_to_a_strictly_lower_layer() {
        assert!(may_use("cv-pr-page", "cv-base"), "many layers down");
        assert!(may_use("cv-settings", "provider-gitlab"), "a domain crate from its row");
        assert!(!may_use("cv-base", "cv-testkit"), "a second cv-base and global allocator");
        assert!(!may_use("cv-widgets", "cv-testkit"), "same layer");
        assert!(!may_use("cv-pr-threads", "cv-pr-assistant"), "same layer");
        assert!(!may_use("cv-code", "cv-pr-diff"), "upward");
        assert!(!may_use("cv-pr-diff", "cv-pr-rendered"), "upward");
        assert!(!may_use("cv-pr-page", "review-app"), "the bin");
        assert!(!may_use("review-core", "cv-base"), "domain crates never use the app's crates");
        assert!(!may_use("cv-syntax", "review-store"), "not one of its domain crates");
    }

    #[test]
    fn every_cv_crate_has_a_layer_and_the_ui_crates_too() {
        for (name, layer, _) in DIRECTION {
            assert_eq!(layer.is_some(), is_ui_owner(name), "{name}");
        }
        assert!(!is_ui_owner("review-store"));
    }

    #[test]
    fn check_crates_names_the_upward_dependency_and_the_ui_crate() {
        let krate = |name: &str, dependencies: &[&str]| Crate {
            name: name.to_owned(),
            dir: format!("crates/{name}"),
            dependencies: dependencies.iter().map(|d| (*d).to_owned()).collect(),
            roots: Vec::new(),
        };
        let crates = [
            krate("cv-base", &["egui", "review-store"]),
            krate("cv-code", &["cv-base", "cv-pr-diff", "egui"]),
            krate("cv-pr-diff", &["cv-code"]),
            krate("review-store", &["review-core", "egui"]),
            krate("review-core", &[]),
        ];
        let found: Vec<(Rule, String)> =
            check_crates(&crates).into_iter().map(|f| (f.rule, f.path)).collect();
        assert_eq!(
            found,
            [
                (Rule::CrateDirection, "crates/cv-code/Cargo.toml".to_owned()),
                (Rule::UiDependency, "crates/review-store/Cargo.toml".to_owned()),
            ]
        );
    }

    #[test]
    fn test_support_is_enabled_only_for_tests() {
        let manifest = "[features]\ntest-support = [\"cv-base/test-support\", \"dep:cv-testkit\"]\n\
            [dependencies]\ncv-base.workspace = true\n\
            cv-media = { workspace = true, features = [\"test-support\"] } # wrong\n\
            [target.'cfg(windows)'.dependencies.cv-shell]\nworkspace = true\nfeatures = [\"test-support\"]\n\
            [dev-dependencies]\ncv-base = { workspace = true, features = [\"test-support\"] }\n\
            [target.'cfg(windows)'.dev-dependencies]\ncv-shell = { workspace = true, features = [\"test-support\"] }\n";
        assert_eq!(test_feature_lines(manifest), [5, 8]);
    }
}
