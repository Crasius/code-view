//! One thread in the panel's list: avatar, name, "line N · time", a two-line excerpt, the
//! replies and the unread dot; a general thread opens its card right under its row.

use std::sync::Arc;

use cv_base::theme::{self, style};
use cv_widgets::{avatar, focus, row, time_text};
use egui::text::{LayoutJob, TextWrapping};
use egui::{Align, Galley, Margin, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use review_core::{ReviewThread, ThreadStatus};

use super::super::ThreadsUi;
use super::super::card::{self, CardOptions, excerpt};
use super::super::{hover, suggestion};
use super::{MARGIN, PanelEvent, ROW_AVATAR, select};

/// Avatar, name, "line N · time", a two-line excerpt and the replies; the unread dot on the right.
/// A thread on its way (`is_pending`) is the same row, inert, with "Posting…" for its time.
pub(super) fn thread_row(
    ui: &mut Ui,
    threads: &mut ThreadsUi,
    thread: &ReviewThread,
    is_pending: bool,
    events: &mut Vec<PanelEvent>,
) {
    let Some(first) = thread.comments.first() else {
        return;
    };
    let is_selected = !is_pending && threads.panel.selected.as_deref() == Some(thread.id.as_str());
    let new = if is_pending { None } else { threads.new_comments(thread) };
    let width = ui.available_width();
    let text_width = (width - ROW_TEXT_LEFT - f32::from(MARGIN)).max(40.0);
    let new_from = new.as_ref().map(|new| new.authors.as_slice());
    let text = RowText::layout(ui, thread, is_pending, new_from, text_width);

    let (rect, response) = ui.allocate_exact_size(
        vec2(width, text.height()),
        if is_pending { Sense::hover() } else { Sense::click() },
    );
    let unread = if new.is_some() { ", unread" } else { "" };
    let outdated = if thread.is_outdated { ", outdated" } else { "" };
    let label = format!(
        "{}, {}{unread}{outdated}: {}",
        first.author.display_name,
        text.meta_text,
        excerpt(&first.body, 120)
    );
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, is_selected, &label));
    if is_selected && threads.panel.scroll_to_selected {
        response.scroll_to_me(Some(Align::Center));
        threads.panel.scroll_to_selected = false;
    }
    if !is_pending && response.contains_pointer() {
        hover::note(ui.ctx(), threads, thread);
    }
    row::list_background(ui, &response, rect, is_selected);
    let replies = text.paint(ui, rect, thread, new.is_some());
    focus::ring(ui, &response, rect);
    // Added after the row, so it takes the click over the row's own.
    if thread.is_outdated && thread.origin.is_some() && origin_link(ui, rect, replies) {
        events.push(PanelEvent::ShowOrigin { thread_id: thread.id.clone() });
    }
    if response.clicked() {
        if thread.anchor.is_none() && is_selected {
            threads.panel.selected = None;
        } else {
            select(threads, thread, events);
        }
    }
    // A general thread has no file to show it in: it opens right here.
    if thread.anchor.is_none() && is_selected {
        egui::Frame::new()
            .inner_margin(Margin { left: MARGIN, right: MARGIN, top: 0, bottom: 14 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some(event) =
                    card::show(ui, thread, threads, &CardOptions { show_location: false })
                {
                    events.push(PanelEvent::Thread(event));
                }
            });
    }
}

/// Where a row's texts start, past the avatar.
const ROW_TEXT_LEFT: f32 = MARGIN as f32 + ROW_AVATAR + 12.0;

/// A thread row's texts, laid out.
struct RowText {
    name: Arc<Galley>,
    meta: Arc<Galley>,
    /// "line N · time", for the row's accessible label.
    meta_text: String,
    body: Arc<Galley>,
    replies: Arc<Galley>,
}

impl RowText {
    /// The texts of `thread`, which has a first comment; the excerpt wraps at `text_width`.
    fn layout(
        ui: &Ui,
        thread: &ReviewThread,
        is_pending: bool,
        new_from: Option<&[String]>,
        text_width: f32,
    ) -> Self {
        let palette = theme::palette(ui.ctx());
        let painter = ui.painter();
        let first = &thread.comments[0];
        let name = painter.layout_no_wrap(
            first.author.display_name.clone(),
            theme::fonts::semibold_font(13.0),
            palette.text,
        );
        let line =
            thread.anchor.as_ref().map_or_else(|| "general".to_owned(), suggestion::lines_label);
        let newest = thread
            .comments
            .iter()
            .map(|comment| comment.created_at)
            .max()
            .unwrap_or(first.created_at);
        let when = if is_pending { card::POSTING.to_owned() } else { time_text::relative(newest) };
        let meta_text = format!("{line} · {when}");
        let small = egui::FontId::proportional(style::SMALL_SIZE);
        let meta = painter.layout_no_wrap(meta_text.clone(), small.clone(), palette.text_secondary);
        let mut job = LayoutJob::simple(
            excerpt(&first.body, 240),
            egui::FontId::proportional(13.0),
            palette.text_secondary,
            text_width,
        );
        job.wrap = TextWrapping {
            max_width: text_width,
            max_rows: 2,
            break_anywhere: false,
            overflow_character: Some('…'),
        };
        let body = painter.layout_job(job);
        let replies =
            painter.layout_no_wrap(replies_text(thread, new_from), small, palette.text_secondary);
        Self { name, meta, meta_text, body, replies }
    }

