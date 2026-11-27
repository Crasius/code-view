//! Virtualized log view over an indexed file on disk: only the visible window is read and laid out.

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use cv_base::platform::{self, shortcut};
use cv_base::theme::{self, icons};
use cv_pr_session::session::PullRequestSession;
use cv_runtime::background::Background;
use cv_runtime::load::Load;
use cv_runtime::toasts::Toasts;
use cv_services::Services;
use cv_shell::focus_mode::{self, strip};
use cv_widgets::find_bar::{self, Step};
use cv_widgets::numbers::group_digits;
use cv_widgets::states;
use cv_widgets::text_selection::{self, Selection};
use cv_widgets::{buttons, elide, glyph, row, surfaces};
use egui::{Align, FontId, Id, Key, Modifiers, Rect, Response, RichText, Sense, Ui, pos2, vec2};
use review_core::{PipelineJob, RunStatus};

use self::exports::LogExports;
use self::fetch::LogFetch;
use self::paint::{Highlight, LineLook, gutter_width, paint_line, strip_title};
use self::panes::NoLog;
use self::search::LogSearch;
use super::super::job_tree::LogTarget;
use super::availability::{self, Availability, Missing};
use super::download::{Fetched, Grown, LoadedLog};

/// Lines read beyond the visible ones, so small scrolls don't touch the disk.
const READ_AHEAD: usize = 200;
/// Copies the whole log.
const COPY_LOG_SHORTCUT: &str = shortcut!(command shift "C");
const NEXT_MATCH_HINT: &str = concat!("Next match (", shortcut!(Enter), ")");
const PREVIOUS_MATCH_HINT: &str = concat!("Previous match (", shortcut!(shift Enter), ")");
const DOWNLOAD_HINT: &str = "Download log (.log, as the platform delivered it)";
const FIRST_ERROR: &str = "First error";
/// The toolbar's search field: its width, narrowed to the minimum while the job's name has less
/// than [`TITLE_MIN_WIDTH`] left.
const SEARCH_WIDTH: f32 = 220.0;
const SEARCH_MIN_WIDTH: f32 = 120.0;
/// The focus strip's search field.
const STRIP_SEARCH_WIDTH: f32 = 200.0;
const TITLE_MIN_WIDTH: f32 = 180.0;

/// What the lines of a downloaded log showed this frame.
enum Lines {
    /// The visible lines, under one widget for the context menu.
    Shown(Response),
    /// Nothing to show (no visible rows), or the log's file could not be read: `is_retried`
    /// when the user asked to download it again.
    Unread { is_retried: bool },
}

#[derive(Default)]
pub(crate) struct LogViewer {
    target: Option<LogTarget>,
    /// The target's download, and fetching it again while its job runs.
    fetch: LogFetch,
    search: LogSearch,
    window: Option<(Range<usize>, Vec<String>)>,
    scroll_to: Option<usize>,
    widest_line: f32,
    /// Font size `widest_line` was measured at; the code zoom changes it.
    widest_at: f32,
    /// "Copy log", Ctrl+C on the selection and "Download log".
    exports: LogExports,
    /// The text selection, as (line, character) positions.
    selecting: selecting::Selecting,
}

impl LogViewer {
    pub(crate) fn target(&self) -> Option<&LogTarget> {
        self.target.as_ref()
    }

    /// Lets go of the log and its download once its page is no longer shown: a finished job's log
    /// opens again from disk, a running job's with one request.
    pub(crate) fn release_unless(&mut self, is_shown: bool, services: &Services) {
        if !is_shown && self.target.is_some() {
            self.fetch.reset(services);
            *self = Self::default();
        }
    }

    /// Shows `target`; its log downloads once the platform serves it (see [`Self::show`]).
    pub(crate) fn open(&mut self, target: LogTarget, services: &Services) {
        self.fetch.reset(services);
        self.target = Some(target);
        self.search.reset();
        self.window = None;
        self.scroll_to = None;
        self.widest_line = 0.0;
        self.exports.forget_copies();
        self.selecting = selecting::Selecting::default();
    }

