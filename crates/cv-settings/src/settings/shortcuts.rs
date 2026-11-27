//! Settings → Keyboard and mouse: the app's shortcuts, read-only. This table is the one list; each
//! entry names a shortcut a module of the app handles, labelled through [`shortcut!`] so macOS
//! reads its own symbols. A shortcut of the assistant stays listed while the assistant is off,
//! its keys inactive beside a link to the switch: the list still says what the keys would do.

use cv_base::platform::{self, shortcut};
use cv_base::theme::{self, style};
use cv_services::self_review;
use cv_widgets::buttons;
use egui::{Stroke, Ui, vec2};

use super::category::Category;
use super::rows::{Placement, Row, RowId};

/// The groups of the table, in the order the section shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutGroup {
    App,
    List,
    PullRequest,
    Files,
    Writing,
    Pipelines,
}

impl ShortcutGroup {
    /// The heading of the group, also a search keyword of its rows.
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::App => "Anywhere",
            Self::List => "Pull request lists",
            Self::PullRequest => "Pull request",
            Self::Files => "Files tab",
            Self::Writing => "Writing a comment",
            Self::Pipelines => "Pipelines tab",
        }
    }
}

pub(crate) struct Shortcut {
    pub group: ShortcutGroup,
    pub label: &'static str,
    pub description: &'static str,
    /// Alternatives, each one key cap.
    pub keys: &'static [&'static str],
    /// Does nothing while the assistant is turned off.
    pub needs_assistant: bool,
}

const fn entry(
    group: ShortcutGroup,
    label: &'static str,
    description: &'static str,
    keys: &'static [&'static str],
) -> Shortcut {
    Shortcut { group, label, description, keys, needs_assistant: false }
}

const BACK_DESCRIPTION: &str = "The previous page, or the previous tab of a pull request.";
#[cfg(not(target_os = "macos"))]
const BACK_KEYS: &[&str] = &[platform::BACK_SHORTCUT, "Mouse 4"];
#[cfg(not(target_os = "macos"))]
const FORWARD_KEYS: &[&str] = &[platform::FORWARD_SHORTCUT, "Mouse 5"];
/// macOS also takes Option and Command with the arrows (outside text fields, where they move the
/// cursor), and a two-finger swipe where the system's "Swipe between pages" is on.
#[cfg(target_os = "macos")]
const BACK_KEYS: &[&str] = &[
    platform::BACK_SHORTCUT,
    shortcut!(command "←"),
    shortcut!(alt "←"),
    "Mouse 4",
    "Swipe right with two fingers",
];
#[cfg(target_os = "macos")]
const FORWARD_KEYS: &[&str] = &[
    platform::FORWARD_SHORTCUT,
    shortcut!(command "→"),
    shortcut!(alt "→"),
    "Mouse 5",
    "Swipe left with two fingers",
];

