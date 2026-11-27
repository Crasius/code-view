//! The Pipelines tab's entry page: every pipeline, job and status check of the head commit in one
//! table, filterable by status and text. A run's row opens it full width, a job's row its log;
//! "Logs" goes straight to the first failed log, the rerun icon offers the check's reruns.

use std::collections::BTreeSet;

use cv_base::theme::{self, icons, motion, style};
use cv_widgets::states::{self, Action};
use cv_widgets::{badges, buttons, elide, glyph, row};
use egui::{
    Align2, Color32, FontId, Key, Rect, Sense, Stroke, StrokeKind, TextEdit, Ui, UiBuilder, pos2,
    vec2,
};
use review_core::{PipelineRun, ProviderKind, RerunAction};
use review_store::CheckFilter;

use super::checks::{self, Check, CheckKey, FILTERS};
use super::rerun;
use super::stage_bars::{self, Stages};

const HEADER_HEIGHT: f32 = 34.0;
const CHIP_HEIGHT: f32 = 28.0;
const STATUS_WIDTH: f32 = 28.0;
const SOURCE_WIDTH: f32 = 120.0;
const DURATION_WIDTH: f32 = 80.0;
const ACTIONS_WIDTH: f32 = 112.0;
const SEARCH_WIDTH: f32 = 240.0;
/// Below this table width the source column gives its room to the name.
const SOURCE_MIN_TABLE: f32 = 760.0;

pub enum TableAction {
    /// Show the run full width, or the job's log.
    Open(CheckKey),
    /// Show the check's first failed log (or first log).
    Logs(CheckKey),
    OpenUrl(String),
    Rerun(String, RerunAction),
    RerunFailed,
    AddToken,
    /// The status chips changed; time to save them.
    FilterChanged,
}

pub struct TableInput<'a> {
    pub runs: &'a [PipelineRun],
    /// Every run and job, in [`checks::order`].
    pub checks: &'a [Check<'a>],
    /// The stage bars of each check, in `checks` order.
    pub bars: &'a [Stages<'a>],
    pub platform: ProviderKind,
    pub has_token: bool,
    /// A rerun is being started.
    pub is_busy: bool,
    /// "Checks for 3f9a2c1".
    pub title: &'a str,
    pub filter: &'a mut BTreeSet<CheckFilter>,
}

/// What the table keeps between frames: the text filter and the keyboard row.
#[derive(Default)]
pub struct ChecksTable {
    search: String,
    /// The check the keyboard is on (↑/↓), opened with Enter.
    cursor: Option<CheckKey>,
    scroll_to_cursor: bool,
}

struct Columns {
    status: Rect,
    name: Rect,
    stages: Rect,
    source: Option<Rect>,
    duration: Rect,
    actions: Rect,
}

