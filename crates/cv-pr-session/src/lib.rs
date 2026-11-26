//! One pull request as every part of its page sees it: the loaded detail, threads, runs, files and
//! mirror ([`session`]), the user's writes ([`actions`], [`lifecycle`]), the merge strategies and
//! auto-complete the merge button offers ([`merge_choice`], [`auto_complete`], [`merge_paint`]),
//! where the links and images of its Markdown go ([`links`], [`images`], [`web_link`]), the
//! loading log of its changes ([`prepare_progress`]), and the mailboxes through which the diff and
//! the Files tab reach the assistant ([`assistant_link`]). The tabs, the threads, the assistant and
//! the pipelines build on it; it names none of them.

#![forbid(unsafe_code)]
// RS-LINT-02: egui lays out in f32 points while counts and indices are usize.
#![expect(
    clippy::cast_precision_loss,
    reason = "egui pixel math converts between f32 points and usize indices"
)]

pub mod actions;
pub mod assistant_link;
pub mod auto_complete;
pub mod images;
pub mod lifecycle;
pub mod links;
pub mod merge_choice;
pub mod merge_paint;
pub mod prepare_progress;
pub mod session;
pub mod web_link;
