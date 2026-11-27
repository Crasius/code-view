//! Whitespace-only changes end to end: the real app against the mocked GitHub host, with files
//! whose only change is line endings, the final newline, trailing whitespace, indentation or the
//! byte order mark. Each shows its changed lines under a banner that names the change, the file
//! tree marks them, and "Ignore whitespace" (button, header switch, Alt+W) hides them behind a
//! notice that shows them again.

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::ReviewApp;
use super::github_mock::{ExtraFile, GitHubHost, open_pull_request};
use super::walkthrough::{app, files_tab, saved_settings, shows, shows_file_view, wait_until};
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

const EOL: &str = "src/eol.rs";
const TABS: &str = "src/tabs.rs";
const TRAILING: &str = "src/trailing.rs";
const EOF: &str = "src/eof.txt";
const BOM: &str = "src/bom.txt";
const MIXED: &str = "src/mixed.txt";
const HIDE_LABEL: &str = "Hide whitespace changes";
const SHOW_LABEL: &str = "Show whitespace changes";
const HIDDEN_TITLE: &str = "Only whitespace changed";
const MARKED: &str = "only whitespace or line endings changed";

fn files() -> Vec<ExtraFile> {
    vec![
        ExtraFile {
            path: EOL,
            base: b"fn main() {\n    println!(\"hello\");\n}\n".to_vec(),
            head: b"fn main() {\r\n    println!(\"hello\");\r\n}\r\n".to_vec(),
        },
        ExtraFile {
            path: TABS,
            base: b"fn add(a: u32, b: u32) -> u32 {\n\tlet sum = a + b;\n\tsum\n}\n".to_vec(),
            head: b"fn add(a: u32, b: u32) -> u32 {\n    let sum = a + b;\n    sum\n}\n".to_vec(),
        },
        ExtraFile {
            path: TRAILING,
            base: b"let first = 1;\nlet second = 2;\n".to_vec(),
            head: b"let first = 1;  \nlet second = 2;\n".to_vec(),
        },
        ExtraFile { path: EOF, base: b"one\ntwo".to_vec(), head: b"one\ntwo\n".to_vec() },
        ExtraFile {
            path: BOM,
            base: "\u{FEFF}one\ntwo\n".as_bytes().to_vec(),
            head: b"one\ntwo\n".to_vec(),
        },
        ExtraFile {
            path: MIXED,
            base: b"one\r\ntwo\nthree\r\n".to_vec(),
            head: b"one\ntwo\nthree\n".to_vec(),
        },
    ]
}

/// Each file and the banner over its diff.
const BANNERS: [(&str, &str); 6] = [
    (EOL, "Line endings changed from LF to CRLF (all 3 lines)."),
    (TABS, "Only whitespace changed: indentation tabs → spaces in 2 lines."),
    (TRAILING, "Only whitespace changed: trailing whitespace in 1 line."),
    (EOF, "Newline at end of file added."),
    (BOM, "Byte order mark (BOM) removed."),
    (MIXED, "Line endings changed from CRLF to LF in 2 lines."),
];

#[test]
fn whitespace_only_changes_are_shown_named_and_can_be_hidden() {
    let scratch = ScratchDir::new("whitespace");
    let host = GitHubHost::with_pull_request_files(scratch.path(), &files());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    open_files_tab(&mut harness);

    // Every whitespace-only file is a real diff under a banner that names its change.
    for (path, banner) in BANNERS {
        open_file(&mut harness, path, banner);
        assert!(!shows(&harness, "No changes to the content"), "{path} has a diff");
    }

    // The tree marks the files whose change "Ignore whitespace" would hide; a BOM is not one.
    for path in [EOL, TABS, TRAILING, EOF, MIXED] {
        assert!(
            tree_description(&harness, path).contains(MARKED),
            "{path} is marked whitespace-only"
        );
    }
    assert!(!tree_description(&harness, BOM).contains(MARKED), "the BOM file is not marked");

    // The banner's button hides the change; the notice says why the diff is empty and shows it again.
    open_file(&mut harness, EOL, BANNERS[0].1);
    harness.click_at(harness.get_by_label(HIDE_LABEL).rect());
    wait_until(&mut harness, "the hidden notice", |harness| {
        shows(harness, HIDDEN_TITLE) && !shows(harness, BANNERS[0].1)
    });
    assert!(harness.state().services.settings.ignore_whitespace);
    assert!(
        saved_settings(&harness).ignore_whitespace,
        "the switch is saved like the other diff filters"
    );
    harness.click_at(harness.get_by_label(SHOW_LABEL).rect());
    wait_until(&mut harness, "the banner again", |harness| shows(harness, BANNERS[0].1));
    assert!(!harness.state().services.settings.ignore_whitespace);

    // Alt+W and the header switch do the same.
    harness.key_press_modifiers(egui::Modifiers::ALT, egui::Key::W);
    wait_until(&mut harness, "the notice after Alt+W", |harness| shows(harness, HIDDEN_TITLE));
    harness.key_press_modifiers(egui::Modifiers::ALT, egui::Key::W);
    wait_until(&mut harness, "the banner after Alt+W", |harness| shows(harness, BANNERS[0].1));
    harness.click_at(harness.get_by_role_and_label(Role::Switch, "Ignore whitespace").rect());
    wait_until(&mut harness, "the notice after the switch", |harness| shows(harness, HIDDEN_TITLE));

    // A BOM is no whitespace to git: its diff stays, under a banner offering to show the hidden ones.
    open_file(&mut harness, BOM, BANNERS[4].1);
    assert!(shows(&harness, SHOW_LABEL));
    harness.click_at(harness.get_by_role_and_label(Role::Switch, "Ignore whitespace").rect());
    wait_until(&mut harness, "whitespace shown", |harness| {
        !harness.state().services.settings.ignore_whitespace && shows(harness, HIDE_LABEL)
    });
}

fn open_files_tab(harness: &mut Harness<'_, ReviewApp>) {
    files_tab(harness);
    wait_until(harness, "the file tree", |harness| {
        harness.query_by_role_and_label(Role::TreeItem, EOL).is_some()
    });
}

/// Selects `path` in the tree and waits for its diff under `banner`.
fn open_file(harness: &mut Harness<'_, ReviewApp>, path: &str, banner: &str) {
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, path).rect());
    wait_until(harness, banner, |harness| shows_file_view(harness, path) && shows(harness, banner));
    // Highlighting and the changed-word bands arrive a few frames later.
    harness.run_steps(30);
}

fn tree_description(harness: &Harness<'_, ReviewApp>, path: &str) -> String {
    let item = harness.get_by_role_and_label(Role::TreeItem, path);
    item.accesskit_node().description().unwrap_or_default()
}
