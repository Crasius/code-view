//! Every setting as one row description: its section, label, description and search keywords. The
//! section view and the search results draw the same rows from this table, so a setting is
//! described once and found where it is shown. What a row's control does lives with its section.

use std::ops::Range;

use cv_base::platform::shortcut;
use cv_base::theme::{self, style};
use cv_widgets::row;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Stroke, Ui};

use super::category::Category;
use super::shortcuts;

/// Which setting a row stands for; the page draws its control from this. A section with several
/// rows has its own enum, so its control matches every one of them and a new row cannot fall
/// through to another section's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowId {
    ConnectionList,
    Appearance(AppearanceRow),
    Merging(MergingRow),
    Assistant(AssistantRow),
    Checkout(CheckoutRow),
    ClipboardLinks,
    BrowserExtension,
    StorageUsage,
    ClearCache,
    Requests,
    /// An entry of [`shortcuts::SHORTCUTS`].
    Shortcut(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppearanceRow {
    Theme,
    DiffLayout,
    CodeFontSize,
    WrapLines,
    Comments,
    SelectionOccurrences,
    ScrollPastEnd,
    ContextLines,
    PullRequestStart,
    FilesStart,
    TreeSide,
    NextUnviewed,
    PullRequestHeader,
    RenderedMarkdown,
    CommentEditor,
    InterfaceScale,
    FrameRate,
    PaneSizes,
    FavoritesLane,
    RepositorySections,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergingRow {
    Favourites,
    Remembered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistantRow {
    Shown,
    Agent,
    ClaudeModel,
    CodexModel,
    MiniChat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutRow {
    Clones,
    Search,
    Pattern,
    Removal,
    Open,
    Terminal,
    Agents,
}

/// Where a row's control sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Right of the label, for switches and buttons.
    Trailing,
    /// Under the label, for segmented controls and sliders.
    Below,
    /// A block with cards of its own; in its section the section's title stands for its label.
    Block,
    /// A block that draws its own title and help; the search shows it without a row header.
    TitledBlock,
}

impl Placement {
    /// The row is a block of its own, not a row in a shared card.
    pub fn is_block(self) -> bool {
        matches!(self, Self::Block | Self::TitledBlock)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Row {
    pub id: RowId,
    pub category: Category,
    pub label: &'static str,
    pub description: &'static str,
    /// Words a user may search for that the label and description do not use.
    pub keywords: &'static str,
    pub placement: Placement,
}

const fn row(
    id: RowId,
    category: Category,
    label: &'static str,
    description: &'static str,
    keywords: &'static str,
    placement: Placement,
) -> Row {
    Row { id, category, label, description, keywords, placement }
}

/// The pull request header row's label and description; its block shows them as its heading.
pub const HEADER_TITLE: &str = "Pull request header";
pub const HEADER_HELP: &str = "The title and review actions above the tabs. Pick per tab how they behave; each card plays what you picked.";

const ROWS: &[Row] = &[
    row(
        RowId::ConnectionList,
        Category::Connections,
        "Connections",
        "Your accounts on GitHub, GitLab and Azure DevOps: check, edit or remove one, or add another.",
        "account host token sign in add organization group",
        Placement::Block,
    ),
    row(
        RowId::Appearance(AppearanceRow::Theme),
        Category::Appearance,
        "Theme",
        "System follows the light or dark mode of your computer.",
        "dark light mode colors color scheme",
        Placement::Below,
    ),
    row(
        RowId::Appearance(AppearanceRow::DiffLayout),
        Category::Appearance,
        "Diff layout",
        "Side by side, inline, or only the new version with the changes marked.",
        "split unified new only",
        Placement::Below,
    ),
    row(
        RowId::Appearance(AppearanceRow::CodeFontSize),
        Category::Appearance,
        "Code font size",
        "Code in diffs, files and logs.",
        "text size monospace zoom",
        Placement::Below,
    ),
    row(
        RowId::Appearance(AppearanceRow::WrapLines),
        Category::Appearance,
        "Wrap long lines",
        concat!("In the text diff. ", shortcut!(alt "Z"), " switches it in the diff."),
        "soft wrap",
        Placement::Trailing,
    ),
    row(
        RowId::Appearance(AppearanceRow::Comments),
        Category::Appearance,
        "Comments in the diff",
        concat!(
            "Off, a count beside a line or block stands for its comments and shows them on a click. ",
            shortcut!(alt "H"),
            " switches it in the diff."
        ),
        "threads inline show hide",
        Placement::Trailing,
    ),
    row(
        RowId::Appearance(AppearanceRow::SelectionOccurrences),
        Category::Appearance,
        "Highlight other occurrences of the selection",
        "Selecting a word or other text within one line in the text diff marks every other place it occurs in the file.",
        "selection highlight matches same word occurrences",
        Placement::Trailing,
    ),
    row(
        RowId::Appearance(AppearanceRow::ScrollPastEnd),
        Category::Appearance,
        "Scroll past the end",
        "The diff scrolls on until its last line is at the top of the view. Right-click the overview strip beside the diff to switch it there.",
        "scroll beyond end last line bottom room overscroll",
        Placement::Trailing,
    ),
    row(
        RowId::Appearance(AppearanceRow::ContextLines),
        Category::Appearance,
        "Context lines",
        "Unchanged lines shown around each change.",
        "hunk surrounding",
        Placement::Below,
    ),
    row(
        RowId::Appearance(AppearanceRow::FavoritesLane),
        Category::Appearance,
        "Show the Favorites lane",
        "The overview's first card, with the pull requests of your starred repositories. Off, they come first in every card instead, marked with a star; the star in the overview's header switches it too.",
        "favorites favourites starred star repositories lane card overview sort top first hide",
        Placement::Trailing,
    ),
    row(
        RowId::Appearance(AppearanceRow::RepositorySections),
        Category::Appearance,
        "Group by repository",
        "Each card of the overview lists its pull requests under a header per repository, which closes and opens its section. Off, they are listed newest first, each with its repository. The books button in the overview's header switches it too.",
        "group sections repositories overview cards headers sort collapse",
        Placement::Trailing,
    ),
    row(
        RowId::Appearance(AppearanceRow::PullRequestStart),
        Category::Appearance,
        "Pull requests open on",
        "The first time. After that, a pull request opens where you left it: the same tab and file, also after a restart.",
        "start tab overview files resume last position restore reopen",
        Placement::Below,
    ),
    row(
        RowId::Appearance(AppearanceRow::FilesStart),
        Category::Appearance,
        "Files tab opens on",
        "A link to a file or comment always opens that file.",
        "start folder view first file all files",
        Placement::Below,
    ),
    row(
        RowId::Appearance(AppearanceRow::TreeSide),
        Category::Appearance,
        "File tree side",
        "The Files tab's tree beside the diff; the button in the tree's header flips it too.",
        "file tree sidebar left right panel position",
        Placement::Below,
    ),
    row(
        RowId::Appearance(AppearanceRow::NextUnviewed),
        Category::Appearance,
        "After marking a file viewed, open the next unviewed one",
        concat!(
            "In the Files tab, ",
            shortcut!(alt "V"),
            " or the viewed mark of the open file moves on to the next file still to review. ",
            shortcut!(shift "J"),
            " opens it any time."
        ),
        "viewed next unviewed auto advance review files mark",
        Placement::Trailing,
    ),
    row(
        RowId::Appearance(AppearanceRow::PullRequestHeader),
        Category::Appearance,
        HEADER_TITLE,
        HEADER_HELP,
        "header title bar toolbar auto hide collapse minimize minimized scroll tab tabs always shown start peek space",
        Placement::TitledBlock,
    ),
    row(
        RowId::Appearance(AppearanceRow::RenderedMarkdown),
        Category::Appearance,
        "Rendered Markdown",
        "The reading column is centered; drag its edges in the preview to resize it, double-click one to reset it.",
        "markdown preview width reading column full width",
        Placement::Below,
    ),
    row(
        RowId::Appearance(AppearanceRow::CommentEditor),
        Category::Appearance,
        "Comment editor",
        "How comment boxes open, per platform: Rich text writes the comment as it will look, Markdown shows its source. The switch above a comment box changes it too.",
        "comment editor rich text wysiwyg markdown source write compose reply",
        Placement::Below,
    ),
    row(
        RowId::Appearance(AppearanceRow::InterfaceScale),
        Category::Appearance,
        "Interface scale",
        concat!(
            shortcut!(command wheel),
            " zooms the code over a diff or file and the whole app elsewhere."
        ),
        "zoom ui size",
        Placement::Below,
    ),
    row(
        RowId::Appearance(AppearanceRow::FrameRate),
        Category::Appearance,
        "Frame rate limit",
        "Caps animations, smooth scrolling and loading effects; clicks and keys always show at once. Lower limits use less power.",
        "fps power battery refresh",
        Placement::Below,
    ),
    row(
        RowId::Appearance(AppearanceRow::PaneSizes),
        Category::Appearance,
        "Pane sizes",
        "The widths you dragged the file tree, threads and other panes to.",
        "reset layout splitter",
        Placement::Trailing,
    ),
    row(
        RowId::Merging(MergingRow::Favourites),
        Category::Merging,
        "Favourite merge type",
        "Per platform; without one the button takes the platform's default, else the first allowed type.",
        "squash rebase merge commit fast-forward semi-linear strategy favorite",
        Placement::Below,
    ),
    row(
        RowId::Merging(MergingRow::Remembered),
        Category::Merging,
        "Remembered per repository",
        "Each repository's favourite, last used merge type and source branch choice.",
        "forget repositories",
        Placement::Trailing,
    ),
    row(
        RowId::Assistant(AssistantRow::Shown),
        Category::Assistant,
        "Show the assistant",
        concat!(
            "The Ask about this PR card on the Overview, ",
            shortcut!(alt "A"),
            " and Ask about these lines in the diff. Off, no agent CLI runs; conversations are kept."
        ),
        "ai enable disable turn off hide chat claude codex",
        Placement::Trailing,
    ),
    row(
        RowId::Assistant(AssistantRow::Agent),
        Category::Assistant,
        "Default agent",
        "New conversations start with it; the chip in a conversation switches.",
        "ai claude codex cli llm chat ask install",
        Placement::Below,
    ),
    row(
        RowId::Assistant(AssistantRow::ClaudeModel),
        Category::Assistant,
        "Claude Code model",
        "Fastest takes the quickest model the CLI lists; CLI default leaves the choice to its own settings.",
        "ai model haiku sonnet opus llm",
        Placement::Below,
    ),
    row(
        RowId::Assistant(AssistantRow::CodexModel),
        Category::Assistant,
        "Codex model",
        "Fastest takes the quickest model the CLI lists; CLI default leaves the choice to its own settings.",
        "ai model gpt llm",
        Placement::Below,
    ),
    row(
        RowId::Assistant(AssistantRow::MiniChat),
        Category::Assistant,
        "Keep the answer beside the code",
        "A citation opens the Files tab with a small chat in its corner: the answer, its references and a follow-up. Off, a citation only opens the file.",
        "ai mini chat floating card citation reference follow up files",
        Placement::Trailing,
    ),
    row(
        RowId::Checkout(CheckoutRow::Clones),
        Category::LocalCheckouts,
        "Your clones",
        "Found by matching the remote URL in the folders below. Add a folder to search, or pin a clone by hand.",
        "repository clone path pin pick worktrees local checkout",
        Placement::Below,
    ),
    row(
        RowId::Checkout(CheckoutRow::Search),
        Category::LocalCheckouts,
        "Search in",
        "Folders looked through for clones, and the folders inside them two levels deep.",
        "search folders find clones directory add",
        Placement::Below,
    ),
    row(
        RowId::Checkout(CheckoutRow::Pattern),
        Category::LocalCheckouts,
        "Folder for new worktrees",
        "{clone}, {number} and {branch} are filled in. A relative pattern starts beside the clone.",
        "worktree path pattern template location",
        Placement::Below,
    ),
    row(
        RowId::Checkout(CheckoutRow::Removal),
        Category::LocalCheckouts,
        "When a pull request is merged or closed elsewhere",
        "Applies when a merge or close is detected on the server, not when you merge from Code View. \
         Worktrees with uncommitted changes are never removed automatically.",
        "delete remove clean up worktree merged closed automatically ask keep",
        Placement::Below,
    ),
    row(
        RowId::Checkout(CheckoutRow::Open),
        Category::LocalCheckouts,
        "Open by default",
        "What Check out offers to open first. Ask each time starts from your last choice.",
        "terminal claude codex agent open after checkout",
        Placement::Below,
    ),
    row(
        RowId::Checkout(CheckoutRow::Terminal),
        Category::LocalCheckouts,
        "Terminal",
        "Where Terminal opens a checkout.",
        "shell console powershell git bash windows terminal",
        Placement::Below,
    ),
    row(
        RowId::Checkout(CheckoutRow::Agents),
        Category::LocalCheckouts,
        "Coding agents",
        "Claude Code and Codex as the Assistant section finds them; Check out opens them in the checkout.",
        "claude codex cli agent version install",
        Placement::Below,
    ),
    row(
        RowId::ClipboardLinks,
        Category::Links,
        "Open pull request links from the clipboard",
        "When the window comes to the front with a link to a pull request of one of your connections on the clipboard. \
         The clipboard is read only then, and nothing from it is kept but the link that opened.",
        "paste url copy",
        Placement::Trailing,
    ),
    row(
        RowId::BrowserExtension,
        Category::Links,
        super::browser_extension::LABEL,
        super::browser_extension::DESCRIPTION,
        "chrome edge browser extension native messaging host register unpacked tab redirect",
        Placement::Below,
    ),
    row(
        RowId::StorageUsage,
        Category::Storage,
        "Space used",
        "Git mirrors, pipeline logs, saved pages and viewed marks, and the limits they are kept under.",
        "disk size cache mirrors logs viewed",
        Placement::Block,
    ),
    row(
        RowId::ClearCache,
        Category::Storage,
        "Clear cache",
        "Deletes all mirrors and logs; viewed marks stay.",
        "delete free space open folder",
        Placement::Block,
    ),
    row(
        RowId::Requests,
        Category::Network,
        "Requests per connection",
        "Per minute now, sent, unchanged (304), shared and rate-limited, and the busiest endpoints.",
        "network traffic api rate limit budget endpoints background tasks",
        Placement::Block,
    ),
];

/// Every row, in section order.
pub fn all() -> impl Iterator<Item = Row> {
    ROWS.iter().copied().chain(shortcuts::rows())
}

/// The rows of one section.
pub fn of(category: Category) -> impl Iterator<Item = Row> {
    all().filter(move |row| row.category == category)
}

/// Whether every word of `query` occurs in the row's label, description, keywords or section,
/// ignoring case. An empty query matches nothing: there is nothing to look for.
pub fn matches(row: &Row, query: &str) -> bool {
    let mut words = query.split_whitespace().peekable();
    if words.peek().is_none() {
        return false;
    }
    let fields = [row.label, row.description, row.keywords, row.category.title()];
    words.all(|word| fields.iter().any(|field| !find(field, word).is_empty()))
}

/// Byte ranges of `text` where a word of `query` occurs, ignoring ASCII case, sorted and merged.
pub fn highlights(text: &str, query: &str) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> =
        query.split_whitespace().flat_map(|word| find(text, word)).collect();
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// Where `word` occurs in `text`. Only ASCII letters fold, so every match starts and ends on a
/// character boundary of `text`.
fn find(text: &str, word: &str) -> Vec<Range<usize>> {
    let (haystack, needle) = (text.as_bytes(), word.as_bytes());
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }
    haystack
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| window.eq_ignore_ascii_case(needle))
        .map(|(start, _)| start..start + needle.len())
        .collect()
}

/// `text` in `font` and `color`, the parts matching `query` on the search highlight.
pub fn highlighted(ui: &Ui, text: &str, query: &str, font: FontId, color: Color32) -> LayoutJob {
    let palette = theme::palette(ui.ctx());
    let plain = TextFormat::simple(font, color);
    let marked =
        TextFormat { background: palette.text_selection, color: palette.text, ..plain.clone() };
    let mut job = LayoutJob::default();
    let mut at = 0;
    for range in highlights(text, query) {
        job.append(text.get(at..range.start).unwrap_or_default(), 0.0, plain.clone());
        job.append(text.get(range.clone()).unwrap_or_default(), 0.0, marked.clone());
        at = range.end;
    }
    job.append(text.get(at..).unwrap_or_default(), 0.0, plain);
    job
}

/// The row's label and description, the search matches marked.
pub fn header(ui: &mut Ui, row: &Row, query: &str) {
    let palette = theme::palette(ui.ctx());
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = style::SPACE_LINE;
        let label = highlighted(
            ui,
            row.label,
            query,
            theme::fonts::semibold_font(style::BODY_SIZE),
            palette.text,
        );
        ui.add(egui::Label::new(label).wrap());
        if !row.description.is_empty() {
            let description = highlighted(
                ui,
                row.description,
                query,
                FontId::proportional(style::SMALL_SIZE),
                palette.text_secondary,
            );
            ui.add(egui::Label::new(description).wrap());
        }
    });
}

/// One row of a card: the header and the control, placed as the row says.
pub fn show(ui: &mut Ui, row: &Row, query: &str, control: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(style::SPACE_L as i8, style::SPACE_M as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            match row.placement {
                Placement::Trailing => {
                    // The control centers on the whole header, name and description.
                    row::with_trailing_block(ui, control, |ui| header(ui, row, query));
                }
                Placement::Below | Placement::Block | Placement::TitledBlock => {
                    header(ui, row, query);
                    ui.add_space(style::SPACE_S);
                    control(ui);
                }
            }
        });
}

