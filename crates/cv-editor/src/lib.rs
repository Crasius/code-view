//! Code View's comment editor: the [`markdown_editor`] every comment, reply and description is
//! written in (Markdown with a formatting toolbar, mentions and a preview) and the rich text editor
//! it switches to, which shows the comment as it will look and writes the same Markdown.
// RS-LINT-02: egui lays out in f32 points while rows, columns and byte offsets are usize, so
// layout math converts between them (`markdown_editor/`, `rich_editor/layout.rs`); list indents
// and heading levels move between usize, isize and u8 (`markdown_editor/formatting.rs`,
// `rich_editor/edit/rules.rs`).
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "egui layout math converts between f32 points and usize indices"
)]
#![forbid(unsafe_code)]

#[cfg(test)]
mod frame_budget;
pub mod markdown_editor;
mod rich_editor;
