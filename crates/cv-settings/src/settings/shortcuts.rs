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

pub struct Shortcut {
    pub group: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// Alternatives, each one key cap.
    pub keys: &'static [&'static str],
    /// Does nothing while the assistant is turned off.
    pub needs_assistant: bool,
}

const fn entry(
    group: &'static str,
    label: &'static str,
    description: &'static str,
    keys: &'static [&'static str],
) -> Shortcut {
    Shortcut { group, label, description, keys, needs_assistant: false }
}

pub(super) const APP: &str = "Anywhere";
pub(super) const LIST: &str = "Pull request lists";
pub(super) const PULL_REQUEST: &str = "Pull request";
pub(super) const FILES: &str = "Files tab";
const WRITING: &str = "Writing a comment";
pub(super) const PIPELINES: &str = "Pipelines tab";

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

pub const SHORTCUTS: &[Shortcut] = &[
    entry(APP, "Back", BACK_DESCRIPTION, BACK_KEYS),
    entry(APP, "Forward", "Returns after Back; a new step clears it.", FORWARD_KEYS),
    entry(
        APP,
        "Find a pull request",
        "Any pull request of your connections by title or number.",
        &[shortcut!(command "K")],
    ),
    entry(
        APP,
        "Open a copied link",
        "Pasting a pull request link outside a text field opens it.",
        &[shortcut!(command "V")],
    ),
    entry(APP, "Settings", "This page.", &[shortcut!(command ",")]),
    entry(
        APP,
        "Keyboard shortcuts",
        "This list, at the shortcuts of the page you came from; outside text fields.",
        &["?"],
    ),
    entry(
        APP,
        "Search the settings",
        "While Settings is open; Esc clears the search.",
        &[shortcut!(command "F")],
    ),
    entry(
        APP,
        "Focus mode",
        "Hides everything but the current view; Esc leaves it.",
        &[shortcut!(focus_mode)],
    ),
    entry(
        APP,
        "Zoom",
        "The code over a diff or file, the whole app elsewhere.",
        &[shortcut!(command "="), shortcut!(command "-"), shortcut!(command wheel)],
    ),
    entry(APP, "Reset the zoom", "Back to 100 %.", &[shortcut!(command "0")]),
    entry(
        APP,
        "Frame statistics",
        "Frame times, for checking how smooth the app runs.",
        &[shortcut!(command shift "F")],
    ),
    entry(
        APP,
        "Open in the browser",
        "On a link that opens in Code View.",
        &[shortcut!(command click)],
    ),
    entry(LIST, "Refresh", "Loads the list again.", &["F5"]),
    entry(
        LIST,
        "Next or previous row",
        "J and K while no text field has the keys; Enter opens the focused row.",
        &["J", "K"],
    ),
    entry(
        LIST,
        "Open the Files tab",
        "Of the focused row's pull request.",
        &[shortcut!(shift Enter)],
    ),
    entry(PULL_REQUEST, "Refresh", "Loads the pull request again.", &["F5"]),
    entry(
        PULL_REQUEST,
        "Switch tabs",
        "Overview, Files, Commits and Pipelines, in this order.",
        &[
            shortcut!(command "1"),
            shortcut!(command "2"),
            shortcut!(command "3"),
            shortcut!(command "4"),
        ],
    ),
    entry(PULL_REQUEST, "Threads panel", "Shows or hides all threads.", &[shortcut!(alt "T")]),
    entry(
        PULL_REQUEST,
        "Next or previous thread",
        "While the threads panel is active.",
        &["N", "P", "↓", "↑"],
    ),
    entry(
        PULL_REQUEST,
        "Reply to the selected thread",
        "While the threads panel is active; the reply box takes the keyboard.",
        &["R"],
    ),
    entry(
        PULL_REQUEST,
        "Resolve or reopen the selected thread",
        "While the threads panel is active.",
        &[shortcut!(shift "R")],
    ),
    entry(
        PULL_REQUEST,
        "Leave the threads panel",
        "Hands the keys back to the diff; the panel stays open.",
        &["Esc"],
    ),
    entry(
        PULL_REQUEST,
        self_review::LABEL,
        "On the Overview, unless you wrote the pull request or review it already; no vote is cast.",
        &[shortcut!(alt "R")],
    ),
    Shortcut {
        needs_assistant: true,
        ..entry(
            PULL_REQUEST,
            "Ask the assistant",
            "About the lines selected in the diff, else opens it on the Overview.",
            &[shortcut!(alt "A")],
        )
    },
    Shortcut {
        needs_assistant: true,
        ..entry(
            PULL_REQUEST,
            "New assistant conversation",
            "While the assistant panel is open or you clicked into its card; Undo brings the last one back.",
            &[shortcut!(command "N")],
        )
    },
    entry(
        PULL_REQUEST,
        "Merge",
        "In the merge menu: arms the button, then Enter merges. Number keys pick a type.",
        &[shortcut!(command Enter)],
    ),
    entry(
        FILES,
        "Next or previous file",
        "J and K while no text field has the keys.",
        &["J", "K", shortcut!(alt "↓"), shortcut!(alt "↑")],
    ),
    entry(
        FILES,
        "Next or previous unviewed file",
        "Skips the viewed files in the tree's order, wrapping around once.",
        &[shortcut!(shift "J"), shortcut!(shift "K")],
    ),
    entry(
        FILES,
        "Go to file",
        "Filters the file tree; ↓ and ↑ pick a match, Enter opens it, Esc goes back.",
        &[shortcut!(command "P")],
    ),
    entry(
        FILES,
        "Next or previous change",
        "Steps through the changes of the open file.",
        &["N", "P", "F7", shortcut!(shift "F7")],
    ),
    entry(
        FILES,
        "On into the next or previous file",
        "At the last change, N again opens the next file at its first change; P mirrors it.",
        &["N", "P"],
    ),
    entry(FILES, "Mark as viewed", "The open file; again to unmark it.", &[shortcut!(alt "V")]),
    entry(
        FILES,
        "Mark the open folder viewed",
        "Every changed file below the folder whose overview shows; again to unmark them.",
        &[shortcut!(alt "V")],
    ),
    entry(
        FILES,
        "New only",
        "Switches between New only and your last layout.",
        &[shortcut!(alt "N")],
    ),
    entry(
        FILES,
        "Comments in the diff",
        "In place, or as counts beside the lines.",
        &[shortcut!(alt "H")],
    ),
    entry(
        FILES,
        "Ignore whitespace",
        "Hides changes of only spaces and line endings.",
        &[shortcut!(alt "W")],
    ),
    entry(FILES, "Wrap long lines", "In the text diff.", &[shortcut!(alt "Z")]),
    entry(FILES, "Comment", "On the line or the selected lines.", &[shortcut!(alt "C")]),
    entry(
        FILES,
        "Extend the selection",
        "Line by line.",
        &[shortcut!(shift "↑"), shortcut!(shift "↓")],
    ),
    entry(
        FILES,
        "Copy or select all",
        "In the diff and in logs.",
        &[shortcut!(command "C"), shortcut!(command "A")],
    ),
    entry(
        FILES,
        "Find in the diff",
        "In the pane with the focus; the next or previous match.",
        &[shortcut!(command "F"), "F3", shortcut!(shift "F3")],
    ),
    entry(FILES, "Match case", "Switches it while finding.", &[shortcut!(alt "C")]),
    entry(FILES, "File tree", "Shows or hides it in focus mode.", &[shortcut!(command "B")]),
    Shortcut {
        needs_assistant: true,
        ..entry(
            FILES,
            "Next or previous reference",
            "Of the answer in the assistant's mini chat, while it shows.",
            &[shortcut!(alt "."), shortcut!(alt ",")],
        )
    },
    entry(WRITING, "Post", "Sends the comment or reply.", &[shortcut!(command Enter)]),
    entry(
        WRITING,
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
        WRITING,
        "Suggestion",
        "Inserts a suggested change for the lines.",
        &[shortcut!(command "G")],
    ),
    entry(PIPELINES, "Next or previous check", "Steps through the checks.", &["J", "K"]),
    entry(PIPELINES, "Back to the checks", "From a job's log.", &["Esc"]),
    entry(
        PIPELINES,
        "Copy the log",
        "The whole log of the open job.",
        &[shortcut!(command shift "C")],
    ),
    entry(
        PIPELINES,
        "Next or previous match",
        "In the log search.",
        &[shortcut!(Enter), shortcut!(shift Enter)],
    ),
];

