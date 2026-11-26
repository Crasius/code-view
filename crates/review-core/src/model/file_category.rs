//! Path-based classification of changed files, so a reviewer can look at only code,
//! only tests, only docs, or any combination.

use serde::{Deserialize, Serialize};

/// Checked in declaration order of [`FileCategory::classify`]: generated wins over tests,
/// tests over docs, so a JSON fixture under `tests/` counts as a test and a lockfile as generated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FileCategory {
    Code,
    Tests,
    Docs,
    Config,
    Generated,
    Assets,
}

const GENERATED_FILE_NAMES: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lockb",
    "cargo.lock",
    "poetry.lock",
    "pipfile.lock",
    "uv.lock",
    "composer.lock",
    "gemfile.lock",
    "go.sum",
    "packages.lock.json",
    "flake.lock",
    "pubspec.lock",
    "podfile.lock",
    "mix.lock",
];
const GENERATED_SUFFIXES: &[&str] = &[
    ".min.js",
    ".min.css",
    ".map",
    ".g.cs",
    ".g.i.cs",
    ".designer.cs",
    ".generated.cs",
    ".generated.ts",
    ".generated.go",
    ".pb.go",
    "_pb2.py",
    "_pb2_grpc.py",
    ".pb.cc",
    ".pb.h",
    ".snap",
    ".lock",
];
const GENERATED_DIRS: &[&str] =
    &["node_modules", "vendor", "dist", "__snapshots__", "generated", "__generated__"];

const TEST_DIRS: &[&str] = &[
    "test",
    "tests",
    "__tests__",
    "__test__",
    "spec",
    "specs",
    "testing",
    "e2e",
    "testdata",
    "test-data",
    "fixtures",
    "__mocks__",
    "testutils",
    "test_utils",
];
const TEST_DIR_SUFFIXES: &[&str] =
    &[".tests", ".test", ".specs", ".unittests", ".integrationtests", "_test", "_tests"];
const TEST_STEM_SUFFIXES: &[&str] =
    &["_test", "_tests", "-test", ".test", ".tests", ".spec", "_spec", "-spec", ".e2e", ".cy"];
/// Pascal-case suffixes, matched case-sensitively after a lowercase letter or digit (`OrderRouterTests`, `UserSpec`).
const TEST_PASCAL_SUFFIXES: &[&str] = &["Tests", "Test", "Specs", "Spec", "IT"];
const TEST_STEM_PREFIXES: &[&str] = &["test_"];

const DOC_EXTENSIONS: &[&str] = &["md", "mdx", "markdown", "rst", "adoc", "asciidoc", "txt", "rtf"];
const DOC_DIRS: &[&str] = &["docs", "doc", "documentation", "wiki"];
const DOC_STEMS: &[&str] = &[
    "readme",
    "changelog",
    "changes",
    "license",
    "licence",
    "contributing",
    "authors",
    "notice",
    "copying",
    "security",
    "code_of_conduct",
];

const ASSET_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "avif", "svg", "tif", "tiff", "psd", "ttf",
    "otf", "woff", "woff2", "eot", "mp3", "mp4", "wav", "ogg", "webm", "mov", "pdf", "zip", "gz",
    "7z", "tar", "jar", "dll", "exe", "so", "dylib", "bin", "pfx", "cer",
];

const CONFIG_EXTENSIONS: &[&str] = &[
    "json",
    "jsonc",
    "yml",
    "yaml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "config",
    "xml",
    "props",
    "targets",
    "csproj",
    "fsproj",
    "vbproj",
    "sln",
    "slnx",
    "gradle",
    "properties",
    "editorconfig",
    "env",
    "plist",
    "tf",
    "tfvars",
    "bicep",
    "nuspec",
    "resx",
];
const CONFIG_FILE_NAMES: &[&str] = &[
    "dockerfile",
    "makefile",
    "cmakelists.txt",
    "justfile",
    "procfile",
    "jenkinsfile",
    "vagrantfile",
    "gemfile",
    "rakefile",
    "podfile",
    ".gitignore",
    ".gitattributes",
    ".gitmodules",
    ".dockerignore",
    ".npmrc",
    ".nvmrc",
    ".prettierrc",
    ".eslintrc",
    ".babelrc",
    "codeowners",
];
const CONFIG_DIRS: &[&str] = &[
    ".github",
    ".gitlab",
    ".azuredevops",
    ".devcontainer",
    ".vscode",
    ".idea",
    ".config",
    "deploy",
    "helm",
    "k8s",
];

