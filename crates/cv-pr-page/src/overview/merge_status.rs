//! The merge-status card at the top of the overview: one line on the pipelines (expandable to
//! the list of pipelines with their stages), merge conflicts, the auto-complete set on it (who
//! set it and the strategy; the merge button cancels it), and approvals. Each line that asks
//! for something offers the way to do it. The pipelines start collapsed; whether they are open is
//! one setting for every pull request, kept across restarts. While a run is unfinished, the card
//! follows it live, as the Pipelines tab does.

use egui::{Align2, FontId, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use review_core::{MergeStatus, PipelineRun, ProviderKind, PullRequestState, ReviewVote};

use super::super::merge_button::MergeTypeLine;
use cv_base::theme::{self, icons, motion, style};
use cv_pr_pipelines::stage_bars;
use cv_pr_pipelines::{checks, ellipsized, follow_running, stages_of};
use cv_pr_session::auto_complete;
use cv_pr_session::session::PullRequestSession;
use cv_services::Services;
use cv_widgets::states;
use cv_widgets::{badges, buttons, glyph, merge_strategy_icon, row, skeleton};

const STATUS_ICON: f32 = 22.0;
/// A status line's detail keeps this much room when the headline is long ("1 of 3…").
const DETAIL_MIN_WIDTH: f32 = 64.0;
const SOURCE_WIDTH: f32 = 130.0;
const BARS_WIDTH: f32 = 150.0;
const DURATION_WIDTH: f32 = 72.0;
const OPEN_WIDTH: f32 = 48.0;
/// Narrower rows drop the source, then the stage bars.
const SOURCE_MIN_ROW: f32 = 720.0;
const BARS_MIN_ROW: f32 = 540.0;
/// Room the merge type keeps while the branch's policy loads.
const MERGE_TYPE_WIDTH: f32 = 150.0;
const SHOW_LABEL: &str = "Show pipelines";
const HIDE_LABEL: &str = "Hide pipelines";

pub enum MergeStatusAction {
    /// "Show pipelines" / "Hide pipelines", or a click anywhere on the pipelines line.
    Toggle,
    /// Show this run full width in the Pipelines tab.
    OpenRun(String),
    /// The Pipelines tab's checks table.
    OpenChecks,
    ReloadRuns,
    Refresh,
    Approve,
    OpenOnPlatform,
}

pub struct CardInput<'a> {
    pub is_expanded: bool,
    /// The user may approve here (token, open pull request, the platform takes votes).
    pub can_approve: bool,
    pub services: &'a Services,
    /// The strategy the merge button runs, for an open pull request.
    pub merge_type: Option<&'a MergeTypeLine>,
}

pub fn show(
    ui: &mut Ui,
    session: &mut PullRequestSession,
    input: &CardInput<'_>,
) -> Option<MergeStatusAction> {
    let palette = theme::palette(ui.ctx());
    follow_running(ui.ctx(), session, input.services);
    if input.is_expanded {
        session.want_head_stages(input.services);
    }
    let session = &*session;
    let mut action = None;
    let response = style::card_frame(&palette).inner_margin(egui::Margin::ZERO).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        action = pipelines_line(ui, session, input);
        if input.is_expanded
            && let Some(runs) = session.runs.ready().filter(|runs| !runs.is_empty())
            && let Some(picked) = pipeline_list(ui, runs, session)
        {
            action = Some(picked);
        }
        separator(ui);
        if let Some(picked) = conflicts_line(ui, session, input.merge_type) {
            action = Some(picked);
        }
        separator(ui);
        if auto_complete_line(ui, session) {
            separator(ui);
        }
        if let Some(picked) = approvals_line(ui, session, input.can_approve) {
            action = Some(picked);
        }
    });
    response
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Group, true, "Merge status"));
    action
}

fn separator(ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, palette.border));
}

