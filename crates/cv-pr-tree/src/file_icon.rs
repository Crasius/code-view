//! File-type icons of the file tree: a Phosphor glyph and a color family per language or format,
//! picked from the path alone. Special file names (manifests, lock files, Dockerfile, LICENSE)
//! come first, then the extension, then a plain page as the fallback.
//!
//! The lookup runs for every visible row every frame, so it never allocates: the file name is
//! lowercased into a stack buffer and matched against static tables.
//!
//! | Family                         | Icon                                   | Hue      |
//! |--------------------------------|----------------------------------------|----------|
//! | Rust, Cargo manifest and lock  | `FILE_RS`, `PACKAGE`, `FILE_LOCK`      | Orange   |
//! | TypeScript, JavaScript         | `FILE_TS`/`TSX`, `FILE_JS`/`JSX`       | Blue, Yellow |
//! | .NET (C#, Razor, projects)     | `FILE_C_SHARP`, `FILE_HTML`, `PACKAGE` | Purple   |
//! | Data and config                | `BRACKETS_CURLY`, `BRACKETS_ANGLE`, `GEAR_SIX` | by format |
//! | Shell, PowerShell, Batch       | `TERMINAL_WINDOW`                      | Green, Blue, Slate |
//! | Build scripts                  | `HAMMER`                               | by tool  |
//! | Containers, CI                 | `SHIPPING_CONTAINER`, `ROCKET_LAUNCH`  | Blue, Green |
//! | Media, design, 3D, games       | `FILE_IMAGE`, `PALETTE`, `CUBE`, `GAME_CONTROLLER` | by kind |
//! | Large binaries                 | `DATABASE`, `TABLE`, `BRAIN`, `CPU`, `DISC`, `FILE_ZIP` | by kind |
//! | Anything else                  | `FILE`                                 | Slate    |

use cv_base::theme::FileHue::{
    self, Amber, Blue, Cyan, Green, Orange, Pink, Purple, Red, Slate, Teal, Yellow,
};
use cv_base::theme::icons;

/// The glyph and color family of one file type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIcon {
    pub glyph: &'static str,
    pub hue: FileHue,
}

const fn icon(glyph: &'static str, hue: FileHue) -> FileIcon {
    FileIcon { glyph, hue }
}

/// Unknown files: a plain page in the quiet hue.
pub const FALLBACK: FileIcon = icon(icons::FILE, Slate);

/// File names up to this length are matched whole; longer ones by their extension alone.
const NAME_CAPACITY: usize = 64;
/// Longer extensions are no file type this table knows.
const EXTENSION_CAPACITY: usize = 16;

/// The icon for the file at `path` (`/`-separated, as git reports it), ignoring case.
pub fn file_icon(path: &str) -> FileIcon {
    let name = path.rsplit_once('/').map_or(path, |(_, name)| name);
    let mut name_buffer = [0; NAME_CAPACITY];
    if let Some(lower) = ascii_lowercase(name, &mut name_buffer) {
        let (stem, extension) = split_name(lower);
        if let Some(found) =
            special_name(lower, stem, extension).or_else(|| pipeline(path, extension))
        {
            return found;
        }
        return by_extension(extension).unwrap_or(FALLBACK);
    }
    let (_, extension) = split_name(name);
    let mut extension_buffer = [0; EXTENSION_CAPACITY];
    ascii_lowercase(extension, &mut extension_buffer).and_then(by_extension).unwrap_or(FALLBACK)
}

/// `text` lowercased (ASCII only, so UTF-8 stays valid) into `buffer`; `None` when it does not fit.
fn ascii_lowercase<'a, const N: usize>(text: &str, buffer: &'a mut [u8; N]) -> Option<&'a str> {
    let target = buffer.get_mut(..text.len())?;
    target.copy_from_slice(text.as_bytes());
    target.make_ascii_lowercase();
    std::str::from_utf8(target).ok()
}

/// Stem before the first dot and extension after the last: `("docker-compose", "yml")`,
/// `("", "")` for `.env`-style dot files, whose whole name is their identity.
fn split_name(name: &str) -> (&str, &str) {
    if name.starts_with('.') {
        return ("", "");
    }
    let stem = name.split_once('.').map_or(name, |(stem, _)| stem);
    let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
    (stem, extension)
}

