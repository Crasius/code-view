//! The rendered views of a changed file: the rendered Markdown diff ([`markdown_diff`]), the
//! before/after preview of images, SVG and Markdown ([`preview`]) and the header above the open
//! file with its layout, preview and viewed controls ([`file_header`]). The Files tab and the
//! folder summary show them beside the text diff.
#![forbid(unsafe_code)]
// RS-LINT-02: egui lays out in f32 points while blocks, lines and pixels are usize or u32.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "egui pixel math converts between f32 points and usize indices"
)]

pub mod file_header;
pub mod markdown_diff;
pub mod preview;
