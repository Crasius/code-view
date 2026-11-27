//! Settings like a user, offline (two connections answered by a scripted provider): Ctrl+Comma
//! opens it on Connections, each section shows only its own rows, the search finds rows across
//! sections and offers the way back when nothing matches, the Appearance sample follows the diff
//! layout, and a connection's row opens its facts. In a 960 px wide window the clip check finds
//! nothing cut, overlapping or off-window in any section.
//! `cargo test -p review-app --bin CodeView app::settings_walkthrough`

use egui::{Key, Modifiers, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{Connection, ProviderKind};
use review_store::{DiffLayout, TreeSide};

use super::ReviewApp;
use super::github_mock::{GitHubHost, assert_tree_at, long_file, open_long_file};
use super::scripted_platforms::platforms;
use super::walkthrough::{
    app, app_with, has, saved_paths, saved_settings, wait_until, wait_until_still,
};
use cv_runtime::route::Route;
use cv_testkit::{Clicks, ScratchDir, samples};

/// Each section's title in the sidebar and a text only that section shows.
const SECTIONS: [(&str, &str); 7] = [
    ("Connections", "One connection per account and host."),
    ("Appearance", "Code font size"),
    ("Merging", "Favourite merge type"),
    ("Links", "Open pull request links from the clipboard"),
    ("Storage", "Open folder"),
    ("Network activity", "Background tasks running: 0"),
    ("Keyboard and mouse", "Find a pull request"),
];

#[test]
fn sections_search_previews_and_connections() {
    let scratch = ScratchDir::new("settings-walk");
    let mut harness = harness(&scratch, 1280.0);
    wait_until(&mut harness, "the pull request list", |harness| {
        harness.state().route == Route::MyPullRequests
    });

    harness.key_press_modifiers(Modifiers::COMMAND, Key::Comma);
    wait_until(&mut harness, "Settings", |harness| harness.state().route == Route::Settings);
    assert_only(&harness, "Connections");

    for (title, _) in SECTIONS {
        open(&mut harness, title);
        assert_only(&harness, title);
        let has_preview = has(&harness, "Live sample") || has(&harness, "Merge button preview");
        assert_eq!(
            has_preview,
            matches!(title, "Appearance" | "Merging"),
            "{title}: preview column"
        );
    }

    // The search: Ctrl+F, a word, the matching rows across sections.
    search(&mut harness, "font");
    assert!(has(&harness, "Code font size"), "the Appearance row");
    assert!(has(&harness, "1 setting matches “font”"));
    assert!(!has(&harness, "Open folder"), "nothing from Storage");
    assert_eq!(harness.query_all_by_label("Storage").count(), 1, "only the sidebar entry");

    // Esc in the field clears it; the section is back.
    harness.key_press(Key::Escape);
    wait_for_no_results(&mut harness, "font");
    assert_only(&harness, "Keyboard and mouse");

    // No match: the empty state, and "Clear search" leads back.
    search(&mut harness, "zzz");
    assert!(has(&harness, "No setting matches “zzz”"));
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Clear search").rect());
    wait_for_no_results(&mut harness, "zzz");
    assert_only(&harness, "Keyboard and mouse");

    // The Appearance sample follows the diff layout.
    open(&mut harness, "Appearance");
    assert!(harness.query_by_role_and_label(Role::Label, "Side by side").is_some());
    harness.click_at(harness.get_by_role_and_label(Role::RadioButton, "Inline").rect());
    wait_until(&mut harness, "the sample to be inline", |harness| {
        harness.query_by_role_and_label(Role::Label, "Inline").is_some()
    });
    assert!(harness.query_by_role_and_label(Role::Label, "Side by side").is_none());

    // A connection's row opens its facts with Check again, Edit and Remove.
    open(&mut harness, "Connections");
    assert!(!has(&harness, "Check again"));
    open_connection(&mut harness);
    assert!(has(&harness, "Edit") && has(&harness, "Remove…"));
    assert!(has(&harness, "github.com"), "the host among the facts");
}

/// At 960 px the preview moves above the rows, and no section cuts, overlaps or loses anything.
#[cfg(debug_assertions)]
#[test]
fn sections_fit_a_narrow_window() {
    let scratch = ScratchDir::new("settings-narrow");
    let mut harness = harness(&scratch, 960.0);
    super::clip_check::install(&harness.ctx);
    harness.state_mut().navigator.go(Route::Settings);
    wait_until(&mut harness, "Settings", |harness| harness.state().route == Route::Settings);
    let mut findings = Vec::new();
    for (title, _) in SECTIONS {
        open(&mut harness, title);
        findings.extend(
            super::clip_check::inspect(&mut harness)
                .into_iter()
                .map(|finding| format!("{title}: {finding}")),
        );
    }
    open(&mut harness, "Connections");
    open_connection(&mut harness);
    findings.extend(super::clip_check::inspect(&mut harness));
    assert!(findings.is_empty(), "layout problems:\n{}", findings.join("\n"));
}

/// At the default code size every sample line shows whole in its pane, in each diff layout, at
/// 1280 px and at the narrowest window the app allows (`main.rs`, `with_min_inner_size`).
#[test]
fn sample_lines_fit_their_panes() {
    const MIN_WIDTH: f32 = 960.0;
    let scratch = ScratchDir::new("settings-sample");
    for (width, name) in [(1280.0, "1280"), (MIN_WIDTH, "min")] {
        let mut harness = harness(&scratch, width);
        harness.state_mut().navigator.go(Route::Settings);
        wait_until(&mut harness, "Settings", |harness| harness.state().route == Route::Settings);
        open(&mut harness, "Appearance");
        for layout in [DiffLayout::SideBySide, DiffLayout::Inline, DiffLayout::NewOnly] {
            let settings = &mut harness.state_mut().services.settings;
            settings.diff_layout = layout;
            settings.wrap_lines = false;
            settings.show_comments_in_diff = true;
            show_sample(&mut harness);
            let slack =
                cv_settings::settings::sample_slack(&harness.ctx).expect("the sample showed");
            assert!(slack >= 0.0, "{layout:?} at {name}: a line is cut by {:.1} pt", -slack);
        }
    }
}

/// "File tree side" in Appearance: Right is saved, and the Files tab then puts the tree right. The
/// search finds the row by the words people use for it.
#[test]
fn file_tree_side_moves_the_tree() {
    let scratch = ScratchDir::new("settings-tree-side");
    let host = GitHubHost::with_pull_request_files(scratch.path(), &[long_file()]);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Comma);
    wait_until(&mut harness, "Settings", |harness| harness.state().route == Route::Settings);

    for word in ["tree", "sidebar", "left", "right"] {
        search(&mut harness, word);
        assert!(has(&harness, "File tree side"), "{word:?} finds the row");
        harness.key_press(Key::Escape);
        wait_for_no_results(&mut harness, word);
    }

    open(&mut harness, "Appearance");
    harness.get_by_label("File tree side").scroll_to_me();
    let right = in_view(&mut harness, Role::RadioButton, "Right");
    harness.click_at(right);
    wait_until(&mut harness, "Right to be saved", |harness| {
        saved_settings(harness).file_tree_side == TreeSide::Right
    });

    harness.key_press_modifiers(Modifiers::ALT, Key::ArrowLeft);
    wait_until(&mut harness, "the pull request list", |harness| {
        harness.state().route == Route::MyPullRequests
    });
    open_long_file(&mut harness);
    assert_tree_at(&harness, TreeSide::Right);
}

