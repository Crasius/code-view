//! The notices that stand in for the open file's diff: it failed to load, the file is binary, or
//! git showed no changed lines. Each returns what the user picked; the platform link opens here.

use review_git::{ChangeKind, ChangedFile};

use cv_base::theme::{self, icons};
use cv_pr_diff::whitespace_notice;
use cv_pr_session::web_link;
use cv_widgets::states::{self, Action};

/// What the user picked on the notice of a diff without changed lines.
pub enum NoChanges {
    /// Turn "Ignore whitespace" off: every change was whitespace.
    ShowWhitespace,
    /// Reload the pull request: the file list is older than the compared commits.
    Reload,
}

/// A diff without changed lines. Content that is the same on both sides (a rename, a mode
/// change, an empty file) says so; other content has changes git did not show: all of them
/// whitespace while "Ignore whitespace" is on, or none in the compared commits because the file
/// list is older than they are.
pub fn without_changes(
    ui: &mut egui::Ui,
    file: &ChangedFile,
    ignores_whitespace: bool,
    web_link: Option<&web_link::WebLink>,
) -> Option<NoChanges> {
    if file.has_same_content() {
        unchanged(ui, file, web_link);
        return None;
    }
    if ignores_whitespace {
        return match whitespace_notice::hidden_notice(ui, web_link) {
            Some(true) => Some(NoChanges::ShowWhitespace),
            Some(false) => {
                open_link(ui, web_link);
                None
            }
            None => None,
        };
    }
    let palette = theme::palette(ui.ctx());
    let reload = Action { icon: icons::ARROW_CLOCKWISE, label: "Reload", is_primary: true };
    let detail = "The file list was made for other commits than the ones compared now. Reload to bring it up to date.";
    show_with_link(
        ui,
        icons::FILE_TEXT,
        palette.text_secondary,
        "No changes in this comparison",
        detail,
        Some(reload),
        web_link,
    )
    .then_some(NoChanges::Reload)
}

/// "Couldn't load the diff" with the reason: returns true for Retry.
pub fn diff_failure(
    ui: &mut egui::Ui,
    message: &str,
    web_link: Option<&web_link::WebLink>,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let retry = Action { icon: icons::ARROW_CLOCKWISE, label: "Retry", is_primary: true };
    let title = "Couldn't load the diff";
    show_with_link(ui, icons::WARNING, palette.error, title, message, Some(retry), web_link)
}

/// A notice with its `primary` action, if it has one, and the platform `link` after it. Opens the
/// link when it is picked; returns true when the primary action is.
fn show_with_link(
    ui: &mut egui::Ui,
    icon: &str,
    color: egui::Color32,
    title: &str,
    detail: &str,
    primary: Option<Action<'_>>,
    link: Option<&web_link::WebLink>,
) -> bool {
    let has_primary = primary.is_some();
    let mut actions: Vec<Action<'_>> = primary.into_iter().collect();
    if let Some(link) = link {
        actions.push(Action { icon: link.icon, label: &link.label, is_primary: false });
    }
    match states::notice(ui, icon, color, title, detail, &actions) {
        Some(0) if has_primary => true,
        Some(_) => {
            open_link(ui, link);
            false
        }
        None => false,
    }
}

/// "Binary file" with the platform link and Download: returns true for Download.
pub fn binary(
    ui: &mut egui::Ui,
    is_downloading: bool,
    web_link: Option<&web_link::WebLink>,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let mut actions = Vec::with_capacity(2);
    if let Some(link) = web_link {
        actions.push(Action { icon: link.icon, label: &link.label, is_primary: true });
    }
    let download_label = if is_downloading { "Downloading…" } else { "Download" };
    actions.push(Action {
        icon: icons::DOWNLOAD_SIMPLE,
        label: download_label,
        is_primary: web_link.is_none(),
    });
    let detail = "Its content is not text, so there are no lines to compare.";
    match states::notice(ui, icons::FILE, palette.text_secondary, "Binary file", detail, &actions) {
        Some(index) if index + 1 == actions.len() => true,
        Some(_) => {
            open_link(ui, web_link);
            false
        }
        None => false,
    }
}

/// A changed file whose content is the same on both sides: a pure rename or copy, an empty file
/// added or deleted, or a mode change.
fn unchanged(ui: &mut egui::Ui, file: &ChangedFile, web_link: Option<&web_link::WebLink>) {
    let palette = theme::palette(ui.ctx());
    let detail = match (&file.old_path, file.kind) {
        (Some(old_path), ChangeKind::Copied) => {
            format!("Copied from {old_path} with the same content.")
        }
        (Some(old_path), _) => format!("Renamed from {old_path} with the same content."),
        (None, ChangeKind::Added) => "The new file is empty.".to_owned(),
        (None, ChangeKind::Deleted) => "The deleted file was empty.".to_owned(),
        (None, ChangeKind::TypeChanged) => "Only the file type changed.".to_owned(),
        (None, ChangeKind::Modified | ChangeKind::Renamed | ChangeKind::Copied) => {
            "Only the file mode changed.".to_owned()
        }
    };
    let title = "No changes to the content";
    show_with_link(ui, icons::FILE_TEXT, palette.text_secondary, title, &detail, None, web_link);
}

pub fn open_link(ui: &egui::Ui, link: Option<&web_link::WebLink>) {
    if let Some(link) = link {
        ui.ctx().open_url(egui::OpenUrl::new_tab(&link.url));
    }
}
