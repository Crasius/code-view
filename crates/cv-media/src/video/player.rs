//! Plays a [`Clip`]: ffmpeg decodes from a position and writes raw RGBA frames at the clip's
//! frame rate into a pipe; a reader thread hands them over through a channel [`QUEUED`] frames
//! deep, so ffmpeg blocks, and costs nothing, while nobody takes frames (paused, or off screen).
//! Each pass the UI takes the frame due at the playback [`Clock`] and uploads it into the one
//! texture the player owns.
//!
//! Time a video was not drawn does not count: one scrolled away pauses where it was and goes on
//! from there when it comes back. [`Players`] keeps the players of the current pages; one nobody
//! drew for [`memory::UNSEEN_TEXTURE_SECONDS`] goes, with its ffmpeg and its texture.

use std::collections::HashMap;
use std::io::Read as _;
use std::process::{Child, ChildStdout, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cv_runtime::cache::memory;
use egui::{ColorImage, Context, Id, TextureHandle, TextureId, TextureOptions};

use super::Clip;
use crate::picture::{DECODER_BEHIND, UNSEEN_GAP};

/// Frames waiting in the channel at most: a little slack against a slow pass, little memory.
const QUEUED: usize = 2;
const STORE_ID: &str = "review.video.players";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Paused,
    Playing,
    Ended,
}

/// Where playback is: `at` seconds into the clip at input time `since` while it plays.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Clock {
    at: f64,
    since: Option<f64>,
}

impl Clock {
    pub fn position(self, now: f64) -> f64 {
        self.at + self.since.map_or(0.0, |since| (now - since).max(0.0))
    }

    fn play(&mut self, now: f64) {
        self.since.get_or_insert(now);
    }

    fn pause(&mut self, now: f64) {
        self.at = self.position(now);
        self.since = None;
    }

    fn seek(&mut self, to: f64, now: f64) {
        self.at = to.max(0.0);
        if self.since.is_some() {
            self.since = Some(now);
        }
    }

    /// The clip was last drawn at `drawn`: a longer gap up to `now` does not count.
    fn skip_unseen(&mut self, drawn: f64, now: f64) {
        if self.since.is_some() && now - drawn > UNSEEN_GAP {
            self.at = self.position(drawn);
            self.since = Some(now);
        }
    }
}

/// One ffmpeg run from `start` seconds and the frames it delivered so far. Dropping it drops the
/// channel: the reader thread's next hand-over fails, and the thread ends ffmpeg, off the UI
/// thread.
struct Stream {
    frames: Receiver<Vec<u8>>,
    start: f64,
    fps: f64,
    taken: u64,
}

