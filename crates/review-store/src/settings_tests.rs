//! `settings.json` across versions: what a save keeps, how a file written before a setting existed
//! or holding a value this version cannot read loads, and what a removed connection takes along.

use std::collections::BTreeSet;
use std::path::PathBuf;

use review_core::{FileCategory, MergeStrategy, ProviderKind, PullRequestScope, StateFilter};

use crate::assistant::settings::MiniChatCorner;
use crate::checkouts::{
    CheckoutPlace, CheckoutSettings, OpenByDefault, OpenWith, RememberedCheckout, TerminalChoice,
    WhenFinished,
};
use crate::settings::{PipelineSettings, ThreadsPanel};
use crate::test_support::TestDir;
use crate::{
    AppSettings, AssistantSettings, CheckFilter, CommentEditor, CommentEditors, DiffLayout,
    Favorites, FileFilters, FilesStart, FrameRateLimit, HeaderMode, HeaderModes, ImageCompare,
    ListFilters, MergePreferences, PaneSizes, PullRequestStart, RenderedWidth, RepositorySections,
    SectionCard, SectionKey, ThemePreference, ThreadFilters, TreeSide, WindowPlacement,
};
use review_core::{AgentKind, ModelChoice};

fn settings_file(dir: &TestDir) -> PathBuf {
    dir.path().join("settings.json")
}

/// Loads `json` as a settings file.
fn load_text(dir: &TestDir, json: &str) -> AppSettings {
    let file = settings_file(dir);
    std::fs::write(&file, json).expect("write the settings file");
    AppSettings::load(&file).expect("the settings load")
}

/// Every setting away from its default. The literal names every field, so a new setting has to
/// join this round trip.
fn customized() -> AppSettings {
    let mut merge = MergePreferences::default();
    merge.set_platform_favourite(ProviderKind::GitHub, Some(MergeStrategy::Squash));
    merge.remember_last_used("gitlab", "group/app", MergeStrategy::Rebase);
    AppSettings {
        theme: ThemePreference::Dark,
        diff_layout: DiffLayout::NewOnly,
        context_lines: 10,
        ui_scale: 1.25,
        code_font_size: 15.0,
        wrap_lines: true,
        ignore_whitespace: true,
        show_comments_in_diff: false,
        show_frame_stats: true,
        frame_rate: FrameRateLimit::MatchDisplay,
        panes: PaneSizes {
            file_tree: 412.0,
            diff_split: 0.35,
            overview_side: 360.0,
            preview_split: 0.6,
            pipeline_jobs: 280.0,
            threads_panel: 420.0,
        },
        collapsed_connections: BTreeSet::from(["gitlab-work".to_owned()]),
        pipelines: PipelineSettings {
            status_filter: BTreeSet::from([CheckFilter::Failed, CheckFilter::Running]),
            is_overview_expanded: true,
        },
        file_filters: FileFilters {
            categories: BTreeSet::from([FileCategory::Code, FileCategory::Tests]),
            hide_viewed: true,
            markdown_preview: false,
            summary_preview: true,
            image_compare: ImageCompare::OnionSkin,
        },
        list_filters: ListFilters {
            scope: PullRequestScope::ReviewRequested,
            state: StateFilter::Merged,
            repository: Some(("ado-work".to_owned(), "Shop/web".to_owned())),
            repository_state: StateFilter::All,
        },
        threads_panel: ThreadsPanel {
            is_open: true,
            filters: ThreadFilters { active: false, resolved: true, unread: true, mine: true },
        },
        merge,
        // Negative coordinates: a monitor left of or above the primary one.
        window: Some(WindowPlacement {
            x: -1700,
            y: -40,
            width: 1600,
            height: 1000,
            is_maximized: true,
        }),
        focus_file_tree: true,
        favorites: customized_favorites(),
        repository_sections: customized_sections(),
        files_start: FilesStart::FirstFile,
        pull_request_start: PullRequestStart::Files,
        open_clipboard_links: false,
        last_clipboard_link: Some("https://github.com/acme/widgets/pull/7".to_owned()),
        rendered_width: RenderedWidth { full: true, reading: 900.0 },
        file_tree_side: TreeSide::Right,
        assistant: customized_assistant(),
        header_modes: HeaderModes {
            overview: HeaderMode::AlwaysShown,
            files: HeaderMode::StartMinimized,
            commits: HeaderMode::AlwaysShown,
            pipelines: HeaderMode::StartMinimized,
        },
        scroll_past_end: false,
        highlight_selection_occurrences: false,
        open_next_unviewed: false,
        comment_editors: CommentEditors {
            github: CommentEditor::Markdown,
            gitlab: CommentEditor::Markdown,
            azure_devops: CommentEditor::Rich,
        },
        checkouts: customized_checkouts(),
    }
}

