//! Loads a video ([`Target::video`]): its body is kept as a plain file for ffmpeg, downloaded once
//! (up to [`video::MAX_BYTES`]) and read again from disk on every later load; the retention sweep
//! removes it 14 days after it was last used. The loaded video shows its first frame as a still
//! picture and carries the [`video::Clip`] its player plays.

use std::path::Path;

use egui::{Context, TextureOptions, vec2};
use review_core::http::image::Validators;

use super::{Fetch, Fetched, Loaded, Natural, Target, disk, fetch_remote, git_body};
use crate::image_state::ImageFailure;
use crate::picture::Picture;
use crate::video::{self, Tools};

pub(super) async fn load(
    ctx: &Context,
    target: &Target,
    file: &Path,
) -> Result<Loaded, ImageFailure> {
    let fail = |reason: String, can_retry: bool| ImageFailure {
        reason,
        place: target.place.clone(),
        can_retry,
        needs_access: false,
        needs_ffmpeg: false,
    };
    let Some(tools) = Tools::locate() else {
        return Err(ImageFailure { needs_ffmpeg: true, ..fail(video::MISSING.to_owned(), true) });
    };
    if is_kept(file) {
        disk::touch(file);
    } else {
        let bytes = match &target.fetch {
            Fetch::Blocked(reason) => return Err(fail(reason.clone(), false)),
            Fetch::Remote { url, owner, .. } => {
                match fetch_remote(url, owner.as_ref(), None, &Validators::NONE, video::MAX_BYTES)
                    .await
                    .map_err(|failure| failure.card(&fail))?
                {
                    Fetched::Changed { bytes, .. } => bytes,
                    Fetched::Unchanged => {
                        return Err(fail("The host's answer could not be read".to_owned(), true));
                    }
                }
            }
            Fetch::Git { git, commit, path } => git_body(git, commit, path, video::MAX_BYTES)
                .await
                .map_err(|(reason, retry)| fail(reason, retry))?,
        };
        keep(file, &bytes).map_err(|reason| fail(reason, true))?;
    }
    let (clip, poster) = video::prepare(&tools, file).await.map_err(|reason| {
        // A body ffmpeg cannot read is not kept: a fixed upload loads again.
        let _kept = std::fs::remove_file(file);
        fail(reason, false)
    })?;
    let texture = ctx.load_texture(
        format!("markdown-video-{:016x}", target.key),
        poster,
        TextureOptions::LINEAR,
    );
    let [width, height] = clip.natural;
    Ok(Loaded {
        picture: Picture::still(texture),
        natural: Natural::Pixels(vec2(width as f32, height as f32)),
        stale: None,
        clip: Some(clip),
    })
}

fn is_kept(file: &Path) -> bool {
    std::fs::metadata(file).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

/// Writes the body whole or not at all.
fn keep(file: &Path, bytes: &[u8]) -> Result<(), String> {
    disk::write_whole(file, bytes).map_err(|_| "The video could not be kept on disk".to_owned())
}