/// Files known by name: manifests, lock files, build and container files, dot files, docs.
#[expect(
    clippy::match_same_arms,
    reason = "a table by ecosystem: names that share an icon stay in their ecosystem's row"
)]
fn special_name(name: &str, stem: &str, extension: &str) -> Option<FileIcon> {
    let found = match name {
        "cargo.toml" => icon(icons::PACKAGE, Orange),
        "cargo.lock" => icon(icons::FILE_LOCK, Orange),
        "rust-toolchain"
        | "rust-toolchain.toml"
        | "rustfmt.toml"
        | ".rustfmt.toml"
        | "clippy.toml" => icon(icons::GEAR_SIX, Orange),
        "package.json" => icon(icons::PACKAGE, Red),
        "package-lock.json" | "npm-shrinkwrap.json" => icon(icons::FILE_LOCK, Red),
        ".npmrc" | ".nvmrc" | ".node-version" => icon(icons::GEAR_SIX, Red),
        "yarn.lock" => icon(icons::FILE_LOCK, Blue),
        ".yarnrc" | ".yarnrc.yml" => icon(icons::GEAR_SIX, Blue),
        "pnpm-lock.yaml" => icon(icons::FILE_LOCK, Amber),
        "pnpm-workspace.yaml" => icon(icons::PACKAGE, Amber),
        "bun.lockb" | "bun.lock" | "deno.lock" => icon(icons::FILE_LOCK, Slate),
        "tsconfig.json" | "jsconfig.json" => icon(icons::GEAR_SIX, Blue),
        "go.mod" | "go.work" => icon(icons::PACKAGE, Cyan),
        "go.sum" | "go.work.sum" => icon(icons::FILE_LOCK, Cyan),
        "pyproject.toml" | "setup.py" | "setup.cfg" | "requirements.txt" | "pipfile" => {
            icon(icons::PACKAGE, Blue)
        }
        "poetry.lock" | "pipfile.lock" | "uv.lock" => icon(icons::FILE_LOCK, Blue),
        "gemfile" => icon(icons::DIAMOND, Red),
        "gemfile.lock" => icon(icons::FILE_LOCK, Red),
        "rakefile" => icon(icons::HAMMER, Red),
        "composer.json" => icon(icons::PACKAGE, Purple),
        "composer.lock" => icon(icons::FILE_LOCK, Purple),
        "pubspec.yaml" => icon(icons::PACKAGE, Cyan),
        "pubspec.lock" => icon(icons::FILE_LOCK, Cyan),
        "packages.lock.json" => icon(icons::FILE_LOCK, Purple),
        "directory.packages.props" => icon(icons::PACKAGE, Purple),
        "nuget.config" | "global.json" | "directory.build.props" | "directory.build.targets" => {
            icon(icons::GEAR_SIX, Purple)
        }
        "flake.lock" | "podfile.lock" | "mix.lock" => icon(icons::FILE_LOCK, Slate),
        "makefile" | "gnumakefile" | "justfile" => icon(icons::HAMMER, Slate),
        "cmakelists.txt" => icon(icons::HAMMER, Green),
        "build.gradle"
        | "settings.gradle"
        | "build.gradle.kts"
        | "settings.gradle.kts"
        | "gradlew"
        | "gradlew.bat" => icon(icons::HAMMER, Teal),
        "pom.xml" => icon(icons::PACKAGE, Orange),
        ".dockerignore" => icon(icons::SHIPPING_CONTAINER, Blue),
        ".gitignore"
        | ".gitattributes"
        | ".gitmodules"
        | ".gitkeep"
        | ".git-blame-ignore-revs"
        | ".mailmap" => icon(icons::GIT_BRANCH, Red),
        ".editorconfig" => icon(icons::SLIDERS_HORIZONTAL, Slate),
        ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" => icon(icons::TERMINAL_WINDOW, Green),
        "codeowners" => icon(icons::USERS, Slate),
        _ => return special_stem(name, stem, extension),
    };
    Some(found)
}

