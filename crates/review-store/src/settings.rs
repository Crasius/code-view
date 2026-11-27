use std::collections::BTreeSet;
use std::io;
use std::path::Path;

use review_core::{FileCategory, ProviderKind, PullRequestScope, StateFilter};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::atomic_file;
use crate::checkouts::CheckoutSettings;
use crate::{AssistantSettings, Favorites, MergePreferences, RepositorySections};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DiffLayout {
    #[default]
    SideBySide,
    Inline,
    /// Only the head version, changed lines marked and removed lines behind a gutter marker.
    NewOnly,
}

/// What the Files tab shows when a pull request's files open and nothing particular was asked for
/// (a file, a comment, a folder).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FilesStart {
    /// The folder view of every changed file.
    #[default]
    AllFiles,
    /// The first file in the file tree's order.
    FirstFile,
}

/// The tab a pull request opens on the first time; after that it opens where the user left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PullRequestStart {
    #[default]
    Overview,
    /// The Files tab, on the first file in the file tree's order.
    Files,
}

/// The window edge the Files tab's file tree sits at, beside the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TreeSide {
    #[default]
    Left,
    Right,
}

impl TreeSide {
    /// The other edge.
    #[must_use]
    pub fn flipped(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// How a pull request's header (its title and review actions above the tabs) behaves on one tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HeaderMode {
    /// The header stays while the tab scrolls.
    AlwaysShown,
    /// The header slides away while the tab scrolls down and comes back from the top.
    #[default]
    HideOnScroll,
    /// The header slides away on entering the tab and peeks back on demand, never by scrolling.
    StartMinimized,
}

/// The header's [`HeaderMode`] on each tab of a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HeaderModes {
    pub overview: HeaderMode,
    pub files: HeaderMode,
    pub commits: HeaderMode,
    pub pipelines: HeaderMode,
}

impl HeaderModes {
    /// `mode` on every tab.
    #[must_use]
    pub const fn all(mode: HeaderMode) -> Self {
        Self { overview: mode, files: mode, commits: mode, pipelines: mode }
    }
}

/// Sizes of the resizable pane boundaries, as the user last dragged them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneSizes {
    /// Width of the Files tab's file tree, in points.
    pub file_tree: f32,
    /// Share of a side-by-side diff's width given to the old (left) side.
    pub diff_split: f32,
    /// Width of the overview's side column (reviewers, labels, checks), in points.
    pub overview_side: f32,
    /// Share of the image and Markdown previews' width given to the Before column.
    pub preview_split: f32,
    /// Width of the job tree, left of a job's log, in points.
    pub pipeline_jobs: f32,
    /// Width of the Files tab's all-threads panel, right of the diff, in points.
    pub threads_panel: f32,
}

impl PaneSizes {
    pub const DEFAULT: PaneSizes = PaneSizes {
        file_tree: 340.0,
        diff_split: 0.5,
        overview_side: 320.0,
        preview_split: 0.5,
        pipeline_jobs: 300.0,
        threads_panel: 380.0,
    };
}

impl Default for PaneSizes {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Which threads the all-threads panel lists. Active and Resolved pick the states (neither: both);
/// Unread and Mine narrow the list further.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent toggles of the panel, stored by name in settings.json; an enum \
              would change the file format"
)]
pub struct ThreadFilters {
    pub active: bool,
    pub resolved: bool,
    pub unread: bool,
    pub mine: bool,
}

impl ThreadFilters {
    /// No filter: every thread is listed.
    pub const ALL: Self = Self { active: false, resolved: false, unread: false, mine: false };
}

impl Default for ThreadFilters {
    fn default() -> Self {
        Self { active: true, resolved: false, unread: false, mine: false }
    }
}

/// The Files tab's all-threads panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadsPanel {
    pub is_open: bool,
    pub filters: ThreadFilters,
}

/// A status group of the pipelines tab's checks filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CheckFilter {
    Failed,
    Running,
    Succeeded,
    Neutral,
}

/// How the pipelines views were left.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PipelineSettings {
    /// Status groups the checks table shows; empty shows every check.
    pub status_filter: BTreeSet<CheckFilter>,
    /// The overview's merge-status card lists the pipelines.
    pub is_overview_expanded: bool,
}

/// How the image preview compares the two versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ImageCompare {
    /// Before and after next to each other ("2-up").
    #[default]
    SideBySide,
    Swipe,
    OnionSkin,
}