impl ChecksTable {
    pub fn show(&mut self, ui: &mut Ui, mut input: TableInput<'_>) -> Option<TableAction> {
        let mut action = None;
        let margin = egui::Margin::symmetric(style::PAGE_MARGIN_X, 20);
        egui::Frame::new().inner_margin(margin).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            action = self.toolbar(ui, &mut input);
            ui.add_space(style::SPACE_M + 2.0);
            let mut shown: Vec<(usize, Check<'_>)> =
                input.checks.iter().copied().enumerate().collect();
            shown.retain(|(_, check)| {
                checks::matches(check, input.filter, &self.search, input.platform)
            });
            if let Some(table) = self.table(ui, &mut input, &shown) {
                action = Some(table);
            }
            if shown.is_empty() {
                return;
            }
            ui.add_space(style::SPACE_S);
            let palette = theme::palette(ui.ctx());
            ui.label(
                egui::RichText::new(
                    "↑/↓ moves, Enter opens. Failed checks come first, then running ones.",
                )
                .size(style::SMALL_SIZE)
                .color(palette.text_secondary),
            );
        });
        action
    }

    fn toolbar(&mut self, ui: &mut Ui, input: &mut TableInput<'_>) -> Option<TableAction> {
        let mut action = None;
        let counts = checks::counts(input.checks.iter());
        let failed_reruns = checks::failed_reruns(input.runs);
        ui.horizontal(|ui| {
            ui.set_min_height(style::CONTROL_HEIGHT);
            ui.spacing_mut().item_spacing.x = style::SPACE_S;
            ui.label(
                egui::RichText::new(input.title)
                    .font(theme::fonts::semibold_font(style::SECTION_SIZE)),
            );
            ui.add_space(style::SPACE_S);
            if chip(ui, "All", input.checks.len(), input.filter.is_empty()).clicked()
                && !input.filter.is_empty()
            {
                input.filter.clear();
                action = Some(TableAction::FilterChanged);
            }
            for ((filter, label), count) in FILTERS.iter().zip(counts) {
                let is_on = input.filter.contains(filter);
                if chip(ui, label, count, is_on).clicked() {
                    if is_on {
                        input.filter.remove(filter);
                    } else {
                        input.filter.insert(*filter);
                    }
                    action = Some(TableAction::FilterChanged);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !failed_reruns.is_empty()
                    && let Some(picked) = rerun_failed_button(ui, input, failed_reruns.len())
                {
                    action = Some(picked);
                }
                let search = ui.add(
                    TextEdit::singleline(&mut self.search)
                        .hint_text(format!("{}  Filter checks", icons::MAGNIFYING_GLASS))
                        .desired_width(SEARCH_WIDTH),
                );
                search.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::SearchInput, true, "Filter checks")
                });
                glyph::hint_as(&search, "Filter checks");
                // Squeezed below its width by the chips (a wide font, a narrow window), the
                // field may cut its hint; the hint then reads in full on hover.
                if self.search.is_empty() && search.rect.width() < SEARCH_WIDTH {
                    search.on_hover_text("Filter checks");
                }
            });
        });
        action
    }

    /// No check passes the chips and the filter text: the way back to all of them.
    fn no_match(&mut self, ui: &mut Ui, input: &mut TableInput<'_>) -> Option<TableAction> {
        let picked = states::message_with_actions(
            ui,
            icons::FUNNEL,
            "No checks match",
            "Nothing reported for this commit passes the status chips and the filter text.",
            &[Action::secondary(icons::X, "Show all checks")],
        );
        picked?;
        self.search.clear();
        input.filter.clear();
        Some(TableAction::FilterChanged)
    }

    fn table(
        &mut self,
        ui: &mut Ui,
        input: &mut TableInput<'_>,
        shown: &[(usize, Check<'_>)],
    ) -> Option<TableAction> {
        let palette = theme::palette(ui.ctx());
        if shown.is_empty() {
            return self.no_match(ui, input);
        }
        let mut action = None;
        self.keyboard(ui, shown, &mut action);
        let footer = 30.0;
        let width = ui.available_width();
        let body_height = shown.len() as f32 * style::ROW_TWO_LINE;
        let height = (HEADER_HEIGHT + body_height)
            .min((ui.available_height() - footer).max(HEADER_HEIGHT + style::ROW_TWO_LINE));
        let (frame, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        ui.painter().rect_filled(frame, style::RADIUS_CARD, palette.surface);
        let header = Rect::from_min_size(frame.min, vec2(width, HEADER_HEIGHT));
        paint_heading(ui, header);

        let body = Rect::from_min_max(pos2(frame.left(), header.bottom()), frame.max);
        let mut body_ui = ui.new_child(UiBuilder::new().id_salt("checks_body").max_rect(body));
        body_ui.set_clip_rect(body.intersect(ui.clip_rect()));
        let scroll_to = self.scroll_to_cursor.then(|| self.cursor.clone()).flatten();
        self.scroll_to_cursor = false;
        let bars = input.bars;
        let input = &*input;
        egui::ScrollArea::vertical().id_salt("checks_table").auto_shrink(false).show_viewport(
            &mut body_ui,
            |ui, viewport| {
                ui.set_min_height(body_height);
                let origin = ui.min_rect().min;
                let row_width = ui.available_width();
                let row_rect = |index: usize| {
                    Rect::from_min_size(
                        origin + vec2(0.0, index as f32 * style::ROW_TWO_LINE),
                        vec2(row_width, style::ROW_TWO_LINE),
                    )
                };
                if let Some(index) =
                    scroll_to.and_then(|key| shown.iter().position(|(_, check)| check.is(&key)))
                {
                    ui.scroll_to_rect(row_rect(index), None);
                }
                let first = (viewport.top() / style::ROW_TWO_LINE).floor().max(0.0) as usize;
                let last =
                    ((viewport.bottom() / style::ROW_TWO_LINE).ceil() as usize).min(shown.len());
                for (index, (check_index, check)) in shown.iter().enumerate().take(last).skip(first)
                {
                    if let Some(picked) =
                        self.row(ui, row_rect(index), index, check, bars[*check_index], input)
                    {
                        action = Some(picked);
                    }
                }
            },
        );
        ui.painter().rect_stroke(
            frame,
            style::RADIUS_CARD,
            Stroke::new(1.0, palette.border),
            StrokeKind::Inside,
        );
        action
    }

    fn row(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        index: usize,
        check: &Check<'_>,
        stages: Stages<'_>,
        input: &TableInput<'_>,
    ) -> Option<TableAction> {
        let palette = theme::palette(ui.ctx());
        let key = check.key();
        let response = ui.interact(rect, ui.scope_id().with(("check_row", &key)), Sense::click());
        let label = check.label();
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Row, true, &label));
        let is_cursor = self.cursor.as_ref() == Some(&key);
        row::list_background(ui, &response, rect, is_cursor);
        if index > 0 {
            ui.painter().hline(rect.x_range(), rect.top(), Stroke::new(1.0, palette.border));
        }
        let columns = columns(rect);
        let painter = ui.painter_at(rect);
        let (icon, color) = badges::run_status_icon(check.status(), &palette);
        painter.text(
            columns.status.center(),
            Align2::CENTER_CENTER,
            icon,
            icons::rest_font(&palette, icons::SIZE),
            color,
        );

        let known = match stages {
            Stages::Ready(stages) => Some(stages),
            _ => None,
        };
        let body = FontId::proportional(style::BODY_SIZE);
        let small = FontId::proportional(style::SMALL_SIZE);
        let width = columns.name.width();
        let name = elide::line(&painter, check.name(), body, palette.text, width);
        let sub_text = checks::check_sub_line(check, known);
        let sub = elide::line(&painter, &sub_text, small.clone(), palette.text_secondary, width);
        let is_cut = name.elided || sub.elided;
        let block = name.size().y + style::SPACE_LINE + sub.size().y;
        let name_top = rect.center().y - block / 2.0;
        let sub_top = name_top + name.size().y + style::SPACE_LINE;
        painter.galley(pos2(columns.name.left(), name_top), name, palette.text);
        painter.galley(pos2(columns.name.left(), sub_top), sub, palette.text_secondary);

        let bar_name = check.stage_name().unwrap_or(check.name());
        stage_bars::show(ui, columns.stages, response.id.with("bars"), bar_name, stages);
        if let Some(source) = columns.source {
            painter.text(
                pos2(source.left(), rect.center().y),
                Align2::LEFT_CENTER,
                checks::source(check.run, input.platform),
                small.clone(),
                palette.text_secondary,
            );
        }
        let duration = checks::check_duration(check).unwrap_or_else(|| "–".to_owned());
        painter.text(
            pos2(columns.duration.right(), rect.center().y),
            Align2::RIGHT_CENTER,
            duration,
            small,
            palette.text,
        );

        let has_log = check.run.has_jobs
            && check.job.is_none_or(|(_, job)| super::job_tree::initial_target(job).is_some());
        let mut action = row_actions(ui, columns.actions, check, has_log, input);
        cv_widgets::focus::ring(ui, &response, rect);
        if response.gained_focus() {
            self.cursor = Some(key.clone());
        }
        let status = badges::run_status_label(check.status());
        let mut tooltip = vec![format!("{} ({status})", check.name())];
        if is_cut {
            tooltip.push(sub_text);
        }
        let click = match (check.job, has_log) {
            (Some(_), true) => "Click to open its log",
            (Some(_), false) => "No log yet; click to see its state",
            (None, _) => "Click to open the run",
        };
        tooltip.push(click.to_owned());
        let response = response.on_hover_text(tooltip.join("\n"));
        if response.clicked() && action.is_none() {
            self.cursor = Some(key.clone());
            action = Some(TableAction::Open(key));
        }
        action
    }

    /// ↑/↓ move the keyboard row, Enter opens it; not while a text field has the keys.
    fn keyboard(
        &mut self,
        ui: &Ui,
        shown: &[(usize, Check<'_>)],
        action: &mut Option<TableAction>,
    ) {
        if ui.ctx().text_edit_focused() {
            return;
        }
        let position =
            self.cursor.as_ref().and_then(|key| shown.iter().position(|(_, check)| check.is(key)));
        let (down, up, enter) = ui.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                input.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                input.consume_key(egui::Modifiers::NONE, Key::Enter),
            )
        });
        let target = match (position, down, up) {
            (None, true, _) | (None, _, true) => Some(0),
            (Some(index), true, _) => Some((index + 1).min(shown.len() - 1)),
            (Some(index), _, true) => Some(index.saturating_sub(1)),
            _ => None,
        };
        if let Some(index) = target {
            self.cursor = Some(shown[index].1.key());
            self.scroll_to_cursor = true;
        }
        if enter && let Some(index) = position {
            *action = Some(TableAction::Open(shown[index].1.key()));
        }
    }
}

