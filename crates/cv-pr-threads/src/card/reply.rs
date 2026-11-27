//! The reply box under a thread card's comments, its composer, and the row that says a post
//! failed.

use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, motion, style};
use cv_editor::markdown_editor::{self, Outcome};
use cv_runtime::intents;
use cv_widgets::{avatar, buttons, focus, glyph, row};
use egui::{Align, Layout, Margin, RichText, Sense, Stroke, Ui, pos2, vec2};
use review_core::{ReviewThread, ThreadStatus};

use super::super::posts::PostState;
use super::super::suggestion;
use super::super::{ThreadEvent, ThreadsUi, drafts};
use super::{AVATAR_GAP, AVATAR_SIZE, section};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureAction {
    Retry,
    Edit,
    FixToken,
}

/// "Not sent: <reason>. Your text is kept." with Retry, and Fix token or Edit as the way out.
pub(crate) fn failure_row(ui: &mut Ui, message: &str, needs_token: bool) -> Option<FailureAction> {
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    failure_frame(ui, palette.error, |ui| {
        ui.horizontal(|ui| {
            glyph::decorative(
                ui,
                RichText::new(icons::WARNING_CIRCLE)
                    .font(icons::rest_font(&palette, icons::SIZE_SMALL))
                    .color(palette.error),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if needs_token {
                    if buttons::subtle(ui, icons::KEY, "Fix token").clicked() {
                        action = Some(FailureAction::FixToken);
                    }
                } else if buttons::subtle(ui, icons::PENCIL_SIMPLE, "Edit").clicked() {
                    action = Some(FailureAction::Edit);
                }
                if buttons::subtle(ui, icons::ARROW_CLOCKWISE, "Retry").clicked() {
                    action = Some(FailureAction::Retry);
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let reason = message.trim_end_matches('.');
                    let text = format!("Not sent: {reason}. Your text is kept.");
                    ui.add(
                        egui::Label::new(
                            RichText::new(text).size(style::SMALL_SIZE + 1.0).color(palette.text),
                        )
                        .wrap(),
                    );
                });
            });
        });
    });
    action
}

/// The tinted panel a failure or a destructive question sits in, full width, tinted by `tone`.
pub(super) fn failure_frame(ui: &mut Ui, tone: egui::Color32, content: impl FnOnce(&mut Ui)) {
    let palette = theme::palette(ui.ctx());
    let fill = tone.gamma_multiply(if palette.is_dark { 0.16 } else { 0.1 });
    egui::Frame::new()
        .fill(fill)
        .corner_radius(style::RADIUS_CONTROL)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui);
        });
}

/// Whether a reply box takes input.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReplyBox {
    Live,
    /// The line under a comment on its way, laid out as it will be once the comment is posted.
    Inert,
}

/// The reply box: one line ("Reply…" and Resolve) until used, then the composer.
pub(super) fn reply_area(
    ui: &mut Ui,
    thread: &ReviewThread,
    threads: &mut ThreadsUi,
    reply_box: ReplyBox,
) -> Option<ThreadEvent> {
    let is_inert = reply_box == ReplyBox::Inert;
    let palette = theme::palette(ui.ctx());
    if !threads.context.can_write {
        section(ui, Margin::symmetric(16, 12), |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Replying needs a token for this connection.")
                        .size(style::SMALL_SIZE + 1.0)
                        .color(palette.text_secondary),
                );
                if ui.link(RichText::new("Add token").size(style::SMALL_SIZE + 1.0)).clicked() {
                    intents::request_fix(ui.ctx(), &threads.context.connection_id);
                }
            });
        });
        return None;
    }
    let key = drafts::reply_key(&thread.id);
    if !is_inert && threads.reply_open.contains(&thread.id) && thread.can_reply {
        return composer(ui, thread, threads, &key);
    }
    let mut event = None;
    section(
        ui,
        Margin { left: 16 + (AVATAR_SIZE + AVATAR_GAP) as i8, right: 16, top: 10, bottom: 12 },
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let resolve = thread.can_resolve.then_some(match thread.status {
                    ThreadStatus::Active => (icons::CHECK, "Resolve", ThreadStatus::Resolved),
                    ThreadStatus::Resolved => {
                        (icons::ARROW_COUNTER_CLOCKWISE, "Reopen", ThreadStatus::Active)
                    }
                });
                let resolve_width = resolve
                    .map_or(0.0, |(_, label, _)| buttons::icon_button_width(ui, label) + 10.0);
                if thread.can_reply {
                    let has_draft = !is_inert && threads.has_draft(&key);
                    let draft = has_draft.then(|| {
                        threads.text_mut(&key).lines().next().unwrap_or_default().to_owned()
                    });
                    let width = (ui.available_width() - resolve_width).max(120.0);
                    if closed_reply(ui, width, draft.as_deref()).clicked() {
                        threads.open_reply(&thread.id);
                    }
                }
                if let Some((icon, label, status)) = resolve {
                    let button = buttons::secondary_icon(ui, icon, label);
                    if button.clicked() {
                        event =
                            Some(ThreadEvent::SetStatus { thread_id: thread.id.clone(), status });
                    }
                }
            });
        },
    );
    event
}

