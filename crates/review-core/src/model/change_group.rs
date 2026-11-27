//! The five groups the change breakdown sums a pull request's changed lines into: Code, Tests,
//! Docs, Mockups and Other. Coarser than [`FileCategory`] and built on it, so a file the category
//! filter calls a test is a test here too. Precedence: Tests > Mockups > Docs > Code > Other.
//!
//! - Tests: what [`FileCategory::classify`] calls tests (test folders such as `tests/`,
//!   `__tests__/`, `spec/`, `e2e/`, `fixtures/`; names such as `*_test.go`, `*.test.ts`,
//!   `*.spec.ts`, `test_*.py`, `*Test.java`, `*Tests.cs`, `*_spec.rb`). A test's image stays a
//!   test: `tests/fixtures/logo.png` is Tests.
//! - Mockups: images and design files ([`MOCKUP_EXTENSIONS`]) wherever they live,
//!   `docs/images/flow.svg` included, and every file below a [`MOCKUP_DIRS`] folder or `mocks/ui`.
//! - Docs: what [`FileCategory::classify`] calls docs. `requirements.txt` and `CMakeLists.txt` are
//!   build files, so Other.
//! - Code: the filter's code whose extension is a source language ([`SOURCE_EXTENSIONS`], html,
//!   css, scss and sql included). A file the filter calls code only because nothing else matched
//!   (no or an unknown extension: `bin/run`, `data.csv`) is Other.
//! - Other: config and CI, lockfiles and generated files, fonts, media, archives and binaries.
//!   Generated output wins over tests, as in the filter: `__snapshots__/*.snap` is Other.

use super::FileCategory;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeGroup {
    Code,
    Tests,
    Docs,
    Mockups,
    Other,
}

const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "cs", "fs", "fsx", "vb", "java", "kt", "kts",
    "scala", "groovy", "go", "py", "pyi", "rb", "php", "swift", "m", "mm", "dart", "lua", "pl",
    "pm", "r", "jl", "ex", "exs", "erl", "hs", "ml", "mli", "clj", "cljs", "elm", "zig", "nim",
    "js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "vue", "svelte", "astro", "html", "htm",
    "css", "scss", "sass", "less", "sql", "sh", "bash", "zsh", "ps1", "psm1", "bat", "cmd",
    "razor", "cshtml", "xaml", "proto", "graphql", "gql",
];
const MOCKUP_EXTENSIONS: &[&str] = &[
    "png",
    "jpg",
    "jpeg",
    "gif",
    "webp",
    "svg",
    "ico",
    "bmp",
    "avif",
    "fig",
    "sketch",
    "xd",
    "psd",
    "ai",
    "pen",
    "excalidraw",
    "drawio",
];
const MOCKUP_DIRS: &[&str] = &["mockups", "designs", "wireframes", "screenshots"];

impl ChangeGroup {
    /// The breakdown's rows, in this order.
    pub const ALL: [ChangeGroup; 5] = [
        ChangeGroup::Code,
        ChangeGroup::Tests,
        ChangeGroup::Docs,
        ChangeGroup::Mockups,
        ChangeGroup::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ChangeGroup::Code => "Code",
            ChangeGroup::Tests => "Tests",
            ChangeGroup::Docs => "Docs",
            ChangeGroup::Mockups => "Mockups",
            ChangeGroup::Other => "Other",
        }
    }

    /// The group of the file at `path` (repository-relative, `/` separators), whose category
    /// [`FileCategory::classify`] gave.
    pub fn of(category: FileCategory, path: &str) -> ChangeGroup {
        if category == FileCategory::Tests {
            return ChangeGroup::Tests;
        }
        let lower = path.to_ascii_lowercase();
        let (dirs, file_name) = lower.rsplit_once('/').unwrap_or(("", lower.as_str()));
        let (_, extension) = super::file_category::split_extension(file_name);
        if is_mockup(dirs, extension) {
            return ChangeGroup::Mockups;
        }
        match category {
            FileCategory::Docs => ChangeGroup::Docs,
            FileCategory::Code if SOURCE_EXTENSIONS.contains(&extension) => ChangeGroup::Code,
            _ => ChangeGroup::Other,
        }
    }
}

fn is_mockup(dirs: &str, extension: &str) -> bool {
    let has_mockup_extension = MOCKUP_EXTENSIONS.contains(&extension);
    let is_in_mockup_dir = dirs.split('/').any(|segment| MOCKUP_DIRS.contains(&segment));
    let is_in_ui_mocks = format!("/{dirs}/").contains("/mocks/ui/");
    has_mockup_extension || is_in_mockup_dir || is_in_ui_mocks
}

/// One group's share of a change: its files and the lines of those already counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GroupChanges {
    pub files: u32,
    pub added: u64,
    pub deleted: u64,
    /// Files whose lines are not counted yet; their lines are missing from `added` and `deleted`.
    pub uncounted: u32,
}

impl GroupChanges {
    pub fn changed(&self) -> u64 {
        self.added + self.deleted
    }

