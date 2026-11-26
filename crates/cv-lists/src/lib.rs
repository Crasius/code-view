//! The lists the user picks a pull request from: the pull request lists ([`pull_request_list`]),
//! the overview across every connection or one repository's list, and the repository browser of a
//! connection ([`repositories`]) that opens a repository's list and stars it for the overview's
//! Favorites card. The title bar's search reads the rows these pages loaded.

#![forbid(unsafe_code)]
// RS-LINT-02: egui lays out in f32 points while counts, margins and line totals are usize, u64 or
// i8.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "egui pixel math converts between f32 points and usize indices"
)]

pub mod pull_request_list;
pub mod repositories;
