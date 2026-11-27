//! A loaded image as the renderer draws it: one texture, or an animation (GIF, animated WebP or
//! PNG). A short animation keeps every frame as a texture with how long it shows; a long one
//! ([`Picture::streamed`]) is decoded one frame after another by a thread of its own into one
//! texture, so it plays at its full size whatever its length. The animation loops from its first
//! frame once it is first drawn and repaints only when the next frame is due; time it was not drawn
//! does not count. Under reduced motion it stays on its first frame, as browsers that honour the
//! setting do.
//!
//! A picture also keeps the body it was decoded from, when it has one: textures are scaled down,
//! and "Copy image" hands over the image at its full resolution.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use egui::{ColorImage, TextureHandle, TextureId, TextureOptions, Vec2};

use crate::image_clipboard::Original;

/// Frame delays below this play at [`SHORT_DELAY_SECONDS`]: browsers treat 0 and 10 ms as "the
/// author did not say" (a GIF quirk), so they play at the speed the author saw.
const SHORTEST_DELAY_SECONDS: f64 = 0.02;
const SHORT_DELAY_SECONDS: f64 = 0.1;
/// A streamed animation or a video not drawn for longer than this was off screen: the time does
/// not count.
pub(crate) const UNSEEN_GAP: f64 = 0.25;
/// How soon to look again when the decoder has not delivered the next frame yet.
pub(crate) const DECODER_BEHIND: Duration = Duration::from_millis(16);

/// The frames of a long animation, each with its delay in seconds, as a decoder delivers them,
/// looping.
pub(crate) type Frames = Receiver<(ColorImage, f64)>;

#[derive(Clone)]
pub struct Picture {
    frames: Arc<[TextureHandle]>,
    /// Seconds into one loop at which each frame ends; empty for a still or streamed image.
    ends: Arc<[f64]>,
    feed: Option<Arc<Mutex<Feed>>>,
    original: Option<Original>,
}

/// Where a streamed animation is.
struct Feed {
    frames: Frames,
    options: TextureOptions,
    /// Animation time at which the frame in the texture ends.
    shown_until: f64,
    /// Elapsed time of the last draw, and how much of the elapsed time did not count.
    drawn: Option<f64>,
    skipped: f64,
}

impl Picture {
    pub fn still(texture: TextureHandle) -> Self {
        Self { frames: Arc::new([texture]), ends: Arc::new([]), feed: None, original: None }
    }

    /// An animation of `frames`, each shown for its delay in seconds.
    pub(crate) fn animation(frames: Vec<(TextureHandle, f64)>) -> Self {
        let mut end = 0.0;
        let ends = frames
            .iter()
            .map(|(_, delay)| {
                end += playable_delay(*delay);
                end
            })
            .collect();
        let frames = frames.into_iter().map(|(texture, _)| texture).collect();
        Self { frames, ends, feed: None, original: None }
    }

    /// A long animation: `texture` holds its first frame, and `frames` delivers every frame from
    /// the first on, looping; each replaces the texture's pixels (with `options`) when it is due.
    pub(crate) fn streamed(
        texture: TextureHandle,
        frames: Frames,
        options: TextureOptions,
    ) -> Self {
        let feed = Feed { frames, options, shown_until: 0.0, drawn: None, skipped: 0.0 };
        Self {
            frames: Arc::new([texture]),
            ends: Arc::new([]),
            feed: Some(Arc::new(Mutex::new(feed))),
            original: None,
        }
    }

    /// The same picture, keeping `original`, the body it was decoded from.
    #[must_use]
    pub fn with_original(self, original: Option<Original>) -> Self {
        Self { original, ..self }
    }

    /// The body it was decoded from; `None` for a picture drawn here (a diagram).
    pub fn original(&self) -> Option<&Original> {
        self.original.as_ref()
    }

    pub(crate) fn textures(&self) -> &[TextureHandle] {
        &self.frames
    }

    /// The size of its frames in pixels.
    pub fn size_vec2(&self) -> Vec2 {
        self.frames[0].size_vec2()
    }

    /// The frame to draw `elapsed` seconds after the animation started, and how long it stays;
    /// `None` for a still image or one held still.
    pub fn frame_at(&self, elapsed: Option<f64>) -> (TextureId, Option<Duration>) {
        let Some(elapsed) = elapsed else {
            return (self.frames[0].id(), None);
        };
        if let Some(feed) = &self.feed {
            let mut feed = feed.lock().unwrap_or_else(PoisonError::into_inner);
            let mut texture = self.frames[0].clone();
            return (texture.id(), feed.advance(&mut texture, elapsed));
        }
        let Some(&total) = self.ends.last() else {
            return (self.frames[0].id(), None);
        };
        let into_loop = elapsed.max(0.0).rem_euclid(total);
        let index = self.ends.partition_point(|end| *end <= into_loop).min(self.ends.len() - 1);
        let left = (self.ends[index] - into_loop).max(0.001);
        (self.frames[index].id(), Some(Duration::from_secs_f64(left)))
    }
}

