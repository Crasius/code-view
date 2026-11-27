//! The parts of the Files tab around the file view: its layout (the frame and the panes beside
//! the file view, `layout`), its diffs and their colors, the diff choices it saves, how the user
//! works through the files (`review`: keys, viewed marks, stepping), the folder summary and the
//! original version in its place, the notices, the binary download, what a rendered Markdown diff
//! asks for (`rendered`), the summary's diff source, the file it opens on (`start`, and `resume`
//! for the place the user left it at), its breadcrumb levels and what it lets go while its page is
//! out of sight (`hibernate`). The tab itself, [`super::FilesTab`], ties them together.

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