/// The row's buttons at its right end, right to left: rerun, then the log, or the check's page
/// on the platform when it has no log here.
fn row_actions(
    ui: &mut Ui,
    rect: Rect,
    check: &Check<'_>,
    has_log: bool,
    input: &TableInput<'_>,
) -> Option<TableAction> {
    let mut actions = ui.new_child(
        UiBuilder::new()
            .id_salt(("check_actions", check.key()))
            .max_rect(rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    actions.spacing_mut().item_spacing.x = style::SPACE_XS;
    let mut action = rerun_icon(&mut actions, check, input);
    if has_log {
        let hint = if check.job.is_some() {
            "Open the job's log"
        } else {
            "Open the first failed log, or the first log"
        };
        if buttons::subtle(&mut actions, icons::FILE_TEXT, "Logs").on_hover_text(hint).clicked() {
            action = Some(TableAction::Logs(check.key()));
        }
    } else if let Some(url) = check.web_url() {
        let glyph = cv_widgets::provider_icon::glyph(input.platform);
        let open = states::open_on_platform(input.platform);
        if buttons::subtle(&mut actions, glyph, "Open").on_hover_text(open).clicked() {
            action = Some(TableAction::OpenUrl(url.to_owned()));
        }
    }
    action
}

/// "Rerun failed (2)": the failed runs' reruns, or the way to a token without one.
fn rerun_failed_button(ui: &mut Ui, input: &TableInput<'_>, count: usize) -> Option<TableAction> {
    let label =
        if count == 1 { "Rerun failed".to_owned() } else { format!("Rerun failed ({count})") };
    let button = buttons::secondary_busy(ui, icons::ARROWS_CLOCKWISE, &label, input.is_busy);
    let button = if input.has_token {
        button.on_hover_text(rerun::label(review_core::RerunKind::RunFailed, input.platform))
    } else {
        button.on_hover_text(format!("{} Click to add one.", rerun::NEEDS_TOKEN))
    };
    if !button.clicked() {
        return None;
    }
    Some(if input.has_token { TableAction::RerunFailed } else { TableAction::AddToken })
}

/// The table's heading row: the column names on a subtle band with the card's top corners.
fn paint_heading(ui: &Ui, header: Rect) {
    let palette = theme::palette(ui.ctx());
    let top = egui::CornerRadius { nw: style::RADIUS_CARD, ne: style::RADIUS_CARD, sw: 0, se: 0 };
    ui.painter().rect_filled(header, top, palette.surface_subtle);
    let columns = columns(header);
    let small = FontId::proportional(style::SMALL_SIZE);
    let heading = |rect: Rect, text: &str, align: Align2| {
        let x = if align == Align2::RIGHT_CENTER { rect.right() } else { rect.left() };
        ui.painter().text(
            pos2(x, rect.center().y),
            align,
            text,
            small.clone(),
            palette.text_secondary,
        );
    };
    heading(columns.name, "Name", Align2::LEFT_CENTER);
    heading(columns.stages, "Stages", Align2::LEFT_CENTER);
    if let Some(source) = columns.source {
        heading(source, "Source", Align2::LEFT_CENTER);
    }
    heading(columns.duration, "Duration", Align2::RIGHT_CENTER);
}

fn columns(rect: Rect) -> Columns {
    let inner = rect.shrink2(vec2(style::SPACE_L, 0.0));
    let gap = style::SPACE_M;
    let has_source = rect.width() >= SOURCE_MIN_TABLE;
    let fixed = STATUS_WIDTH
        + DURATION_WIDTH
        + ACTIONS_WIDTH
        + if has_source { SOURCE_WIDTH + gap } else { 0.0 }
        + gap * 4.0;
    let flexible = (inner.width() - fixed).max(0.0);
    let name_width = flexible * 2.2 / 4.2;
    let stages_width = flexible - name_width;
    let mut x = inner.left();
    let mut take = |width: f32| {
        let column = Rect::from_min_max(pos2(x, rect.top()), pos2(x + width, rect.bottom()));
        x += width + gap;
        column
    };
    let status = take(STATUS_WIDTH);
    let name = take(name_width);
    let stages = take(stages_width);
    let source = has_source.then(|| take(SOURCE_WIDTH));
    let duration = take(DURATION_WIDTH);
    let actions = take(ACTIONS_WIDTH);
    Columns { status, name, stages, source, duration, actions }
}

/// A status chip: "Failed 2", tinted while it filters.
fn chip(ui: &mut Ui, label: &str, count: usize, is_on: bool) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let font = FontId::proportional(style::SMALL_SIZE);
    let count_text = count.to_string();
    let label_width = ui.fonts_mut(|fonts| {
        fonts.layout_no_wrap(label.to_owned(), font.clone(), Color32::WHITE).size().x
    });
    let count_width = ui.fonts_mut(|fonts| {
        fonts.layout_no_wrap(count_text.clone(), font.clone(), Color32::WHITE).size().x
    });
    let width = style::SPACE_M * 2.0 + label_width + style::SPACE_XS + 2.0 + count_width;
    let (rect, response) = ui.allocate_exact_size(vec2(width, CHIP_HEIGHT), Sense::click());
    let name = format!("{label}, {}", checks::plural(count, "check"));
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, is_on, &name));
    let on = motion::toggle(ui.ctx(), response.id.with("chip_on"), is_on, motion::FAST);
    let hover = row::hover_amount(ui, &response);
    let rest = motion::mix(palette.hover.gamma_multiply(hover), palette.selection, on);
    let fill = if response.is_pointer_button_down_on() { palette.pressed } else { rest };
    let radius = (CHIP_HEIGHT / 2.0) as u8;
    let stroke = Stroke::new(1.0, motion::mix(palette.border_strong, palette.selection, on));
    ui.painter().rect(rect, radius, fill, stroke, StrokeKind::Inside);
    let text = motion::mix(palette.text, palette.accent_text, on);
    let left = rect.left() + style::SPACE_M;
    ui.painter().text(pos2(left, rect.center().y), Align2::LEFT_CENTER, label, font.clone(), text);
    let count_color = motion::mix(palette.text_secondary, palette.accent_text, on);
    ui.painter().text(
        pos2(rect.right() - style::SPACE_M, rect.center().y),
        Align2::RIGHT_CENTER,
        count_text,
        font,
        count_color,
    );
    cv_widgets::focus::ring(ui, &response, rect);
    let hint = if is_on {
        "Click to stop filtering by this status"
    } else {
        "Show only checks with this status"
    };
    response.on_hover_text(hint)
}

