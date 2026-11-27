//! The head of a check shown full width: back to the checks, the switcher ("Pipeline #2279,
//! 1 of 10") with a searchable list of every check, previous/next (K/J), the check's trigger or
//! stage and duration, "Open on …" and its reruns.

use cv_base::theme::{self, icons, motion, style};
use cv_widgets::{badges, buttons, glyph, row};
use egui::{
    Align2, FontId, Key, Modifiers, PopupCloseBehavior, Rect, Sense, Stroke, StrokeKind, TextEdit,
    Ui, UiBuilder, pos2, vec2,
};
use review_core::ProviderKind;

use super::checks::{self, Check, CheckKey};
use super::graph::stage_card::ellipsized;
use super::rerun::{Picked, RerunButton};

const HEIGHT: f32 = 36.0;
const POPUP_WIDTH: f32 = 420.0;
const POPUP_LIST_HEIGHT: f32 = 420.0;
const GROUP_HEIGHT: f32 = 28.0;
/// The switcher shows this much of the check's name, and cuts it to no less than the minimum.
const SWITCHER_NAME_MAX: f32 = 360.0;
const SWITCHER_NAME_MIN: f32 = 80.0;
/// Rows narrower than this show "Open on …" as the platform mark alone.
const COMPACT_ROW: f32 = 1000.0;

pub enum HeaderAction {
    /// Back to the checks table.
    Back,
    Select(CheckKey),
    Rerun(Picked),
    OpenUrl(String),
}

pub struct HeaderInput<'a> {
    /// The check shown: the run, or the job whose log is open.
    pub check: Check<'a>,
    /// Every check in the table's order (failed, running, then the platform's).
    pub order: &'a [Check<'a>],
    pub platform: ProviderKind,
    pub has_token: bool,
    pub is_busy: bool,
    pub web_url: Option<&'a str>,
    pub open_label: &'a str,
}

/// The switcher's search text and the option the keyboard is on.
#[derive(Default)]
pub struct RunHeader {
    search: String,
    highlighted: Option<CheckKey>,
    was_open: bool,
}