/// The table as rows of the Keyboard and mouse section.
pub fn rows() -> impl Iterator<Item = Row> {
    SHORTCUTS.iter().enumerate().map(|(index, shortcut)| Row {
        id: RowId::Shortcut(index),
        category: Category::Keyboard,
        label: shortcut.label,
        description: shortcut.description,
        keywords: shortcut.group,
        placement: Placement::Trailing,
    })
}

/// Where a new group starts in the section view.
pub fn group_of(index: usize) -> Option<&'static str> {
    let group = SHORTCUTS.get(index)?.group;
    let is_first =
        index == 0 || SHORTCUTS.get(index - 1).is_some_and(|before| before.group != group);
    is_first.then_some(group)
}

/// The key caps of entry `index`, right to left as the row lays them out. An assistant shortcut
/// while the assistant is off shows them inactive, after "Turned off", which leads to the switch;
/// returns whether that was clicked.
pub fn keys(ui: &mut Ui, index: usize, is_assistant_on: bool) -> bool {
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

#[cfg(test)]
mod tests {
    use super::{PULL_REQUEST, SHORTCUTS};
    use cv_base::platform::shortcut;
    use cv_services::self_review;

    #[test]
    fn the_table_lists_add_me_as_reviewer() {
        let entry = SHORTCUTS.iter().find(|entry| entry.label == self_review::LABEL);
        let entry = entry.expect("Alt+R on the Overview is in the one list of shortcuts");
        assert_eq!(entry.group, PULL_REQUEST);
        assert_eq!(entry.keys, [shortcut!(alt "R")]);
    }
}
