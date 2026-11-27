//! The agent's work on a turn as one quiet line between the question and the answer: "Working ·
//! 9 s · 3 steps" while it runs, "Thought 6 s · read 3 files · 1 search · ran 1 command" once it
//! is done. Clicked (or Enter or Space on focus), it shows the steps below it: the latest thinking
//! while the turn runs, then one row per tool call with its status, verb and detail.
//!
//! A running turn shows its steps, a finished one only its line; when the turn ends the steps
//! fold away (at once under reduced motion: egui's collapse animation takes the theme's animation
//! time). The open state is view state per turn in egui's memory, never stored with the chat. The
//! running clock starts when the view first sees the turn working, or at its first step if that
//! came before.

use std::fmt::Write as _;
use std::time::Duration;

use cv_base::theme::{self, icons, style};
use cv_widgets::numbers::counted;
use cv_widgets::{buttons, elide, focus, row, states};
use egui::collapsing_header::CollapsingState;
use egui::{
    Align2, Color32, FontId, Id, Rect, Response, RichText, Sense, Ui, WidgetInfo, pos2, vec2,
};
use review_agent::StepKind;
use review_store::{ChatStep, ChatStepKind};

use super::super::transcript;

/// Whether the agent still works on the turn.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Working,
    Finished,
}

/// A step's state as its row shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Done,
    Running,
    Failed,
}

/// Height of a step row: a small icon with even room above and below.
const STEP_ROW: f32 = icons::SIZE + style::SPACE_S;
/// Characters of thinking that make about two lines of the excerpt, at most.
const EXCERPT_CHARS: usize = 240;

/// Draws the activity of a turn with `steps`; `id` names the turn across frames. False when it
/// drew nothing: a finished turn without steps.
pub fn show(ui: &mut Ui, id: Id, steps: &[ChatStep], phase: Phase) -> bool {
    let now = transcript::now_ms();
    let text = match phase {
        Phase::Working => working_line(elapsed_ms(ui.ctx(), id, steps, now), steps),
        Phase::Finished => match finished_line(steps) {
            Some(text) => text,
            None => return false,
        },
    };
    let has_rows = steps.iter().any(|step| step.kind != Some(ChatStepKind::Thinking));
    let can_open = phase == Phase::Working || has_rows;
    let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, phase == Phase::Working);
    let phase_id = id.with("phase");
    let was_working = ui.data(|data| data.get_temp::<bool>(phase_id)).unwrap_or(false);
    if was_working && phase == Phase::Finished {
        state.set_open(false);
    }
    ui.data_mut(|data| data.insert_temp(phase_id, phase == Phase::Working));

    let open = can_open.then_some(state.is_open());
    if line(ui, &text, open).clicked() && can_open {
        state.toggle(ui);
    }
    state.show_body_unindented(ui, |ui| body(ui, steps, phase));
    true
}

/// The running line of a working turn with its latest step, "Working · 3 s · Reading src/a.rs",
/// for the mini chat; `id` is the turn's, so its clock is the one the conversation shows.
pub fn working_summary(ctx: &egui::Context, id: Id, steps: &[ChatStep]) -> String {
    let elapsed = elapsed_ms(ctx, id, steps, transcript::now_ms());
    let mut text = format!("Working · {}", seconds(elapsed));
    if let Some(step) = steps.iter().rev().find(|step| step.kind != Some(ChatStepKind::Thinking)) {
        let verb = verb(kind_of(step), &step.tool, status(step, Phase::Working) == Status::Running);
        // Writing to a String cannot fail.
        let _infallible = write!(text, " · {verb} {}", step.detail);
    }
    text
}

/// Milliseconds since the turn started working, for the running clock; asks for the next repaint
/// when the shown second turns.
fn elapsed_ms(ctx: &egui::Context, id: Id, steps: &[ChatStep], now: u64) -> u64 {
    let since_id = id.with("since");
    let seen = ctx.data_mut(|data| *data.get_temp_mut_or_insert_with(since_id, || now));
    let first_step = steps.iter().filter_map(|step| step.started_ms).min();
    let since = first_step.map_or(seen, |first| first.min(seen));
    let elapsed = now.saturating_sub(since);
    ctx.request_repaint_after(Duration::from_millis(1_000 - elapsed % 1_000));
    elapsed
}