/// One status line: icon, text, quiet detail and an optional action at the right. `content`
/// draws the right side (right to left) and returns what was clicked.
fn status_line<R>(
    ui: &mut Ui,
    icon: (&str, egui::Color32),
    text: &str,
    detail: Option<&str>,
    right: impl FnOnce(&mut Ui) -> Option<R>,
) -> Option<R> {
    let palette = theme::palette(ui.ctx());
    let mut picked = None;
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(style::SPACE_L as i8, style::SPACE_L as i8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(style::CONTROL_HEIGHT - style::SPACE_S);
                ui.spacing_mut().item_spacing.x = style::SPACE_M;
                let font = theme::fonts::semibold_font(style::BODY_SIZE);
                // The action first, then the detail in what the headline leaves; when even the
                // headline does not fit, both are cut (each shows in full on hover).
                let headline = ui.fonts_mut(|fonts| {
                    fonts
                        .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::PLACEHOLDER)
                        .size()
                        .x
                });
                row::with_trailing(
                    ui,
                    egui::Align::Center,
                    |ui| {
                        picked = right(ui);
                        if let Some(detail) = detail {
                            let leading =
                                STATUS_ICON + headline + 2.0 * ui.spacing().item_spacing.x;
                            ui.scope(|ui| {
                                ui.set_max_width(
                                    (ui.available_width() - leading).max(DETAIL_MIN_WIDTH),
                                );
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(detail)
                                            .size(style::SMALL_SIZE)
                                            .color(palette.text_secondary),
                                    )
                                    .truncate(),
                                );
                            });
                        }
                    },
                    |ui| {
                        glyph::decorative(ui, badges::icon_text(icon.0, icon.1, STATUS_ICON));
                        ui.add(egui::Label::new(egui::RichText::new(text).font(font)).truncate());
                    },
                );
            });
        });
    picked
}

fn pipelines_line(
    ui: &mut Ui,
    session: &PullRequestSession,
    input: &CardInput<'_>,
) -> Option<MergeStatusAction> {
    let palette = theme::palette(ui.ctx());
    match (session.runs.ready(), session.runs.error()) {
        (Some(runs), _) if runs.is_empty() => status_line(
            ui,
            (icons::INFO, palette.text_secondary),
            "No pipelines for this commit",
            None,
            |ui| {
                buttons::subtle(ui, icons::ARROW_CLOCKWISE, "Refresh")
                    .clicked()
                    .then_some(MergeStatusAction::ReloadRuns)
            },
        ),
        (Some(runs), _) => {
            let summary = checks::summary(runs);
            let icon = badges::run_status_icon(summary.status, &palette);
            let (label, caret) = if input.is_expanded {
                (HIDE_LABEL, icons::CARET_UP)
            } else {
                (SHOW_LABEL, icons::CARET_DOWN)
            };
            // The whole line toggles, its button on top of it. The line takes no focus: the button
            // stays the way for the keyboard and screen readers.
            let background = ui.painter().add(egui::Shape::Noop);
            let line = ui.scope_builder(UiBuilder::new().sense(Sense::CLICK), |ui| {
                // Selectable text would take the click for a selection.
                ui.style_mut().interaction.selectable_labels = false;
                status_line(ui, icon, &summary.headline, Some(&summary.detail), |ui| {
                    let toggle = buttons::subtle(ui, caret, label);
                    toggle.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::Role::Button,
                            true,
                            input.is_expanded,
                            label,
                        )
                    });
                    // Right to left: the room of the wider label, so the detail never moves.
                    ui.add_space(toggle_slack(ui, label));
                    toggle.clicked().then_some(MergeStatusAction::Toggle)
                })
            });
            let fill = row::state_fill(ui, &line.response, egui::Color32::TRANSPARENT);
            if fill != egui::Color32::TRANSPARENT {
                let corner = egui::CornerRadius {
                    nw: style::RADIUS_CARD,
                    ne: style::RADIUS_CARD,
                    sw: 0,
                    se: 0,
                };
                ui.painter().set(
                    background,
                    egui::Shape::rect_filled(line.response.rect.shrink(1.0), corner, fill),
                );
            }
            line.inner.or_else(|| line.response.clicked().then_some(MergeStatusAction::Toggle))
        }
        (None, Some(message)) => {
            let text = format!("Pipelines did not load: {message}");
            status_line(ui, (icons::WARNING_CIRCLE, palette.error), &text, None, |ui| {
                buttons::subtle(ui, icons::ARROW_CLOCKWISE, "Retry")
                    .clicked()
                    .then_some(MergeStatusAction::ReloadRuns)
            })
        }
        (None, None) => {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(style::SPACE_L as i8, style::SPACE_L as i8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.set_min_height(style::CONTROL_HEIGHT - style::SPACE_S);
                        skeleton::circle(ui, STATUS_ICON);
                        ui.add_space(style::SPACE_M);
                        skeleton::bar(ui, 220.0, 14.0);
                    });
                });
            None
        }
    }
}