/// Families of names: `Dockerfile.dev`, `docker-compose.override.yml`, `.env.local`,
/// `README.md`, `LICENSE-MIT`.
fn special_stem(name: &str, stem: &str, extension: &str) -> Option<FileIcon> {
    if name.starts_with(".env") {
        return Some(icon(icons::KEY, Amber));
    }
    if name.starts_with(".eslintrc")
        || name.starts_with(".prettierrc")
        || name.starts_with(".babelrc")
    {
        return Some(icon(icons::GEAR_SIX, Purple));
    }
    let is_yaml = matches!(extension, "yml" | "yaml");
    if matches!(stem, "dockerfile" | "containerfile")
        || ((stem.starts_with("docker-compose") || stem == "compose") && is_yaml)
    {
        return Some(icon(icons::SHIPPING_CONTAINER, Blue));
    }
    if !matches!(extension, "" | "md" | "markdown" | "txt" | "rst" | "adoc") {
        return None;
    }
    if matches!(stem, "code_of_conduct" | "code-of-conduct") {
        return Some(icon(icons::USERS, Slate));
    }
    // Docs known by name, with or without a text extension or a suffix (`LICENSE-MIT`, `README.md`).
    let base = stem.split(['-', '_']).next().unwrap_or(stem);
    let found = match base {
        "readme" => icon(icons::BOOK_OPEN_TEXT, Blue),
        "license" | "licence" | "copying" | "unlicense" | "notice" => icon(icons::SCALES, Amber),
        "changelog" | "changes" | "history" | "releases" => {
            icon(icons::CLOCK_COUNTER_CLOCKWISE, Green)
        }
        "contributing" | "authors" | "maintainers" => icon(icons::USERS, Slate),
        "security" => icon(icons::SHIELD_CHECK, Green),
        _ => return None,
    };
    Some(found)
}

/// CI definitions: GitHub workflows by folder, the others by name.
fn pipeline(path: &str, extension: &str) -> Option<FileIcon> {
    let name = path.rsplit_once('/').map_or(path, |(_, name)| name);
    let is_yaml = matches!(extension, "yml" | "yaml");
    let in_workflows =
        path.starts_with(".github/workflows/") || path.contains("/.github/workflows/");
    let is_named = [
        "azure-pipelines.yml",
        ".gitlab-ci.yml",
        "bitbucket-pipelines.yml",
        ".travis.yml",
        "appveyor.yml",
        "jenkinsfile",
    ]
    .iter()
    .any(|known| name.eq_ignore_ascii_case(known));
    (is_named || (in_workflows && is_yaml)).then_some(icon(icons::ROCKET_LAUNCH, Green))
}

