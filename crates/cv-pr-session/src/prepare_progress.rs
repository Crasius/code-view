//! Getting a pull request's changes onto the machine, shown while the Files tab waits: a header
//! with the running step, one bar segment per step, and a live log of what happens (steps with
//! their durations, the remote, the mirror, the git command and git's own progress lines). The
//! log itself, and its redaction, is [`progress_log`].

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cv_base::theme::{self, icons, style};
use cv_widgets::progress_log::{self, LineKind, ProgressLog, format_duration, redact};
use cv_widgets::{buttons, glyph};
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Color32, FontId, Galley, Layout, Margin, Rect, Sense, Ui, UiBuilder, pos2, vec2,
};
use review_git::GitEvent;

/// How often the elapsed time and the log refresh while loading.
const REPAINT: Duration = Duration::from_millis(100);
const MAX_WIDTH: f32 = 820.0;
const HEADER_HEIGHT: f32 = buttons::SUBTLE_HEIGHT;
const SEGMENT_HEIGHT: f32 = 4.0;
const SEGMENT_GAP: f32 = 4.0;
const LOG_MIN_HEIGHT: f32 = 160.0;
const LOG_MAX_HEIGHT: f32 = 420.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    LoadingDetails,
    ResolvingRemote,
    OpeningMirror,
    Fetching,
    FindingMergeBase,
    ListingFiles,
}

impl Step {
    const ALL: [Step; 6] = [
        Step::LoadingDetails,
        Step::ResolvingRemote,
        Step::OpeningMirror,
        Step::Fetching,
        Step::FindingMergeBase,
        Step::ListingFiles,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Step::LoadingDetails => "Loading pull request details",
            Step::ResolvingRemote => "Resolving the repository",
            Step::OpeningMirror => "Opening the local mirror",
            Step::Fetching => "Fetching commits and trees",
            Step::FindingMergeBase => "Finding the merge base",
            Step::ListingFiles => "Listing changed files",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug)]
struct PrepareProgress {
    step: Step,
    started: Instant,
    step_started: Instant,
    log: ProgressLog,
    /// Total time until the failure; set once, freezes the elapsed time.
    failed_after: Option<Duration>,
}

impl PrepareProgress {
    fn new(now: Instant) -> Self {
        let mut progress = Self {
            step: Step::LoadingDetails,
            started: now,
            step_started: now,
            log: ProgressLog::default(),
            failed_after: None,
        };
        progress.log.push(LineKind::Step, &format!("{}…", Step::LoadingDetails.label()));
        progress
    }

    fn enter(&mut self, step: Step, now: Instant) {
        if self.step == step || self.failed_after.is_some() {
            return;
        }
        let elapsed = now.saturating_duration_since(self.step_started);
        let finished = self.step;
        self.log.push(
            LineKind::Step,
            &format!("{} done in {}", finished.label(), format_duration(elapsed)),
        );
        self.step = step;
        self.step_started = now;
        self.log.push(LineKind::Step, &format!("{}…", step.label()));
    }

    fn fail(&mut self, message: &str, now: Instant) {
        if self.failed_after.is_some() {
            return;
        }
        self.failed_after = Some(now.saturating_duration_since(self.started));
        let step_elapsed = now.saturating_duration_since(self.step_started);
        self.log.push(
            LineKind::Error,
            &format!("{} failed after {}", self.step.label(), format_duration(step_elapsed)),
        );
        for line in message.lines().filter(|line| !line.trim().is_empty()) {
            self.log.push(LineKind::Error, &format!("error: {line}"));
        }
    }

    fn elapsed(&self, now: Instant) -> Duration {
        self.failed_after.unwrap_or_else(|| now.saturating_duration_since(self.started))
    }

    fn is_failed(&self) -> bool {
        self.failed_after.is_some()
    }
}

/// Written by the background task, read by the UI each frame while loading.
#[derive(Clone)]
pub struct SharedProgress(Arc<Mutex<PrepareProgress>>);

impl Default for SharedProgress {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(PrepareProgress::new(Instant::now()))))
    }
}

impl SharedProgress {
    /// Starts over, unless the first load is still in its first step: then the details just
    /// arrived and their step keeps its start time.
    pub fn restart(&self) {
        let mut progress = self.lock();
        let is_first_step = progress.step == Step::LoadingDetails && !progress.is_failed();
        if !is_first_step {
            *progress = PrepareProgress::new(Instant::now());
        }
    }

