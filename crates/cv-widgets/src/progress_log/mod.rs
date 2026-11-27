//! The live log of long git work (getting a pull request's changes, checking it out): timestamped
//! monospace lines, git's progress lines overwriting themselves like git's own `\r`, bounded to
//! [`MAX_LINES`]. Every line passes [`redact()`] on its way in, so neither the screen nor "Copy
//! log" ever carries a token: URL userinfo, `Authorization` headers, `http.extraHeader` and `-c`
//! config values, token query parameters and token-shaped words are replaced.

mod redact;

use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use cv_base::theme::{self, Palette, icons, style};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Color32, FontId, Galley, Margin, Rect, Sense, Ui, pos2, vec2};
use review_git::{GitEvent, GitProgress};
use time::{OffsetDateTime, UtcOffset};

pub use self::redact::redact;
use crate::numbers::group_digits;
use crate::{buttons, copy_chip};

/// Older lines fall out of the log once it holds this many.
pub(crate) const MAX_LINES: usize = 500;
const LOG_FONT_SIZE: f32 = 12.0;
/// Extra space between log rows on top of the font's row height.
const LOG_LEADING: f32 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// A step started or finished.
    Step,
    /// What a step found: the remote, the mirror, the merge base, git's other output.
    Info,
    /// The git command being run.
    Command,
    /// A git progress line, replaced in place by the next update of its phase.
    Progress,
    /// The failure and its message.
    Error,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub at: OffsetDateTime,
    pub kind: LineKind,
    pub text: String,
    /// The git phase of a progress line ("Receiving objects").
    phase: Option<String>,
}

/// The lines of one run, newest last.
#[derive(Debug, Default)]
pub struct ProgressLog {
    lines: VecDeque<LogLine>,
    /// Lines that fell out of the ring buffer.
    dropped: usize,
}

impl ProgressLog {
    #[cfg(any(test, feature = "test-support"))]
    pub fn lines(&self) -> &VecDeque<LogLine> {
        &self.lines
    }

    /// One line of the git command, a git message, or the progress line of its phase.
    pub fn report_git(&mut self, event: GitEvent) {
        match event {
            GitEvent::Command(command) => self.push(LineKind::Command, &format!("$ {command}")),
            GitEvent::Message(message) => self.push(LineKind::Info, &message),
            GitEvent::Progress(progress) => self.progress_line(&progress),
        }
    }

    /// Like git's `\r`: a newer update of the phase on the last line overwrites it.
    fn progress_line(&mut self, progress: &GitProgress) {
        let text = redact(&format_git_progress(progress));
        if let Some(last) = self.lines.back_mut()
            && last.kind == LineKind::Progress
            && last.phase.as_deref() == Some(progress.phase.as_str())
        {
            last.text = text;
            last.at = OffsetDateTime::now_utc();
            return;
        }
        self.push_line(LogLine {
            at: OffsetDateTime::now_utc(),
            kind: LineKind::Progress,
            text,
            phase: Some(progress.phase.clone()),
        });
    }

    /// One log line per line of `text`, each redacted.
    pub fn push(&mut self, kind: LineKind, text: &str) {
        for line in text.split(['\n', '\r']).filter(|line| !line.trim().is_empty()) {
            self.push_line(LogLine {
                at: OffsetDateTime::now_utc(),
                kind,
                text: redact(line.trim_end()),
                phase: None,
            });
        }
    }

    fn push_line(&mut self, line: LogLine) {
        if self.lines.len() == MAX_LINES {
            self.lines.pop_front();
            self.dropped += 1;
        }
        self.lines.push_back(line);
    }

    /// The log as plain text for the clipboard, one timestamped line each.
    pub fn text(&self) -> String {
        let mut text = if self.dropped > 0 {
            format!("({} earlier lines dropped)\n", group_digits(self.dropped))
        } else {
            String::new()
        };
        for line in &self.lines {
            text.push_str(&timestamp(line.at));
            text.push_str("  ");
            text.push_str(&line.text);
            text.push('\n');
        }
        text
    }
}

/// How the log panel sits on its page.
pub struct Panel<'a> {
    /// Keeps the panel's scroll position apart from other logs.
    pub id_salt: &'a str,
    /// The panel's name for screen readers ("Loading log").
    pub name: &'a str,
    pub min_height: f32,
    pub max_height: f32,
    /// The run goes on: the last line gets a caret.
    pub is_live: bool,
}

