//! The parts of the Files tab around the file view: its layout (the frame and the panes beside
//! the file view, `layout`), its diffs and their colors, the diff choices it saves, how the user
//! works through the files (`review`: keys, viewed marks, stepping), the folder summary and the
//! original version in its place, the notices, the binary download, what a rendered Markdown diff
//! asks for (`rendered`), the summary's diff source, the file it opens on (`start`, and `resume`
//! for the place the user left it at), its breadcrumb levels and what it lets go while its page is
//! out of sight (`hibernate`). The tab itself, [`super::FilesTab`], ties them together; `probe`
//! reads its open file for other crates' walkthroughs.

pub mod choices;
pub mod download;
pub mod file_diffs;
pub(crate) mod hibernate;
pub mod layout;
pub mod notices;
pub mod origin;
pub mod rendered;
pub mod resume;
pub mod review;
pub mod source;
pub mod start;
pub mod summary_pane;

/// What the open file shows, read by other crates' walkthroughs. Free functions, since a second
/// `impl FilesTab` outside `lib.rs` is denied (`clippy::multiple_inherent_impl`).
#[cfg(any(test, feature = "test-support"))]
pub mod probe {
    use std::sync::Arc;

    use cv_pr_rendered::preview::Preview;
    use review_git::FileDiff;

    use crate::FilesTab;

    /// The open file's path.
    pub fn selected_path(tab: &FilesTab) -> Option<&str> {
        tab.selected_path.as_deref()
    }

    /// The rendered diffs of Markdown and image files.
    pub fn preview(tab: &FilesTab) -> &Preview {
        &tab.preview
    }

    /// The open file's diff, once it loaded.
    pub fn ready_diff(tab: &FilesTab) -> Option<&Arc<FileDiff>> {
        tab.diffs.ready()
    }

    /// The open file's versions are loaded and colored.
    pub fn coloring_is_done(tab: &FilesTab) -> bool {
        tab.diffs.coloring_is_done()
    }

    /// The text diff's top, sideways offsets and build of the rows ([`DiffView::kept_place`]).
    ///
    /// [`DiffView::kept_place`]: cv_pr_diff::diff_view::DiffView::kept_place
    pub fn kept_place(tab: &FilesTab) -> (f32, [f32; 2], u64) {
        tab.view.kept_place()
    }
}