/// The closed reply box: a field-like button with "Reply…", or the start of a kept draft.
fn closed_reply(ui: &mut Ui, width: f32, draft: Option<&str>) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(width, style::CONTROL_HEIGHT), Sense::click());
    let label = match draft {
        Some(text) => format!("Draft: {text}"),
        None => "Reply…".to_owned(),
    };
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            true,
            if draft.is_some() { "Continue reply" } else { "Reply" },
        )
    });
    let hover = row::hover_amount(ui, &response);
    let stroke = motion::mix(palette.border_strong, palette.text_secondary, hover * 0.5);
    ui.painter().rect(
        rect,
        style::RADIUS_CONTROL,
        palette.surface,
        Stroke::new(1.0, stroke),
        egui::StrokeKind::Inside,
    );
    let color = if draft.is_some() { palette.text } else { palette.text_secondary };
    let icon_center = pos2(rect.left() + 10.0 + icons::SIZE_SMALL / 2.0, rect.center().y);
    ui.painter().text(
        icon_center,
        egui::Align2::CENTER_CENTER,
        icons::ARROW_BEND_UP_LEFT,
        icons::rest_font(&palette, icons::SIZE_SMALL),
        palette.text_secondary,
    );
    let text_left = icon_center.x + icons::SIZE_SMALL / 2.0 + 8.0;
    let mut job = egui::text::LayoutJob::simple_singleline(
        label,
        egui::FontId::proportional(style::BODY_SIZE - 1.0),
        color,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.right() - 10.0 - text_left);
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(pos2(text_left, rect.center().y - galley.size().y / 2.0), galley, color);
    focus::ring(ui, &response, rect);
    response.on_hover_cursor(egui::CursorIcon::Text)
}

/// The open reply composer: avatar, Write/Preview editor, Reply and Reply & resolve.
fn composer(
    ui: &mut Ui,
    thread: &ReviewThread,
    threads: &mut ThreadsUi,
    key: &str,
) -> Option<ThreadEvent> {
    let me = threads.me_or_you();
    let failure = match threads.post_state(key) {
        Some(PostState::Failed { message, needs_token }) => Some((message.clone(), *needs_token)),
        _ => None,
    };
    let grab_focus = threads.focus.as_deref() == Some(key);
    if grab_focus {
        threads.focus = None;
    }
    let suggestion =
        thread.anchor.as_ref().and_then(|anchor| suggestion::suggestion_lines(ui.ctx(), anchor));
    let can_resolve = thread.can_resolve && thread.status == ThreadStatus::Active;
    let platform = threads.context.platform;
    let mut event = None;
    section(ui, Margin::same(16), |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = AVATAR_GAP;
            avatar::avatar(ui, &me, AVATAR_SIZE);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                let options = markdown_editor::Options {
                    hint: "Reply…",
                    submit_label: "Reply",
                    can_cancel: false,
                    grab_focus,
                    min_rows: 2,
                    suggestion: suggestion.as_deref(),
                    platform: Some(platform),
                    secondary_label: can_resolve.then_some("Reply & resolve"),
                    footer_hint: Some(concat!(
                        shortcut!(command Enter),
                        " reply · Esc keeps draft"
                    )),
                    leading_buttons: true,
                    ..markdown_editor::Options::default()
                };
                let id = egui::Id::unique(("thread_reply", &thread.id));
                let text = threads.text_mut(key);
                let output = markdown_editor::show(ui, id, text, &options);
                let submitted = match output {
                    Some(Outcome::Submitted { with_secondary }) => {
                        Some((text.clone(), with_secondary))
                    }
                    _ => None,
                };
                threads.keep_draft(key, None);
                if let Some((body, resolve)) = submitted {
                    let thread_id = thread.id.clone();
                    event = Some(ThreadEvent::Reply { thread_id, body, resolve });
                } else if output == Some(Outcome::Dismissed) {
                    threads.reply_open.remove(&thread.id);
                }
                // Not sent: the text stays in the editor, with the reason and Retry under it.
                if let Some((message, needs_token)) = &failure {
                    threads.failure_actions(ui, key, message, *needs_token);
                }
            });
        });
    });
    event
}