/// A card holding rows: surface fill, a border, dividers between the rows `add` draws.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = theme::palette(ui.ctx());
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(style::RADIUS_CARD)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner
}

/// An on/off setting as a checkbox named by its row's label; the row's header shows the label.
pub fn switch(ui: &mut Ui, value: &mut bool, row: &Row) -> bool {
    let response = cv_widgets::checkbox::checkbox(ui, value, "");
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, *value, row.label));
    response.changed()
}

#[cfg(test)]
mod tests {
    use super::{Category, Placement, RowId, highlights, matches, row};

    #[test]
    fn a_query_matches_when_every_word_occurs_in_a_field_of_the_row() {
        let cache = row(
            RowId::ClearCache,
            Category::Storage,
            "Clear cache",
            "Deletes all mirrors.",
            "free space",
            Placement::Block,
        );
        let cases = [
            ("a word of the label", "cache", true),
            ("a word of the description", "mirrors", true),
            ("a keyword", "space", true),
            ("the section's title", Category::Storage.title(), true),
            ("ASCII case folds", "CLEAR Cache", true),
            ("every word of the label and the keywords", "clear free", true),
            ("one word that occurs nowhere", "clear zzz", false),
            ("a word of another row", "font", false),
            ("a blank query", "   ", false),
            ("an empty query", "", false),
        ];
        for (case, query, expected) in cases {
            assert_eq!(matches(&cache, query), expected, "{case}: {query:?}");
        }
    }

    #[test]
    fn highlights_cover_each_match_once_ignoring_case() {
        assert_eq!(highlights("Code font size", "FONT"), vec![5..9]);
        assert_eq!(highlights("Diff layout", "diff lay layout"), vec![0..4, 5..11]);
        assert_eq!(
            highlights("Größe font", "font"),
            vec![8..12],
            "byte ranges after non-ASCII text"
        );
        assert_eq!(highlights("Theme", "zzz"), Vec::<std::ops::Range<usize>>::new());
    }
}