/// The clickable summary line: chevron (when it opens, `open` says how it is) and text, as tall
/// as a subtle button. Screen readers get the whole text and whether it is expanded.
fn line(ui: &mut Ui, text: &str, open: Option<bool>) -> Response {
    let palette = theme::palette(ui.ctx());
    let font = FontId::proportional(style::SMALL_SIZE);
    let chevron = if open.is_some() { icons::SIZE_SMALL + style::SPACE_S } else { 0.0 };
    let inset = style::SPACE_S;
    let room = (ui.available_width() - 2.0 * inset - chevron).max(0.0);
    let shown = elide::reference(ui, text, &font, room);
    let text_width = elide::text_width(ui, &shown, &font);
    let size = vec2(2.0 * inset + chevron + text_width, buttons::SUBTLE_HEIGHT);
    let sense = if open.is_some() { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    if open.is_some() {
        row::card_background(ui, &response, rect, Color32::TRANSPARENT, style::RADIUS_CONTROL);
        focus::ring(ui, &response, rect);
    }
    let mut x = rect.left() + inset;
    if let Some(is_open) = open {
        let icon = if is_open { icons::CARET_DOWN } else { icons::CARET_RIGHT };
        let center = pos2(x + icons::SIZE_SMALL / 2.0, rect.center().y);
        let icon_font = icons::rest_font(&palette, icons::SIZE_SMALL);
        ui.painter().text(center, Align2::CENTER_CENTER, icon, icon_font, palette.text_secondary);
        x += chevron;
    }
    let anchor = pos2(x, rect.center().y);
    ui.painter().text(anchor, Align2::LEFT_CENTER, shown, font, palette.text_secondary);
    let role = if open.is_some() { egui::Role::Button } else { egui::Role::Label };
    response.widget_info(|| WidgetInfo::labeled(role, true, text));
    if let Some(is_open) = open {
        ui.ctx().accesskit_node_builder(response.id, |node| node.set_expanded(is_open));
    }
    response.on_hover_text(text)
}

/// The steps under the line, indented to its text: the latest thinking while the turn works,
/// then one row per tool call.
fn body(ui: &mut Ui, steps: &[ChatStep], phase: Phase) {
    let indent = style::SPACE_S + icons::SIZE_SMALL + style::SPACE_S;
    ui.horizontal(|ui| {
        ui.add_space(indent);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(style::SPACE_XS);
            if phase == Phase::Working {
                excerpt(ui, steps);
            }
            for step in steps.iter().filter(|step| step.kind != Some(ChatStepKind::Thinking)) {
                step_row(ui, step, status(step, phase));
            }
            ui.add_space(style::SPACE_XS);
        });
    });
}

/// The end of the latest thinking, italic and muted, cut at its start.
fn excerpt(ui: &mut Ui, steps: &[ChatStep]) {
    let latest = steps.iter().rev().find(|step| step.kind == Some(ChatStepKind::Thinking));
    let Some(text) = latest.map(|step| tail(&step.thinking, EXCERPT_CHARS)) else {
        return;
    };
    if text.is_empty() {
        return;
    }
    let palette = theme::palette(ui.ctx());
    let label = RichText::new(text).italics().size(style::SMALL_SIZE).color(palette.text_secondary);
    ui.add(egui::Label::new(label).wrap());
    ui.add_space(style::SPACE_XS);
}

/// One tool call: status icon, verb and detail in the code font, centered in a row of their own.
fn step_row(ui: &mut Ui, step: &ChatStep, status: Status) {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), STEP_ROW), Sense::hover());
    let icon = Rect::from_center_size(
        pos2(rect.left() + icons::SIZE_SMALL / 2.0, rect.center().y),
        vec2(icons::SIZE_SMALL, icons::SIZE_SMALL),
    );
    status_icon(ui, icon, status);
    let kind = kind_of(step);
    let verb = verb(kind, &step.tool, status == Status::Running);
    let font = FontId::proportional(style::SMALL_SIZE);
    let verb_x = icon.right() + style::SPACE_S;
    let verb_rect = ui.painter().text(
        pos2(verb_x, rect.center().y),
        Align2::LEFT_CENTER,
        verb,
        font,
        palette.text_secondary,
    );
    let code = theme::fonts::code_font(style::SMALL_SIZE);
    let detail_x = verb_rect.right() + style::SPACE_S;
    let room = (rect.right() - detail_x).max(0.0);
    let detail = match kind {
        StepKind::Read | StepKind::Edit => middle(ui, &step.detail, &code, room),
        StepKind::Search | StepKind::Run | StepKind::Other => {
            elide::reference(ui, &step.detail, &code, room)
        }
    };
    let anchor = pos2(detail_x, rect.center().y);
    ui.painter().text(anchor, Align2::LEFT_CENTER, detail, code, palette.text);
    let label = format!("{verb} {}", step.detail);
    let state = match status {
        Status::Done => "done",
        Status::Running => "running",
        Status::Failed => "failed",
    };
    response.widget_info(|| WidgetInfo::labeled(egui::Role::ListItem, true, &label));
    ui.ctx().accesskit_node_builder(response.id, |node| node.set_description(state));
    response.on_hover_text(label);
}