/// How much narrower `label` is than the wider of the toggle's two labels.
fn toggle_slack(ui: &Ui, label: &str) -> f32 {
    let width = |text: &str| {
        ui.painter()
            .layout_no_wrap(
                text.to_owned(),
                FontId::proportional(style::BODY_SIZE),
                egui::Color32::PLACEHOLDER,
            )
            .size()
            .x
    };
    (width(SHOW_LABEL).max(width(HIDE_LABEL)) - width(label)).max(0.0)
}

/// The pipelines, one row each, and the way to everything else in the Pipelines tab.
fn pipeline_list(
    ui: &mut Ui,
    runs: &[PipelineRun],
    session: &PullRequestSession,
) -> Option<MergeStatusAction> {
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 16, top: 0, bottom: 16 }).show(
        ui,
        |ui| {
            let width = ui.available_width();
            let footer = style::ROW_COMPACT;
            let pipelines = checks::latest_pipelines(runs);
            let height = pipelines.len() as f32 * style::ROW_TWO_LINE + footer;
            let (frame, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
            ui.painter().rect_filled(frame, style::RADIUS_CONTROL, palette.ground);
            for (index, run) in pipelines.into_iter().enumerate() {
                let rect = Rect::from_min_size(
                    pos2(frame.left(), frame.top() + index as f32 * style::ROW_TWO_LINE),
                    vec2(width, style::ROW_TWO_LINE),
                );
                if index > 0 {
                    ui.painter().hline(
                        rect.x_range(),
                        rect.top(),
                        Stroke::new(1.0, palette.border),
                    );
                }
                if pipeline_row(ui, rect, run, session) {
                    action = Some(MergeStatusAction::OpenRun(run.id.clone()));
                }
            }
            let footer_rect =
                Rect::from_min_max(pos2(frame.left(), frame.bottom() - footer), frame.max);
            ui.painter().hline(
                footer_rect.x_range(),
                footer_rect.top(),
                Stroke::new(1.0, palette.border),
            );
            let mut link_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(footer_rect.shrink2(vec2(style::SPACE_ROW_X, 0.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            if buttons::text_link(
                &mut link_ui,
                "All checks and jobs in Pipelines",
                "Open the Pipelines tab",
            )
            .clicked()
            {
                action = Some(MergeStatusAction::OpenChecks);
            }
            ui.painter().rect_stroke(
                frame,
                style::RADIUS_CONTROL,
                Stroke::new(1.0, palette.border),
                StrokeKind::Inside,
            );
        },
    );
    action
}

/// The columns of a pipeline row, right to left: "Open", the duration, and, where the row is
/// wide enough, the stage bars and the source; the status icon and the name take the rest.
struct RunColumns {
    icon_rect: Rect,
    name_rect: Rect,
    source: Option<Rect>,
    bars: Option<Rect>,
    duration: Rect,
    open: Rect,
}

impl RunColumns {
    fn for_row(rect: Rect) -> Self {
        let inner = rect.shrink2(vec2(style::SPACE_ROW_X, 0.0));
        let gap = style::SPACE_M;
        let has_source = inner.width() >= SOURCE_MIN_ROW;
        let has_bars = inner.width() >= BARS_MIN_ROW;
        let mut right = inner.right();
        let mut take = |width: f32| {
            let column =
                Rect::from_min_max(pos2(right - width, rect.top()), pos2(right, rect.bottom()));
            right -= width + gap;
            column
        };
        let open = take(OPEN_WIDTH);
        let duration = take(DURATION_WIDTH);
        let bars = has_bars.then(|| take(BARS_WIDTH));
        let source = has_source.then(|| take(SOURCE_WIDTH));
        let icon_rect =
            Rect::from_min_size(pos2(inner.left(), rect.top()), vec2(20.0, rect.height()));
        let name_rect = Rect::from_min_max(
            pos2(icon_rect.right() + gap, rect.top()),
            pos2(right, rect.bottom()),
        );
        Self { icon_rect, name_rect, source, bars, duration, open }
    }
}

/// Returns true when the row (or its "Open") was clicked.
fn pipeline_row(ui: &mut Ui, rect: Rect, run: &PipelineRun, session: &PullRequestSession) -> bool {
    let palette = theme::palette(ui.ctx());
    let response =
        ui.interact(rect, ui.scope_id().with(("merge_status_run", &run.id)), Sense::click());
    let status = badges::run_status_label(run.status);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Link, true, checks::Check { run, job: None }.label())
    });
    let hover = row::hover_amount(ui, &response);
    if hover > 0.0 || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(
            rect.shrink(1.0),
            0,
            row::state_fill(ui, &response, egui::Color32::TRANSPARENT),
        );
    }
    let RunColumns { icon_rect, name_rect, source, bars, duration, open } =
        RunColumns::for_row(rect);
    let painter = ui.painter_at(rect);
    let (icon, color) = badges::run_status_icon(run.status, &palette);
    painter.text(
        icon_rect.center(),
        Align2::CENTER_CENTER,
        icon,
        FontId::proportional(16.0),
        color,
    );
    let known = session.stages(&run.id).and_then(|load| load.ready()).map(Vec::as_slice);
    let small = FontId::proportional(style::SMALL_SIZE);
    let name_width = name_rect.width().max(0.0);
    let (name, is_cut) =
        ellipsized(ui, &run.name, FontId::proportional(style::BODY_SIZE), palette.text, name_width);
    let sub_text = checks::sub_line(run, known);
    let (sub, _) = ellipsized(ui, &sub_text, small.clone(), palette.text_secondary, name_width);
    let block = name.size().y + style::SPACE_LINE + sub.size().y;
    let top = rect.center().y - block / 2.0;
    let sub_top = top + name.size().y + style::SPACE_LINE;
    painter.galley(pos2(name_rect.left(), top), name, palette.text);
    painter.galley(pos2(name_rect.left(), sub_top), sub, palette.text_secondary);
    if let Some(source) = source {
        painter.text(
            pos2(source.left(), rect.center().y),
            Align2::LEFT_CENTER,
            checks::source(run, session.kind),
            small.clone(),
            palette.text_secondary,
        );
    }
    if let Some(bars) = bars {
        let stages = stages_of(run, session);
        stage_bars::show(ui, bars, response.id.with("bars"), &run.name, stages);
    }
    let time = checks::duration(run).unwrap_or_else(|| "–".to_owned());
    painter.text(
        pos2(duration.right(), rect.center().y),
        Align2::RIGHT_CENTER,
        time,
        small,
        palette.text,
    );
    let open_color = motion::mix(palette.accent_text, palette.accent_hover, hover);
    painter.text(
        pos2(open.right(), rect.center().y),
        Align2::RIGHT_CENTER,
        "Open",
        FontId::proportional(style::BODY_SIZE),
        open_color,
    );
    cv_widgets::focus::ring(ui, &response, rect);
    let tooltip = if is_cut {
        format!("{} ({status})\nOpen in the Pipelines tab", run.name)
    } else {
        "Open in the Pipelines tab".to_owned()
    };
    response.on_hover_text(tooltip).clicked()
}