impl RunHeader {
    pub fn show(&mut self, ui: &mut Ui, input: &HeaderInput<'_>) -> Option<HeaderAction> {
        let (row_rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::hover());
        let mut right = ui.new_child(
            UiBuilder::new()
                .id_salt("run_header_right")
                .max_rect(row_rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        right.spacing_mut().item_spacing.x = style::SPACE_S;
        let mut action = right_controls(&mut right, input, row_rect.width());
        let right_width = right.min_rect().width();
        let left_rect = Rect::from_min_max(
            row_rect.min,
            pos2(row_rect.right() - right_width - style::SPACE_L, row_rect.bottom()),
        );
        let mut left = ui.new_child(
            UiBuilder::new()
                .id_salt("run_header_left")
                .max_rect(left_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        left.spacing_mut().item_spacing.x = style::SPACE_S;
        if buttons::icon(&mut left, icons::ARROW_LEFT, "Back to the checks (Esc)").clicked() {
            action = Some(HeaderAction::Back);
        }
        // The switcher leaves room for the previous/next steps after it.
        let steps = 2.0 * buttons::SUBTLE_HEIGHT + 2.0 + style::SPACE_S;
        let switcher_room = (left.available_width() - steps).max(0.0);
        if let Some(picked) = self.switcher(&mut left, input, switcher_room) {
            action = Some(picked);
        }
        if let Some(picked) = step_buttons(&mut left, input) {
            action = Some(picked);
        }
        left.add_space(style::SPACE_S);
        paint_meta(&mut left, &meta_text(input.check));
        action
    }

    /// The button naming the run and its place in the list, at most `max_width` wide (the name
    /// gives way), and the list it opens.
    fn switcher(
        &mut self,
        ui: &mut Ui,
        input: &HeaderInput<'_>,
        max_width: f32,
    ) -> Option<HeaderAction> {
        let palette = theme::palette(ui.ctx());
        let check = input.check;
        let current = check.key();
        let position =
            input.order.iter().position(|other| other.is(&current)).map_or(0, |index| index + 1);
        let place = format!("{position} of {}", input.order.len());
        let name_font = theme::fonts::semibold_font(style::SECTION_SIZE);
        let small = FontId::proportional(style::SMALL_SIZE);
        let name = ui.painter().layout_no_wrap(check.name().to_owned(), name_font, palette.text);
        let place_width = ui.painter().layout_no_wrap(place.clone(), small, palette.text).size().x;
        let chrome = style::SPACE_M * 2.0
            + 18.0
            + style::SPACE_S
            + style::SPACE_S
            + place_width
            + style::SPACE_S
            + 14.0;
        let name_width =
            name.size().x.min(SWITCHER_NAME_MAX).min((max_width - chrome).max(SWITCHER_NAME_MIN));
        let width = chrome + name_width;
        let (rect, button) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click());
        let popup_id = egui::Popup::default_response_id(&button);
        let is_open = egui::Popup::is_id_open(ui.ctx(), popup_id);
        let label = format!("{}, {place}. Switch check", check.name());
        button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, &label));
        let face = SwitcherFace { check, place: &place, name_width, is_open };
        let is_cut = face.paint(ui, &button, rect);
        let tooltip = if is_cut {
            format!("{}\nSwitch to another check", check.name())
        } else {
            "Switch to another check".to_owned()
        };
        let button = button.on_hover_text(tooltip);

        let mut action = None;
        let shown = egui::Popup::from_toggle_button_response(&button)
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .width(POPUP_WIDTH)
            .gap(style::SPACE_XS)
            .accessible_name("Switch check")
            .show(|ui| self.list(ui, input));
        if let Some(inner) = shown {
            if let Some(picked) = inner.inner {
                egui::Popup::close_id(ui.ctx(), popup_id);
                action = Some(picked);
            }
            self.was_open = true;
        } else if self.was_open {
            self.was_open = false;
            self.search.clear();
            self.highlighted = None;
        }
        action
    }

    /// Search box and the grouped options; returns the run picked.
    fn list(&mut self, ui: &mut Ui, input: &HeaderInput<'_>) -> Option<HeaderAction> {
        let just_opened = !self.was_open;
        let options = grouped(input.order, &self.search);
        let flat: Vec<Check<'_>> =
            options.iter().flat_map(|(_, checks)| checks.iter().copied()).collect();
        let current = input.check.key();
        let mut keys = self.keys(ui, &flat, &current);
        let mut picked = keys.picked.take();

        ui.add_space(style::SPACE_XS);
        let search = ui.add(
            TextEdit::singleline(&mut self.search)
                .hint_text(format!("{}  Jump to check…", icons::MAGNIFYING_GLASS))
                .desired_width(f32::INFINITY),
        );
        search.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::SearchInput, true, "Jump to check")
        });
        glyph::hint_as(&search, "Jump to check…");
        if just_opened {
            search.request_focus();
        }
        ui.add_space(style::SPACE_XS);
        let list = OptionList { options: &options, flat: &flat, current: &current, keys: &keys };
        if let Some(clicked) = list.show(ui) {
            picked = Some(clicked);
        }
        ui.add_space(style::SPACE_XS);
        ui.separator();
        let palette = theme::palette(ui.ctx());
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(style::SPACE_S as i8, style::SPACE_XS as i8))
            .show(ui, |ui| {
                let hint =
                    "↑/↓ selects, Enter opens. J and K step through the checks without this list.";
                ui.label(
                    egui::RichText::new(hint).size(style::SMALL_SIZE).color(palette.text_secondary),
                );
            });
        picked
    }

    /// The arrow keys move the highlight through `flat`, Enter picks it and Escape closes the
    /// list.
    fn keys(&mut self, ui: &Ui, flat: &[Check<'_>], current: &CheckKey) -> ListKeys {
        let (up, down, enter, escape) = ui.input_mut(|keys| {
            (
                keys.consume_key(Modifiers::NONE, Key::ArrowUp),
                keys.consume_key(Modifiers::NONE, Key::ArrowDown),
                keys.consume_key(Modifiers::NONE, Key::Enter),
                keys.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        let highlighted =
            self.highlighted.as_ref().and_then(|key| flat.iter().position(|check| check.is(key)));
        let at = highlighted.or_else(|| flat.iter().position(|check| check.is(current)));
        let moved = match (at, up, down) {
            (Some(index), true, _) => Some(index.saturating_sub(1)),
            (Some(index), _, true) => Some((index + 1).min(flat.len().saturating_sub(1))),
            (None, true, _) | (None, _, true) if !flat.is_empty() => Some(0),
            _ => None,
        };
        if let Some(index) = moved {
            self.highlighted = Some(flat[index].key());
        }
        if escape {
            egui::Popup::close_all(ui.ctx());
        }
        let highlighted = moved.or(at).map(|index| flat[index].key());
        let picked = highlighted.clone().filter(|_| enter).map(HeaderAction::Select);
        ListKeys { highlighted, has_moved: moved.is_some(), picked }
    }
}

/// What the keyboard did in the switcher's list this frame.
struct ListKeys {
    /// The option the keyboard is on.
    highlighted: Option<CheckKey>,
    /// The highlight moved this frame, so its option scrolls into view.
    has_moved: bool,
    /// Enter on the highlighted option.
    picked: Option<HeaderAction>,
}

/// The switcher's grouped options under their titles.
struct OptionList<'a, 'c> {
    options: &'a [(String, Vec<Check<'c>>)],
    flat: &'a [Check<'c>],
    current: &'a CheckKey,
    keys: &'a ListKeys,
}