/// The monospace log: bounded height, follows the newest line until the user scrolls up.
pub fn show(ui: &mut Ui, log: &ProgressLog, panel: &Panel<'_>) {
    let palette = theme::palette(ui.ctx());
    let font = theme::fonts::code_font(LOG_FONT_SIZE);
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font)) + LOG_LEADING;
    let height = ui.available_height().clamp(panel.min_height, panel.max_height);
    let fill = if palette.is_dark { palette.ground } else { palette.surface_subtle };
    let caret_width = ui.fonts_mut(|fonts| fonts.glyph_width(&font, 'M'));
    let frame = egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(style::RADIUS_CARD)
        .inner_margin(Margin::symmetric(12, 8));
    let inner_height = height - frame.inner_margin.sum().y;
    let response = frame.show(ui, |ui| {
        ui.set_min_size(vec2(ui.available_width(), inner_height));
        ui.spacing_mut().item_spacing.y = 0.0;
        let last = log.lines.len().saturating_sub(1);
        egui::ScrollArea::both()
            .id_salt(panel.id_salt)
            .auto_shrink(false)
            .max_height(inner_height)
            .stick_to_bottom(true)
            .show_rows(ui, row_height, log.lines.len(), |ui, range| {
                for index in range {
                    let line = &log.lines[index];
                    let galley = log_row(ui, line, &font, &palette);
                    let has_caret = index == last && panel.is_live;
                    let width = galley.size().x + if has_caret { caret_width + 2.0 } else { 0.0 };
                    let (rect, _) = ui.allocate_exact_size(
                        vec2(width.max(ui.available_width()), row_height),
                        Sense::hover(),
                    );
                    let text_top = rect.center().y - galley.size().y / 2.0;
                    if has_caret {
                        let caret = Rect::from_min_size(
                            pos2(
                                rect.left() + galley.size().x + 2.0,
                                rect.center().y - LOG_FONT_SIZE / 2.0,
                            ),
                            vec2(caret_width, LOG_FONT_SIZE),
                        );
                        ui.painter().rect_filled(caret, 1.0, palette.text_secondary);
                    }
                    ui.painter().galley(pos2(rect.left(), text_top), galley, palette.text);
                }
            });
    });
    response.response.widget_info(|| {
        let label = format!("{}, {} lines", panel.name, log.lines.len());
        egui::WidgetInfo::labeled(egui::Role::Log, true, label)
    });
}

/// "Copy log", reading "Copied" for a moment after a click; copies [`ProgressLog::text`].
pub fn copy_button(ui: &mut Ui, log: &ProgressLog) -> egui::Response {
    copy_text_button(ui, || log.text())
}

/// "Copy log" for a log kept as plain lines (a checkout's saved log).
pub fn copy_text_button(ui: &mut Ui, text: impl FnOnce() -> String) -> egui::Response {
    // Keyed before the button exists: its label depends on the confirmation.
    let id = ui.next_auto_id();
    let copy = if copy_chip::is_showing_copied(ui, id) {
        buttons::subtle(ui, icons::CHECK, "Copied")
    } else {
        buttons::subtle(ui, icons::COPY, "Copy log")
    };
    if copy.clicked() {
        copy_chip::mark_copied(ui.ctx(), id, text());
    }
    copy
}

/// "14:03:07.412  Receiving objects: 64% (41,200/64,380)" with the timestamp muted.
fn log_row(ui: &Ui, line: &LogLine, font: &FontId, palette: &Palette) -> Arc<Galley> {
    let color = match line.kind {
        LineKind::Step | LineKind::Info | LineKind::Progress => palette.text,
        LineKind::Command => palette.accent_text,
        LineKind::Error => palette.error,
    };
    let mut job = LayoutJob::default();
    let format =
        |color: Color32| TextFormat { font_id: font.clone(), color, ..TextFormat::default() };
    job.append(&timestamp(line.at), 0.0, format(palette.text_secondary));
    job.append("  ", 0.0, format(palette.text_secondary));
    job.append(&line.text, 0.0, format(color));
    job.wrap = TextWrapping::no_max_width();
    ui.painter().layout_job(job)
}

