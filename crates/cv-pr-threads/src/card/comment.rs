//! The comments of a thread card. Each: avatar, name, badges and time, then its body, with suggestion
//! blocks drawn as "Suggested change" panels. The user's own comment adds its menu, and the
//! composer or the delete question while it is edited or deleted ([`own_comment`]).

use std::borrow::Cow;

use cv_base::theme::{self, Palette, icons, style};
use cv_markdown as markdown;
use cv_markdown::RenderOptions;
use cv_widgets::{avatar, buttons, scroll, time_text};
use egui::{Align, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};
use review_core::provider::CommentActions;
use review_core::{ReviewThread, ThreadComment};

use super::super::edits::View;
use super::super::suggestion::{self, Part};
use super::super::{ThreadEvent, ThreadsUi};
use super::own_comment;
use super::{AVATAR_GAP, AVATAR_SIZE, NEW_DOT, NewComments, POSTING, Phase, hairline, section};

/// The comments, hairline above each; the user's own carry their menu unless the thread is on its
/// way. Deleted comments are left out until the reloaded threads drop them.
pub(super) fn comments(
    ui: &mut Ui,
    thread: &ReviewThread,
    threads: &mut ThreadsUi,
    new: Option<&NewComments>,
    phase: Phase,
) -> Option<ThreadEvent> {
    let palette = theme::palette(ui.ctx());
    let mut event = None;
    let jump = threads.jump_to_new.take_if(|(id, _)| *id == thread.id).map(|(_, index)| index);
    let last = thread.comments.len().saturating_sub(1);
    for (index, comment) in thread.comments.iter().enumerate() {
        if threads.edits.is_deleted(&thread.id, &comment.id) {
            continue;
        }
        hairline(ui, &palette);
        let is_new = new.is_some_and(|new| index >= new.first && is_from_others(threads, comment));
        let note = (phase == Phase::Sending && index == last).then_some(POSTING);
        let actions = match phase {
            Phase::Sending | Phase::Sent => CommentActions::NONE,
            Phase::Loaded => own_actions(threads, thread, comment),
        };
        let (shown, comment_event) =
            comment_row(ui, thread, comment, threads, is_new, note, actions);
        if jump == Some(index) {
            shown.scroll_to_me(Some(Align::Center));
        }
        if comment_event.is_some() {
            event = comment_event;
        }
    }
    event
}

/// What the user may do with `comment`: Edit and Delete on their own comments where the platform
/// allows them.
fn own_actions(
    threads: &mut ThreadsUi,
    thread: &ReviewThread,
    comment: &ThreadComment,
) -> CommentActions {
    // Asking starts the one request that says who the user is; until then nothing is offered.
    threads.wants_me = true;
    threads.edits.actions(thread, comment, threads.known_me(), threads.context.can_write)
}

fn is_from_others(threads: &ThreadsUi, comment: &ThreadComment) -> bool {
    threads.known_me().is_none_or(|me| me.id != comment.author.id)
}

/// Avatar, name, badges and time (or `note` in its place), then the body. The user's own comment
/// (`actions` not none) carries the "Comment actions" menu; while it is edited the composer takes
/// the body's place, and the delete question sits under the body. Returns the row, for scrolling
/// to it, and what the user asked for.
pub(super) fn comment_row(
    ui: &mut Ui,
    thread: &ReviewThread,
    comment: &ThreadComment,
    threads: &mut ThreadsUi,
    is_new: bool,
    note: Option<&str>,
    actions: CommentActions,
) -> (egui::Response, Option<ThreadEvent>) {
    let view = threads.edits.view(&thread.id, &comment.id);
    let shown = match &view {
        // Optimistic: the new text shows at once, already marked as edited.
        View::Saving { body } => Cow::Owned(ThreadComment {
            body: body.clone(),
            updated_at: Some(comment.updated_at.unwrap_or(comment.created_at)),
            ..comment.clone()
        }),
        View::Saved(stored) => Cow::Borrowed(stored),
        _ => Cow::Borrowed(comment),
    };
    let note = match view {
        View::Saving { .. } => Some(SAVING),
        View::Deleting => Some(DELETING),
        _ => note,
    };
    let line =
        Header { is_new, note, has_menu: !actions.is_none() && view == View::Plain, actions };
    let platform = threads.context.platform;
    let body_id = ("comment", thread.id.as_str(), comment.id.as_str());
    let mut event = None;
    let response = ui
        .scope(|ui| {
            if matches!(view, View::Saving { .. } | View::Deleting) {
                super::inert(ui, true);
            }
            section(ui, Margin { left: 16, right: 16, top: 16, bottom: 14 }, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = AVATAR_GAP;
                    avatar::avatar(ui, &shown.author, AVATAR_SIZE);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        header(ui, thread, &shown, threads, line);
                        event = match &view {
                            View::Editing { failure } => {
                                own_comment::editor(ui, thread, comment, threads, failure.as_ref())
                            }
                            View::Confirming { failure } => {
                                body(ui, body_id, &shown.body, platform);
                                let failure = failure.as_ref();
                                own_comment::delete_question(ui, thread, comment, threads, failure)
                            }
                            _ => {
                                body(ui, body_id, &shown.body, platform);
                                None
                            }
                        };
                    });
                })
                .response
            })
        })
        .inner;
    (response, event)
}

/// Where the time of an edit on its way goes until the platform has it.
const SAVING: &str = "Saving…";
/// Where the time of a comment being deleted goes until the platform answered.
const DELETING: &str = "Deleting…";

/// What the header line of a comment shows besides the comment itself.
#[derive(Clone, Copy)]
struct Header<'a> {
    is_new: bool,
    note: Option<&'a str>,
    has_menu: bool,
    actions: CommentActions,
}

