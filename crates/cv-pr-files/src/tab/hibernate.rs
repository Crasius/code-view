//! A pull request page out of sight keeps what the user did on it and gives back what loads again.
//! The app keeps the last few pages it showed, so going back to one is instant; while one waits in
//! the background, its Files tab drops the diffs, their colors and the file versions, which come
//! back from the local mirror in tens of milliseconds, and the preview drops what it loaded for a
//! file no longer open. The open file, the scroll position, the viewed marks and every draft stay,
//! so the page shows again where the user left it, the open file's diff loading into the same view.
//! The page's own impl, which hands down to this one, lives in `cv-pr-page` (`hibernate.rs`).

use cv_runtime::cache::memory::Hibernate;

use crate::FilesTab;

impl Hibernate for FilesTab {
    fn hibernate(&mut self) {
        self.diffs.hibernate();
        self.preview.hibernate(self.selected_path.as_deref());
    }
}