/// "Receiving objects: 64% (41,200/64,380), 38.10 MiB | 9.40 MiB/s", the way git prints it.
fn format_git_progress(progress: &GitProgress) -> String {
    let percent = progress.percent.map(|percent| format!(" {percent}%")).unwrap_or_default();
    let counts = match (progress.done, progress.total, progress.percent) {
        (Some(done), Some(total), _) => {
            format!(" ({}/{})", group_digits(done as usize), group_digits(total as usize))
        }
        (Some(done), None, None) => format!(" {}", group_digits(done as usize)),
        _ => String::new(),
    };
    let transfer =
        progress.transfer.as_ref().map(|transfer| format!(", {transfer}")).unwrap_or_default();
    format!("{}:{percent}{counts}{transfer}", progress.phase)
}

/// "14:03:07.412" in local time (UTC when the local offset is unknown).
pub fn timestamp(at: OffsetDateTime) -> String {
    static OFFSET: OnceLock<UtcOffset> = OnceLock::new();
    let offset =
        *OFFSET.get_or_init(|| UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC));
    let local = at.to_offset(offset);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        local.hour(),
        local.minute(),
        local.second(),
        local.millisecond()
    )
}

/// "250 ms", "2.4 s", "1 min 5 s".
pub fn format_duration(duration: Duration) -> String {
    let millis = duration.as_millis();
    match millis {
        ..1_000 => format!("{millis} ms"),
        1_000..60_000 => format!("{:.1} s", duration.as_secs_f32()),
        _ => format!("{} min {} s", duration.as_secs() / 60, duration.as_secs() % 60),
    }
}

#[cfg(test)]
mod tests {
    use review_git::{GitEvent, GitProgress};

    use super::{LineKind, MAX_LINES, ProgressLog};

    fn receiving(percent: u8, done: u64) -> GitEvent {
        GitEvent::Progress(GitProgress {
            phase: "Receiving objects".into(),
            percent: Some(percent),
            done: Some(done),
            total: Some(64_380),
            transfer: Some("38.10 MiB | 9.40 MiB/s".into()),
        })
    }

    #[test]
    fn the_log_keeps_the_newest_lines_up_to_its_bound() {
        let mut log = ProgressLog::default();
        for index in 0..MAX_LINES + 120 {
            log.report_git(GitEvent::Message(format!("line {index}")));
        }
        assert_eq!(log.lines.len(), MAX_LINES);
        assert_eq!(log.lines.back().unwrap().text, format!("line {}", MAX_LINES + 119));
        assert!(log.text().starts_with("(120 earlier lines dropped)\n"));
    }

    #[test]
    fn git_progress_overwrites_its_line_until_the_phase_changes() {
        let mut log = ProgressLog::default();
        log.report_git(GitEvent::Command("git fetch --progress origin".into()));
        log.report_git(receiving(12, 7_725));
        log.report_git(receiving(64, 41_200));
        let progress_lines: Vec<&str> = log
            .lines
            .iter()
            .filter(|line| line.kind == LineKind::Progress)
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(
            progress_lines,
            ["Receiving objects: 64% (41,200/64,380), 38.10 MiB | 9.40 MiB/s"]
        );

        log.report_git(GitEvent::Progress(GitProgress {
            phase: "Resolving deltas".into(),
            percent: Some(5),
            done: Some(10),
            total: Some(200),
            transfer: None,
        }));
        log.report_git(GitEvent::Message("From https://github.com/cli/cli".into()));
        log.report_git(receiving(70, 45_000));
        let texts: Vec<&str> = log.lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "$ git fetch --progress origin",
                "Receiving objects: 64% (41,200/64,380), 38.10 MiB | 9.40 MiB/s",
                "Resolving deltas: 5% (10/200)",
                "From https://github.com/cli/cli",
                "Receiving objects: 70% (45,000/64,380), 38.10 MiB | 9.40 MiB/s",
            ],
            "another phase or another line in between starts a new line"
        );
    }

    #[test]
    fn every_way_into_the_log_redacts() {
        let mut log = ProgressLog::default();
        log.push(
            LineKind::Info,
            "Remote https://x-access-token:ghs_0123456789abcdef@github.com/o/r.git",
        );
        log.report_git(GitEvent::Command(
            "git -c http.extraHeader=Authorization: Basic c2VjcmV0 fetch".into(),
        ));
        log.report_git(GitEvent::Message("From https://user:pass@example.com/r".into()));
        log.push(LineKind::Error, "fatal: 'https://user:hunter2@example.com/r' not found");
        let text = log.text();
        for secret in ["ghs_", "x-access-token", "c2VjcmV0", "user:", "hunter2", "pass@"] {
            assert!(!text.contains(secret), "{secret} leaked into:\n{text}");
        }
    }
}
