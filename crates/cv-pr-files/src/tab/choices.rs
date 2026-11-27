//! The diff choices the Files tab works with and saves: how diffs are worked out, the layout a
//! file shows in (added and deleted files whole), Alt+N's New only, "Ignore whitespace",
//! "Show comments in the diff" and "Scroll past the end".

use review_core::DiffSide;
use review_git::{ChangeKind, ChangedFile, DiffOptions};
use review_store::{AppSettings, DiffLayout};

use cv_code::diff_rows::Layout;
use cv_services::Services;

/// How the diff of every file is worked out, from the user's choices.
pub fn diff_options(settings: &AppSettings) -> DiffOptions {
    DiffOptions { context: settings.context_lines, ignore_whitespace: settings.ignore_whitespace }
}

/// The diff toolbar's "Ignore whitespace", from the banner, a notice or Alt+W; saved like the
/// other diff choices.
pub fn toggle_ignore_whitespace(services: &mut Services) {
    services.settings.ignore_whitespace = !services.settings.ignore_whitespace;
    services.save_settings();
}

/// Alt+H: "Show comments in the diff", saved like the other diff choices.
pub fn toggle_comments_in_diff(services: &mut Services) {
    services.settings.show_comments_in_diff = !services.settings.show_comments_in_diff;
    services.save_settings();
}

/// "Scroll past the end" switched in the overview strip's menu of the rendered diff, the text
/// diff or a thread's original version: saved for every diff.
pub fn switch_past_end(services: &mut Services, is_on: bool) {
    services.settings.scroll_past_end = is_on;
    services.save_settings();
}

/// An added file has only a head version and a deleted file only a base version: either is shown
/// whole, in one full-width pane, whatever layout the user picked.
pub fn whole_file_side(file: &ChangedFile) -> Option<DiffSide> {
    match file.kind {
        ChangeKind::Added => Some(DiffSide::New),
        ChangeKind::Deleted => Some(DiffSide::Old),
        ChangeKind::Modified
        | ChangeKind::Renamed
        | ChangeKind::Copied
        | ChangeKind::TypeChanged => None,
    }
}

/// The layout a file is shown in: whole for added and deleted files, the user's choice otherwise
/// (which stays saved, so the next changed file shows it again).
pub fn file_layout(whole_file: Option<DiffSide>, chosen: DiffLayout) -> Layout {
    match (whole_file, chosen) {
        (Some(side), _) => Layout::WholeFile(side),
        (None, DiffLayout::SideBySide) => Layout::SideBySide,
        (None, DiffLayout::Inline) => Layout::Inline,
        (None, DiffLayout::NewOnly) => Layout::NewOnly,
    }
}

/// Alt+N switches New only on and off.
pub fn toggles_new_only(ui: &egui::Ui) -> bool {
    ui.input_mut(|input| input.consume_key(egui::Modifiers::ALT, egui::Key::N))
}

/// The layout after Alt+N: into New only, or back to the layout it was entered from.
pub fn new_only_toggled(current: DiffLayout, before_new_only: DiffLayout) -> DiffLayout {
    match current {
        DiffLayout::NewOnly => before_new_only,
        DiffLayout::SideBySide | DiffLayout::Inline => DiffLayout::NewOnly,
    }
}

#[cfg(test)]
mod tests {
    use review_core::DiffSide;
    use review_store::DiffLayout;

    use super::{file_layout, new_only_toggled, toggle_comments_in_diff};
    use cv_code::diff_rows::Layout;
    use cv_testkit::ScratchDir;

    #[test]
    fn shows_added_and_deleted_files_whole_and_keeps_the_choice_for_the_rest() {
        assert_eq!(
            file_layout(Some(DiffSide::New), DiffLayout::SideBySide),
            Layout::WholeFile(DiffSide::New)
        );
        assert_eq!(
            file_layout(Some(DiffSide::Old), DiffLayout::NewOnly),
            Layout::WholeFile(DiffSide::Old)
        );
        assert_eq!(
            file_layout(None, DiffLayout::SideBySide),
            Layout::SideBySide,
            "the next modified file gets the saved layout"
        );
        assert_eq!(file_layout(None, DiffLayout::NewOnly), Layout::NewOnly);
    }

    /// Alt+H calls the toggle (`review::keys`); the header's switch flips the setting itself.
    #[test]
    fn the_comments_toggle_saves_its_choice() {
        let scratch = ScratchDir::new("comments-in-diff");
        let mut services = crate::tests::services_in(&scratch);
        assert!(services.settings.show_comments_in_diff, "comments show by default");
        for expected in [false, true] {
            toggle_comments_in_diff(&mut services);
            assert_eq!(services.settings.show_comments_in_diff, expected);
            let restarted = crate::tests::services_in(&scratch);
            assert_eq!(restarted.settings.show_comments_in_diff, expected, "saved");
        }
    }

    #[test]
    fn alt_n_enters_new_only_and_returns_to_the_previous_layout() {
        assert_eq!(
            new_only_toggled(DiffLayout::Inline, DiffLayout::SideBySide),
            DiffLayout::NewOnly
        );
        assert_eq!(new_only_toggled(DiffLayout::NewOnly, DiffLayout::Inline), DiffLayout::Inline);
        assert_eq!(
            new_only_toggled(DiffLayout::NewOnly, DiffLayout::SideBySide),
            DiffLayout::SideBySide
        );
    }
}
