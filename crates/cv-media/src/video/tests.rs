//! Probing: size, upright turn, frame rate and length from `ffprobe`'s answer, and frames that fit
//! the pipe. With ffmpeg installed, a real clip prepares and plays to its end.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cv_testkit::ScratchDir;

use super::player::{Player, Status};
use super::{Probe, Tools, fitted, parse_probe, parse_rate, prepare};

#[test]
fn reads_size_turn_rate_and_length_from_the_probe() {
    let json = r#"{"streams":[{"width":1920,"height":1080,"r_frame_rate":"60/1",
        "avg_frame_rate":"30000/1001"}],"format":{"duration":"12.500000"}}"#;
    let probe = parse_probe(json).expect("a video stream");
    assert_eq!(probe.natural, [1920, 1080]);
    assert!((probe.fps - 29.97).abs() < 0.01, "the average rate first");
    assert!((probe.duration - 12.5).abs() < 1e-9);

    let turned = r#"{"streams":[{"width":320,"height":240,"avg_frame_rate":"0/0",
        "r_frame_rate":"240/1","side_data_list":[{"rotation":-90}]}],"format":{}}"#;
    assert_eq!(
        parse_probe(turned),
        Some(Probe { natural: [240, 320], duration: 0.0, fps: 60.0 }),
        "a phone video turned by a quarter is upright; fast rates play at 60; no length is zero"
    );
    assert_eq!(parse_probe(r#"{"streams":[],"format":{}}"#), None, "sound only: no video");
    assert_eq!(parse_rate("0/0"), None);
}

#[test]
fn frames_fit_the_pipe_keeping_their_aspect() {
    assert_eq!(fitted([3840, 2160], 1280), [1280, 720]);
    assert_eq!(fitted([320, 240], 1280), [320, 240], "small videos keep their size");
    assert_eq!(fitted([720, 1280], 1280), [720, 1280]);
}

/// Encodes a one-second VP9 clip with the installed ffmpeg.
pub(crate) async fn clip_file(tools: &Tools, folder: &ScratchDir) -> std::path::PathBuf {
    let file = folder.path().join("clip.webm");
    let status = tokio::process::Command::new(&tools.ffmpeg)
        .args(["-v", "error", "-f", "lavfi", "-i", "testsrc=size=64x48:rate=20", "-t", "1"])
        .args(["-c:v", "libvpx-vp9", "-y"])
        .arg(&file)
        .status()
        .await
        .expect("ffmpeg runs");
    assert!(status.success());
    file
}

#[test]
#[ignore = "needs ffmpeg and ffprobe installed"]
fn a_webm_prepares_and_plays_to_its_end() {
    let folder = ScratchDir::new("video-play");
    let tools = Tools::locate().expect("ffmpeg installed");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let file = runtime.block_on(clip_file(&tools, &folder));
    let (clip, poster) = runtime.block_on(prepare(&tools, &file)).expect("prepares");
    assert_eq!((clip.natural, poster.size), ([64, 48], [64, 48]));
    assert!((clip.fps - 20.0).abs() < 0.01 && (clip.duration - 1.0).abs() < 0.1);

    let ctx = egui::Context::default();
    let mut player = Player::new(Arc::clone(&clip));
    let started = Instant::now();
    player.toggle(0.0);
    let mut frames = 0;
    while player.status() == Status::Playing && started.elapsed() < Duration::from_secs(20) {
        let now = started.elapsed().as_secs_f64();
        if player.update(&ctx, now).0.is_some() {
            frames += 1;
        }
        cv_testkit::let_threads_run(Duration::from_millis(10));
    }
    assert_eq!(player.status(), Status::Ended, "plays to its end");
    assert!(frames > 0, "frames arrived");
    assert!((player.position(started.elapsed().as_secs_f64()) - 1.0).abs() < 0.2);
}
