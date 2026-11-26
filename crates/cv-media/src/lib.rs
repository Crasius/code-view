//! Code View's media, below the Markdown renderer that shows them: the [`images`] store (loads,
//! decoding, textures and the disk cache), "Copy image" on the system clipboard
//! ([`image_clipboard`]), [`video`] playback through ffmpeg, [`mermaid`] diagrams drawn in a
//! renderer process, and what the renderer draws and asks about: a decoded [`picture`] and an
//! image's [`image_state`].
// RS-LINT-02: decoding and rasterizing convert between pixel sizes (u32), points (f32), texture
// sizes (usize) and seconds (f64): `images/decode.rs`, `mermaid/`, `video/`.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "image and texture math converts between pixel sizes, f32 points and seconds"
)]
#![forbid(unsafe_code)]

pub mod image_clipboard;
pub mod image_state;
pub mod images;
pub mod mermaid;
pub mod picture;
pub mod video;