fn conflicts_line(
    ui: &mut Ui,
    session: &PullRequestSession,
    merge_type: Option<&MergeTypeLine>,
) -> Option<MergeStatusAction> {
    let palette = theme::palette(ui.ctx());
    let Some(detail) = session.detail.ready() else {
        return skeleton_line(ui);
    };
    let open_label = states::open_on_platform(session.kind);
    let open = |ui: &mut Ui| {
        buttons::subtle(ui, cv_widgets::provider_icon::glyph(session.kind), &open_label)
            .clicked()
            .then_some(MergeStatusAction::OpenOnPlatform)
    };
    let refresh = |ui: &mut Ui| {
        buttons::subtle(ui, icons::ARROW_CLOCKWISE, "Refresh")
            .clicked()
            .then_some(MergeStatusAction::Refresh)
    };
    let none = |_: &mut Ui| None;
    match detail.merge_status {
        MergeStatus::Mergeable => status_line(
            ui,
            (icons::CHECK_CIRCLE, palette.success),
            "No merge conflicts",
            None,
            with_type(merge_type, none),
        ),
        MergeStatus::Conflicts => status_line(
            ui,
            (icons::WARNING_CIRCLE, palette.error),
            "Merge conflicts",
            Some("Resolve them before merging"),
            with_type(merge_type, open),
        ),
        MergeStatus::Blocked => status_line(
            ui,
            (icons::PROHIBIT, palette.warning),
            "Merging is blocked",
            Some(blocked_reason(session.kind)),
            with_type(merge_type, open),
        ),
        MergeStatus::Unknown => status_line(
            ui,
            (icons::CLOCK, palette.text_secondary),
            "Merge status not checked yet",
            None,
            with_type(merge_type, refresh),
        ),
    }
}