/// Every on/off row's checkbox centers on the row's name and description together. It used to
/// center on a button's height at the row's top, beside the name only.
#[test]
fn checkboxes_center_on_their_rows() {
    // Section, the row's name, the start of its description.
    const SWITCHES: [(&str, &str, &str); 3] = [
        ("Appearance", "Wrap long lines", "In the text diff."),
        ("Appearance", "Comments in the diff", "Off, a count beside a line"),
        ("Links", "Open pull request links from the clipboard", "When the window comes to"),
    ];
    let scratch = ScratchDir::new("settings-switches");
    let mut harness = harness(&scratch, 1280.0);
    harness.state_mut().navigator.go(Route::Settings);
    wait_until(&mut harness, "Settings", |harness| harness.state().route == Route::Settings);
    for (section, name, description) in SWITCHES {
        open(&mut harness, section);
        harness.get_by_role_and_label(Role::CheckBox, name).scroll_to_me();
        let control = in_view(&mut harness, Role::CheckBox, name).center().y;
        let block = harness
            .get_by_role_and_label(Role::Label, name)
            .rect()
            .union(harness.get_by_label_contains(description).rect())
            .center()
            .y;
        assert!((control - block).abs() <= 1.0, "{name}: box at {control}, text block at {block}");
    }
}

