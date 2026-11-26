//! The change breakdown behind the Files tab's line totals, offline against the mocked GitHub
//! host ([`super::github_mock`]) with one changed file of every group: `src/lib.rs` (+2 −2) and
//! the files below.
//!
//! Hovering the file tree's totals, then the folder summary's, shows the same five rows in their
//! fixed order with the mock's numbers and the total; each row's name sits centered in its row.
//! Keyboard focus on the totals shows it too.
//! `cargo test -p review-app change_breakdown`

use egui::{Rect, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::ReviewApp;
use super::action_e2e::{app, open_pull_request, wait_until};
use super::clicks::Clicks;
use super::github_mock::{ExtraFile, GitHubHost};
use cv_testkit::ScratchDir;
use cv_widgets::{focus, row};

/// The tree's totals and the folder summary's, by their accessible names.
const TREE_TOTALS: &str = "+6 added, −3 deleted";
const SUMMARY_TOTALS: &str = "+6 −3";

/// Each group's row in its fixed order, by its name and its accessible summary.
const ROWS: [(&str, &str); 6] = [
    ("Code", "Code: 1 file, +2 −2"),
    ("Tests", "Tests: 1 file, +2 −0"),
    ("Docs", "Docs: 1 file, +0 −1"),
    ("Mockups", "Mockups: 1 file, +1 −0"),
    ("Other", "Other: 1 file, +1 −0"),
    ("Total", "Total: 5 files, +6 −3"),
];

fn file(path: &'static str, base: &str, head: &str) -> ExtraFile {
    ExtraFile { path, base: base.as_bytes().to_vec(), head: head.as_bytes().to_vec() }
}

/// Besides `src/lib.rs`: a test (+2), a doc (−1), a mockup (+1) and a manifest (+1).
fn files() -> [ExtraFile; 4] {
    [
        file(
            "tests/greet_test.rs",
            "use widgets::greeting;\n",
            "use widgets::greeting;\n\n#[test]\n",
        ),
        file("docs/guide.md", "# Guide\nSay hello.\n", "# Guide\n"),
        file("mockups/home.svg", "<svg>\n</svg>\n", "<svg>\n<rect/>\n</svg>\n"),
        file("Cargo.toml", "[package]\n", "[package]\nname = \"widgets\"\n"),
    ]
}

#[test]
fn change_breakdown_shows_on_the_tree_and_summary_totals() {
    let scratch = ScratchDir::new("change-breakdown");
    let host = GitHubHost::with_pull_request_files(scratch.path(), &files());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::Tab
                    && node.label().is_some_and(|label| label.starts_with("Files"))
            })
            .rect(),
    );
    wait_until(&mut harness, "the counted totals", |harness| {
        harness.query_by_label(TREE_TOTALS).is_some()
            && harness.query_by_label(SUMMARY_TOTALS).is_some()
    });

    harness.get_by_label(TREE_TOTALS).hover();
    wait_until(&mut harness, "the tree's breakdown", |harness| {
        harness.query_by_label(ROWS[5].1).is_some()
    });
    assert_rows(&harness);

    close(&mut harness);
    harness.get_by_label(SUMMARY_TOTALS).hover();
    wait_until(&mut harness, "the summary's breakdown", |harness| {
        harness.query_by_label(ROWS[5].1).is_some()
    });
    assert_rows(&harness);

    // Keyboard focus on the totals shows it without a pointer.
    close(&mut harness);
    focus::use_keyboard(&harness.ctx);
    harness.get_by_label(TREE_TOTALS).focus();
    wait_until(&mut harness, "the breakdown on focus", |harness| {
        harness.query_by_label(ROWS[5].1).is_some()
    });
    assert_rows(&harness);
}

/// Moves the pointer away until the breakdown closed.
fn close(harness: &mut Harness<'_, ReviewApp>) {
    harness.hover_at(egui::pos2(1.0, 1.0));
    wait_until(harness, "the breakdown to close", |harness| {
        harness.query_by_label(ROWS[5].1).is_none()
    });
}

/// The six rows top to bottom in their fixed order, each name centered in its row.
fn assert_rows(harness: &Harness<'_, ReviewApp>) {
    let mut above = f32::NEG_INFINITY;
    for (name, summary) in ROWS {
        let row_rect = harness.get_by_role_and_label(Role::Row, summary).rect();
        let name_rect = name_inside(harness, name, row_rect);
        row::assert_centered_in(row_rect, name_rect);
        assert!(row_rect.top() >= above, "{name} below the row before it");
        above = row_rect.bottom();
    }
}

/// The popover's label `name` inside `row`: other labels may carry the same word.
fn name_inside(harness: &Harness<'_, ReviewApp>, name: &str, row: Rect) -> Rect {
    harness
        .query_all_by_role_and_label(Role::Label, name)
        .map(|node| node.rect())
        .find(|rect| row.contains_rect(*rect))
        .unwrap_or_else(|| panic!("the name {name} inside its row {row:?}"))
}