impl OptionList<'_, '_> {
    /// Returns the option clicked.
    fn show(&self, ui: &mut Ui) -> Option<HeaderAction> {
        let palette = theme::palette(ui.ctx());
        let mut picked = None;
        egui::ScrollArea::vertical().id_salt("run_switcher").max_height(POPUP_LIST_HEIGHT).show(
            ui,
            |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if self.flat.is_empty() {
                    ui.add_space(style::SPACE_S);
                    ui.label(
                        egui::RichText::new("No check matches. Clear the text to see them all.")
                            .color(palette.text_secondary),
                    );
                    ui.add_space(style::SPACE_S);
                }
                for (title, checks) in self.options {
                    group_title(ui, title);
                    for check in checks {
                        let key = check.key();
                        let is_highlighted = self.keys.highlighted.as_ref() == Some(&key);
                        let is_current = key == *self.current;
                        let scroll_into_view = self.keys.has_moved && is_highlighted;
                        if option(ui, check, is_current, is_highlighted, scroll_into_view) {
                            picked = Some(HeaderAction::Select(key));
                        }
                    }
                }
            },
        );
        picked
    }
}

/// A group's title above its options.
fn group_title(ui: &mut Ui, title: &str) {
    let palette = theme::palette(ui.ctx());
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), GROUP_HEIGHT), Sense::hover());
    ui.painter().text(
        pos2(rect.left() + style::SPACE_S, rect.bottom() - style::SPACE_XS),
        Align2::LEFT_BOTTOM,
        title,
        theme::fonts::semibold_font(style::SMALL_SIZE),
        palette.text_secondary,
    );
}

/// What the switcher button shows: the check's status and name, its place and the caret.
struct SwitcherFace<'a, 'c> {
    check: Check<'c>,
    place: &'a str,
    name_width: f32,
    is_open: bool,
}

