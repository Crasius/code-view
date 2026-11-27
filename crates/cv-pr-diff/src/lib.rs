//! The text diff of the Files tab's open file and of the folder summary's cards: the virtualized
//! view of the diff's rows ([`diff_view`]), its find widget ([`find`]), the overview strip beside
//! it ([`diff_overview`]), the bars of collapsed lines, the notices in its place, the new
//! comment's card, and the syntax colors worked out from the diff's own text ([`patch_colors`])
//! with the cache of recently shown diffs ([`diff_cache`]). The rendered Markdown diff, the
//! folder summary and the Files tab build on it.
#![forbid(unsafe_code)]
// RS-LINT-02: egui lays out in f32 points while rows, columns and line numbers are usize or u32.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "egui pixel math converts between f32 points and usize indices"
)]
#![cfg_attr(
    test,
    expect(
        clippy::cast_possible_wrap,
        reason = "the tests' pixel probes and synthetic threads convert indices to i64"
    )
)]

#[cfg(test)]
mod coloring_latency;
#[cfg(test)]
mod comment_handoff_check;
mod detached_threads;
pub mod diff_cache;
pub mod diff_composer;
pub mod diff_overview;
#[cfg(test)]
mod diff_selection_check;
pub mod diff_skeleton;
mod diff_status;
pub mod diff_view;
#[cfg(test)]
mod diff_view_check;
pub mod find;
#[cfg(test)]
mod frame_budget;
pub mod gap_bar;
mod h_scroll;
#[cfg(test)]
mod instant_colors_check;
mod intraline;
mod line_anchor;
pub mod notice;
pub mod patch_colors;
#[cfg(test)]
mod placed_scroll_check;
mod removal_popover;
pub mod selection;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
#[cfg(test)]
mod thread_cards_check;
pub mod warm_up;
pub mod whitespace_notice;
mod word_highlights;
