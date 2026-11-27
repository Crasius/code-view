//! Videos in rendered Markdown, played through ffmpeg run as a process of its own (a sidecar): the
//! app links no decoder, a crash stays in ffmpeg's process, and videos play once ffmpeg is
//! installed, beside `CodeView` or on `PATH` ([`Tools::locate`]). Without it a video shows a card
//! with the install command.
//!
//! A video is a file on disk, kept next to the image cache ([`crate::images`] writes it).
//! [`prepare`] reads its size, length and frame rate with `ffprobe` and draws its first frame as
//! the poster; [`player`] streams the frames while it plays. Every run reads only that one file in
//! a known container (`-protocol_whitelist file`, `-format_whitelist`), so a crafted file cannot
//! make ffmpeg open addresses or other files, and no run decodes sound.

pub mod player;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use egui::ColorImage;

/// Videos larger than this are not downloaded: our budget.
pub(crate) const MAX_BYTES: u64 = 100 * 1024 * 1024;
/// Longest edge of the frames ffmpeg delivers: our budget for the pipe (about 110 MB/s of RGBA at
/// 1280 × 720 and 30 frames a second) and the texture.
const PLAY_EDGE: u32 = 1280;
/// Frame rates above this are played at it.
const MAX_FPS: f64 = 60.0;
/// The containers ffmpeg may read: MP4/MOV, Matroska/WebM, Ogg and AVI.
const FORMATS: &str = "mov,mp4,m4a,3gp,3g2,mj2,matroska,webm,ogg,avi";
/// A probe or a poster of a local file answers within this; one that hangs is given up.
const PREPARE_TIMEOUT: Duration = Duration::from_secs(20);

/// The reason a video shows when ffmpeg is missing.
pub const MISSING: &str = "Videos play with ffmpeg, which is not installed";

/// The command that installs ffmpeg on this system.
pub fn install_command() -> &'static str {
    if cfg!(windows) {
        "winget install Gyan.FFmpeg"
    } else if cfg!(target_os = "macos") {
        "brew install ffmpeg"
    } else {
        "sudo apt install ffmpeg"
    }
}

/// Where ffmpeg and ffprobe are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

impl Tools {
    /// Beside the app's own executable first, then on `PATH` as a shell opened now finds it (an
    /// install while the app runs counts at once).
    pub(crate) fn locate() -> Option<Self> {
        let beside = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_owned));
        let find = |name: &str| {
            beside
                .as_deref()
                .map(|dir| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
                .filter(|path| path.is_file())
                .or_else(|| review_agent::find_program(name))
        };
        Some(Self { ffmpeg: find("ffmpeg")?, ffprobe: find("ffprobe")? })
    }
}

/// A video ready to play.
#[derive(Debug)]
pub struct Clip {
    pub file: PathBuf,
    /// Its own size in pixels, upright.
    pub natural: [u32; 2],
    /// The size of the frames ffmpeg delivers: `natural` within [`PLAY_EDGE`].
    pub frame: [usize; 2],
    /// Seconds; zero when the container does not say.
    pub duration: f64,
    pub fps: f64,
    ffmpeg: PathBuf,
}

impl Clip {
    /// A clip of `natural` pixels and `duration` seconds whose ffmpeg is missing: it shows, and
    /// playing it fails to start.
    #[cfg(any(test, feature = "test-support"))]
    pub fn without_ffmpeg(natural: [u32; 2], duration: f64) -> Arc<Self> {
        Arc::new(Self {
            file: PathBuf::from("missing.webm"),
            natural,
            frame: fitted(natural, PLAY_EDGE),
            duration,
            fps: 30.0,
            ffmpeg: PathBuf::from("missing-ffmpeg"),
        })
    }

    /// ffmpeg reading the clip from `start` seconds, without sound.
    fn command(&self, start: f64) -> std::process::Command {
        let mut command = std::process::Command::new(&self.ffmpeg);
        command.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
        command.args(["-protocol_whitelist", "file", "-format_whitelist", FORMATS]);
        if start > 0.0 {
            command.arg("-ss").arg(format!("{start:.3}"));
        }
        command.arg("-i").arg(&self.file).args(["-an", "-sn", "-dn"]);
        command.stdin(Stdio::null()).stderr(Stdio::null());
        cv_base::platform::hide_std_console(&mut command);
        command
    }

    /// The video filter that delivers frames of [`Self::frame`], `fps` of them a second.
    fn filter(&self, fps: Option<f64>) -> String {
        let [width, height] = self.frame;
        let scale = format!("scale={width}:{height}:flags=bilinear");
        match fps {
            Some(fps) => format!("fps={fps},{scale}"),
            None => scale,
        }
    }
}

