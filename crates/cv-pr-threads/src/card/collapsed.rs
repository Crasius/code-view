//! A resolved or collapsed thread as one line, and a comment's plain-text excerpt.

use cv_base::theme::{self, icons, motion, style};
use cv_widgets::{focus, glyph, row};
use egui::{Align, Layout, RichText, Sense, Stroke, Ui, vec2};
use review_core::{ReviewThread, ThreadStatus};

use super::super::suggestion;
use super::super::{ThreadEvent, ThreadsUi};
use super::{COLLAPSED_HEIGHT, NEW_DOT};

/// One line for a resolved or collapsed thread: status, lines, the first comment, the reply count
/// and Show (and Reopen where allowed).
pub(super) fn collapsed(
    ui: &mut Ui,
    thread: &ReviewThread,
    threads: &mut ThreadsUi,
) -> Option<ThreadEvent> {
    let palette = theme::palette(ui.ctx());
    let is_resolved = thread.status == ThreadStatus::Resolved;
    let is_unread = threads.is_unread(thread);
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, COLLAPSED_HEIGHT), Sense::click());
    let summary = collapsed_summary(thread);
    let name =
        format!("{}: {summary}", if is_resolved { "Resolved thread" } else { "Collapsed thread" });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
    let fill = row::state_fill(ui, &response, motion::mix(palette.ground, palette.surface, 0.55));
    ui.painter().rect(
        rect,
        style::RADIUS_CARD,
        fill,
        Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );
    focus::ring(ui, &response, rect);
    let mut event = None;
    let mut expand = response.clicked();
    let inner = rect.shrink2(vec2(14.0, 0.0));
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)),
        |ui| {
            // The whole bar is the button: its text must not take the press for selecting.
            ui.style_mut().interaction.selectable_labels = false;
            ui.spacing_mut().item_spacing.x = 8.0;
            let (icon, color, label) = if is_resolved {
                (icons::CHECK, palette.success, "Resolved")
            } else {
                (icons::CHAT_CIRCLE_TEXT, palette.accent, "Active")
            };
            glyph::decorative(
                ui,
                RichText::new(icon)
                    .font(icons::rest_font(&palette, icons::SIZE_SMALL))
                    .color(color),
            );
            ui.label(RichText::new(label).size(style::BODY_SIZE - 1.0).color(palette.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                if is_resolved
                    && thread.can_resolve
                    && threads.context.can_write
                    && ui.link(RichText::new("Reopen").size(style::BODY_SIZE - 1.0)).clicked()
                {
                    event = Some(ThreadEvent::SetStatus {
                        thread_id: thread.id.clone(),
                        status: ThreadStatus::Active,
                    });
                }
                if ui.link(RichText::new("Show").size(style::BODY_SIZE - 1.0)).clicked() {
                    expand = true;
                }
                if is_unread {
                    let (dot, _) = ui.allocate_exact_size(vec2(NEW_DOT, NEW_DOT), Sense::hover());
                    ui.painter().circle_filled(dot.center(), NEW_DOT / 2.0, palette.accent);
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("· {summary}"))
                                .size(style::BODY_SIZE - 1.0)
                                .color(palette.text_secondary),
                        )
                        .truncate(),
                    );
                });
            });
        },
    );
    if expand {
        threads.set_collapsed(&thread.id, false);
    }
    event
}

/// "Lines 96 · Jonas: “Rename to `TryIssue`?” · 3 replies".
fn collapsed_summary(thread: &ReviewThread) -> String {
    let mut parts = Vec::with_capacity(3);
    if let Some(anchor) = &thread.anchor {
        parts.push(suggestion::lines_title(anchor));
    }
    if let Some(first) = thread.comments.first() {
        parts.push(format!(
            "{}: “{}”",
            super::first_name(&first.author.display_name),
            excerpt(&first.body, 80)
        ));
    }
    let replies = thread.comments.len().saturating_sub(1);
    parts.push(match replies {
        0 => "no replies".to_owned(),
        1 => "1 reply".to_owned(),
        count => format!("{count} replies"),
    });
    parts.join(" · ")
}

/// The first `max` characters of a comment as one line of plain text.
pub fn excerpt(body: &str, max: usize) -> String {
    let flat: String = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("```"))
        .collect::<Vec<_>>()
        .join(" ");
    let flat = flat.replace(['*', '`', '#', '>'], "");
    if flat.chars().count() <= max {
        return flat.trim().to_owned();
    }
    let cut: String = flat.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpts_are_one_plain_line() {
        assert_eq!(
            excerpt("**Rename** to `TryIssue`?\n\n```suggestion\nfn x()\n```", 80),
            "Rename to TryIssue? fn x()"
        );
        assert_eq!(excerpt("abcdefghij", 4), "abcd…");
    }
}
