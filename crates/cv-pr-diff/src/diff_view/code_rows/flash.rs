//! A brief accent tint over lines the view was sent to from elsewhere (lines the assistant cited,
//! a thread shown in its file), so the eye finds them. It starts when a painted row first shows
//! one of the lines, not when they were asked for: loading, collapsed lines and the scroll come
//! first. It holds, then fades out, easing out; under reduced motion it stays still for as long
//! and then goes. It is timed, so scrolling away does not end it early, and it asks for repaints
//! only while it runs on screen. The lines last cited stay known after the tint ended
//! ([`Flash::is_cited`]): the assistant's mini chat fades while it covers them.

use std::ops::RangeInclusive;
use std::time::Duration;

use egui::Context;
use review_core::DiffSide;

use cv_base::theme::motion;
use cv_code::line_source::{CodeLine, Pane};

/// How long the tint shows, hold and fade (our own choice).
pub const DURATION: f64 = 2.8;
/// The tint stays at full strength this long before it fades (our own choice).
const HOLD: f64 = 0.7;
/// How long the lines may take to come on screen once asked for; later the view went elsewhere,
/// and a flash then would come out of nowhere (our own budget).
const ARRIVAL: f64 = 5.0;

/// Lines to flash: `lines` (1-based) counted in `side`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub side: DiffSide,
    pub lines: RangeInclusive<u32>,
}

impl Target {
    /// Whether `pane` shows one of the lines with `line`: in side by side only the pane of their
    /// side does.
    fn covers(&self, pane: Pane, line: &CodeLine<'_>) -> bool {
        let (number, is_their_pane) = match self.side {
            DiffSide::Old => (line.old_line, pane != Pane::Right),
            DiffSide::New => (line.new_line, pane != Pane::Left),
        };
        is_their_pane && self.lines.contains(&number)
    }
}

/// The view's flash: none, waiting for its lines to show, or running since they did.
#[derive(Default)]
pub struct Flash {
    state: Option<(Target, Phase)>,
    /// The lines the assistant cited last in this view.
    cited: Option<Target>,
}

#[derive(Clone, Copy)]
enum Phase {
    /// Asked for; no frame painted since.
    Asked,
    /// The lines are not on screen yet; the first frame after they were asked for was at this time.
    Waiting(f64),
    /// The lines came on screen at this time.
    Running(f64),
}

impl Flash {
    /// Flashes `target` once its lines are on screen, in place of a flash still running; `None`
    /// leaves the flash as it is.
    pub fn start(&mut self, target: Option<Target>) {
        if let Some(target) = target {
            self.state = Some((target, Phase::Asked));
        }
    }

    /// Keeps `target`, lines the assistant cited, as the view's cited lines.
    pub fn cite(&mut self, target: &Target) {
        self.cited = Some(target.clone());
    }

    /// Whether `pane` shows one of the lines cited last with `line`.
    pub fn is_cited(&self, pane: Pane, line: Option<&CodeLine<'_>>) -> bool {
        self.cited.as_ref().zip(line).is_some_and(|(cited, line)| cited.covers(pane, line))
    }

    /// The tint's strength (0 to 1) on `line` painted in `pane` this frame. The first line of the
    /// target painted starts the flash; while it runs, the next frame is asked for.
    pub fn strength(&mut self, ctx: &Context, pane: Pane, line: Option<&CodeLine<'_>>) -> f32 {
        let Some((target, phase)) = &mut self.state else {
            return 0.0;
        };
        let now = ctx.input(|input| input.time);
        if let Phase::Asked = *phase {
            *phase = Phase::Waiting(now);
        }
        if let Phase::Waiting(asked) = *phase
            && now - asked > ARRIVAL
        {
            self.state = None;
            return 0.0;
        }
        if !line.is_some_and(|line| target.covers(pane, line)) {
            return 0.0;
        }
        let started = match *phase {
            Phase::Asked | Phase::Waiting(_) => {
                *phase = Phase::Running(now);
                now
            }
            Phase::Running(started) => started,
        };
        let elapsed = now - started;
        let is_reduced = motion::is_reduced(ctx);
        let strength = fade(elapsed, is_reduced);
        if strength <= 0.0 {
            self.state = None;
            return 0.0;
        }
        if is_reduced {
            ctx.request_repaint_after(Duration::from_secs_f64(DURATION - elapsed));
        } else {
            ctx.request_repaint();
        }
        #[cfg(any(test, feature = "test-support"))]
        note(ctx, pane, line, strength);
        strength
    }
}