fn customized_assistant() -> AssistantSettings {
    AssistantSettings {
        enabled: false,
        agent: AgentKind::Codex,
        claude_model: ModelChoice::CliDefault,
        codex_model: ModelChoice::Named("gpt-6-luna".to_owned()),
        card_height: Some(520.0),
        mini_chat: false,
        mini_chat_corner: MiniChatCorner { left: true, top: true },
    }
}

fn customized_favorites() -> Favorites {
    let mut favorites = Favorites::default();
    favorites.toggle("gh", "cli/cli");
    favorites.toggle("gitlab", "group/app");
    favorites.is_collapsed = true;
    favorites.is_lane_shown = false;
    favorites
}

/// Grouping off, with a closed section of each card.
fn customized_sections() -> RepositorySections {
    let mut sections = RepositorySections::default();
    sections.is_grouped = false;
    for (card, connection_id) in
        [(SectionCard::Favorites, "gh"), (SectionCard::Connection, "gitlab")]
    {
        let repository_key = "group/app".to_owned();
        sections.toggle(SectionKey {
            card,
            connection_id: connection_id.to_owned(),
            repository_key,
        });
    }
    sections
}

/// Local checkouts, every setting off its default.
fn customized_checkouts() -> CheckoutSettings {
    CheckoutSettings {
        search_folders: Some(vec![PathBuf::from("/dev"), PathBuf::from("/work/src")]),
        worktree_pattern: "{clone}/../reviews/{number}".to_owned(),
        when_finished: WhenFinished::Keep,
        open_by_default: OpenByDefault::Always(OpenWith::Agent(AgentKind::Codex)),
        terminal: TerminalChoice::GitBash,
        last_opened: OpenWith::Terminal,
        pinned_clones: [("gh|cli/cli".to_owned(), PathBuf::from("/dev/cli"))].into(),
        remembered: [(
            "gh|cli/cli".to_owned(),
            RememberedCheckout { place: CheckoutPlace::Clone, then: OpenWith::Nothing },
        )]
        .into(),
    }
}

#[test]
fn every_setting_survives_save_and_load() {
    let dir = TestDir::new("settings-round-trip");
    let file = settings_file(&dir);
    for settings in [AppSettings::default(), customized()] {
        settings.save(&file).expect("saved");
        assert_eq!(AppSettings::load(&file).expect("loaded"), settings);
    }
}