pub(crate) const SHORTCUTS: &[Shortcut] = &[
    entry(ShortcutGroup::App, "Back", BACK_DESCRIPTION, BACK_KEYS),
    entry(ShortcutGroup::App, "Forward", "Returns after Back; a new step clears it.", FORWARD_KEYS),
    entry(
        ShortcutGroup::App,
        "Find a pull request",
        "Any pull request of your connections by title or number.",
        &[shortcut!(command "K")],
    ),
    entry(
        ShortcutGroup::App,
        "Open a copied link",
        "Pasting a pull request link outside a text field opens it.",
        &[shortcut!(command "V")],
    ),
    entry(ShortcutGroup::App, "Settings", "This page.", &[shortcut!(command ",")]),
    entry(
        ShortcutGroup::App,
        "Keyboard shortcuts",
        "This list, at the shortcuts of the page you came from; outside text fields.",
        &["?"],
    ),
    entry(
        ShortcutGroup::App,
        "Search the settings",
        "While Settings is open; Esc clears the search.",
        &[shortcut!(command "F")],
    ),
    entry(
        ShortcutGroup::App,
        "Focus mode",
        "Hides everything but the current view; Esc leaves it.",
        &[shortcut!(focus_mode)],
    ),
    entry(
        ShortcutGroup::App,
        "Zoom",
        "The code over a diff or file, the whole app elsewhere.",
        &[shortcut!(command "="), shortcut!(command "-"), shortcut!(command wheel)],
    ),
    entry(ShortcutGroup::App, "Reset the zoom", "Back to 100 %.", &[shortcut!(command "0")]),
    entry(
        ShortcutGroup::App,
        "Frame statistics",
        "Frame times, for checking how smooth the app runs.",
        &[shortcut!(command shift "F")],
    ),
    entry(
        ShortcutGroup::App,
        "Open in the browser",
        "On a link that opens in Code View.",
        &[shortcut!(command click)],
    ),
    entry(ShortcutGroup::List, "Refresh", "Loads the list again.", &["F5"]),
    entry(
        ShortcutGroup::List,
        "Next or previous row",
        "J and K while no text field has the keys; Enter opens the focused row.",
        &["J", "K"],
    ),
    entry(
        ShortcutGroup::List,
        "Open the Files tab",
        "Of the focused row's pull request.",
        &[shortcut!(shift Enter)],
    ),
    entry(ShortcutGroup::PullRequest, "Refresh", "Loads the pull request again.", &["F5"]),
    entry(
        ShortcutGroup::PullRequest,
        "Switch tabs",
        "Overview, Files, Commits and Pipelines, in this order.",
        &[
            shortcut!(command "1"),
            shortcut!(command "2"),
            shortcut!(command "3"),
            shortcut!(command "4"),
        ],
    ),
    entry(
        ShortcutGroup::PullRequest,
        "Threads panel",
        "Shows or hides all threads.",
        &[shortcut!(alt "T")],
    ),
    entry(
        ShortcutGroup::PullRequest,
        "Next or previous thread",
        "While the threads panel is active.",
        &["N", "P", "↓", "↑"],
    ),
    entry(
        ShortcutGroup::PullRequest,
        "Reply to the selected thread",
        "While the threads panel is active; the reply box takes the keyboard.",
        &["R"],
    ),
    entry(
        ShortcutGroup::PullRequest,
        "Resolve or reopen the selected thread",
        "While the threads panel is active.",
        &[shortcut!(shift "R")],
    ),
    entry(
        ShortcutGroup::PullRequest,
        "Leave the threads panel",
        "Hands the keys back to the diff; the panel stays open.",
        &["Esc"],
    ),
    entry(
        ShortcutGroup::PullRequest,
        self_review::LABEL,
        "On the Overview, unless you wrote the pull request or review it already; no vote is cast.",
        &[shortcut!(alt "R")],
    ),
    Shortcut {
        needs_assistant: true,
        ..entry(
            ShortcutGroup::PullRequest,
            "Ask the assistant",
            "About the lines selected in the diff, else opens it on the Overview.",
            &[shortcut!(alt "A")],
        )
    },
    Shortcut {
        needs_assistant: true,
        ..entry(
            ShortcutGroup::PullRequest,
            "New assistant conversation",
            "While the assistant panel is open or you clicked into its card; Undo brings the last one back.",
            &[shortcut!(command "N")],
        )
    },
    entry(
        ShortcutGroup::PullRequest,
        "Merge",
        "In the merge menu: arms the button, then Enter merges. Number keys pick a type.",
        &[shortcut!(command Enter)],
    ),
    entry(
        ShortcutGroup::Files,
        "Next or previous file",
        "J and K while no text field has the keys.",
        &["J", "K", shortcut!(alt "↓"), shortcut!(alt "↑")],
    ),
    entry(
        ShortcutGroup::Files,
        "Next or previous unviewed file",
        "Skips the viewed files in the tree's order, wrapping around once.",
        &[shortcut!(shift "J"), shortcut!(shift "K")],
    ),
    entry(
        ShortcutGroup::Files,
        "Go to file",
        "Filters the file tree; ↓ and ↑ pick a match, Enter opens it, Esc goes back.",
        &[shortcut!(command "P")],
    ),
    entry(
        ShortcutGroup::Files,
        "Next or previous change",
        "Steps through the changes of the open file.",
        &["N", "P", "F7", shortcut!(shift "F7")],
    ),
    entry(
        ShortcutGroup::Files,
        "On into the next or previous file",
        "At the last change, N again opens the next file at its first change; P mirrors it.",
        &["N", "P"],
    ),
    entry(
        ShortcutGroup::Files,
        "Mark as viewed",
        "The open file; again to unmark it.",
        &[shortcut!(alt "V")],
    ),
    entry(
        ShortcutGroup::Files,
        "Mark the open folder viewed",
        "Every changed file below the folder whose overview shows; again to unmark them.",
        &[shortcut!(alt "V")],
    ),
    entry(
        ShortcutGroup::Files,
        "New only",
        "Switches between New only and your last layout.",
        &[shortcut!(alt "N")],
    ),
    entry(
        ShortcutGroup::Files,
        "Comments in the diff",
        "In place, or as counts beside the lines.",
        &[shortcut!(alt "H")],
    ),
    entry(
        ShortcutGroup::Files,
        "Ignore whitespace",
        "Hides changes of only spaces and line endings.",
        &[shortcut!(alt "W")],
    ),
    entry(ShortcutGroup::Files, "Wrap long lines", "In the text diff.", &[shortcut!(alt "Z")]),
    entry(
        ShortcutGroup::Files,
        "Comment",
        "On the line or the selected lines.",
        &[shortcut!(alt "C")],
    ),
    entry(
        ShortcutGroup::Files,
        "Extend the selection",
        "Line by line.",
        &[shortcut!(shift "↑"), shortcut!(shift "↓")],
    ),
    entry(
        ShortcutGroup::Files,
        "Copy or select all",
        "In the diff and in logs.",
        &[shortcut!(command "C"), shortcut!(command "A")],
    ),
    entry(
        ShortcutGroup::Files,
        "Find in the diff",
        "In the pane with the focus; the next or previous match.",
        &[shortcut!(command "F"), "F3", shortcut!(shift "F3")],
    ),
    entry(ShortcutGroup::Files, "Match case", "Switches it while finding.", &[shortcut!(alt "C")]),
    entry(
        ShortcutGroup::Files,
        "File tree",
        "Shows or hides it in focus mode.",
        &[shortcut!(command "B")],
    ),
    Shortcut {
        needs_assistant: true,
        ..entry(
            ShortcutGroup::Files,
            "Next or previous reference",
            "Of the answer in the assistant's mini chat, while it shows.",
            &[shortcut!(alt "."), shortcut!(alt ",")],
        )
    },
    entry(
        ShortcutGroup::Writing,
        "Post",
        "Sends the comment or reply.",
        &[shortcut!(command Enter)],
    ),
    entry(
        ShortcutGroup::Writing,
        "Bold, italic, code, link",
        "Formats the selection.",
        &[
            shortcut!(command "B"),
            shortcut!(command "I"),
            shortcut!(command "E"),
            shortcut!(command "K"),
        ],
    ),
    entry(
        ShortcutGroup::Writing,
        "Suggestion",
        "Inserts a suggested change for the lines.",
        &[shortcut!(command "G")],
    ),
    entry(
        ShortcutGroup::Pipelines,
        "Next or previous check",
        "Steps through the checks.",
        &["J", "K"],
    ),
    entry(ShortcutGroup::Pipelines, "Back to the checks", "From a job's log.", &["Esc"]),
    entry(
        ShortcutGroup::Pipelines,
        "Copy the log",
        "The whole log of the open job.",
        &[shortcut!(command shift "C")],
    ),
    entry(
        ShortcutGroup::Pipelines,
        "Next or previous match",
        "In the log search.",
        &[shortcut!(Enter), shortcut!(shift Enter)],
    ),
];