impl FileCategory {
    pub const ALL: [FileCategory; 6] = [
        FileCategory::Code,
        FileCategory::Tests,
        FileCategory::Docs,
        FileCategory::Config,
        FileCategory::Generated,
        FileCategory::Assets,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FileCategory::Code => "Code",
            FileCategory::Tests => "Tests",
            FileCategory::Docs => "Docs",
            FileCategory::Config => "Config & build",
            FileCategory::Generated => "Generated",
            FileCategory::Assets => "Assets",
        }
    }

    /// Classifies a repository-relative path with `/` separators.
    pub fn classify(path: &str) -> FileCategory {
        let original_file_name = path.rsplit('/').next().unwrap_or(path);
        let (original_stem, _) = split_extension(original_file_name);
        let lower = path.to_ascii_lowercase();
        let (dirs, file_name) = match lower.rsplit_once('/') {
            Some((dirs, file_name)) => (dirs, file_name),
            None => ("", lower.as_str()),
        };
        let mut segments = dirs.split('/').filter(|segment| !segment.is_empty());
        let (stem, extension) = split_extension(file_name);

        if is_generated(file_name, dirs) {
            return FileCategory::Generated;
        }
        if is_test(&mut segments.clone(), stem, original_stem) {
            return FileCategory::Tests;
        }
        if is_doc(&mut segments.clone(), file_name, stem, extension) {
            return FileCategory::Docs;
        }
        if ASSET_EXTENSIONS.contains(&extension) {
            return FileCategory::Assets;
        }
        if is_config(&mut segments, file_name, extension) {
            return FileCategory::Config;
        }
        FileCategory::Code
    }
}

fn split_extension(file_name: &str) -> (&str, &str) {
    match file_name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, extension),
        _ => (file_name, ""),
    }
}

fn is_generated(file_name: &str, dirs: &str) -> bool {
    let is_lockfile = GENERATED_FILE_NAMES.contains(&file_name);
    let has_generated_suffix = GENERATED_SUFFIXES.iter().any(|suffix| file_name.ends_with(suffix));
    let is_in_generated_dir = dirs.split('/').any(|segment| GENERATED_DIRS.contains(&segment));
    is_lockfile || has_generated_suffix || is_in_generated_dir
}

fn is_test<'a>(
    segments: &mut impl Iterator<Item = &'a str>,
    stem: &str,
    original_stem: &str,
) -> bool {
    let is_in_test_dir = segments.any(|segment| {
        let is_test_dir_name = TEST_DIRS.contains(&segment);
        let has_test_dir_suffix = TEST_DIR_SUFFIXES.iter().any(|suffix| segment.ends_with(suffix));
        is_test_dir_name || has_test_dir_suffix
    });
    let has_test_stem_suffix =
        TEST_STEM_SUFFIXES.iter().any(|suffix| stem.len() > suffix.len() && stem.ends_with(suffix));
    let has_test_stem_prefix = TEST_STEM_PREFIXES.iter().any(|prefix| stem.starts_with(prefix));
    let has_pascal_suffix =
        TEST_PASCAL_SUFFIXES.iter().any(|suffix| has_pascal_suffix(original_stem, suffix));
    is_in_test_dir || has_test_stem_suffix || has_test_stem_prefix || has_pascal_suffix
}

fn has_pascal_suffix(stem: &str, suffix: &str) -> bool {
    let Some(head) = stem.strip_suffix(suffix) else {
        return false;
    };
    head.chars().next_back().is_some_and(|last| last.is_ascii_lowercase() || last.is_ascii_digit())
}