/// The check's reruns behind one icon; without a token the menu offers adding one instead.
fn rerun_icon(ui: &mut Ui, check: &Check<'_>, input: &TableInput<'_>) -> Option<TableAction> {
    let reruns = check.reruns();
    if reruns.is_empty() {
        // Keeps the Logs button in place whether or not the check can be rerun.
        ui.add_space(buttons::icon_width() + ui.spacing().item_spacing.x);
        return None;
    }
    let mut action = None;
    let tooltip = if input.is_busy { "A rerun is being started…" } else { "Rerun" };
    let button = ui
        .add_enabled_ui(!input.is_busy, |ui| buttons::icon(ui, icons::ARROWS_CLOCKWISE, tooltip))
        .inner;
    egui::Popup::menu(&button).show(|ui| {
        ui.set_min_width(220.0);
        if input.has_token {
            for rerun_action in reruns {
                if buttons::menu_item(
                    ui,
                    icons::ARROWS_CLOCKWISE,
                    rerun::label(rerun_action.kind, input.platform),
                    None,
                    None,
                )
                .clicked()
                {
                    action = Some(TableAction::Rerun(check.run.id.clone(), rerun_action.clone()));
                }
            }
        } else if buttons::menu_item(ui, icons::KEY, "Add a token to rerun", None, None)
            .on_hover_text(rerun::NEEDS_TOKEN)
            .clicked()
        {
            action = Some(TableAction::AddToken);
        }
    });
    action
}