    /// Clears the log and the failure for a retry.
    pub fn reset(&self) {
        *self.lock() = PrepareProgress::new(Instant::now());
    }

    pub fn enter(&self, step: Step) {
        self.lock().enter(step, Instant::now());
    }

    pub fn report_git(&self, event: GitEvent) {
        self.lock().log.report_git(event);
    }

    /// A line about what the running step found.
    pub fn log(&self, message: &str) {
        self.lock().log.push(LineKind::Info, message);
    }

    /// Marks the running step as failed with `message`; later calls keep the first failure.
    pub fn fail(&self, message: &str) {
        self.lock().fail(message, Instant::now());
    }

    fn lock(&self) -> MutexGuard<'_, PrepareProgress> {
        // A panic while holding the lock leaves at worst a half-written line.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The failure shown above the log, with the way out.
pub struct Failure<'a> {
    pub message: &'a str,
    /// Icon and label of the link to the pull request on its platform.
    pub open: Option<(&'a str, &'a str)>,
}

pub enum FailureAction {
    Retry,
    Open,
}

/// The L5 loading state: header, step segments, the failure (if any) and the live log.
pub fn show(
    ui: &mut Ui,
    progress: &SharedProgress,
    failure: Option<&Failure<'_>>,
) -> Option<FailureAction> {
    let progress = progress.lock();
    let now = Instant::now();
    let mut action = None;
    let margin = Margin::from(vec2(f32::from(style::PAGE_MARGIN_X), style::SPACE_L));
    egui::Frame::new().inner_margin(margin).show(ui, |ui| {
        ui.set_max_width(MAX_WIDTH.min(ui.available_width()));
        ui.spacing_mut().item_spacing.y = 0.0;
        header(ui, &progress, now);
        ui.add_space(style::SPACE_S);
        segments(ui, &progress);
        ui.add_space(style::SPACE_L);
        if let Some(failure) = failure {
            action = failure_block(ui, failure);
            ui.add_space(style::SPACE_L);
        }
        let panel = progress_log::Panel {
            id_salt: "prepare_log",
            name: "Loading log",
            min_height: LOG_MIN_HEIGHT,
            max_height: LOG_MAX_HEIGHT,
            is_live: !progress.is_failed(),
        };
        progress_log::show(ui, &progress.log, &panel);
    });
    if !progress.is_failed() {
        ui.ctx().request_repaint_after(REPAINT);
    }
    action
}

/// Spinner (or the failure icon), the step, "step N of 6 · 4.2 s" and "Copy log" on the right.
fn header(ui: &mut Ui, progress: &PrepareProgress, now: Instant) {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), HEADER_HEIGHT), Sense::hover());
    let mut right =
        ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::right_to_left(Align::Center)));
    let copy = progress_log::copy_button(&mut right, &progress.log);

    let is_failed = progress.is_failed();
    let slot = Rect::from_center_size(
        pos2(rect.left() + icons::SIZE / 2.0, rect.center().y),
        vec2(icons::SIZE, icons::SIZE),
    );
    if is_failed {
        let font = icons::rest_font(&palette, icons::SIZE);
        ui.painter().text(
            slot.center(),
            egui::Align2::CENTER_CENTER,
            icons::WARNING,
            font,
            palette.error,
        );
    } else {
        egui::Spinner::new().size(icons::SIZE - 2.0).color(palette.accent).paint_at(ui, slot);
    }
    let step = progress.step;
    let title =
        if is_failed { format!("{} failed", step.label()) } else { step.label().to_owned() };
    let meta = format!(
        "step {} of {} · {}",
        step.index() + 1,
        Step::ALL.len(),
        format_duration(progress.elapsed(now))
    );
    let text_left = slot.right() + style::SPACE_S;
    let text_width = (copy.rect.left() - style::SPACE_M - text_left).max(0.0);
    let meta_galley = one_line(
        ui,
        &meta,
        FontId::proportional(style::SMALL_SIZE),
        palette.text_secondary,
        text_width,
    );
    let title_width = (text_width - meta_galley.size().x - style::SPACE_S).max(0.0);
    let title_galley = one_line(
        ui,
        &title,
        theme::fonts::semibold_font(style::BODY_SIZE),
        palette.text,
        title_width,
    );
    let title_pos = pos2(text_left, rect.center().y - title_galley.size().y / 2.0);
    let meta_left = title_pos.x + title_galley.size().x + style::SPACE_S;
    let meta_pos = pos2(meta_left, rect.center().y - meta_galley.size().y / 2.0);
    ui.painter().galley(title_pos, title_galley, palette.text);
    ui.painter().galley(meta_pos, meta_galley, palette.text_secondary);
    let description = format!("{title}, {meta}");
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, true, &description)
    });
}