/// How often animations, scrolling and loading indicators may redraw. Input is never held back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameRateLimit {
    /// At most this many frames per second.
    Fps(u16),
    /// As often as the monitor refreshes (vsync paces the frames).
    MatchDisplay,
}

impl FrameRateLimit {
    pub const DEFAULT: FrameRateLimit = FrameRateLimit::Fps(45);
    /// The limits the settings offer, besides [`FrameRateLimit::MatchDisplay`].
    pub const PRESETS: [u16; 7] = [30, 45, 60, 90, 120, 144, 240];

    /// The frames per second to stay at or below; `None` leaves pacing to the display.
    pub fn cap(self) -> Option<u16> {
        match self {
            FrameRateLimit::Fps(fps) => Some(fps.max(1)),
            FrameRateLimit::MatchDisplay => None,
        }
    }
}

impl Default for FrameRateLimit {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The Files tab's filters and view choices: one set for every pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileFilters {
    /// Categories the file tree shows; empty means every category.
    pub categories: BTreeSet<FileCategory>,
    pub hide_viewed: bool,
    /// Markdown and SVG files show their preview rather than the text diff.
    pub markdown_preview: bool,
    /// Markdown files show rendered in the folder summary's cards too. Off by default: the cards
    /// started out as text diffs.
    pub summary_preview: bool,
    pub image_compare: ImageCompare,
}

impl Default for FileFilters {
    fn default() -> Self {
        Self {
            categories: BTreeSet::new(),
            hide_viewed: false,
            markdown_preview: true,
            summary_preview: false,
            image_compare: ImageCompare::SideBySide,
        }
    }
}

/// How wide the rendered Markdown view (preview and rendered diff) sets its text.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderedWidth {
    /// The text fills the pane; otherwise it is a centered reading column.
    pub full: bool,
    /// The reading column's text width in points at code zoom 1 (side by side, of each column).
    pub reading: f32,
}

impl RenderedWidth {
    /// About 100 characters of body text: wide enough for tables and code, short enough to read.
    pub const DEFAULT_READING: f32 = 720.0;
    pub const MIN_READING: f32 = 320.0;
}

impl Default for RenderedWidth {
    fn default() -> Self {
        Self { full: false, reading: Self::DEFAULT_READING }
    }
}

/// The pull request lists' filters: one set for every visit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ListFilters {
    /// The overview's role: everything visible, created by me or assigned to me.
    pub scope: PullRequestScope,
    /// The overview's state.
    pub state: StateFilter,
    /// The overview's repository as (connection id, repository key); `None` shows all.
    pub repository: Option<(String, String)>,
    /// The state tab of a repository's own list.
    pub repository_state: StateFilter,
}

impl Default for ListFilters {
    fn default() -> Self {
        Self {
            scope: PullRequestScope::Visible,
            state: StateFilter::Open,
            repository: None,
            repository_state: StateFilter::Open,
        }
    }
}

/// How a comment box edits: as the comment will look, or as its Markdown source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommentEditor {
    Rich,
    Markdown,
}

/// The editor comment boxes open in, per platform. Azure DevOps opens in Markdown: which HTML its
/// pull request comments keep is not checked yet, and the rich editor writes HTML for some marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommentEditors {
    pub github: CommentEditor,
    pub gitlab: CommentEditor,
    pub azure_devops: CommentEditor,
}

impl CommentEditors {
    #[must_use]
    pub fn get(self, kind: ProviderKind) -> CommentEditor {
        match kind {
            ProviderKind::GitHub => self.github,
            ProviderKind::GitLab => self.gitlab,
            ProviderKind::AzureDevOps => self.azure_devops,
        }
    }

    pub fn set(&mut self, kind: ProviderKind, editor: CommentEditor) {
        match kind {
            ProviderKind::GitHub => self.github = editor,
            ProviderKind::GitLab => self.gitlab = editor,
            ProviderKind::AzureDevOps => self.azure_devops = editor,
        }
    }
}

impl Default for CommentEditors {
    fn default() -> Self {
        Self {
            github: CommentEditor::Rich,
            gitlab: CommentEditor::Rich,
            azure_devops: CommentEditor::Markdown,
        }
    }
}