    fn add(&mut self, other: &GroupChanges) {
        self.files += other.files;
        self.added += other.added;
        self.deleted += other.deleted;
        self.uncounted += other.uncounted;
    }
}

/// A change's files and lines by [`ChangeGroup`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeBreakdown {
    groups: [GroupChanges; ChangeGroup::ALL.len()],
}

impl ChangeBreakdown {
    /// Counts one file into `group`; `lines` is (added, deleted), `None` while not counted. A
    /// binary file counts with `Some((0, 0))`.
    pub fn add(&mut self, group: ChangeGroup, lines: Option<(u32, u32)>) {
        let entry = &mut self.groups[group as usize];
        entry.files += 1;
        match lines {
            Some((added, deleted)) => {
                entry.added += u64::from(added);
                entry.deleted += u64::from(deleted);
            }
            None => entry.uncounted += 1,
        }
    }

    pub fn group(&self, group: ChangeGroup) -> GroupChanges {
        self.groups[group as usize]
    }

    pub fn total(&self) -> GroupChanges {
        let mut total = GroupChanges::default();
        for group in &self.groups {
            total.add(group);
        }
        total
    }

    /// `group`'s part of all changed lines, 0 to 1; 0 while no line is counted.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a share to draw: rounding counts above 2^24 lines moves it by less than a pixel"
    )]
    pub fn share(&self, group: ChangeGroup) -> f32 {
        let total = self.total().changed();
        if total == 0 {
            return 0.0;
        }
        self.group(group).changed() as f32 / total as f32
    }
}

#[cfg(test)]
mod tests {
    use super::ChangeGroup::{self, *};
    use super::{ChangeBreakdown, FileCategory, GroupChanges};

    fn assert_group(path: &str, expected: ChangeGroup) {
        let group = ChangeGroup::of(FileCategory::classify(path), path);
        assert_eq!(group, expected, "{path}");
    }

    #[test]
    fn mockups_are_images_design_files_and_mockup_folders() {
        for path in [
            "assets/logo.png",
            "web/public/hero.JPEG",
            "icons/app.ico",
            "art/banner.avif",
            "design/checkout.fig",
            "design/flow.sketch",
            "design/board.excalidraw",
            "design/system.drawio",
            "design/cover.psd",
            "mockups/checkout.html",
            "docs/designs/notes.md",
            "web/mocks/ui/cart.json",
            "screenshots/home.txt",
        ] {
            assert_group(path, Mockups);
        }
    }

    #[test]
    fn code_is_source_languages_markup_and_sql() {
        for path in [
            "src/main.rs",
            "web/app/page.tsx",
            "web/index.html",
            "web/site.scss",
            "web/site.css",
            "db/migrations/001_init.sql",
            "scripts/deploy.sh",
            "src/App/Index.razor",
        ] {
            assert_group(path, Code);
        }
    }

    #[test]
    fn other_is_config_generated_build_and_unknown_files() {
        for path in [
            "Cargo.lock",
            "package-lock.json",
            "Cargo.toml",
            ".github/workflows/ci.yml",
            "config/settings.json",
            "Dockerfile",
            "fonts/Inter.woff2",
            "vendor/native.dll",
            "requirements.txt",
            "CMakeLists.txt",
            "bin/run",
            "data/export.csv",
            "web/__snapshots__/page.test.tsx.snap",
        ] {
            assert_group(path, Other);
        }
    }

    #[test]
    fn precedence_is_tests_mockups_docs_code_other() {
        assert_group("tests/fixtures/logo.png", Tests);
        assert_group("tests/README.md", Tests);
        assert_group("docs/images/flow.svg", Mockups);
        assert_group("mockups/README.md", Mockups);
        assert_group("docs/api/client.md", Docs);
        assert_group("designs/tokens.rs", Mockups);
    }

    #[test]
    fn breakdown_sums_files_and_lines_by_group() {
        let mut breakdown = ChangeBreakdown::default();
        breakdown.add(Code, Some((10, 2)));
        breakdown.add(Code, Some((5, 3)));
        breakdown.add(Tests, Some((20, 0)));
        breakdown.add(Mockups, Some((0, 0)));
        breakdown.add(Docs, None);
        assert_eq!(
            breakdown.group(Code),
            GroupChanges { files: 2, added: 15, deleted: 5, uncounted: 0 }
        );
        assert_eq!(
            breakdown.group(Docs),
            GroupChanges { files: 1, uncounted: 1, ..Default::default() }
        );
        assert_eq!(breakdown.group(Other), GroupChanges::default());
        assert_eq!(
            breakdown.total(),
            GroupChanges { files: 5, added: 35, deleted: 5, uncounted: 1 }
        );
        assert!((breakdown.share(Code) - 0.5).abs() < f32::EPSILON);
        assert!((breakdown.share(Tests) - 0.5).abs() < f32::EPSILON);
        assert_eq!(breakdown.share(Mockups), 0.0, "a binary file adds no lines");
    }

    #[test]
    fn shares_are_zero_while_no_line_is_counted() {
        let mut breakdown = ChangeBreakdown::default();
        breakdown.add(Code, None);
        assert_eq!(breakdown.share(Code), 0.0);
    }
}