fn is_doc<'a>(
    segments: &mut impl Iterator<Item = &'a str>,
    file_name: &str,
    stem: &str,
    extension: &str,
) -> bool {
    // `CMakeLists.txt` and `requirements.txt` are text files that build the project.
    let has_doc_extension = DOC_EXTENSIONS.contains(&extension) && !is_build_file(file_name);
    let is_doc_stem = DOC_STEMS.contains(&stem);
    let is_in_doc_dir = segments.any(|segment| DOC_DIRS.contains(&segment));
    let is_doc_dir_text = is_in_doc_dir && !CONFIG_EXTENSIONS.contains(&extension);
    has_doc_extension || is_doc_stem || is_doc_dir_text
}

fn is_build_file(file_name: &str) -> bool {
    CONFIG_FILE_NAMES.contains(&file_name)
        || (file_name.starts_with("requirements") && split_extension(file_name).1 == "txt")
}

fn is_config<'a>(
    segments: &mut impl Iterator<Item = &'a str>,
    file_name: &str,
    extension: &str,
) -> bool {
    let has_config_extension = CONFIG_EXTENSIONS.contains(&extension);
    let is_config_file = is_build_file(file_name) || file_name.starts_with("dockerfile");
    let is_ci_file =
        file_name.starts_with(".gitlab-ci") || file_name.starts_with("azure-pipelines");
    let is_in_config_dir = segments.any(|segment| CONFIG_DIRS.contains(&segment));
    has_config_extension || is_config_file || is_ci_file || is_in_config_dir
}

#[cfg(test)]
mod tests {
    use super::FileCategory::{self, *};

    fn assert_category(path: &str, expected: FileCategory) {
        assert_eq!(FileCategory::classify(path), expected, "{path}");
    }

    #[test]
    fn classifies_code() {
        assert_category("src/main.rs", Code);
        assert_category("src/Orders/OrderRouter.cs", Code);
        assert_category("web/app/page.tsx", Code);
        assert_category("contest/latest.py", Code);
        assert_category("src/Inspect.cs", Code);
        assert_category("src/Manifest.rs", Code);
    }

    #[test]
    fn classifies_tests() {
        assert_category("tests/ReviewApp.Tests/OrderRouterTests.cs", Tests);
        assert_category("src/ReviewApp.Tests/Foo.cs", Tests);
        assert_category("pkg/router/router_test.go", Tests);
        assert_category("web/app/page.test.tsx", Tests);
        assert_category("web/app/page.spec.ts", Tests);
        assert_category("lib/test_parser.py", Tests);
        assert_category("tests/fixtures/payload.json", Tests);
        assert_category("src/__tests__/a.js", Tests);
        assert_category("src/main/java/com/acme/OrderServiceTest.java", Tests);
        assert_category("src/Orders/OrderRouterTests.cs", Tests);
    }

    #[test]
    fn classifies_docs() {
        assert_category("README.md", Docs);
        assert_category("docs/architecture/overview.md", Docs);
        assert_category("LICENSE", Docs);
        assert_category("docs/diagram.puml", Docs);
    }

    #[test]
    fn classifies_config() {
        assert_category(".github/workflows/ci.yml", Config);
        assert_category("src/App/App.csproj", Config);
        assert_category("Dockerfile.dev", Config);
        assert_category(".gitlab-ci.yml", Config);
        assert_category("azure-pipelines.yml", Config);
        assert_category("Cargo.toml", Config);
        assert_category("CMakeLists.txt", Config);
        assert_category("requirements-dev.txt", Config);
    }

    #[test]
    fn classifies_generated() {
        assert_category("package-lock.json", Generated);
        assert_category("Cargo.lock", Generated);
        assert_category("web/dist/bundle.js", Generated);
        assert_category("src/Data/Migrations/20240101_Init.Designer.cs", Generated);
        assert_category("web/__snapshots__/page.test.tsx.snap", Generated);
    }

    #[test]
    fn classifies_assets() {
        assert_category("assets/logo.png", Assets);
        assert_category("fonts/Inter.woff2", Assets);
        assert_category("docs/images/flow.svg", Docs);
    }
}