/// Shows the sample: beside the rows it is always in view, above them it is at the top.
fn show_sample(harness: &mut Harness<'_, ReviewApp>) {
    wait_until(harness, "the live sample", |harness| {
        harness.query_by_label("Live sample").is_some()
    });
    harness.get_by_label("Live sample").scroll_to_me();
    let window = harness.ctx.content_rect();
    wait_until_still(harness, "the live sample's top in view", |harness| {
        let sample = harness.query_by_label("Live sample")?.rect();
        window.contains(sample.left_top()).then_some(sample)
    });
}

/// Waits until the widget of `role` labelled `label`, scrolled to, rests inside the window, and
/// returns where it is.
fn in_view(harness: &mut Harness<'_, ReviewApp>, role: Role, label: &str) -> egui::Rect {
    let window = harness.ctx.content_rect();
    wait_until_still(harness, &format!("{label} in view"), |harness| {
        let rect = harness.query_by_role_and_label(role, label)?.rect();
        window.contains_rect(rect).then_some(rect)
    })
}

/// Ctrl+F, then `word` typed into the search, once the search field has the keys.
fn search(harness: &mut Harness<'_, ReviewApp>, word: &str) {
    harness.key_press_modifiers(Modifiers::COMMAND, Key::F);
    wait_until(harness, "the search field to take the keys", |harness| {
        harness.ctx.text_edit_focused()
    });
    harness.event(egui::Event::Text(word.to_owned()));
    let quoted = format!("“{word}”");
    wait_until(harness, &format!("the results for {quoted}"), |harness| has(harness, &quoted));
}

/// Waits until the results for `word` are gone: the search was cleared.
fn wait_for_no_results(harness: &mut Harness<'_, ReviewApp>, word: &str) {
    let quoted = format!("“{word}”");
    wait_until(harness, &format!("the results for {quoted} to go"), |harness| {
        !has(harness, &quoted)
    });
}

/// Clicks the Contoso GitHub connection's row and waits for its facts.
fn open_connection(harness: &mut Harness<'_, ReviewApp>) {
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Contoso GitHub").rect());
    wait_until(harness, "the connection's facts", |harness| has(harness, "Check again"));
}

/// The section's own text shows, every other section's does not.
fn assert_only(harness: &Harness<'_, ReviewApp>, shown: &str) {
    for (title, marker) in SECTIONS {
        assert_eq!(has(harness, marker), title == shown, "{marker:?} with {shown} open");
    }
}

/// Clicks the section `title` in the sidebar and waits for the text only it shows.
fn open(harness: &mut Harness<'_, ReviewApp>, title: &str) {
    let (_, marker) =
        SECTIONS.into_iter().find(|(section, _)| *section == title).expect("a section");
    harness.click_at(harness.get_by_role_and_label(Role::Tab, title).rect());
    wait_until(harness, &format!("the {title} section"), |harness| has(harness, marker));
}

/// The app on `scratch` with a GitHub and a GitLab connection, in a window `width` wide.
fn harness(scratch: &ScratchDir, width: f32) -> Harness<'static, ReviewApp> {
    let connections = [
        ("gh", "Contoso GitHub", ProviderKind::GitHub, "https://github.com"),
        ("gl", "Tabletop", ProviderKind::GitLab, "https://gitlab.com"),
    ]
    .map(|(id, name, kind, base_url)| Connection {
        name: name.to_owned(),
        token: Some("scripted".to_owned()),
        ..samples::connection(id, kind, base_url)
    });
    let builder = Harness::builder().with_size([width, 900.0]).with_step_dt(1.0 / 60.0);
    app_with(builder, saved_paths(scratch, &connections), platforms(None))
}