/// Six equal segments: done, running (or failed) and waiting.
fn segments(ui: &mut Ui, progress: &PrepareProgress) {
    let palette = theme::palette(ui.ctx());
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), SEGMENT_HEIGHT), Sense::hover());
    let count = Step::ALL.len() as f32;
    let width = ((rect.width() - SEGMENT_GAP * (count - 1.0)) / count).max(0.0);
    let current = progress.step.index();
    for (index, _) in Step::ALL.iter().enumerate() {
        let color = match index.cmp(&current) {
            std::cmp::Ordering::Less => palette.success,
            std::cmp::Ordering::Equal if progress.is_failed() => palette.error,
            std::cmp::Ordering::Equal => palette.accent,
            std::cmp::Ordering::Greater => palette.border_strong,
        };
        let left = rect.left() + index as f32 * (width + SEGMENT_GAP);
        let segment = Rect::from_min_size(pos2(left, rect.top()), vec2(width, SEGMENT_HEIGHT));
        ui.painter().rect_filled(segment, SEGMENT_HEIGHT / 2.0, color);
    }
}

/// What went wrong and the way out: Retry, or the pull request on its platform.
fn failure_block(ui: &mut Ui, failure: &Failure<'_>) -> Option<FailureAction> {
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    ui.horizontal(|ui| {
        glyph::decorative(
            ui,
            egui::RichText::new(icons::WARNING)
                .font(icons::rest_font(&palette, icons::SIZE))
                .color(palette.error),
        );
        ui.label(
            egui::RichText::new("Couldn't load the changed files")
                .font(theme::fonts::semibold_font(style::BODY_SIZE)),
        );
    });
    ui.add_space(style::SPACE_XS);
    ui.label(egui::RichText::new(redact(failure.message)).color(palette.text_secondary));
    ui.add_space(style::SPACE_M);
    ui.horizontal(|ui| {
        if buttons::primary_icon(ui, icons::ARROW_CLOCKWISE, "Retry").clicked() {
            action = Some(FailureAction::Retry);
        }
        if let Some((icon, label)) = failure.open
            && buttons::secondary_icon(ui, icon, label).clicked()
        {
            action = Some(FailureAction::Open);
        }
    });
    action
}

fn one_line(ui: &Ui, text: &str, font: FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping::truncate_at_width(max_width);
    ui.painter().layout_job(job)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{LineKind, PrepareProgress, Step};

    fn step_lines(progress: &PrepareProgress) -> Vec<&str> {
        progress
            .log
            .lines()
            .iter()
            .filter(|line| line.kind == LineKind::Step)
            .map(|line| line.text.as_str())
            .collect()
    }

    #[test]
    fn steps_log_their_start_and_their_duration() {
        let start = Instant::now();
        let mut progress = PrepareProgress::new(start);
        progress.enter(Step::ResolvingRemote, start + Duration::from_millis(250));
        progress.enter(Step::ResolvingRemote, start + Duration::from_millis(400));
        progress.enter(Step::OpeningMirror, start + Duration::from_millis(1_750));
        assert_eq!(
            step_lines(&progress),
            [
                "Loading pull request details…",
                "Loading pull request details done in 250 ms",
                "Resolving the repository…",
                "Resolving the repository done in 1.5 s",
                "Opening the local mirror…",
            ]
        );

        progress.fail(
            "git failed: fatal: unable to access\nThe requested URL returned error: 401",
            start + Duration::from_millis(3_250),
        );
        progress.fail("a later failure", start + Duration::from_secs(9));
        progress.enter(Step::Fetching, start + Duration::from_secs(10));
        assert!(
            !step_lines(&progress).contains(&"Fetching commits and trees…"),
            "a failed load stays on its step"
        );
        assert_eq!(
            progress.elapsed(start + Duration::from_secs(60)),
            Duration::from_millis(3_250),
            "the elapsed time stops at the failure"
        );
        let errors: Vec<&str> = progress
            .log
            .lines()
            .iter()
            .filter(|line| line.kind == LineKind::Error)
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(
            errors,
            [
                "Opening the local mirror failed after 1.5 s",
                "error: git failed: fatal: unable to access",
                "error: The requested URL returned error: 401",
            ]
        );
    }
}