/// An update from a version that did not know a setting yet loads it with its default and keeps
/// everything else.
#[test]
fn a_setting_missing_from_an_older_file_loads_with_its_default() {
    let dir = TestDir::new("settings-older");
    let older = load_text(&dir, r#"{"ui_scale":1.25}"#);
    assert_eq!(older, AppSettings { ui_scale: 1.25, ..AppSettings::default() });
    assert!(older.highlight_selection_occurrences, "other occurrences highlight from the start");
    assert!(older.open_next_unviewed, "marking a file viewed opens the next unviewed one");
    assert!(older.scroll_past_end, "the diff scrolls past its last row from the start");
    assert!(older.repository_sections.is_grouped, "rows are grouped from the start");
    assert!(older.assistant.enabled, "the assistant is shown from the start");
    // A pane an older version had and this one does not.
    let gone_pane = load_text(&dir, r#"{"ui_scale":1.0,"panes":{"pipeline_runs":300.0}}"#);
    assert_eq!(gone_pane, AppSettings::default());
    let first_start = AppSettings::load(&dir.path().join("missing.json")).expect("no file is fine");
    assert_eq!(first_start, AppSettings::default());
}

/// A setting an older version saved and this one dropped (the find bar's side, now the focused
/// pane; the assistant panel's own width and the model chip's place) is ignored, and everything
/// beside it loads.
#[test]
fn a_setting_this_version_dropped_still_loads() {
    let dir = TestDir::new("settings-dropped");
    let older = load_text(
        &dir,
        r#"{"ui_scale":1.25,"diff_find_scope":"Left","wrap_lines":true,"assistant":{"panel_width":640.0,"model_in_title":false,"card_height":300.0}}"#,
    );
    let assistant = AssistantSettings { card_height: Some(300.0), ..AssistantSettings::default() };
    assert_eq!(
        older,
        AppSettings { ui_scale: 1.25, wrap_lines: true, assistant, ..AppSettings::default() }
    );
}

/// A value this version cannot read (a variant a newer version added, a hand edit) resets only the
/// setting it is in.
#[test]
fn an_unreadable_value_resets_only_its_own_setting() {
    let dir = TestDir::new("settings-unreadable");
    let unreadable = [
        r#""frame_rate":"Adaptive""#,
        r#""window":{"x":"left"}"#,
        r#""open_clipboard_links":"yes","last_clipboard_link":7"#,
        r#""focus_file_tree":"yes""#,
        r#""show_comments_in_diff":"no""#,
        r#""rendered_width":"wide""#,
        r#""favorites":{"repositories":"all"}"#,
        r#""repository_sections":{"is_grouped":"no"}"#,
        r#""files_start":"LastFile""#,
        r#""pull_request_start":"Commits""#,
        r#""file_tree_side":"Top""#,
        r#""hide_header_on_scroll":"no""#,
        r#""header_modes":{"files":"Sideways"}"#,
        r#""header_modes":"StartMinimized""#,
        r#""scroll_past_end":"yes""#,
        r#""highlight_selection_occurrences":"no""#,
        r#""open_next_unviewed":"no""#,
        r#""comment_editors":{"github":"Wysiwyg"}"#,
        r#""file_filters":{"categories":["Fonts"],"hide_viewed":true}"#,
    ];
    let expected = AppSettings { ui_scale: 1.25, ..AppSettings::default() };
    for fragment in unreadable {
        let loaded = load_text(&dir, &format!(r#"{{"ui_scale":1.25,{fragment}}}"#));
        assert_eq!(loaded, expected, "{fragment}");
    }
    let beside = r#"{"file_filters":{"categories":["Fonts"]},"list_filters":{"state":"Closed"}}"#;
    assert_eq!(load_text(&dir, beside).list_filters.state, StateFilter::Closed);
}

/// Settings saved while the tree had no side setting (also those of the build that put it on the
/// right unconditionally) open with the tree on the left, where it sat before.
#[test]
fn an_older_file_puts_the_file_tree_on_the_left() {
    let dir = TestDir::new("settings-tree-side");
    let older = load_text(&dir, r#"{"focus_file_tree":true}"#);
    assert_eq!(older.file_tree_side, TreeSide::Left);
    assert!(older.focus_file_tree, "the rest of the file loads");
}

/// Settings from before the header's modes per tab carry the one switch "Hide the header while
/// scrolling" over to every tab: off keeps the header shown everywhere, on (and a file without
/// the switch) lets it slide away everywhere. The next save writes only the modes.
#[test]
fn the_old_header_switch_carries_over_to_every_tab() {
    let dir = TestDir::new("settings-header-modes");
    let off = load_text(&dir, r#"{"hide_header_on_scroll":false,"wrap_lines":true}"#);
    assert_eq!(off.header_modes, HeaderModes::all(HeaderMode::AlwaysShown));
    assert!(off.wrap_lines, "the rest of the file loads");
    let on = load_text(&dir, r#"{"hide_header_on_scroll":true}"#);
    assert_eq!(on.header_modes, HeaderModes::all(HeaderMode::HideOnScroll));
    let missing = load_text(&dir, "{}");
    assert_eq!(missing.header_modes, HeaderModes::all(HeaderMode::HideOnScroll));

    let file = settings_file(&dir);
    off.save(&file).expect("saved");
    let saved = std::fs::read_to_string(&file).expect("read the settings file");
    assert!(!saved.contains("hide_header_on_scroll"), "the switch is gone: {saved}");
    assert_eq!(AppSettings::load(&file).expect("loaded").header_modes, off.header_modes);
    let partial = load_text(&dir, r#"{"header_modes":{"commits":"StartMinimized"}}"#);
    let commits = HeaderModes { commits: HeaderMode::StartMinimized, ..HeaderModes::default() };
    assert_eq!(partial.header_modes, commits, "a tab left out keeps its default");
}

/// A file that chose the editor for one platform keeps the defaults of the others.
#[test]
fn a_comment_editor_choice_for_one_platform_keeps_the_others() {
    let dir = TestDir::new("settings-comment-editors");
    let partial = load_text(&dir, r#"{"comment_editors":{"github":"Markdown"}}"#).comment_editors;
    assert_eq!(
        partial,
        CommentEditors { github: CommentEditor::Markdown, ..CommentEditors::default() }
    );
}

#[test]
fn a_removed_connection_takes_its_settings_along() {
    let mut settings = customized();
    settings.collapsed_connections.insert("removed".to_owned());
    settings.merge.remember_last_used("removed", "x/y", MergeStrategy::Merge);
    settings.retain_connections(|id| id == "gitlab" || id == "gitlab-work");
    assert_eq!(settings.collapsed_connections, BTreeSet::from(["gitlab-work".to_owned()]));
    assert!(!settings.favorites.contains("gh", "cli/cli"));
    assert!(settings.favorites.contains("gitlab", "group/app"));
    let sections = &settings.repository_sections;
    assert!(!sections.is_collapsed(SectionCard::Favorites, "gh", "group/app"));
    assert!(sections.is_collapsed(SectionCard::Connection, "gitlab", "group/app"));
    assert_eq!(settings.merge.last_used("removed", "x/y"), None);
    assert_eq!(settings.list_filters.repository, None, "the filter on an ado-work repository goes");
    assert_eq!(settings.list_filters.scope, PullRequestScope::ReviewRequested, "the rest stays");
}

/// The frame pacing reads the limit through `cap()`: a hand-edited `{"Fps":0}` caps at one frame
/// per second instead of an infinite frame period (which `Duration::from_secs_f64` panics on), and
/// matching the display means no cap.
#[test]
fn the_frame_rate_limit_maps_to_a_frame_cap() {
    let dir = TestDir::new("settings-frame-cap");
    let cases = [
        (r#"{"frame_rate":{"Fps":45}}"#, Some(45)),
        (r#"{"frame_rate":{"Fps":0}}"#, Some(1)),
        (r#"{"frame_rate":"MatchDisplay"}"#, None),
        ("{}", Some(45)),
    ];
    for (json, cap) in cases {
        assert_eq!(load_text(&dir, json).frame_rate.cap(), cap, "{json}");
    }
}

/// A hand-edited file may list a favorite twice or out of order: it counts once, in order.
#[test]
fn hand_edited_favorites_load_sorted_and_once() {
    let dir = TestDir::new("settings-favorites");
    let twice = r#"{"favorites":{"repositories":[{"connection_id":"gh","repository_key":"b/b"},{"connection_id":"gh","repository_key":"a/a"},{"connection_id":"gh","repository_key":"b/b"}]}}"#;
    let loaded = load_text(&dir, twice);
    let keys: Vec<&str> =
        loaded.favorites.repositories().iter().map(|f| f.repository_key.as_str()).collect();
    assert_eq!(keys, ["a/a", "b/b"]);
}