/// The line's action, then (right to left: left of it) the merge type.
fn with_type<'a>(
    merge_type: Option<&'a MergeTypeLine>,
    action: impl FnOnce(&mut Ui) -> Option<MergeStatusAction> + 'a,
) -> impl FnOnce(&mut Ui) -> Option<MergeStatusAction> + 'a {
    move |ui: &mut Ui| {
        let picked = action(ui);
        if let Some(line) = merge_type {
            merge_type_chip(ui, line);
        }
        picked
    }
}

/// The merge button's strategy, compact: glyph and name; the hover lists what else is allowed.
/// Laid out right to left, in a fixed slot while the policy loads.
fn merge_type_chip(ui: &mut Ui, line: &MergeTypeLine) {
    let palette = theme::palette(ui.ctx());
    ui.add_space(style::SPACE_S);
    match line {
        MergeTypeLine::Pending => {
            let (rect, _) =
                ui.allocate_exact_size(vec2(MERGE_TYPE_WIDTH, icons::SIZE), Sense::hover());
            skeleton::paint_bar(
                ui,
                Rect::from_min_size(
                    pos2(rect.right() - 110.0, rect.center().y - 6.0),
                    vec2(110.0, 12.0),
                ),
            );
        }
        MergeTypeLine::Ready { strategy, name, allowed, is_reported } => {
            let font = FontId::proportional(style::BODY_SIZE);
            let galley =
                ui.painter().layout_no_wrap((*name).to_owned(), font, palette.text_secondary);
            let width = (icons::SIZE + style::SPACE_XS + galley.size().x).max(MERGE_TYPE_WIDTH);
            let (rect, response) = ui
                .allocate_exact_size(vec2(width, galley.size().y.max(icons::SIZE)), Sense::hover());
            let text_left = rect.right() - galley.size().x;
            let icon_center =
                pos2(text_left - style::SPACE_XS - icons::SIZE / 2.0, rect.center().y);
            merge_strategy_icon::paint(
                ui.painter(),
                &palette,
                icon_center,
                icons::SIZE,
                *strategy,
                palette.text_secondary,
                palette.surface,
            );
            ui.painter().galley(
                pos2(text_left, rect.center().y - galley.size().y / 2.0),
                galley,
                palette.text_secondary,
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Label, true, format!("Merge type: {name}"))
            });
            let note = if *is_reported {
                "Allowed on this branch"
            } else {
                "The platform didn't report this branch's merge types; it can merge with"
            };
            response.on_hover_text(format!(
                "Completing runs {name}.
{note}: {}",
                allowed.join(", ")
            ));
        }
    }
}