impl SwitcherFace<'_, '_> {
    /// Paints the button into `rect`; returns true when the name had to be cut.
    fn paint(&self, ui: &Ui, button: &egui::Response, rect: Rect) -> bool {
        let palette = theme::palette(ui.ctx());
        let check = self.check;
        let hover = row::hover_amount(ui, button);
        let fill = if button.is_pointer_button_down_on() {
            palette.pressed
        } else {
            motion::mix(palette.surface_subtle, palette.hover, hover)
        };
        let border = if self.is_open {
            palette.accent
        } else {
            motion::mix(palette.border_strong, palette.text_secondary, hover)
        };
        let painter = ui.painter();
        painter.rect(
            rect,
            style::RADIUS_CONTROL,
            fill,
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        );
        let (icon, color) = badges::run_status_icon(check.status(), &palette);
        let mut x = rect.left() + style::SPACE_M;
        painter.text(
            pos2(x + 9.0, rect.center().y),
            Align2::CENTER_CENTER,
            icon,
            FontId::proportional(18.0),
            color,
        );
        x += 18.0 + style::SPACE_S;
        let (name, is_cut) = ellipsized(
            ui,
            check.name(),
            theme::fonts::semibold_font(style::SECTION_SIZE),
            palette.text,
            self.name_width,
        );
        painter.galley(pos2(x, rect.center().y - name.size().y / 2.0), name, palette.text);
        x += self.name_width + style::SPACE_S;
        painter.text(
            pos2(x, rect.center().y),
            Align2::LEFT_CENTER,
            self.place,
            FontId::proportional(style::SMALL_SIZE),
            palette.text_secondary,
        );
        let caret = if self.is_open { icons::CARET_UP } else { icons::CARET_DOWN };
        painter.text(
            pos2(rect.right() - style::SPACE_M, rect.center().y),
            Align2::RIGHT_CENTER,
            caret,
            FontId::proportional(14.0),
            palette.text,
        );
        cv_widgets::focus::ring(ui, button, rect);
        is_cut
    }
}

/// Laid out right to left: the rerun button sits rightmost, "Open on …" left of it.
fn right_controls(right: &mut Ui, input: &HeaderInput<'_>, row_width: f32) -> Option<HeaderAction> {
    let mut action = None;
    if let Some(picked) = (RerunButton {
        reruns: input.check.reruns(),
        platform: input.platform,
        has_token: input.has_token,
        is_busy: input.is_busy,
        is_full_width: false,
    })
    .show(right)
    {
        action = Some(HeaderAction::Rerun(picked));
    }
    if let Some(url) = input.web_url {
        // A narrow row keeps "Open on …" as its platform mark (named on hover), so the
        // switcher and the steps beside it keep their room.
        let glyph = cv_widgets::provider_icon::glyph(input.platform);
        let open = if row_width < COMPACT_ROW {
            buttons::icon(right, glyph, input.open_label)
        } else {
            buttons::subtle(right, glyph, input.open_label)
        };
        if open.clicked() {
            action = Some(HeaderAction::OpenUrl(url.to_owned()));
        }
    }
    action
}

/// Previous and next check (K and J) after the switcher.
fn step_buttons(left: &mut Ui, input: &HeaderInput<'_>) -> Option<HeaderAction> {
    let current = input.check.key();
    let position = input.order.iter().position(|check| check.is(&current));
    left.spacing_mut().item_spacing.x = 2.0;
    let previous = position.and_then(|index| index.checked_sub(1)).map(|index| &input.order[index]);
    let next = position.and_then(|index| input.order.get(index + 1));
    let mut action = None;
    for (target, icon, tooltip) in [
        (previous, icons::CARET_UP, "Previous check (K)"),
        (next, icons::CARET_DOWN, "Next check (J)"),
    ] {
        let clicked = left
            .add_enabled_ui(target.is_some(), |ui| buttons::icon(ui, icon, tooltip).clicked())
            .inner;
        if clicked && let Some(target) = target {
            action = Some(HeaderAction::Select(target.key()));
        }
    }
    action
}

/// The check's trigger or run and stage, whether it waits to be started, its outcome and its
/// duration, joined by " · ".
fn meta_text(check: Check<'_>) -> String {
    let mut meta: Vec<String> = match check.job {
        None => check.run.description.iter().cloned().collect(),
        Some(_) => [Some(check.run.name.clone()), check.stage_name().map(str::to_owned)]
            .into_iter()
            .flatten()
            .collect(),
    };
    if check.is_manual() {
        meta.push("manual, not started".to_owned());
    }
    if let Some(outcome) = check.job.and_then(|(_, job)| super::attempts::outcome(job)) {
        meta.push(outcome);
    }
    if let Some(duration) = super::duration(check.started_at(), check.finished_at()) {
        meta.push(duration);
    }
    meta.join(" · ")
}