/// Check in the success colour, an error mark, or the busy mark in the accent colour.
fn status_icon(ui: &Ui, rect: Rect, status: Status) {
    let palette = theme::palette(ui.ctx());
    let (glyph, color) = match status {
        Status::Done => (icons::CHECK, palette.success),
        Status::Failed => (icons::X, palette.error),
        Status::Running => {
            states::spinner(ui, rect, palette.accent);
            return;
        }
    };
    let font = icons::rest_font(&palette, icons::SIZE_SMALL);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, font, color);
}

fn status(step: &ChatStep, phase: Phase) -> Status {
    let is_open = step.started_ms.is_some() && step.duration_ms.is_none();
    if step.is_failed {
        Status::Failed
    } else if phase == Phase::Working && is_open {
        Status::Running
    } else {
        Status::Done
    }
}

/// The kind of a tool step; chats saved before kinds take it from the tool's name.
fn kind_of(step: &ChatStep) -> StepKind {
    match step.kind {
        Some(ChatStepKind::Read) => StepKind::Read,
        Some(ChatStepKind::Search) => StepKind::Search,
        Some(ChatStepKind::Run) => StepKind::Run,
        Some(ChatStepKind::Edit) => StepKind::Edit,
        Some(ChatStepKind::Other) => StepKind::Other,
        Some(ChatStepKind::Thinking) | None => StepKind::of_tool(&step.tool),
    }
}

/// "Read" or "Reading", ...; other tools by their own name.
fn verb(kind: StepKind, tool: &str, is_running: bool) -> &str {
    match (kind, is_running) {
        (StepKind::Read, false) => "Read",
        (StepKind::Read, true) => "Reading",
        (StepKind::Search, false) => "Searched",
        (StepKind::Search, true) => "Searching",
        (StepKind::Run, false) => "Ran",
        (StepKind::Run, true) => "Running",
        (StepKind::Edit, false) => "Edited",
        (StepKind::Edit, true) => "Editing",
        (StepKind::Other, _) => tool,
    }
}

/// "Working · 9 s · 3 steps"; the steps are tool calls.
fn working_line(elapsed_ms: u64, steps: &[ChatStep]) -> String {
    let mut text = format!("Working · {}", seconds(elapsed_ms));
    let count = steps.iter().filter(|step| step.kind != Some(ChatStepKind::Thinking)).count();
    if count > 0 {
        // Writing to a String cannot fail.
        let _infallible = write!(text, " · {}", counted(count, "step", "steps"));
    }
    text
}