    /// `shown` is the target's item and `owner` the job (or task) its log belongs to, as the stages
    /// loaded last report them: whether the platform serves the log yet depends on the owner's
    /// state, what the pane says about an item without a log on the item's.
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        shown: Option<&PipelineJob>,
        owner: Option<&PipelineJob>,
        session: &PullRequestSession,
        services: &mut Services,
    ) {
        self.poll_loads(&services.background);
        let Some(target) = self.target.take() else {
            states::message(
                ui,
                icons::FILE_TEXT,
                "Select a job",
                "Pick a job or step to see its log.",
            );
            return;
        };
        if self.show_target(ui, &target, shown, owner, session, services) {
            self.open(target, services);
        } else {
            self.target = Some(target);
        }
    }

    /// [`Self::show`] for the target, held apart meanwhile; true when the user asked to download
    /// its log again.
    fn show_target(
        &mut self,
        ui: &mut Ui,
        target: &LogTarget,
        shown: Option<&PipelineJob>,
        owner: Option<&PipelineJob>,
        session: &PullRequestSession,
        services: &mut Services,
    ) -> bool {
        let owner = owner.or(shown);
        let status = owner.map_or(RunStatus::Neutral, |job| job.status);
        let running_logs = session.capabilities.running_logs;
        let finished_ago = owner.and_then(|job| job.finished_at).map(|finished| {
            let elapsed = time::OffsetDateTime::now_utc() - finished;
            Duration::try_from(elapsed).unwrap_or_default()
        });
        let availability = availability::of(running_logs, target.log_id.is_some(), status);
        let missing = match &self.fetch.log {
            Load::Ready(Fetched::Failed(error)) => {
                Some(availability::missing(running_logs, status, error.status(), finished_ago))
            }
            _ => None,
        };
        if let Some(log_id) = &target.log_id {
            self.fetch.sync(ui.ctx(), log_id, (availability, missing), session, services);
        }
        let pane = match availability {
            Availability::Absent => Some(NoLog::of(
                shown.map_or(RunStatus::Queued, |job| job.status),
                &target.title,
                session.kind,
            )),
            Availability::AfterFinish => Some(NoLog::after_finish(target, owner, session.kind)),
            Availability::Fetch { .. } => {
                match missing.or(self.fetch.missing.filter(|_| self.fetch.log.is_loading())) {
                    Some(Missing::AfterFinish) => {
                        Some(NoLog::after_finish(target, owner, session.kind))
                    }
                    Some(Missing::NotYet) => {
                        Some(NoLog::of(RunStatus::Running, &target.title, session.kind))
                    }
                    Some(Missing::Publishing) => {
                        panes::publishing(ui, &target.title, session.kind);
                        return false;
                    }
                    Some(Missing::Failed) | None => None,
                }
            }
        };
        if let Some(pane) = pane {
            pane.show(ui, target, session.kind);
            return false;
        }
        let (cause, details) = match &self.fetch.log {
            Load::Ready(Fetched::Log(log)) => {
                let log = log.clone();
                let font_size = services.settings.code_font_size;
                let (background, toasts) = (&services.background, &mut services.toasts);
                return self.show_log(ui, target, &log, background, toasts, font_size);
            }
            Load::Ready(Fetched::Failed(error)) => {
                panes::failure(error, &target.title, session.kind)
            }
            Load::Failed(message) => (message.clone(), None),
            Load::Idle | Load::Loading(_) => {
                panes::downloading(ui, &target.title, self.fetch.received());
                return false;
            }
        };
        panes::failed(ui, &cause, details.as_deref(), target, session)
    }

    /// The log grew (or was rewritten): the lines on screen are read again, a search runs again
    /// in place, the view keeps its place (or follows the end, where it was).
    fn take_grown(&mut self, grown: Grown, background: &Background) {
        let (log, is_replaced) = match grown {
            Grown::Same => return,
            Grown::Appended(log) => (log, false),
            Grown::Replaced(log) => (log, true),
        };
        self.window = None;
        if is_replaced {
            self.selecting = selecting::Selecting::default();
        }
        self.fetch.log = Load::Ready(Fetched::Log(log.clone()));
        self.search.rerun(log, background);
    }

    /// Toolbar and lines of a downloaded log; true when the user asked to download it again after
    /// its file could not be read.
    fn show_log(
        &mut self,
        ui: &mut Ui,
        target: &LogTarget,
        log: &Arc<LoadedLog>,
        background: &Background,
        toasts: &mut Toasts,
        font_size: f32,
    ) -> bool {
        self.exports.finish(ui, toasts, log, background);
        self.shortcuts(ui, log, background);
        if !focus_mode::is_shown(ui.ctx()) {
            self.toolbar(ui, target, log, background);
            ui.separator();
        }
        match self.lines(ui, log, font_size) {
            Lines::Shown(area) => self.context_menu(&area, log, background),
            Lines::Unread { is_retried } => return is_retried,
        }
        false
    }

    /// Ctrl+Shift+C copies the log; Ctrl+C the selection, Ctrl+A (while the log has focus)
    /// selects every line, Esc lets go of the selection. Only Ctrl+Shift+C works while typing.
    fn shortcuts(&mut self, ui: &Ui, log: &Arc<LoadedLog>, background: &Background) {
        let has_focus = ui.memory(|memory| memory.has_focus(text_id(ui)));
        let is_typing = ui.ctx().text_edit_focused();
        let (copy_log, copy, select_all, escape) = ui.input_mut(|input| {
            let copy_log = input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::C);
            if is_typing {
                return (copy_log, false, false, false);
            }
            // egui-winit reports Ctrl+Shift+C as a copy event as well.
            let copy = text_selection::copy_pressed(input) && !copy_log;
            let select_all = has_focus && text_selection::select_all_pressed(input);
            (copy_log, copy, select_all, input.key_pressed(Key::Escape))
        });
        if copy_log {
            self.exports.start_copy(log, background);
        }
        if copy {
            self.start_copy_selection(log, background);
        }
        if select_all {
            self.select_all(log);
        }
        // Esc that let go of a selection is used up: focus mode stays for the next one.
        if escape
            && !self.selecting.dragging
            && self.selecting.selection.take().is_some_and(|selection| !selection.is_empty())
        {
            ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        }
    }

    fn select_all(&mut self, log: &LoadedLog) {
        if let Some(last) = log.file.line_count().checked_sub(1) {
            self.selecting.selection = Some(Selection::lines((), 0, last));
        }
    }

    fn start_copy_selection(&mut self, log: &Arc<LoadedLog>, background: &Background) {
        if let Some(selection) = self.selecting.current() {
            self.exports.start_copy_selection(selection, log, background);
        }
    }

    /// Right-click on the lines: copy the selection, select all, copy the log.
    fn context_menu(&mut self, area: &Response, log: &Arc<LoadedLog>, background: &Background) {
        let has_selection = self.selecting.current().is_some();
        let (mut copy, mut select_all, mut copy_log) = (false, false, false);
        area.context_menu(|ui| {
            let copy_button = egui::Button::new(format!("{}  Copy selection", icons::COPY));
            let copy_button = ui.add_enabled(has_selection, copy_button);
            glyph::speak_as(&copy_button, "Copy selection");
            if copy_button.on_hover_text(shortcut!(command "C")).clicked() {
                copy = true;
                ui.close();
            }
            if glyph::button(ui, icons::ROWS, "Select all")
                .on_hover_text(shortcut!(command "A"))
                .clicked()
            {
                select_all = true;
                ui.close();
            }
            if glyph::button(ui, icons::COPY, "Copy log").on_hover_text(COPY_LOG_SHORTCUT).clicked()
            {
                copy_log = true;
                ui.close();
            }
        });
        if copy {
            self.start_copy_selection(log, background);
        }
        if select_all {
            self.select_all(log);
            area.request_focus();
        }
        if copy_log {
            self.exports.start_copy(log, background);
        }
    }

    fn toolbar(
        &mut self,
        ui: &mut Ui,
        target: &LogTarget,
        log: &Arc<LoadedLog>,
        background: &Background,
    ) {
        let palette = theme::palette(ui.ctx());
        // A running job's log says it is live, and when fetching more of it failed.
        let (live, live_hint) = match (self.fetch.is_live, &self.fetch.grow_error) {
            (false, _) => ("", None),
            (true, None) => (
                " · live",
                Some("The job is running; what it adds shows up here every few seconds".to_owned()),
            ),
            (true, Some(error)) => (
                " · live, update failed",
                Some(format!(
                    "The job is running; fetching what it added failed and is tried again: {error}"
                )),
            ),
        };
        ui.horizontal(|ui| {
            // The tools first (right to left); the job's name is cut to what they leave, its line
            // count and "First error" keep their room.
            let ((), first_error) = row::with_trailing(
                ui,
                Align::Center,
                |ui| self.toolbar_tools(ui, log, target, background),
                |ui| {
                    let lines = RichText::new(format!(
                        "{} lines{live}",
                        group_digits(log.file.line_count())
                    ))
                    .size(theme::style::SMALL_SIZE)
                    .color(palette.text_secondary);
                    let small = FontId::proportional(theme::style::SMALL_SIZE);
                    let lines_width = elide::text_width(ui, lines.text(), &small);
                    let error_width = if log.first_error.is_some() {
                        buttons::subtle_width(ui, FIRST_ERROR) + ui.spacing().item_spacing.x
                    } else {
                        0.0
                    };
                    let title_room = (ui.available_width()
                        - lines_width
                        - error_width
                        - ui.spacing().item_spacing.x)
                        .max(0.0);
                    ui.scope(|ui| {
                        ui.set_max_width(title_room);
                        ui.add(
                            egui::Label::new(
                                RichText::new(&target.title)
                                    .font(theme::fonts::semibold_font(theme::style::BODY_SIZE)),
                            )
                            .truncate(),
                        );
                    });
                    let label = ui.label(lines);
                    if let Some(hint) = live_hint {
                        label.on_hover_text(hint);
                    }
                    log.first_error.filter(|_| {
                        buttons::subtle(ui, icons::WARNING_CIRCLE, FIRST_ERROR).clicked()
                    })
                },
            );
            if let Some(line) = first_error {
                self.scroll_to = Some(line);
            }
        });
    }

    /// The toolbar's right side, laid out right to left: focus mode, open, save, copy, then the
    /// search with its steps. The search narrows before the job's name runs out of room.
    fn toolbar_tools(
        &mut self,
        ui: &mut Ui,
        log: &Arc<LoadedLog>,
        target: &LogTarget,
        background: &Background,
    ) {
        let palette = theme::palette(ui.ctx());
        focus_mode::enter_button(ui);
        if buttons::icon(ui, icons::ARROW_SQUARE_OUT, "Open the log file").clicked() {
            ui.ctx()
                .open_url(egui::OpenUrl::new_tab(format!("file:///{}", log.file.path().display())));
        }
        if platform::HAS_SAVE_DIALOG
            && buttons::icon(ui, icons::DOWNLOAD_SIMPLE, DOWNLOAD_HINT).clicked()
        {
            self.exports.start_download(&target.title, log, background);
        }
        if self.exports.copy_button(ui) {
            self.exports.start_copy(log, background);
        }
        ui.add_space(theme::style::SPACE_M);
        let step = find_bar::step_buttons(ui, NEXT_MATCH_HINT, PREVIOUS_MATCH_HINT);
        let status = RichText::new(self.search.status()).size(theme::style::SMALL_SIZE);
        self.search.explain_failure(ui.label(status.color(palette.text_secondary)));
        let search_width =
            (ui.available_width() - TITLE_MIN_WIDTH).clamp(SEARCH_MIN_WIDTH, SEARCH_WIDTH);
        let step = self.search.field(ui, log, background, search_width).or(step);
        self.step(step);
    }

    /// Moves to the next or previous match and scrolls it into view.
    fn step(&mut self, step: Option<Step>) {
        if let Some(line) = self.search.step(step) {
            self.scroll_to = Some(line);
        }
    }

    /// Takes in the log and the search once they are done; the first match scrolls into view.
    fn poll_loads(&mut self, background: &Background) {
        if let Some(grown) = self.fetch.poll() {
            self.take_grown(grown, background);
        }
        if let Some(line) = self.search.poll() {
            self.scroll_to = Some(line);
        }
    }

    /// Focus mode's bar in place of the toolbar, at the top of `ui` above the log: the job and its
    /// state on the left, where dragging moves the window; the search, the first error, copy and
    /// download and the way out on the right, then what `extra` adds (the attempt switcher). The
    /// log's keys stay the same.
    pub(crate) fn focus_strip(
        &mut self,
        ui: &mut Ui,
        shown: Option<&PipelineJob>,
        background: &Background,
        extra: impl FnOnce(&mut Ui),
    ) {
        self.poll_loads(background);
        let log = self.fetch.loaded();
        let title = self
            .target
            .as_ref()
            .map_or_else(|| "Select a job".to_owned(), |target| target.title.clone());
        strip::show(ui, "log_focus_strip", |bar| {
            bar.right(|ui| {
                strip::exit_button(ui);
                if let Some(log) = &log {
                    self.strip_tools(ui, log, &title, background);
                }
            });
            bar.right(extra);
            let width = bar.middle_width();
            bar.middle(|ui| strip_title(ui, &title, shown, log.as_deref(), width));
        });
    }

    /// The focus strip's tools for a downloaded log, right to left: save, copy, the search with
    /// its steps, the first error.
    fn strip_tools(
        &mut self,
        ui: &mut Ui,
        log: &Arc<LoadedLog>,
        title: &str,
        background: &Background,
    ) {
        strip::separator(ui);
        if platform::HAS_SAVE_DIALOG
            && strip::button(ui, icons::DOWNLOAD_SIMPLE, DOWNLOAD_HINT).clicked()
        {
            self.exports.start_download(title, log, background);
        }
        let (icon, text) = self.exports.copy_face(ui);
        let copy = ui
            .add_enabled_ui(!self.exports.is_copying(), |ui| {
                strip::button(ui, icon, &format!("{text} ({COPY_LOG_SHORTCUT})"))
            })
            .inner;
        if copy.clicked() {
            self.exports.start_copy(log, background);
        }
        strip::separator(ui);
        let mut step = None;
        if strip::button(ui, icons::ARROW_DOWN, NEXT_MATCH_HINT).clicked() {
            step = Some(Step::Next);
        }
        if strip::button(ui, icons::ARROW_UP, PREVIOUS_MATCH_HINT).clicked() {
            step = Some(Step::Previous);
        }
        let note = ui.scope(|ui| strip::note(ui, &self.search.status(), 0.0)).response;
        self.search.explain_failure(note);
        let step = self.search.field(ui, log, background, STRIP_SEARCH_WIDTH).or(step);
        self.step(step);
        if let Some(line) = log.first_error
            && strip::button(ui, icons::WARNING_CIRCLE, "Jump to the first error").clicked()
        {
            self.scroll_to = Some(line);
        }
    }

    /// `font_size` is the code font size from the settings, the one place it is set (and zoomed).
    /// Returns the area of the visible lines, for its context menu.
    fn lines(&mut self, ui: &mut Ui, log: &Arc<LoadedLog>, font_size: f32) -> Lines {
        let palette = theme::palette(ui.ctx());
        let font = theme::fonts::code_font(font_size);
        // Any other size, however close, lays the lines out at other widths.
        if self.widest_at.to_bits() != font_size.to_bits() {
            self.widest_line = 0.0;
            self.widest_at = font_size;
        }
        let row_height = (font.size * 1.5).round();
        let line_count = log.file.line_count();
        let (scroll, placed) = self.scroll_area(ui, line_count, row_height);
        let look = LineLook {
            font: &font,
            palette: &palette,
            gutter: gutter_width(font.size, line_count),
            row_height,
            fill: selecting::Fill {
                color: palette.text_selection,
                char_width: ui.fonts_mut(|fonts| fonts.glyph_width(&font, '0')),
            },
        };
        let text_id = text_id(ui);
        let mut read_error = None;
        let output = ui
            .scope(|ui| {
                let scroll = cv_widgets::scroll::placed(ui, scroll, placed);
                scroll.show_rows(ui, row_height, line_count, |ui, range| {
                    if let Err(error) = self.ensure_window(log, range.clone()) {
                        read_error = Some(error.to_string());
                        return None;
                    }
                    self.visible_lines(ui, &look, range, line_count, text_id)
                })
            })
            .inner;
        #[cfg(any(test, feature = "test-support"))]
        note_top_line(ui, output.state.clamped_offset().y, row_height);
        let limit = output.content_size.y - output.inner_rect.height();
        self.selecting.scroll_while_dragging(
            ui,
            output.inner_rect,
            output.state.clamped_offset().y,
            limit,
        );
        // Ctrl+wheel over the log zooms the code font; middle-click autoscroll goes both ways.
        surfaces::mark_code(ui, output.inner_rect);
        surfaces::mark_horizontal_scroll(ui, output.inner_rect);
        if let Some(message) = read_error {
            let retry = [states::Action::primary(icons::ARROW_CLOCKWISE, "Retry")];
            let is_retried = states::error_with_actions(ui, &message, &retry).is_some();
            return Lines::Unread { is_retried };
        }
        output.inner.map_or(Lines::Unread { is_retried: false }, Lines::Shown)
    }

    /// The scroll area over every line, and the offset it goes to this frame: the end of a live
    /// log, a match or the first error, or where dragging a selection past the edge takes it.
    fn scroll_area(
        &mut self,
        ui: &Ui,
        line_count: usize,
        row_height: f32,
    ) -> (egui::ScrollArea, Option<f32>) {
        // A live log follows its end while the view is there; scrolled up, it keeps its place.
        let scroll = egui::ScrollArea::both()
            .id_salt("log_lines")
            .auto_shrink(false)
            .stick_to_bottom(self.fetch.is_live);
        if std::mem::take(&mut self.fetch.to_end) && self.fetch.is_live {
            // Past the end on purpose: the area clamps it to the end, which then sticks.
            let end = (line_count as f32 + 1.0) * (row_height + ui.spacing().item_spacing.y);
            return (scroll, Some(end));
        }
        if let Some(line) = self.scroll_to.take() {
            let height = ui.available_height();
            return (scroll, Some((line as f32 * row_height - height / 3.0).max(0.0)));
        }
        (scroll, self.selecting.scroll_offset.take())
    }

    /// The lines in `range`, read in by [`Self::ensure_window`], under one widget (`text_id`)
    /// that takes presses, drags and focus; the lines only paint.
    fn visible_lines(
        &mut self,
        ui: &mut Ui,
        look: &LineLook<'_>,
        range: Range<usize>,
        line_count: usize,
        text_id: Id,
    ) -> Option<Response> {
        let Some((window_range, window)) = &self.window else {
            return None;
        };
        ui.set_min_width(self.widest_line + look.gutter + 16.0);
        let spacing = ui.spacing().item_spacing.y;
        let width = ui.available_width().max(self.widest_line + look.gutter);
        let visible_top = ui.max_rect().top();
        let rows = selecting::Rows {
            top: visible_top - range.start as f32 * (look.row_height + spacing),
            pitch: look.row_height + spacing,
            text_left: ui.max_rect().left() + look.gutter,
            line_count,
            font: look.font,
        };
        let visible = Rect::from_min_size(
            pos2(ui.max_rect().left(), visible_top),
            vec2(width, range.len() as f32 * rows.pitch),
        );
        let area = ui.interact(visible, text_id, Sense::click_and_drag());
        area.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Document, true, self.selecting.label())
        });
        let line_text = |line: usize| {
            line.checked_sub(window_range.start)
                .and_then(|index| window.get(index))
                .map(String::as_str)
        };
        self.selecting.track_pointer(ui, &area, &rows, look.palette, line_text);
        let matches = self.search.matches();
        let current = self.search.current_line();
        for line_index in range {
            let text = &window[line_index - window_range.start];
            let highlight = if current == Some(line_index) {
                Highlight::Current
            } else if matches.is_some_and(|matches| matches.binary_search(&line_index).is_ok()) {
                Highlight::Match
            } else {
                Highlight::None
            };
            let (rect, _) = ui.allocate_exact_size(vec2(width, look.row_height), Sense::hover());
            let selected = self.selecting.on_line(line_index, text);
            let text_width = paint_line(ui, look, rect, line_index, text, highlight, selected);
            self.widest_line = self.widest_line.max(text_width);
        }
        Some(area)
    }

    fn ensure_window(&mut self, log: &LoadedLog, visible: Range<usize>) -> std::io::Result<()> {
        let is_covered = self
            .window
            .as_ref()
            .is_some_and(|(range, _)| range.start <= visible.start && visible.end <= range.end);
        if is_covered {
            return Ok(());
        }
        let start = visible.start.saturating_sub(READ_AHEAD);
        let end = (visible.end + READ_AHEAD).min(log.file.line_count());
        let lines = log.file.read_lines(start..end)?;
        self.window = Some((start..start + lines.len(), lines));
        Ok(())
    }
}

/// Tells the walkthroughs which line is at the top of the view and how far it is scrolled past.
#[cfg(any(test, feature = "test-support"))]
fn note_top_line(ui: &Ui, offset: f32, row_height: f32) {
    let pitch = row_height + ui.spacing().item_spacing.y;
    let line = (offset / pitch).floor().max(0.0);
    focus_mode::note_top_line(
        ui.ctx(),
        focus_mode::Surface::Log,
        line as usize,
        offset - line * pitch,
    );
}

/// The widget under the visible lines: it takes the pointer and holds the keyboard focus.
fn text_id(ui: &Ui) -> Id {
    ui.scope_id().with("log_text")
}

mod exports;
mod fetch;
mod paint;
pub(crate) mod panes;
mod raw_log;
mod search;
mod selecting;

#[cfg(test)]
mod copy_tests;

#[cfg(test)]
mod frame_budget;

#[cfg(test)]
mod placed_scroll_tests;

#[cfg(test)]
mod selection_tests;

#[cfg(test)]
mod test_harness;
