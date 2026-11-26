//! Where the Files tab opens and what it saves for the next time. The file the user left open
//! opens again while it is still changed and nothing else was asked for; without one, "Pull
//! requests open on: Files" opens the first file, else "Files tab opens on" decides. The place
//! (tab and open file) is saved with the viewed marks on every change, and the grammar of the
//! file the tab opens on compiles first.

use std::sync::Arc;

use review_git::ChangedFile;
use review_store::{AppSettings, FilesStart, LastTab, PullRequestStart};

use super::review::viewed::ViewedMarks;
use super::start::Start;
use crate::FilesTab;
use cv_services::Services;

/// What the tab knows from the last visit and the settings when the pull request opened.
pub struct Resume {
    /// The file open when the user last left the pull request, until the files first show.
    file: Option<String>,
    /// "Pull requests open on: Files": without a file to resume, the first file opens rather than
    /// the "Files tab opens on" choice.
    opens_first_file: bool,
}

impl Resume {
    pub fn new(viewed: &ViewedMarks, settings: &AppSettings) -> Self {
        Self {
            file: viewed.last_place().and_then(|place| place.file.clone()),
            opens_first_file: settings.pull_request_start == PullRequestStart::Files,
        }
    }

    /// "Files tab opens on", unless the pull request opens on its first file.
    pub fn choice(&self, choice: FilesStart) -> FilesStart {
        if self.opens_first_file { FilesStart::FirstFile } else { choice }
    }
}

/// The tab the user left the pull request on, if they ever left it.
pub fn last_tab(tab: &FilesTab) -> Option<LastTab> {
    tab.viewed.last_place().map(|place| place.tab)
}

/// Saves where the user is: `shown`, and the open file once the tab chose it (until then, the file
/// to resume stays saved).
pub fn remember_place(tab: &mut FilesTab, services: &mut Services, shown: LastTab) {
    let file = if tab.start == Start::Undecided { &tab.resume.file } else { &tab.selected_path };
    tab.viewed.remember_place(services, shown, file.as_deref());
}

/// The first time the files show: the file the user left open opens again while it is still
/// changed and nothing else was asked for. One that left the list falls back to the tab's default
/// without a hint; nothing the user did took it away.
pub fn open_left_file(tab: &mut FilesTab, files: &[ChangedFile]) {
    let Some(path) = tab.resume.file.take() else {
        return;
    };
    if tab.start != Start::Undecided || tab.selected_path.is_some() {
        return;
    }
    if let Some(index) = files.iter().position(|file| file.path == path) {
        tab.tree.scroll_to(index);
        tab.selected_path = Some(path);
    }
}

/// Once per list of changed files: compiles the grammars of all their languages in the
/// background, the file the tab opens first first, so the first file of each colors at once.
pub fn warm_up_grammars(tab: &mut FilesTab, files: &Arc<Vec<ChangedFile>>, choice: FilesStart) {
    if tab.diffs.is_warmed(files) {
        return;
    }
    let first = first_file(tab, files, choice);
    tab.diffs.warm_up(files, first);
}

/// The file the tab opens on (or has open): one asked for, else the one the user left open, else
/// the first file when the choice says so.
fn first_file(
    tab: &mut FilesTab,
    files: &Arc<Vec<ChangedFile>>,
    choice: FilesStart,
) -> Option<String> {
    if let Some(path) = &tab.pending_reveal {
        return Some(path.clone());
    }
    if tab.selected_path.is_some() || tab.start != Start::Undecided {
        return tab.selected_path.clone();
    }
    if let Some(path) = &tab.resume.file
        && files.iter().any(|file| file.path == *path)
    {
        return Some(path.clone());
    }
    if tab.resume.choice(choice) != FilesStart::FirstFile {
        return None;
    }
    let viewed = &tab.viewed;
    tab.tree.refresh(files, &|file| viewed.is_viewed(file), viewed.version());
    tab.tree.file_order().next().map(|index| files[index].path.clone())
}