/// "Thought 6 s · read 3 files · 1 search · ran 1 command": the thinking time, then each kind of
/// tool call that happened; `None` without either.
fn finished_line(steps: &[ChatStep]) -> Option<String> {
    let mut parts = Vec::new();
    let thinking: Vec<&ChatStep> =
        steps.iter().filter(|step| step.kind == Some(ChatStepKind::Thinking)).collect();
    if !thinking.is_empty() {
        let durations = thinking.iter().filter_map(|step| step.duration_ms);
        let total = durations.reduce(|sum, duration| sum + duration);
        parts.push(
            total.map_or_else(|| "Thought".to_owned(), |ms| format!("Thought {}", seconds(ms))),
        );
    }
    let kinds: Vec<StepKind> = steps
        .iter()
        .filter(|step| step.kind != Some(ChatStepKind::Thinking))
        .map(kind_of)
        .collect();
    let count = |kind: StepKind| kinds.iter().filter(|seen| **seen == kind).count();
    let kinds_shown = [
        (StepKind::Read, "read", "file", "files"),
        (StepKind::Search, "", "search", "searches"),
        (StepKind::Run, "ran", "command", "commands"),
        (StepKind::Edit, "edited", "file", "files"),
        (StepKind::Other, "used", "tool", "tools"),
    ];
    for (kind, verb, one, many) in kinds_shown {
        let count = count(kind);
        if count == 0 {
            continue;
        }
        let calls = counted(count, one, many);
        parts.push(if verb.is_empty() { calls } else { format!("{verb} {calls}") });
    }
    let text = parts.join(" · ");
    let mut chars = text.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

/// "6 s", "1 min 5 s"; at least a second, so a short stretch never reads as none.
fn seconds(ms: u64) -> String {
    let total = (ms.saturating_add(500) / 1_000).max(1);
    match (total / 60, total % 60) {
        (0, seconds) => format!("{seconds} s"),
        (minutes, 0) => format!("{minutes} min"),
        (minutes, seconds) => format!("{minutes} min {seconds} s"),
    }
}

/// The last `max` characters of `text` in one line, "…" in front when cut.
fn tail(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = flat.chars().count();
    if count <= max {
        return flat;
    }
    let kept: String = flat.chars().skip(count - max).collect();
    format!("…{}", kept.trim_start())
}

/// `text` cut in the middle to fit `width` in `font`: a path keeps its start and its file name.
fn middle(ui: &Ui, text: &str, font: &FontId, width: f32) -> String {
    fit_middle(text, width, |candidate| elide::text_width(ui, candidate, font))
}

/// The longest form of `text`, cut in the middle, that `measure` says fits `width`.
fn fit_middle(text: &str, width: f32, measure: impl Fn(&str) -> f32) -> String {
    if measure(text) <= width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let cut = |kept: usize| {
        let head: String = chars.iter().take(kept / 2).collect();
        let tail: String = chars.iter().skip(chars.len() - kept.div_ceil(2)).collect();
        format!("{head}…{tail}")
    };
    // Binary search over the kept characters: fewer never get wider.
    let (mut low, mut high) = (0, chars.len().saturating_sub(1));
    while low < high {
        let middle = (low + high).div_ceil(2);
        if measure(&cut(middle)) <= width {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    cut(low)
}

#[cfg(test)]
mod tests {
    use review_store::ChatStep;
    use review_store::ChatStepKind::{self, Edit, Other, Read, Run, Search, Thinking};

    use super::{finished_line, fit_middle, seconds, tail, working_line};

    fn tool(tool: &str, kind: Option<ChatStepKind>) -> ChatStep {
        ChatStep { tool: tool.into(), kind, ..ChatStep::default() }
    }

    fn thought(duration_ms: Option<u64>) -> ChatStep {
        ChatStep { kind: Some(Thinking), duration_ms, ..ChatStep::default() }
    }

    #[test]
    fn the_finished_line_counts_each_kind_and_leaves_out_the_rest() {
        let steps = [
            thought(Some(2_400)),
            tool("Read", Some(Read)),
            tool("Grep", Some(Search)),
            thought(Some(3_700)),
            tool("Read", Some(Read)),
            tool("commandExecution", Some(Read)),
            tool("Bash", Some(Run)),
        ];
        let line = finished_line(&steps);
        assert_eq!(line.as_deref(), Some("Thought 6 s · read 3 files · 1 search · ran 1 command"));
        let steps = [tool("Bash", Some(Run)), tool("Bash", Some(Run)), tool("Edit", Some(Edit))];
        assert_eq!(finished_line(&steps).as_deref(), Some("Ran 2 commands · edited 1 file"));
        let steps =
            [tool("Grep", Some(Search)), tool("Glob", Some(Search)), tool("mcp/x", Some(Other))];
        assert_eq!(finished_line(&steps).as_deref(), Some("2 searches · used 1 tool"));
        assert_eq!(finished_line(&[]), None, "no activity, no line");
    }

    #[test]
    fn older_steps_count_by_their_tool_name_and_thinking_without_time_is_just_thought() {
        let steps = [thought(None), tool("Read", None), tool("Grep", None)];
        assert_eq!(finished_line(&steps).as_deref(), Some("Thought · read 1 file · 1 search"));
    }

    #[test]
    fn the_working_line_shows_seconds_and_tool_steps() {
        assert_eq!(working_line(9_200, &[thought(None)]), "Working · 9 s");
        let steps =
            [thought(None), tool("Read", Some(Read)), tool("Bash", Some(Run)), tool("Grep", None)];
        assert_eq!(working_line(9_200, &steps), "Working · 9 s · 3 steps");
        assert_eq!(working_line(0, &steps[..2]), "Working · 1 s · 1 step");
    }

    #[test]
    fn durations_round_to_seconds_and_minutes() {
        let shown: Vec<String> = [0, 1_499, 1_500, 59_400, 60_000, 65_000].map(seconds).into();
        assert_eq!(shown, ["1 s", "1 s", "2 s", "59 s", "1 min", "1 min 5 s"]);
    }

    #[test]
    fn the_thinking_excerpt_keeps_its_end() {
        assert_eq!(tail("  I should\nread it. ", 40), "I should read it.");
        assert_eq!(tail("The parser branches on the first byte.", 15), "…the first byte.");
    }

    #[test]
    fn paths_are_cut_in_the_middle() {
        let fit = |text: &str, width: usize| {
            fit_middle(text, width as f32, |text| text.chars().count() as f32)
        };
        assert_eq!(fit("src/a.rs", 8), "src/a.rs");
        assert_eq!(fit("crates/review-diff/src/parse.rs", 17), "crates/r…parse.rs");
        assert_eq!(fit("crates/review-diff/src/parse.rs", 1), "…");
    }
}