/// `meta` in the room left, cut with an ellipsis and named in full on hover.
fn paint_meta(left: &mut Ui, meta: &str) {
    let width = left.available_width();
    if width <= 40.0 || meta.is_empty() {
        return;
    }
    let palette = theme::palette(left.ctx());
    let (galley, is_cut) = ellipsized(
        left,
        meta,
        FontId::proportional(style::SMALL_SIZE + 1.0),
        palette.text_secondary,
        width,
    );
    let (rect, response) = left.allocate_exact_size(galley.size(), Sense::hover());
    left.painter().galley(rect.min, galley, palette.text_secondary);
    if is_cut {
        response.on_hover_text(meta);
    }
}

/// One option: status, name, duration. Returns true when clicked.
fn option(
    ui: &mut Ui,
    check: &Check<'_>,
    is_current: bool,
    is_highlighted: bool,
    scroll_into_view: bool,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), style::ROW_COMPACT), Sense::click());
    let label = check.label();
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::ListBoxOption, true, is_current, &label)
    });
    if scroll_into_view {
        ui.scroll_to_rect(rect, None);
    }
    let hover = row::hover_amount(ui, &response).max(if is_highlighted { 1.0 } else { 0.0 });
    let fill = if is_current { palette.selection } else { palette.hover.gamma_multiply(hover) };
    ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
    let (icon, color) = badges::run_status_icon(check.status(), &palette);
    ui.painter().text(
        pos2(rect.left() + style::SPACE_S + 8.0, rect.center().y),
        Align2::CENTER_CENTER,
        icon,
        icons::rest_font(&palette, icons::SIZE),
        color,
    );
    let small = FontId::proportional(style::SMALL_SIZE);
    let duration =
        if check.is_manual() { Some("manual".to_owned()) } else { checks::check_duration(check) };
    let duration_width = if duration.is_some() { 72.0 } else { 0.0 };
    let left = rect.left() + style::SPACE_S + 16.0 + style::SPACE_S;
    let room = rect.right() - style::SPACE_S - duration_width - left;
    let (name, is_cut) =
        ellipsized(ui, check.name(), FontId::proportional(style::BODY_SIZE), palette.text, room);
    let name_width = name.size().x;
    ui.painter().galley(pos2(left, rect.center().y - name.size().y / 2.0), name, palette.text);
    // Job names repeat across runs; the run tells them apart.
    let run_room = room - name_width - style::SPACE_S;
    if check.job.is_some() && run_room > 40.0 {
        let (run, _) =
            ellipsized(ui, &check.run.name, small.clone(), palette.text_secondary, run_room);
        ui.painter().galley(
            pos2(left + name_width + style::SPACE_S, rect.center().y - run.size().y / 2.0),
            run,
            palette.text_secondary,
        );
    }
    if let Some(duration) = duration {
        ui.painter().text(
            pos2(rect.right() - style::SPACE_S, rect.center().y),
            Align2::RIGHT_CENTER,
            duration,
            small,
            palette.text_secondary,
        );
    }
    cv_widgets::focus::ring(ui, &response, rect);
    let response = if is_cut { response.on_hover_text(check.name()) } else { response };
    response.clicked()
}

/// The head commit's "Pipelines" (runs with jobs) first, then its "Status checks", then its jobs
/// by status, then "Older runs" with their jobs; each matching `search`.
fn grouped<'a>(order: &[Check<'a>], search: &str) -> Vec<(String, Vec<Check<'a>>)> {
    let needle = search.trim().to_lowercase();
    let matches =
        |check: &Check<'_>| needle.is_empty() || check.name().to_lowercase().contains(&needle);
    let mut groups: Vec<(String, Vec<Check<'a>>)> = Vec::new();
    let mut push = |title: String, keep: &dyn Fn(&Check<'_>) -> bool| {
        let checks: Vec<Check<'a>> =
            order.iter().copied().filter(|check| keep(check) && matches(check)).collect();
        if !checks.is_empty() {
            groups.push((title, checks));
        }
    };
    push("Pipelines".to_owned(), &|check| {
        check.run.is_head && check.job.is_none() && check.run.has_jobs
    });
    push("Status checks".to_owned(), &|check| {
        check.run.is_head && check.job.is_none() && !check.run.has_jobs
    });
    for (filter, label) in checks::FILTERS {
        push(format!("Jobs · {}", label.to_lowercase()), &|check| {
            check.run.is_head && check.job.is_some() && checks::group(check.status()) == filter
        });
    }
    push("Older runs".to_owned(), &|check| !check.run.is_head);
    groups
}

/// True when `key` was pressed and no text field takes the keys; consumes it.
pub fn is_free_key(ui: &Ui, key: egui::Key) -> bool {
    let focused = ui.memory(egui::Memory::focused);
    let is_typing = focused.is_some_and(|id| egui::TextEdit::load_state(ui.ctx(), id).is_some());
    !is_typing && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key))
}