/// "Auto-complete set by Ada: Squash merge" while one is set on an open pull request; true when
/// shown.
fn auto_complete_line(ui: &mut Ui, session: &PullRequestSession) -> bool {
    let Some(detail) =
        session.detail.ready().filter(|detail| detail.summary.state == PullRequestState::Open)
    else {
        return false;
    };
    let Some(auto_complete) = &detail.auto_complete else {
        return false;
    };
    let palette = theme::palette(ui.ctx());
    let text = auto_complete::state_text(auto_complete, session.kind);
    let waits = auto_complete::waits_for(session.kind);
    status_line(ui, (icons::CLOCK, palette.accent_text), &text, Some(waits), |_| None::<()>);
    true
}

fn blocked_reason(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::AzureDevOps => "Branch policies are not met",
        ProviderKind::GitHub => "Branch protection rules are not met",
        ProviderKind::GitLab => "Merge checks are not met",
    }
}

fn approvals_line(
    ui: &mut Ui,
    session: &PullRequestSession,
    can_approve: bool,
) -> Option<MergeStatusAction> {
    let palette = theme::palette(ui.ctx());
    let Some(detail) = session.detail.ready() else {
        return skeleton_line(ui);
    };
    let reviewers = &detail.summary.reviewers;
    let names = |votes: &[ReviewVote]| -> Vec<&str> {
        reviewers
            .iter()
            .filter(|reviewer| votes.contains(&reviewer.vote))
            .map(|reviewer| reviewer.identity.display_name.as_str())
            .collect()
    };
    let approved = names(&[ReviewVote::Approved, ReviewVote::ApprovedWithSuggestions]);
    let rejected = names(&[ReviewVote::ChangesRequested]);
    let required = reviewers.iter().filter(|reviewer| reviewer.is_required).count();
    let required_approved = reviewers
        .iter()
        .filter(|reviewer| {
            reviewer.is_required
                && matches!(
                    reviewer.vote,
                    ReviewVote::Approved | ReviewVote::ApprovedWithSuggestions
                )
        })
        .count();
    let detail_text = (required > 0)
        .then(|| format!("{required_approved} of {required} required reviewers approved"));
    let approve = |ui: &mut Ui| {
        (can_approve && buttons::secondary_icon(ui, icons::CHECK, "Approve").clicked())
            .then_some(MergeStatusAction::Approve)
    };
    let is_open = detail.summary.state == PullRequestState::Open;
    if !rejected.is_empty() {
        let text = format!("Changes requested by {}", rejected.join(", "));
        status_line(ui, (icons::X_CIRCLE, palette.error), &text, detail_text.as_deref(), approve)
    } else if !approved.is_empty() {
        let text = format!("Approved by {}", approved.join(", "));
        status_line(
            ui,
            (icons::CHECK_CIRCLE, palette.success),
            &text,
            detail_text.as_deref(),
            |_| None,
        )
    } else if is_open {
        let count = checks::plural(reviewers.len(), "reviewer");
        let detail_text = detail_text.unwrap_or(if reviewers.is_empty() {
            "No reviewers yet".to_owned()
        } else {
            count
        });
        status_line(
            ui,
            (icons::WARNING_CIRCLE, palette.warning),
            "No approvals yet",
            Some(&detail_text),
            approve,
        )
    } else {
        status_line(ui, (icons::MINUS_CIRCLE, palette.text_secondary), "No approvals", None, |_| {
            None
        })
    }
}

fn skeleton_line(ui: &mut Ui) -> Option<MergeStatusAction> {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(style::SPACE_L as i8, style::SPACE_L as i8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(style::CONTROL_HEIGHT - style::SPACE_S);
                skeleton::circle(ui, STATUS_ICON);
                ui.add_space(style::SPACE_M);
                skeleton::bar(ui, 180.0, 14.0);
            });
        });
    None
}