/// What `ffprobe` tells of a video.
#[derive(Debug, Clone, PartialEq)]
struct Probe {
    natural: [u32; 2],
    duration: f64,
    fps: f64,
}

/// Probes the video in `file` and draws its first frame: the clip and its poster.
pub(crate) async fn prepare(tools: &Tools, file: &Path) -> Result<(Arc<Clip>, ColorImage), String> {
    let mut probe = std::process::Command::new(&tools.ffprobe);
    probe.args(["-v", "error", "-protocol_whitelist", "file", "-format_whitelist", FORMATS]);
    probe.args(["-select_streams", "v:0", "-of", "json", "-show_entries"]);
    probe.arg(
        "stream=width,height,avg_frame_rate,r_frame_rate:stream_side_data=rotation:format=duration",
    );
    probe.arg(file).stdin(Stdio::null());
    let output = run(probe).await?;
    let probe = parse_probe(&String::from_utf8_lossy(&output))
        .ok_or_else(|| "The file holds no video ffmpeg can play".to_owned())?;
    let clip = Clip {
        file: file.to_owned(),
        natural: probe.natural,
        frame: fitted(probe.natural, PLAY_EDGE),
        duration: probe.duration,
        fps: probe.fps,
        ffmpeg: tools.ffmpeg.clone(),
    };
    let mut poster = clip.command(0.0);
    poster.args(["-frames:v", "1", "-vf"]).arg(clip.filter(None));
    poster.args(["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]);
    let pixels = run(poster).await?;
    let [width, height] = clip.frame;
    if pixels.len() != width * height * 4 {
        return Err("ffmpeg could not read the video".to_owned());
    }
    let image = ColorImage::from_rgba_unmultiplied(clip.frame, &pixels);
    Ok((Arc::new(clip), image))
}

/// Runs `command` and returns what it wrote; fails on an error exit, a hang or a missing program.
async fn run(command: std::process::Command) -> Result<Vec<u8>, String> {
    let mut command = tokio::process::Command::from(command);
    command.stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    let output = tokio::time::timeout(PREPARE_TIMEOUT, command.output())
        .await
        .map_err(|_| "ffmpeg did not finish reading the video".to_owned())?
        .map_err(|_| MISSING.to_owned())?;
    if !output.status.success() {
        return Err("ffmpeg could not read the video".to_owned());
    }
    Ok(output.stdout)
}

/// The size, length and frame rate of `ffprobe -of json` output; a stream turned by a quarter
/// (phone videos) is upright here, as ffmpeg delivers it.
fn parse_probe(json: &str) -> Option<Probe> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let stream = value.get("streams")?.get(0)?;
    let side = |key: &str| u32::try_from(stream.get(key)?.as_u64()?).ok().filter(|side| *side > 0);
    let (width, height) = (side("width")?, side("height")?);
    let rotation = stream
        .get("side_data_list")
        .and_then(serde_json::Value::as_array)
        .and_then(|list| list.iter().find_map(|data| data.get("rotation")?.as_i64()))
        .unwrap_or(0);
    let natural = if rotation.rem_euclid(180) == 90 { [height, width] } else { [width, height] };
    let rate = |key: &str| stream.get(key)?.as_str().and_then(parse_rate);
    let fps = rate("avg_frame_rate").or_else(|| rate("r_frame_rate")).unwrap_or(30.0).min(MAX_FPS);
    let duration = value
        .get("format")
        .and_then(|format| format.get("duration")?.as_str()?.parse::<f64>().ok())
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .unwrap_or(0.0);
    Some(Probe { natural, duration, fps })
}

/// `30000/1001` as frames a second; `None` for `0/0` and other non-rates.
fn parse_rate(rate: &str) -> Option<f64> {
    let (numerator, denominator) = rate.split_once('/')?;
    let fps = numerator.parse::<f64>().ok()? / denominator.parse::<f64>().ok()?;
    (fps.is_finite() && fps >= 1.0).then_some(fps)
}

/// `natural` scaled down to fit `edge`, keeping its aspect.
fn fitted([width, height]: [u32; 2], edge: u32) -> [usize; 2] {
    let scale = (f64::from(edge) / f64::from(width.max(height))).min(1.0);
    let side = |side: u32| ((f64::from(side) * scale).round() as usize).max(2);
    [side(width), side(height)]
}

#[cfg(test)]
pub(crate) mod tests;