/// Where the main window was when it was last moved, resized or closed.
///
/// The rect is in physical pixels of the virtual screen (Windows screen coordinates), which
/// stay unambiguous across monitors with different DPI. It is the restored (not maximized) outer
/// rect; a snapped window keeps its snapped rect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// The window was maximized (or minimized from maximized); it opens maximized again.
    pub is_maximized: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the user's independent on/off settings, each stored by name in settings.json"
)]
pub struct AppSettings {
    pub theme: ThemePreference,
    pub diff_layout: DiffLayout,
    /// Context lines around changes; `review_git::FULL_FILE_CONTEXT` shows whole files.
    pub context_lines: u32,
    pub ui_scale: f32,
    pub code_font_size: f32,
    pub wrap_lines: bool,
    /// Diffs leave out lines that differ only in whitespace or their line ending.
    pub ignore_whitespace: bool,
    /// Comment threads show inline in the text and the rendered diff; off, a count in the gutter
    /// stands for them and shows them on a click. Settings from before it, or a value this version
    /// cannot read, show them.
    #[serde(deserialize_with = "or_shown")]
    pub show_comments_in_diff: bool,
    /// The Ctrl+Shift+F frame stats overlay.
    pub show_frame_stats: bool,
    /// Settings from before the limit existed, or with a value this version does not know, get the default.
    #[serde(deserialize_with = "or_default")]
    pub frame_rate: FrameRateLimit,
    pub panes: PaneSizes,
    /// Connection ids whose group is collapsed in the pull request overview.
    pub collapsed_connections: BTreeSet<String>,
    pub pipelines: PipelineSettings,
    /// A value this version does not know (a newer category, say) resets only these filters.
    #[serde(deserialize_with = "or_default")]
    pub file_filters: FileFilters,
    #[serde(deserialize_with = "or_default")]
    pub list_filters: ListFilters,
    /// The all-threads panel: open or not, and its filters.
    #[serde(deserialize_with = "or_default")]
    pub threads_panel: ThreadsPanel,
    /// Favourite and last used merge strategies.
    #[serde(deserialize_with = "or_default")]
    pub merge: MergePreferences,
    /// The main window's placement; `None` before the first close opens the window centered.
    #[serde(deserialize_with = "or_default")]
    pub window: Option<WindowPlacement>,
    /// Focus mode shows the Files tab's file tree beside the diff (Ctrl+B there); hidden by default.
    #[serde(deserialize_with = "or_default")]
    pub focus_file_tree: bool,
    /// Starred repositories, listed together in the overview's Favorites lane.
    #[serde(deserialize_with = "crate::overview::favorites::deserialize")]
    pub favorites: Favorites,
    /// The overview's cards group their rows by repository, and the sections the user closed.
    #[serde(deserialize_with = "or_default")]
    pub repository_sections: RepositorySections,
    /// What the Files tab opens on.
    #[serde(deserialize_with = "or_default")]
    pub files_start: FilesStart,
    /// The tab a pull request opens on until the user left it somewhere. Settings from before it,
    /// or a value this version cannot read, open on the Overview.
    #[serde(deserialize_with = "or_default")]
    pub pull_request_start: PullRequestStart,
    /// A pull request link copied in another app opens when the window gains focus. Settings from
    /// before it, or a value this version cannot read, have it on.
    #[serde(deserialize_with = "or_shown")]
    pub open_clipboard_links: bool,
    /// The pull request link last opened from the clipboard, so a restart does not open it again.
    /// Nothing else from the clipboard is kept.
    #[serde(deserialize_with = "or_default")]
    pub last_clipboard_link: Option<String>,
    /// The rendered Markdown view's reading column, or full width.
    #[serde(deserialize_with = "or_default")]
    pub rendered_width: RenderedWidth,
    /// The window edge the Files tab's file tree sits at. Settings from before it, or a value this
    /// version cannot read, keep it on the left.
    #[serde(deserialize_with = "or_default")]
    pub file_tree_side: TreeSide,
    /// The pull request assistant's default agent and models.
    #[serde(deserialize_with = "or_default")]
    pub assistant: AssistantSettings,
    /// How a pull request's header behaves on each tab. Settings from before it hold the single
    /// switch `hide_header_on_scroll` instead, which carries over to every tab; a value this
    /// version cannot read gets the default.
    #[serde(alias = "hide_header_on_scroll", deserialize_with = "header_modes_or_switch")]
    pub header_modes: HeaderModes,
    /// The text and the rendered diff scroll on past their last row until it is the only one left
    /// at the top of the view; the overview strip still measures the rows alone. On by default;
    /// settings from before it, or a value this version cannot read, have it on.
    #[serde(deserialize_with = "or_shown")]
    pub scroll_past_end: bool,
    /// Selecting a word (or any text within one line) in the diff highlights its other
    /// occurrences. On by default; settings from before it, or a value this version cannot read,
    /// have it on.
    #[serde(deserialize_with = "or_shown")]
    pub highlight_selection_occurrences: bool,
    /// Marking the open file viewed opens the next unviewed file in the tree's order. On by
    /// default; settings from before it, or a value this version cannot read, have it on.
    #[serde(deserialize_with = "or_shown")]
    pub open_next_unviewed: bool,
    /// The editor comment boxes open in, per platform; each box's mode switch changes it too.
    /// Settings from before it, or a value this version cannot read, get the defaults.
    #[serde(deserialize_with = "or_default")]
    pub comment_editors: CommentEditors,
    /// Settings › Local checkouts.
    #[serde(deserialize_with = "or_default")]
    pub checkouts: CheckoutSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            diff_layout: DiffLayout::SideBySide,
            context_lines: 3,
            ui_scale: Self::DEFAULT_UI_SCALE,
            code_font_size: Self::DEFAULT_CODE_FONT_SIZE,
            wrap_lines: false,
            ignore_whitespace: false,
            show_comments_in_diff: true,
            show_frame_stats: false,
            frame_rate: FrameRateLimit::DEFAULT,
            panes: PaneSizes::DEFAULT,
            collapsed_connections: BTreeSet::new(),
            pipelines: PipelineSettings::default(),
            file_filters: FileFilters::default(),
            list_filters: ListFilters::default(),
            threads_panel: ThreadsPanel::default(),
            merge: MergePreferences::default(),
            window: None,
            focus_file_tree: false,
            favorites: Favorites::default(),
            repository_sections: RepositorySections::default(),
            files_start: FilesStart::AllFiles,
            pull_request_start: PullRequestStart::Overview,
            open_clipboard_links: true,
            last_clipboard_link: None,
            rendered_width: RenderedWidth::default(),
            file_tree_side: TreeSide::Left,
            assistant: AssistantSettings::default(),
            header_modes: HeaderModes::default(),
            scroll_past_end: true,
            highlight_selection_occurrences: true,
            open_next_unviewed: true,
            comment_editors: CommentEditors::default(),
            checkouts: CheckoutSettings::default(),
        }
    }
}

