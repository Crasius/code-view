//! What an image of a rendered text is right now ([`ImageState`], [`ImageFailure`]), and the
//! counter that moves whenever an image's size became known or changed ([`image_generation`]).
//!
//! It lives here, beside the image store, video and mermaid that produce these states, and not in
//! the Markdown renderer that draws them: the producers sit below the renderer, so they name the
//! states from this crate and the renderer reads them from here.

use std::sync::Arc;
use std::time::Duration;

use egui::{Context, Id, TextureId, Vec2};

use crate::video::Clip;

const GENERATION_ID: &str = "review.markdown.image_generation";

/// What an image is right now.
#[derive(Clone)]
pub enum ImageState {
    /// Not on screen yet; `size` (points) when an earlier load told it.
    Waiting {
        size: Option<Vec2>,
    },
    /// `size` is its natural size in points: one image pixel per screen pixel (SVG at its own size).
    /// `texture` is the frame to draw now; an animation's next frame is due in `next_frame`. A
    /// video's texture is its first frame, and `clip` what its player plays.
    Ready {
        texture: TextureId,
        size: Vec2,
        next_frame: Option<Duration>,
        clip: Option<Arc<Clip>>,
    },
    Failed(Arc<ImageFailure>),
}

/// Why an image is not shown, for its card.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageFailure {
    /// "Not found (404)", "Larger than 20 MB", …
    pub reason: String,
    /// The host or repository path it was asked from.
    pub place: String,
    pub can_retry: bool,
    /// The image is private and the connection does not reach it: the card offers to fix its token.
    pub needs_access: bool,
    /// A video that needs ffmpeg, which is not installed: the card offers its install command.
    pub needs_ffmpeg: bool,
}

/// Bumped whenever an image's size became known or changed: views that keep laid-out heights
/// (the rendered diff) measure again when it moves.
pub fn image_generation(ctx: &Context) -> u64 {
    ctx.data(|data| data.get_temp::<u64>(Id::unique(GENERATION_ID))).unwrap_or(0)
}

/// Called by the image source the Markdown renderer loads through (`cv_markdown::ImageSource`)
/// when an image arrived or failed.
pub fn bump_image_generation(ctx: &Context) {
    ctx.data_mut(|data| *data.get_temp_mut_or_default::<u64>(Id::unique(GENERATION_ID)) += 1);
}