/// The tint's strength `elapsed` seconds after the lines came on screen: full while it holds,
/// then easing out to 0 at [`DURATION`]. Under reduced motion it stays full until then.
pub fn fade(elapsed: f64, is_reduced: bool) -> f32 {
    if !(0.0..DURATION).contains(&elapsed) {
        return if elapsed < 0.0 { 1.0 } else { 0.0 };
    }
    if is_reduced || elapsed <= HOLD {
        return 1.0;
    }
    let left = 1.0 - (elapsed - HOLD) / (DURATION - HOLD);
    (left * left) as f32
}

/// Notes `line` flashed in `pane`, for tests (see `diff_paint::flashed_lines`).
#[cfg(any(test, feature = "test-support"))]
fn note(ctx: &Context, pane: Pane, line: Option<&CodeLine<'_>>, strength: f32) {
    if let Some(line) = line {
        let number =
            if pane == Pane::Left { line.old_line } else { line.new_line.max(line.old_line) };
        cv_code::diff_paint::note_flashed(ctx, pane, number, strength);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cv_code::diff_paint;

    #[test]
    fn the_flash_holds_then_eases_out_and_ends() {
        assert_eq!(fade(0.0, false), 1.0, "full when the lines arrive");
        assert_eq!(fade(HOLD, false), 1.0, "and while it holds");
        let middle = fade(f64::midpoint(HOLD, DURATION), false);
        assert!(middle > 0.1 && middle < 0.5, "eased out: under half way at the middle ({middle})");
        assert!(fade(DURATION - 0.1, false) < 0.02, "nearly gone near the end");
        assert_eq!(fade(DURATION, false), 0.0, "gone at the end");
        let steps: Vec<f32> = (0..=14).map(|step| fade(f64::from(step) / 10.0, false)).collect();
        assert!(steps.windows(2).all(|pair| pair[1] <= pair[0]), "never brightens: {steps:?}");
    }

    #[test]
    fn under_reduced_motion_the_tint_stays_still_and_then_goes() {
        for elapsed in [0.0, HOLD, 0.9, DURATION - 0.01] {
            assert_eq!(fade(elapsed, true), 1.0, "still at {elapsed}");
        }
        assert_eq!(fade(DURATION, true), 0.0);
    }

    /// Only the target's lines on their side flash; the flash starts when they are painted, asks
    /// for repaints while it runs and none once it ended.
    #[test]
    fn the_flash_starts_on_arrival_and_stops_asking_for_frames() {
        let ctx = Context::default();
        let line = |old_line, new_line| CodeLine {
            text: "",
            kind: review_git::LineKind::Context,
            old_line,
            new_line,
            number: new_line,
            spans: &[],
            has_file_spans: false,
        };
        let mut flash = Flash::default();
        let target = Target { side: DiffSide::New, lines: 3..=4 };
        let run = |time: f64, flash: &mut Flash, pane: Pane, shown: &CodeLine<'_>| {
            let mut strength = 0.0;
            let input = egui::RawInput { time: Some(time), ..egui::RawInput::default() };
            let mut output = ctx.run_ui(input, |ui| {
                strength = flash.strength(ui.ctx(), pane, Some(shown));
            });
            output.textures_delta.clear();
            // egui reports a pass's repaint causes in the pass after it.
            let input = egui::RawInput { time: Some(time), ..egui::RawInput::default() };
            ctx.run_ui(input, |_| {}).textures_delta.clear();
            (strength, diff_paint::flash_asks_for_frames(&ctx))
        };
        let _first = run(0.0, &mut flash, Pane::Inline, &line(1, 1));
        flash.start(Some(target));
        assert_eq!(run(1.0, &mut flash, Pane::Inline, &line(1, 1)), (0.0, false), "waiting");
        assert_eq!(run(2.0, &mut flash, Pane::Right, &line(9, 9)).0, 0.0, "another line");
        assert_eq!(run(2.1, &mut flash, Pane::Left, &line(3, 3)).0, 0.0, "the other side");
        assert_eq!(run(2.2, &mut flash, Pane::Right, &line(2, 3)), (1.0, true), "arrived");
        let fade_time = 2.2 + f64::midpoint(HOLD, DURATION);
        let (fading, repaints) = run(fade_time, &mut flash, Pane::Inline, &line(4, 4));
        assert!(fading > 0.0 && fading < 1.0 && repaints, "fading from arrival: {fading}");
        let end_time = 2.2 + DURATION + 0.1;
        assert_eq!(run(end_time, &mut flash, Pane::Inline, &line(4, 4)), (0.0, false), "ended");
        assert!(flash.state.is_none());
    }
}