    fn line_height(&self) -> f32 {
        self.name.size().y.max(self.meta.size().y)
    }

    fn height(&self) -> f32 {
        14.0 + self.line_height() + 4.0 + self.body.size().y + 4.0 + self.replies.size().y + 14.0
    }

    /// Paints the row into `rect` with the unread dot when `has_new`; returns the replies' rect.
    fn paint(self, ui: &Ui, rect: Rect, thread: &ReviewThread, has_new: bool) -> Rect {
        let palette = theme::palette(ui.ctx());
        let painter = ui.painter();
        let left = f32::from(MARGIN);
        let text_left = rect.left() + ROW_TEXT_LEFT;
        let line_height = self.line_height();
        painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, palette.border));
        let top = rect.top() + 14.0;
        avatar::paint(
            painter,
            pos2(rect.left() + left + ROW_AVATAR / 2.0, top + ROW_AVATAR / 2.0),
            ROW_AVATAR,
            &thread.comments[0].author,
        );
        let (name_size, meta_size) = (self.name.size(), self.meta.size());
        painter.galley(
            pos2(text_left, top + (line_height - name_size.y) / 2.0),
            self.name,
            palette.text,
        );
        let meta_left = text_left + name_size.x + 6.0;
        let meta_room = rect.right() - left - 14.0 - meta_left;
        if meta_size.x <= meta_room {
            painter.galley(
                pos2(meta_left, top + (line_height - meta_size.y) / 2.0 + 1.0),
                self.meta,
                palette.text_secondary,
            );
        }
        if has_new {
            painter.circle_filled(
                pos2(rect.right() - left - 4.0, top + line_height / 2.0),
                4.0,
                palette.accent,
            );
        }
        let body_top = top + line_height + 4.0;
        let body_height = self.body.size().y;
        painter.galley(pos2(text_left, body_top), self.body, palette.text_secondary);
        let replies =
            Rect::from_min_size(pos2(text_left, body_top + body_height + 4.0), self.replies.size());
        painter.galley(replies.min, self.replies, palette.text_secondary);
        replies
    }
}

/// "Show original" at the right end of an outdated thread's replies line; true when clicked.
fn origin_link(ui: &mut Ui, rect: Rect, replies: Rect) -> bool {
    let palette = theme::palette(ui.ctx());
    let label = RichText::new(card::SHOW_ORIGIN).size(style::SMALL_SIZE);
    let width = ui
        .painter()
        .layout_no_wrap(
            card::SHOW_ORIGIN.to_owned(),
            egui::FontId::proportional(style::SMALL_SIZE),
            palette.text,
        )
        .size()
        .x;
    let min_left = replies.right() + 8.0;
    let link_rect = Rect::from_min_size(
        pos2((rect.right() - f32::from(MARGIN) - width).max(min_left), replies.top()),
        vec2(width, replies.height()),
    );
    ui.put(link_rect, egui::Link::new(label)).clicked()
}

/// "2 replies · new from Mara", "no replies", "1 reply · suggested change".
fn replies_text(thread: &ReviewThread, new_from: Option<&[String]>) -> String {
    let replies = thread.comments.len().saturating_sub(1);
    let mut text = match replies {
        0 => "no replies".to_owned(),
        1 => "1 reply".to_owned(),
        count => format!("{count} replies"),
    };
    if let Some(authors) = new_from.filter(|authors| !authors.is_empty()) {
        text.push_str(" · new from ");
        text.push_str(&authors.join(", "));
    } else if thread.comments.iter().any(|comment| suggestion::has_suggestion(&comment.body)) {
        text.push_str(" · suggested change");
    }
    if thread.status == ThreadStatus::Resolved {
        text.push_str(" · resolved");
    }
    if thread.is_outdated {
        text.push_str(" · outdated");
    }
    text
}