impl Feed {
    /// Puts the frame due at `elapsed` into `texture`; how long until the next one is due, `None`
    /// once the decoder stopped (a damaged frame): the last frame stays.
    fn advance(&mut self, texture: &mut TextureHandle, elapsed: f64) -> Option<Duration> {
        if let Some(drawn) = self.drawn
            && elapsed - drawn > UNSEEN_GAP
        {
            self.skipped += elapsed - drawn;
        }
        self.drawn = Some(elapsed);
        let now = elapsed - self.skipped;
        let mut latest = None;
        let mut wait = None;
        while self.shown_until <= now {
            match self.frames.try_recv() {
                Ok((image, delay)) => {
                    // A pass that came late shows the frame due now, not every one it missed.
                    self.shown_until = self.shown_until.max(now - 1.0) + playable_delay(delay);
                    latest = Some(image);
                }
                Err(TryRecvError::Empty) => {
                    wait = Some(DECODER_BEHIND);
                    break;
                }
                Err(TryRecvError::Disconnected) => break,
            }
        }
        if let Some(image) = latest {
            texture.set(image, self.options);
        }
        if self.shown_until > now {
            return Some(Duration::from_secs_f64((self.shown_until - now).max(0.001)));
        }
        wait
    }
}

fn playable_delay(seconds: f64) -> f64 {
    if seconds < SHORTEST_DELAY_SECONDS { SHORT_DELAY_SECONDS } else { seconds }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use egui::{Color32, ColorImage, Context, TextureOptions};

    use super::Picture;

    fn textures(ctx: &Context, count: usize) -> Vec<egui::TextureHandle> {
        (0..count)
            .map(|index| {
                let image = ColorImage::filled([2, 2], Color32::WHITE);
                ctx.load_texture(format!("frame-{index}"), image, TextureOptions::LINEAR)
            })
            .collect()
    }

    #[test]
    fn plays_each_frame_for_its_delay_and_loops() {
        let ctx = Context::default();
        let frames = textures(&ctx, 3);
        let ids: Vec<_> = frames.iter().map(egui::TextureHandle::id).collect();
        let picture = Picture::animation(frames.into_iter().zip([0.5, 0.25, 0.25]).collect());
        let at = |seconds: f64| picture.frame_at(Some(seconds));
        assert_eq!(at(0.0), (ids[0], Some(Duration::from_millis(500))));
        assert_eq!(at(0.6).0, ids[1]);
        assert_eq!(at(0.75).0, ids[2], "a frame starts where the one before ends");
        assert_eq!(at(1.1).0, ids[0], "the loop starts over");
        assert_eq!(picture.frame_at(None), (ids[0], None), "held still on the first frame");
    }

    #[test]
    fn zero_delays_play_at_the_speed_browsers_use() {
        let ctx = Context::default();
        let frames = textures(&ctx, 2);
        let ids: Vec<_> = frames.iter().map(egui::TextureHandle::id).collect();
        let picture = Picture::animation(frames.into_iter().zip([0.0, 0.01]).collect());
        assert_eq!(picture.frame_at(Some(0.05)).0, ids[0]);
        assert_eq!(picture.frame_at(Some(0.15)).0, ids[1]);
    }

    #[test]
    fn a_still_image_asks_for_no_repaint() {
        let ctx = Context::default();
        let texture = textures(&ctx, 1).remove(0);
        let id = texture.id();
        assert_eq!(Picture::still(texture).frame_at(Some(3.0)), (id, None));
    }

    /// A long animation shows each delivered frame in its one texture for the frame's delay, waits
    /// for a decoder that is behind, and does not count the time it was off screen.
    #[test]
    fn a_streamed_animation_plays_frames_as_they_are_due() {
        let ctx = Context::default();
        let texture = textures(&ctx, 1).remove(0);
        let id = texture.id();
        let (sender, frames) = mpsc::sync_channel(8);
        let picture = Picture::streamed(texture, frames, TextureOptions::LINEAR);
        let frame = |shade: u8| (ColorImage::filled([2, 2], Color32::from_gray(shade)), 0.5);
        // Milliseconds until the next draw.
        let next = |seconds: f64| {
            let (texture, next) = picture.frame_at(Some(seconds));
            assert_eq!(texture, id, "one texture throughout");
            next.map(|next| (next.as_secs_f64() * 1000.0).round())
        };
        assert_eq!(next(0.0), Some(16.0), "nothing delivered yet: look again soon");
        sender.send(frame(1)).unwrap();
        sender.send(frame(2)).unwrap();
        assert_eq!(next(0.01), Some(490.0), "the first frame shows for its delay");
        assert_eq!(next(0.2), Some(300.0), "still the first");
        assert_eq!(next(0.4), Some(100.0), "draws closer together than a gap count");
        assert_eq!(next(0.6), Some(400.0), "the second");
        assert_eq!(next(30.0), Some(400.0), "time off screen does not count");
        drop(sender);
        assert_eq!(next(30.2), Some(200.0));
        assert_eq!(next(30.4), None, "a stopped decoder holds the last frame");
        assert_eq!(picture.frame_at(None), (id, None));
    }
}