/// The table as rows of the Keyboard and mouse section.
pub(crate) fn rows() -> impl Iterator<Item = Row> {
    SHORTCUTS.iter().enumerate().map(|(index, shortcut)| Row {
        id: RowId::Shortcut(index),
        category: Category::Keyboard,
        label: shortcut.label,
        description: shortcut.description,
        keywords: shortcut.group.title(),
        placement: Placement::Trailing,
    })
}

/// Where a new group starts in the section view.
pub(crate) fn group_of(index: usize) -> Option<ShortcutGroup> {
    let group = SHORTCUTS.get(index)?.group;
    let is_first =
        index == 0 || SHORTCUTS.get(index - 1).is_some_and(|before| before.group != group);
    is_first.then_some(group)
}

/// The key caps of entry `index`, right to left as the row lays them out. An assistant shortcut
/// while the assistant is off shows them inactive, after "Turned off", which leads to the switch;
/// returns whether that was clicked.
pub(crate) fn keys(ui: &mut Ui, index: usize, is_assistant_on: bool) -> bool {
    let Some(shortcut) = SHORTCUTS.get(index) else {
        return false;
    };
    ui.spacing_mut().item_spacing.x = style::SPACE_XS;
    let is_off = shortcut.needs_assistant && !is_assistant_on;
    ui.add_enabled_ui(!is_off, |ui| {
        for key in shortcut.keys.iter().rev() {
            key_cap(ui, key);
        }
    });
    if !is_off {
        return false;
    }
    ui.add_space(style::SPACE_S);
    buttons::text_link(ui, "Turned off", "Settings → Assistant → Show the assistant").clicked()
}

fn key_cap(ui: &mut Ui, key: &str) {
    let palette = theme::palette(ui.ctx());
    let galley = ui.painter().layout_no_wrap(
        key.to_owned(),
        egui::FontId::proportional(style::SMALL_SIZE),
        palette.text,
    );
    let size = galley.size() + vec2(style::SPACE_S * 2.0, style::SPACE_XS);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, key));
    ui.painter().rect(
        rect,
        style::RADIUS_CONTROL,
        palette.surface_subtle,
        Stroke::new(1.0, palette.border_strong),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, palette.text);
}
