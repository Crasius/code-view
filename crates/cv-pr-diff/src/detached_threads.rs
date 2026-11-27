//! Threads whose lines are not shown: outdated ones, and ones on unchanged lines that are still
//! collapsed. The latter get a button that reveals their line and scrolls to them.

use egui::{RichText, Ui};

use crate::diff_view::{DiffInputs, DiffRows};
use cv_base::theme::{self, icons};
use cv_pr_threads::{self as threads, CardOptions, ThreadEvent, ThreadsUi};
use cv_widgets::{buttons, glyph};

pub(crate) enum DetachedEvent {
    Thread(ThreadEvent),
    /// Reveal gap `gap` and scroll to the thread.
    ShowLine {
        gap: usize,
        thread_id: String,
    },
}

pub(crate) struct Detached<'a> {
    pub threads: &'a [usize],
    pub rows: &'a DiffRows,
    pub can_expand: bool,
}

pub(crate) fn show(
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
    let gap = detached.rows.thread_gap(thread, detached.can_expand);
    if let (Some(gap), Some(line)) = (gap, thread_line(thread)) {
        let label = format!("Show line {line} in the diff");
        let clicked = buttons::subtle(ui, icons::ARROWS_OUT_LINE_VERTICAL, &label).clicked();
        return clicked.then(|| DetachedEvent::ShowLine { gap, thread_id: thread.id.clone() });
    }
    let palette = theme::palette(ui.ctx());
    let note = line_note_text(thread);
    ui.label(RichText::new(note).size(theme::style::SMALL_SIZE).color(palette.text_secondary));
    None
}

/// The line the thread is on, the new side first; none for a comment on the whole file.
fn thread_line(thread: &review_core::ReviewThread) -> Option<u32> {
    thread.anchor.as_ref().and_then(|anchor| anchor.new_line.or(anchor.old_line))
}

/// Which line a thread is on, or that it is on the file, and whether that changed since.
fn line_note_text(thread: &review_core::ReviewThread) -> String {
    match (thread_line(thread), thread.is_outdated) {
        (Some(line), false) => format!("Line {line}"),
        (Some(line), true) => format!("Line {line} changed after this comment"),
        (None, false) => "On the file".to_owned(),
        (None, true) => "The file changed after this comment".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::line_note_text;
    use crate::testing::thread_on_old_line;

    /// GitHub anchors a comment on the whole file with neither line: it read "Line 0".
    #[test]
    fn a_file_level_comment_is_labelled_on_the_file_not_line_0() {
        let on_line = thread_on_old_line(7, "on a line");
        let mut outdated = on_line.clone();
        outdated.is_outdated = true;
        let mut on_file = on_line.clone();
        on_file.anchor.as_mut().expect("the sample is anchored").old_line = None;
        let mut file_outdated = on_file.clone();
        file_outdated.is_outdated = true;
        let cases = [
            (on_line, "Line 7"),
            (outdated, "Line 7 changed after this comment"),
            (on_file, "On the file"),
            (file_outdated, "The file changed after this comment"),
        ];
        for (thread, expected) in cases {
            assert_eq!(line_note_text(&thread), expected);
        }
    }
}
