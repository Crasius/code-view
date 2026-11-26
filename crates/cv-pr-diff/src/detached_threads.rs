//! Threads whose lines are not shown: outdated ones, and ones on unchanged lines that are still
//! collapsed. The latter get a button that reveals their line and scrolls to them.

use egui::{RichText, Ui};

use crate::diff_view::DiffInputs;
use cv_base::theme::{self, icons};
use cv_code::gaps::{Gap, Reveal};
use cv_pr_threads::{self as threads, CardOptions, ThreadEvent, ThreadsUi};
use cv_widgets::{buttons, glyph};

pub enum DetachedEvent {
    Thread(ThreadEvent),
    /// Reveal gap `gap` and scroll to the thread.
    ShowLine {
        gap: usize,
        thread_id: String,
    },
}

pub struct Detached<'a> {
    pub threads: &'a [usize],
    pub gaps: &'a [Gap],
    pub reveals: &'a [Reveal],
    pub can_expand: bool,
}

pub fn show(
    ui: &mut Ui,
    detached: &Detached<'_>,
    inputs: &DiffInputs<'_>,
    threads_ui: &mut ThreadsUi,
) -> Option<DetachedEvent> {
    let palette = theme::palette(ui.ctx());
    let mut event = None;
    let count = detached.threads.len();
    let title = if count == 1 {
        "1 comment on lines not shown in this diff".to_owned()
    } else {
        format!("{count} comments on lines not shown in this diff")
    };
    egui::Frame::new()
        .fill(palette.surface)
        .inner_margin(egui::Margin::symmetric(12, 2))
        .stroke(egui::Stroke::new(1.0, palette.border))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let section = egui::CollapsingHeader::new(
                RichText::new(format!("{}  {title}", icons::CHAT_CIRCLE_TEXT))
                    .color(palette.text_secondary),
            )
            .id_salt("detached_threads")
            .show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("detached_scroll").max_height(280.0).show(
                    ui,
                    |ui| {
                        let options = CardOptions { show_location: false };
                        for index in detached.threads {
                            let thread = inputs.threads[*index];
                            if let Some(show_line) = line_note(ui, detached, thread) {
                                event = Some(show_line);
                            }
                            if let Some(thread_event) =
                                threads::show_card(ui, thread, threads_ui, &options)
                            {
                                event = Some(DetachedEvent::Thread(thread_event));
                            }
                            ui.add_space(6.0);
                        }
                    },
                );
            });
            glyph::speak_as(&section.header_response, &title);
        });
    event
}

/// Above a card: the button that reveals the thread's collapsed line, or which line it is on.
fn line_note(
    ui: &mut Ui,
    detached: &Detached<'_>,
    thread: &review_core::ReviewThread,
) -> Option<DetachedEvent> {
    let line =
        thread.anchor.as_ref().and_then(|anchor| anchor.new_line.or(anchor.old_line)).unwrap_or(0);
    if let Some(gap) = hidden_gap(detached, thread) {
        let label = format!("Show line {line} in the diff");
        let clicked = buttons::subtle(ui, icons::ARROWS_OUT_LINE_VERTICAL, &label).clicked();
        return clicked.then(|| DetachedEvent::ShowLine { gap, thread_id: thread.id.clone() });
    }
    let note = if thread.is_outdated {
        format!("Line {line} changed after this comment")
    } else {
        format!("Line {line}")
    };
    let palette = theme::palette(ui.ctx());
    ui.label(RichText::new(note).size(theme::style::SMALL_SIZE).color(palette.text_secondary));
    None
}

/// The collapsed gap holding the thread's line, when revealing it would place the thread inline.
fn hidden_gap(detached: &Detached<'_>, thread: &review_core::ReviewThread) -> Option<usize> {
    if thread.is_outdated || !detached.can_expand {
        return None;
    }
    let anchor = thread.anchor.as_ref()?;
    detached.gaps.iter().enumerate().position(|(index, gap)| {
        let is_hidden = detached.reveals.get(index).copied().unwrap_or_default().hidden(gap) > 0;
        is_hidden && gap.contains(anchor.old_line, anchor.new_line)
    })
}