/// J/K step through the checks, as in the switcher; not while typing or with a menu open.
pub fn step_keys(ui: &Ui, order: &[Check<'_>], position: Option<usize>) -> Option<HeaderAction> {
    let position = position?;
    if egui::Popup::is_any_open(ui.ctx()) {
        return None;
    }
    if is_free_key(ui, egui::Key::J) {
        return order.get(position + 1).map(|check| HeaderAction::Select(check.key()));
    }
    if is_free_key(ui, egui::Key::K) {
        return position.checked_sub(1).map(|index| HeaderAction::Select(order[index].key()));
    }
    None
}

#[cfg(test)]
mod tests {
    use review_core::{PipelineJob, PipelineRun, RunStatus};

    use super::*;

    fn run(name: &str, status: RunStatus, has_jobs: bool) -> PipelineRun {
        PipelineRun {
            id: name.to_owned(),
            name: name.to_owned(),
            status,
            description: None,
            web_url: None,
            started_at: None,
            finished_at: None,
            has_jobs,
            is_head: true,
            reruns: Vec::new(),
        }
    }

    #[test]
    fn pipelines_come_first_then_status_checks_then_jobs_by_status() {
        let job = |name: &str, status: RunStatus| PipelineJob {
            id: name.to_owned(),
            name: name.to_owned(),
            status,
            ..PipelineJob::default()
        };
        let stages = vec![PipelineJob {
            id: "Analyze".to_owned(),
            children: vec![
                job("Detect changes", RunStatus::Succeeded),
                job("Quality", RunStatus::Failed),
                job("Stop review", RunStatus::Neutral),
            ],
            ..PipelineJob::default()
        }];
        let runs = [
            run("Pipeline #2279", RunStatus::Failed, true),
            run("Security scan", RunStatus::Succeeded, false),
        ];
        let order = checks::list(&runs, |run| run.has_jobs.then_some(stages.as_slice()));
        let titles: Vec<String> = grouped(&order, "").into_iter().map(|(title, _)| title).collect();
        assert_eq!(
            titles,
            ["Pipelines", "Status checks", "Jobs · failed", "Jobs · succeeded", "Jobs · neutral"]
        );
        let found = grouped(&order, "STOP");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1[0].name(), "Stop review");
    }

    #[test]
    fn older_runs_and_their_jobs_group_apart() {
        let job = |name: &str, status: RunStatus| PipelineJob {
            id: name.to_owned(),
            name: name.to_owned(),
            status,
            ..PipelineJob::default()
        };
        let stages = vec![PipelineJob {
            id: "Test".to_owned(),
            children: vec![job("unit", RunStatus::Failed)],
            ..PipelineJob::default()
        }];
        let older =
            PipelineRun { is_head: false, ..run("Pipeline #2275", RunStatus::Failed, true) };
        let runs = [run("Pipeline #2279", RunStatus::Succeeded, true), older];
        let order = checks::list(&runs, |run| (!run.is_head).then_some(stages.as_slice()));
        let groups: Vec<(String, Vec<&str>)> = grouped(&order, "")
            .into_iter()
            .map(|(title, checks)| (title, checks.iter().map(Check::name).collect()))
            .collect();
        assert_eq!(
            groups,
            [
                ("Pipelines".to_owned(), vec!["Pipeline #2279"]),
                ("Older runs".to_owned(), vec!["Pipeline #2275", "unit"])
            ],
            "an older run's failed job is no head failure"
        );
    }
}