impl Stream {
    fn spawn(clip: &Clip, start: f64) -> Option<Self> {
        let mut command = clip.command(start);
        command.arg("-vf").arg(clip.filter(Some(clip.fps)));
        command.args(["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]).stdout(Stdio::piped());
        let mut child = command.spawn().ok()?;
        let stdout = child.stdout.take()?;
        let bytes = clip.frame[0] * clip.frame[1] * 4;
        let (sender, frames) = mpsc::sync_channel(QUEUED);
        // A thread that does not start drops the pipe with it, and ffmpeg ends on the broken pipe.
        std::thread::Builder::new()
            .name("video-frames".to_owned())
            .spawn(move || read_frames(child, stdout, bytes, &sender))
            .ok()?;
        Some(Self { frames, start, fps: clip.fps, taken: 0 })
    }

    /// Clip time of the next frame to take.
    fn next_at(&self) -> f64 {
        self.start + self.taken as f64 / self.fps
    }
}

/// Hands over frames of `bytes` until the clip ends or nobody takes them, then ends ffmpeg.
fn read_frames(
    mut child: Child,
    mut stdout: ChildStdout,
    bytes: usize,
    sender: &SyncSender<Vec<u8>>,
) {
    loop {
        let mut frame = vec![0; bytes];
        if stdout.read_exact(&mut frame).is_err() || sender.send(frame).is_err() {
            break;
        }
    }
    // An ffmpeg that already ended cannot be killed; either way it is reaped.
    let _ended = child.kill();
    let _reaped = child.wait();
}

/// Plays one clip into one texture.
pub struct Player {
    clip: Arc<Clip>,
    clock: Clock,
    status: Status,
    stream: Option<Stream>,
    texture: Option<TextureHandle>,
    /// Input time of the last [`Self::update`].
    drawn: f64,
    /// A frame is wanted though playback is paused: after a seek.
    wants_frame: bool,
}

impl Player {
    pub fn new(clip: Arc<Clip>) -> Self {
        Self {
            clip,
            clock: Clock::default(),
            status: Status::Paused,
            stream: None,
            texture: None,
            drawn: 0.0,
            wants_frame: false,
        }
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn duration(&self) -> f64 {
        self.clip.duration
    }

    pub fn position(&self, now: f64) -> f64 {
        let position = self.clock.position(now);
        if self.clip.duration > 0.0 { position.min(self.clip.duration) } else { position }
    }

    /// Plays a paused clip, pauses a playing one, and plays an ended one from its start.
    pub fn toggle(&mut self, now: f64) {
        match self.status {
            Status::Playing => {
                self.clock.pause(now);
                self.status = Status::Paused;
            }
            Status::Paused => {
                self.clock.play(now);
                self.status = Status::Playing;
            }
            Status::Ended => {
                self.seek(0.0, now);
                self.clock.play(now);
                self.status = Status::Playing;
            }
        }
        self.drawn = now;
    }

    /// Goes to `to` seconds; a paused clip shows the frame there.
    pub fn seek(&mut self, to: f64, now: f64) {
        let to = if self.clip.duration > 0.0 { to.clamp(0.0, self.clip.duration) } else { to };
        self.stream = None;
        self.clock.seek(to, now);
        if self.status == Status::Ended {
            self.status = Status::Paused;
        }
        self.wants_frame = self.status != Status::Playing;
    }

    /// Takes the frame due at input time `now`: the texture to draw (`None` until the first frame
    /// arrived) and when to draw again.
    pub fn update(&mut self, ctx: &Context, now: f64) -> (Option<TextureId>, Option<Duration>) {
        self.clock.skip_unseen(self.drawn, now);
        self.drawn = now;
        let repaint = match self.status {
            Status::Playing => self.advance(ctx, now),
            Status::Paused if self.wants_frame => self.take_one(ctx, now),
            Status::Paused | Status::Ended => None,
        };
        (self.texture.as_ref().map(TextureHandle::id), repaint)
    }

    fn advance(&mut self, ctx: &Context, now: f64) -> Option<Duration> {
        let position = self.clock.position(now);
        let stream = self.stream_from(position)?;
        let mut latest = None;
        let mut has_ended = false;
        while stream.next_at() <= position {
            match stream.frames.try_recv() {
                Ok(frame) => {
                    stream.taken += 1;
                    latest = Some(frame);
                }
                // ffmpeg is behind: the next pass takes what it delivered by then.
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    has_ended = true;
                    break;
                }
            }
        }
        let next = stream.next_at();
        if let Some(frame) = latest {
            self.upload(ctx, &frame);
        }
        if has_ended {
            self.stream = None;
            self.clock.pause(now);
            self.status = Status::Ended;
            return None;
        }
        Some(Duration::from_secs_f64((next - position).max(0.004)))
    }

    /// One frame at the paused position, once ffmpeg delivers it.
    fn take_one(&mut self, ctx: &Context, now: f64) -> Option<Duration> {
        let position = self.clock.position(now);
        let stream = self.stream_from(position)?;
        match stream.frames.try_recv() {
            Ok(frame) => {
                stream.taken += 1;
                self.wants_frame = false;
                self.upload(ctx, &frame);
                None
            }
            Err(TryRecvError::Empty) => Some(DECODER_BEHIND),
            Err(TryRecvError::Disconnected) => {
                self.wants_frame = false;
                None
            }
        }
    }

    /// The running stream, or one started at `position`; `None` when ffmpeg does not start.
    fn stream_from(&mut self, position: f64) -> Option<&mut Stream> {
        if self.stream.is_none() {
            self.stream = Stream::spawn(&self.clip, position);
            if self.stream.is_none() {
                self.status = Status::Paused;
                self.wants_frame = false;
            }
        }
        self.stream.as_mut()
    }

    fn upload(&mut self, ctx: &Context, frame: &[u8]) {
        let image = ColorImage::from_rgba_unmultiplied(self.clip.frame, frame);
        match &mut self.texture {
            Some(texture) => texture.set(image, TextureOptions::LINEAR),
            None => {
                self.texture =
                    Some(ctx.load_texture("markdown-video", image, TextureOptions::LINEAR));
            }
        }
    }
}

/// The players of the videos on the current pages, by widget, with the input time each was last
/// asked for.
#[derive(Default)]
pub struct Players {
    players: Mutex<HashMap<Id, (Player, f64)>>,
}

impl Players {
    /// The store of `ctx`, made on first use.
    pub fn of(ctx: &Context) -> Arc<Self> {
        ctx.data_mut(|data| data.get_temp_mut_or_default::<Arc<Self>>(Id::unique(STORE_ID)).clone())
    }

    /// Runs `run` on the player of widget `id`, made for `clip` when it has none or another clip.
    pub fn with<T>(
        &self,
        id: Id,
        clip: &Arc<Clip>,
        now: f64,
        run: impl FnOnce(&mut Player) -> T,
    ) -> T {
        let mut players = self.players.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = players.entry(id).or_insert_with(|| (Player::new(clip.clone()), now));
        if !Arc::ptr_eq(&entry.0.clip, clip) {
            *entry = (Player::new(clip.clone()), now);
        }
        entry.1 = now;
        run(&mut entry.0)
    }

    /// Lets go of the players nobody asked for since [`memory::UNSEEN_TEXTURE_SECONDS`], in the
    /// store of `ctx` if it has one. Call once per frame.
    pub fn release_unseen(ctx: &Context) {
        let now = ctx.input(|input| input.time);
        let Some(store) = ctx.data(|data| data.get_temp::<Arc<Self>>(Id::unique(STORE_ID))) else {
            return;
        };
        let mut players = store.players.lock().unwrap_or_else(PoisonError::into_inner);
        players.retain(|_, (_, seen)| now - *seen < memory::UNSEEN_TEXTURE_SECONDS);
    }
}

#[cfg(test)]
mod tests {
    use super::Clock;

    #[test]
    fn the_clock_runs_while_playing_and_holds_while_paused() {
        let mut clock = Clock::default();
        clock.play(10.0);
        assert!((clock.position(12.5) - 2.5).abs() < 1e-9);
        clock.pause(12.5);
        assert!((clock.position(40.0) - 2.5).abs() < 1e-9, "paused, it holds");
        clock.seek(7.0, 40.0);
        clock.play(41.0);
        assert!((clock.position(42.0) - 8.0).abs() < 1e-9);
    }

    /// A video scrolled out of view goes on where it was when it comes back.
    #[test]
    fn time_off_screen_does_not_count() {
        let mut clock = Clock::default();
        clock.play(0.0);
        clock.skip_unseen(0.1, 0.12);
        assert!((clock.position(0.2) - 0.2).abs() < 1e-9, "a short pass gap counts");
        clock.skip_unseen(1.0, 9.0);
        assert!((clock.position(9.5) - 1.5).abs() < 1e-9, "eight seconds away do not");
    }
}
