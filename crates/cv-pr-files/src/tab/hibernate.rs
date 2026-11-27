//! A pull request page out of sight keeps what the user did on it and gives back what loads again.
//! The app keeps the last few pages it showed, so going back to one is instant; while one waits in
//! the background, its Files tab drops the diffs, their colors and the file versions, which come
//! back from the local mirror in tens of milliseconds, and the preview drops what it loaded for a
//! file no longer open. The open file, the scroll position, the viewed marks and every draft stay,
//! so the page shows again where the user left it, the open file's diff loading into the same view.
//! The page's `PullRequestPage::hibernate` (`cv-pr-page`) hands down to this one.

use crate::FilesTab;

/// The body of [`FilesTab::hibernate`].
pub(crate) fn hibernate(tab: &mut FilesTab) {
    tab.diffs.hibernate();
    tab.preview.hibernate(tab.selected_path.as_deref());
}