/// Everything else by its (lowercase) extension.
#[expect(
    clippy::match_same_arms,
    clippy::too_many_lines,
    reason = "one flat table by family: extensions that share an icon stay in their family's row"
)]
fn by_extension(extension: &str) -> Option<FileIcon> {
    let found = match extension {
        // Languages with their own Phosphor page.
        "rs" => icon(icons::FILE_RS, Orange),
        "ts" | "mts" | "cts" => icon(icons::FILE_TS, Blue),
        "tsx" => icon(icons::FILE_TSX, Blue),
        "js" | "mjs" | "cjs" => icon(icons::FILE_JS, Yellow),
        "jsx" => icon(icons::FILE_JSX, Yellow),
        "py" | "pyi" | "pyw" | "pyx" => icon(icons::FILE_PY, Blue),
        "c" => icon(icons::FILE_C, Blue),
        "h" => icon(icons::FILE_C, Purple),
        "cpp" | "cc" | "cxx" | "c++" | "ixx" | "cppm" => icon(icons::FILE_CPP, Blue),
        "hpp" | "hh" | "hxx" | "h++" | "inl" => icon(icons::FILE_CPP, Purple),
        "cs" | "csx" => icon(icons::FILE_C_SHARP, Purple),
        "css" => icon(icons::FILE_CSS, Blue),
        "scss" | "sass" => icon(icons::FILE_CSS, Pink),
        "less" | "styl" | "pcss" => icon(icons::FILE_CSS, Purple),
        "html" | "htm" | "xhtml" => icon(icons::FILE_HTML, Orange),
        "razor" | "cshtml" | "vbhtml" => icon(icons::FILE_HTML, Purple),
        "md" | "markdown" => icon(icons::FILE_MD, Blue),
        "mdx" => icon(icons::FILE_MD, Amber),
        "sql" | "psql" | "pgsql" | "mysql" => icon(icons::FILE_SQL, Amber),
        "vue" => icon(icons::FILE_VUE, Green),
        "svg" => icon(icons::FILE_SVG, Amber),
        // Languages without one: a code page in the language's color, or its emblem.
        "go" => icon(icons::FILE_CODE, Cyan),
        "java" => icon(icons::COFFEE, Orange),
        "class" => icon(icons::COFFEE, Slate),
        "kt" | "kts" => icon(icons::FILE_CODE, Pink),
        "scala" | "sc" => icon(icons::FILE_CODE, Red),
        "groovy" => icon(icons::FILE_CODE, Teal),
        "fs" | "fsi" | "fsx" => icon(icons::FILE_CODE, Teal),
        "vb" => icon(icons::FILE_CODE, Blue),
        "php" | "phtml" => icon(icons::FILE_CODE, Purple),
        "rb" | "erb" | "rake" | "gemspec" => icon(icons::DIAMOND, Red),
        "swift" => icon(icons::BIRD, Orange),
        "dart" => icon(icons::FILE_CODE, Cyan),
        // Lua is Portuguese for moon.
        "lua" => icon(icons::MOON, Blue),
        "svelte" | "astro" => icon(icons::FILE_CODE, Orange),
        "ex" | "exs" | "hs" | "elm" | "jl" => icon(icons::FILE_CODE, Purple),
        "zig" | "nim" | "ml" | "clj" | "cljs" | "r" | "pl" | "pm" | "erl" | "m" | "mm" => {
            icon(icons::FILE_CODE, Slate)
        }
        "ipynb" => icon(icons::NOTEBOOK, Orange),
        "sh" | "bash" | "zsh" | "fish" | "ksh" => icon(icons::TERMINAL_WINDOW, Green),
        "ps1" | "psm1" | "psd1" => icon(icons::TERMINAL_WINDOW, Blue),
        "bat" | "cmd" => icon(icons::TERMINAL_WINDOW, Slate),
        "graphql" | "gql" => icon(icons::GRAPH, Pink),
        "proto" => icon(icons::SHARE_NETWORK, Teal),
        "tf" | "tfvars" | "hcl" => icon(icons::STACK, Purple),
        "bicep" | "bicepparam" => icon(icons::CLOUD, Blue),
        "dockerfile" | "containerfile" => icon(icons::SHIPPING_CONTAINER, Blue),
        // Data and config.
        "json" | "jsonc" | "json5" | "geojson" | "jsonl" => icon(icons::BRACKETS_CURLY, Amber),
        "xml" | "xaml" | "xsd" | "xsl" | "xslt" | "plist" | "resx" | "nuspec" | "manifest"
        | "wxs" => icon(icons::BRACKETS_ANGLE, Orange),
        "yml" | "yaml" => icon(icons::GEAR_SIX, Pink),
        "toml" | "ini" | "cfg" | "conf" | "config" | "properties" | "env" => {
            icon(icons::GEAR_SIX, Slate)
        }
        "props" | "targets" => icon(icons::GEAR_SIX, Purple),
        "csproj" | "fsproj" | "vbproj" | "vcxproj" | "sqlproj" | "proj" | "shproj" => {
            icon(icons::PACKAGE, Purple)
        }
        "sln" | "slnx" | "slnf" => icon(icons::TREE_STRUCTURE, Purple),
        "mk" | "make" => icon(icons::HAMMER, Slate),
        "cmake" => icon(icons::HAMMER, Green),
        "gradle" => icon(icons::HAMMER, Teal),
        "lock" => icon(icons::FILE_LOCK, Slate),
        "pem" | "crt" | "cer" | "der" | "csr" => icon(icons::CERTIFICATE, Amber),
        "key" | "pfx" | "p12" | "snk" | "jks" | "keystore" | "asc" | "gpg" => {
            icon(icons::KEY, Amber)
        }
        // Text and documents.
        "txt" | "text" => icon(icons::FILE_TXT, Slate),
        "rst" | "adoc" | "asciidoc" | "org" | "tex" => icon(icons::FILE_TEXT, Slate),
        "log" => icon(icons::SCROLL, Slate),
        "diff" | "patch" => icon(icons::GIT_DIFF, Green),
        "csv" | "tsv" => icon(icons::FILE_CSV, Green),
        "pdf" => icon(icons::FILE_PDF, Red),
        "doc" | "docx" | "odt" | "rtf" => icon(icons::FILE_DOC, Blue),
        "xls" | "xlsx" | "xlsm" | "ods" => icon(icons::FILE_XLS, Green),
        "ppt" | "pptx" | "odp" => icon(icons::FILE_PPT, Orange),
        // Images and design files.
        "png" => icon(icons::FILE_PNG, Teal),
        "jpg" | "jpeg" => icon(icons::FILE_JPG, Teal),
        "gif" | "webp" | "bmp" | "ico" | "icns" | "avif" | "tif" | "tiff" | "heic" | "tga"
        | "dds" | "exr" | "hdr" => icon(icons::FILE_IMAGE, Teal),
        "psd" | "psb" => icon(icons::PALETTE, Blue),
        "ai" | "eps" => icon(icons::PALETTE, Orange),
        "sketch" | "fig" | "xd" | "xcf" | "kra" | "afdesign" | "afphoto" => {
            icon(icons::PALETTE, Pink)
        }
        "ttf" | "otf" | "woff" | "woff2" | "eot" => icon(icons::TEXT_AA, Purple),
        // Audio and video.
        "wav" | "mp3" | "ogg" | "flac" | "aac" | "m4a" | "opus" | "aif" | "aiff" | "wma" => {
            icon(icons::FILE_AUDIO, Pink)
        }
        "mid" | "midi" => icon(icons::MUSIC_NOTES, Pink),
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" | "wmv" | "flv" => {
            icon(icons::FILE_VIDEO, Red)
        }
        // 3D and game engines.
        "blend" => icon(icons::CUBE, Orange),
        "fbx" | "obj" | "glb" | "gltf" | "stl" | "3ds" | "dae" | "usd" | "usda" | "usdz"
        | "ply" | "abc" | "max" | "ma" | "mb" => icon(icons::CUBE, Teal),
        "unity" | "prefab" | "asset" | "mat" | "anim" | "controller" => {
            icon(icons::GAME_CONTROLLER, Slate)
        }
        "uasset" | "umap" | "uproject" => icon(icons::GAME_CONTROLLER, Purple),
        // Large binaries: databases, datasets, models, archives, packages, executables, images.
        "sqlite" | "sqlite3" | "db" | "db3" | "mdb" | "accdb" | "duckdb" | "mdf" | "ldf" => {
            icon(icons::DATABASE, Cyan)
        }
        "parquet" | "arrow" | "feather" | "avro" | "orc" | "npy" | "npz" => {
            icon(icons::TABLE, Green)
        }
        "onnx" | "safetensors" | "pt" | "pth" | "ckpt" | "h5" | "gguf" | "tflite" | "pb"
        | "keras" | "mlmodel" | "pkl" | "pickle" => icon(icons::BRAIN, Pink),
        "zip" | "tar" | "gz" | "tgz" | "7z" | "rar" | "bz2" | "xz" | "zst" | "lz4" => {
            icon(icons::FILE_ZIP, Amber)
        }
        "jar" | "war" | "ear" => icon(icons::FILE_ARCHIVE, Orange),
        "nupkg" | "snupkg" => icon(icons::PACKAGE, Purple),
        "whl" | "egg" => icon(icons::PACKAGE, Blue),
        "gem" => icon(icons::DIAMOND, Red),
        "crate" => icon(icons::PACKAGE, Orange),
        "apk" | "aab" => icon(icons::PACKAGE, Green),
        "deb" | "rpm" | "msi" | "msix" | "pkg" | "unitypackage" => icon(icons::PACKAGE, Slate),
        "wasm" => icon(icons::CPU, Purple),
        "dll" | "exe" | "so" | "dylib" | "lib" | "a" | "o" | "pdb" | "bin" | "sys" | "elf"
        | "ko" => icon(icons::CPU, Slate),
        "iso" | "img" | "dmg" | "vhd" | "vhdx" | "vmdk" | "qcow2" => icon(icons::DISC, Slate),
        _ => return None,
    };
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::{FALLBACK, FileIcon, file_icon};
    use cv_base::theme::FileHue::{Amber, Blue, Green, Orange, Pink, Purple, Red, Slate, Teal};
    use cv_base::theme::{FileHue, Palette, icons};

    fn assert_icon(path: &str, glyph: &'static str, hue: FileHue) {
        assert_eq!(file_icon(path), FileIcon { glyph, hue }, "{path}");
    }

    /// One case per rule that runs before the extension table, named after the rule.
    #[test]
    fn special_file_names_win_over_extensions() {
        let cases: &[(&str, &str, &str, FileHue)] = &[
            ("a special name beats its extension", "Cargo.toml", icons::PACKAGE, Orange),
            ("a special name beats its extension", "package-lock.json", icons::FILE_LOCK, Red),
            (
                "a special name beats its extension",
                "Directory.Packages.props",
                icons::PACKAGE,
                Purple,
            ),
            ("a special name in a folder", "crates/core/Cargo.toml", icons::PACKAGE, Orange),
            ("a dot file in a folder", "src/.gitattributes", icons::GIT_BRANCH, Red),
            ("a special name in a folder", ".github/CODEOWNERS", icons::USERS, Slate),
            ("a container stem", "build/Dockerfile.dev", icons::SHIPPING_CONTAINER, Blue),
            ("a compose stem", "docker-compose.override.yml", icons::SHIPPING_CONTAINER, Blue),
            ("a compose stem", "compose.yaml", icons::SHIPPING_CONTAINER, Blue),
            ("an env file", ".env.local", icons::KEY, Amber),
            ("a doc with a suffix", "LICENSE-MIT", icons::SCALES, Amber),
            ("a doc with a text extension", "LICENSE.txt", icons::SCALES, Amber),
            ("a doc without an extension", "docs/readme", icons::BOOK_OPEN_TEXT, Blue),
            ("a doc matched before the split", "CODE_OF_CONDUCT.md", icons::USERS, Slate),
            ("CI by folder", ".github/workflows/release.yaml", icons::ROCKET_LAUNCH, Green),
            ("CI by name", "azure-pipelines.yml", icons::ROCKET_LAUNCH, Green),
            ("CI by name", "Jenkinsfile", icons::ROCKET_LAUNCH, Green),
            ("YAML outside the workflows", "deploy/values.yaml", icons::GEAR_SIX, Pink),
            ("starts like a doc, is code", "src/history.ts", icons::FILE_TS, Blue),
            ("starts like a doc, is code", "src/license_check.rs", icons::FILE_RS, Orange),
            ("a container by extension", "api.dockerfile", icons::SHIPPING_CONTAINER, Blue),
        ];
        for &(rule, path, glyph, hue) in cases {
            assert_eq!(file_icon(path), FileIcon { glyph, hue }, "{rule}: {path}");
        }
    }

    #[test]
    fn lookup_ignores_case() {
        assert_icon("SRC/MAIN.RS", icons::FILE_RS, Orange);
        assert_icon("Assets/Logo.PNG", icons::FILE_PNG, Teal);
        assert_icon("CARGO.LOCK", icons::FILE_LOCK, Orange);
        assert_icon("readme.MD", icons::BOOK_OPEN_TEXT, Blue);
        assert_icon("DOCKERFILE", icons::SHIPPING_CONTAINER, Blue);
        assert_icon(".GitIgnore", icons::GIT_BRANCH, Red);
        // Names longer than the stack buffer still match by extension.
        let long = format!("docs/{}.MD", "x".repeat(120));
        assert_icon(&long, icons::FILE_MD, Blue);
        // Non-ASCII names lowercase their ASCII part only and stay valid.
        assert_icon("docs/Überblick.Md", icons::FILE_MD, Blue);
    }

    #[test]
    fn unknown_files_fall_back_to_a_plain_page() {
        for path in [
            "x.unknownext",
            "bin/tool",
            "notes.",
            ".hidden",
            "a/b/c",
            "",
            "dir/",
            "weird.extension-longer-than-sixteen",
        ] {
            assert_eq!(file_icon(path), FALLBACK, "{path}");
        }
        assert_eq!(file_icon(&format!("{}.whatever", "y".repeat(200))), FALLBACK);
    }

    fn luminance(color: egui::Color32) -> f32 {
        let linear = egui::Rgba::from(color);
        0.2126 * linear.r() + 0.7152 * linear.g() + 0.0722 * linear.b()
    }

    fn contrast(a: egui::Color32, b: egui::Color32) -> f32 {
        let (light, dark) = (luminance(a).max(luminance(b)), luminance(a).min(luminance(b)));
        (light + 0.05) / (dark + 0.05)
    }

    /// Every hue keeps 3:1 (WCAG non-text contrast) on each fill a tree row can have.
    #[test]
    fn every_hue_has_three_to_one_contrast_on_tree_rows() {
        for palette in [Palette::LIGHT, Palette::DARK] {
            for hue in FileHue::ALL {
                for (row, background) in [
                    ("surface", palette.surface),
                    ("hover", palette.hover),
                    ("pressed", palette.pressed),
                    ("selection", palette.selection),
                ] {
                    let ratio = contrast(palette.file_hue(hue), background);
                    assert!(
                        ratio >= 3.0,
                        "{hue:?} on {row} (dark {}): {ratio:.2}",
                        palette.is_dark
                    );
                }
            }
        }
    }
}