impl AppSettings {
    pub const DEFAULT_UI_SCALE: f32 = 1.0;
    pub const DEFAULT_CODE_FONT_SIZE: f32 = 13.0;

    pub fn load(file: &Path) -> io::Result<Self> {
        let settings = atomic_file::read_json(file)?.unwrap_or_default();
        Ok(settings)
    }

    pub fn save(&self, file: &Path) -> io::Result<()> {
        atomic_file::write_json(file, self)
    }

    /// Drops per-connection state of connections that no longer exist.
    pub fn retain_connections(&mut self, is_known: impl Fn(&str) -> bool) {
        self.collapsed_connections.retain(|id| is_known(id));
        self.merge.retain_connections(&is_known);
        self.favorites.retain_connections(&is_known);
        self.repository_sections.retain_connections(&is_known);
        self.checkouts.retain_connections(&is_known);
        if self
            .list_filters
            .repository
            .as_ref()
            .is_some_and(|(connection_id, _)| !is_known(connection_id))
        {
            self.list_filters.repository = None;
        }
    }
}

/// The value, or its default where it does not parse.
pub(crate) fn or_default<'de, D: Deserializer<'de>, T: DeserializeOwned + Default>(
    deserializer: D,
) -> Result<T, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).unwrap_or_default())
}

/// The header's modes per tab; from settings of before them the switch "Hide the header while
/// scrolling" for every tab: off kept the header shown, on let it slide away. Anything else this
/// version cannot read gets the default.
fn header_modes_or_switch<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<HeaderModes, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::Bool(false) => HeaderModes::all(HeaderMode::AlwaysShown),
        serde_json::Value::Bool(true) => HeaderModes::all(HeaderMode::HideOnScroll),
        value => HeaderModes::deserialize(value).unwrap_or_default(),
    })
}

/// The flag, or true where it does not parse.
fn or_shown<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(bool::deserialize(value).unwrap_or(true))
}