/// Name, badges, time (or the note), "edited", "New", and the menu at the end of the line.
fn header(
    ui: &mut Ui,
    thread: &ReviewThread,
    comment: &ThreadComment,
    threads: &mut ThreadsUi,
    line: Header<'_>,
) {
    let palette = theme::palette(ui.ctx());
    let is_author = threads
        .context
        .author_id
        .as_deref()
        .is_some_and(|author| !author.is_empty() && author == comment.author.id);
    let Header { is_new, note, has_menu, actions } = line;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if is_new {
            let (dot, _) = ui.allocate_exact_size(vec2(NEW_DOT, NEW_DOT), Sense::hover());
            ui.painter().circle_filled(dot.center(), NEW_DOT / 2.0, palette.accent);
        }
        let when = note.map_or_else(|| time_text::relative(comment.created_at), str::to_owned);
        // A long name is cut (in full on hover); the badges, the time and the menu keep their room.
        let small = egui::FontId::proportional(style::SMALL_SIZE);
        let measure = |text: &str, font: &egui::FontId| {
            ui.painter().layout_no_wrap(text.to_owned(), font.clone(), palette.text).size().x + 8.0
        };
        let after = measure(&when, &small)
            + if is_author {
                measure(AUTHOR, &egui::FontId::proportional(style::TINY_SIZE)) + 10.0
            } else {
                0.0
            }
            + if comment.updated_at.is_some() { measure("edited", &small) } else { 0.0 }
            + if is_new { measure("New", &small) } else { 0.0 }
            + if has_menu { buttons::icon_width() + 8.0 } else { 0.0 };
        ui.scope(|ui| {
            ui.set_max_width((ui.available_width() - after).max(0.0));
            ui.add(
                egui::Label::new(
                    RichText::new(&comment.author.display_name)
                        .font(theme::fonts::semibold_font(style::BODY_SIZE)),
                )
                .truncate(),
            );
        });
        if is_author {
            author_badge(ui, &palette);
        }
        let time =
            ui.label(RichText::new(when).size(style::SMALL_SIZE).color(palette.text_secondary));
        if note.is_none() {
            time.on_hover_text(time_text::exact(comment.created_at));
        }
        if comment.updated_at.is_some() {
            ui.label(RichText::new("edited").size(style::SMALL_SIZE).color(palette.text_secondary));
        }
        if is_new {
            ui.label(RichText::new("New").size(style::SMALL_SIZE).color(palette.accent_text));
        }
        if has_menu {
            // At the end of the line: a nested right-to-left layout would take the whole height.
            ui.add_space((ui.available_width() - buttons::icon_width()).max(0.0));
            own_comment::menu(ui, thread, comment, threads, actions);
        }
    });
}

const AUTHOR: &str = "Author";

fn author_badge(ui: &mut Ui, palette: &Palette) {
    let font = egui::FontId::proportional(style::TINY_SIZE);
    let galley = ui.painter().layout_no_wrap(AUTHOR.to_owned(), font, palette.text_secondary);
    let (rect, response) = ui.allocate_exact_size(galley.size() + vec2(10.0, 2.0), Sense::hover());
    ui.painter().rect_stroke(
        rect,
        3,
        Stroke::new(1.0, palette.border_strong),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, palette.text_secondary);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, AUTHOR));
    response.on_hover_text("Opened this pull request");
}

/// Markdown with its suggestion blocks drawn as "Suggested change" panels.
fn body(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug + Copy,
    text: &str,
    platform: review_core::ProviderKind,
) {
    let parts = suggestion::split(text);
    for (index, part) in parts.iter().enumerate() {
        match part {
            Part::Markdown(markdown) => {
                markdown::render(ui, (id, index), markdown, RenderOptions::for_platform(platform));
            }
            Part::Suggestion(code) => suggested_change(ui, ("suggestion", id, index), code),
        }
    }
}

/// A read-only "Suggested change" panel: none of the platforms documents an API that applies a
/// suggestion, so the panel offers the code to copy instead of an "Apply" button that cannot work.
fn suggested_change(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, code: &str) {
    let palette = theme::palette(ui.ctx());
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(palette.ground)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(style::RADIUS_CONTROL)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::Frame::new().inner_margin(Margin { left: 10, right: 4, top: 2, bottom: 2 }).show(
                ui,
                |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Suggested change")
                                .size(style::SMALL_SIZE)
                                .color(palette.text_secondary),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if buttons::icon(ui, icons::COPY, "Copy the suggested code").clicked() {
                                ui.ctx().copy_text(code.to_owned());
                            }
                        });
                    });
                },
            );
            hairline(ui, &palette);
            let added = palette.success.gamma_multiply(if palette.is_dark { 0.16 } else { 0.09 });
            egui::Frame::new().fill(added).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if code.is_empty() {
                    ui.label(
                        RichText::new("Removes the commented lines")
                            .size(style::SMALL_SIZE + 1.0)
                            .color(palette.text_secondary),
                    );
                    return;
                }
                // Long lines scroll sideways inside the panel instead of running past the card.
                scroll::sideways(ui, salt, |ui| {
                    // The size of the code blocks in the comment's own Markdown.
                    let font =
                        theme::fonts::code_font(style::BODY_SIZE - markdown::CODE_SIZE_DELTA);
                    for line in code.lines() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            ui.label(RichText::new("+").font(font.clone()).color(palette.success));
                            ui.add(
                                egui::Label::new(
                                    RichText::new(line).font(font.clone()).color(palette.text),
                                )
                                .extend(),
                            );
                        });
                    }
                });
            });
        });
}
