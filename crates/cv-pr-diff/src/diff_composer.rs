//! The inline "new thread" card that opens under a diff line, or under the last line of a range
//! (over its first line when the range was picked dragging up): the lines as a pill, the editor
//! with Comment and Cancel, and the draft kept on disk until the thread is posted.
//!
//! Posting hands the card over in place: the editor gives way to the thread as it will be posted
//! (the very card the thread will have, marked "Posting…"), and the loaded thread takes that
//! card's place at the same height (see `DiffView::rebuild_rows`). A failure brings the editor
//! back with the text, the reason and Retry.

use egui::{Margin, RichText, Ui, vec2};
use review_core::{DiffSide, NewThread, ThreadAnchor};
use review_git::FileDiff;

use crate::line_anchor;
use cv_base::platform::shortcut;
use cv_base::theme::{self, style};
use cv_code::diff_rows::ComposerAnchor;
use cv_editor::markdown_editor::{self, Outcome};
use cv_pr_threads::{self as threads, CardOptions, PostState, ThreadsUi, suggestion};
use cv_widgets::glyph;

pub(crate) enum ComposerOutcome {
    Open,
    /// Post the thread; the card shows it as it will be posted until the post is through.
    Create {
        key: String,
        thread: NewThread,
    },
    /// Cancelled.
    Close,
}

pub struct Composer {
    pub anchor: ComposerAnchor,
    thread_anchor: ThreadAnchor,
    /// The draft key in [`ThreadsUi`].
    key: String,
    needs_focus: bool,
    /// Why the comment now sits on another line than the user picked (a push moved or changed it).
    moved_note: Option<String>,
    /// The thread was sent and the card shows it on its way (until it fails).
    is_handed_off: bool,
}

impl Composer {
    /// A composer the user just opened, taking the focus; `above` puts it over the first line of
    /// a range picked dragging up, where the pointer is (a single line keeps it below).
    pub(crate) fn new(thread_anchor: ThreadAnchor, above: bool) -> Self {
        let mut composer = Self::restored(thread_anchor);
        composer.anchor.above = above && composer.anchor.range.is_some();
        composer.needs_focus = true;
        composer
    }

    /// A composer for a draft kept from an earlier visit: it opens without taking the focus.
    pub(crate) fn restored(thread_anchor: ThreadAnchor) -> Self {
        Self {
            anchor: ComposerAnchor {
                old_line: thread_anchor.old_line,
                new_line: thread_anchor.new_line,
                range: thread_anchor.range.clone(),
                above: false,
            },
            key: threads::new_thread_key(&thread_anchor),
            thread_anchor,
            needs_focus: false,
            moved_note: None,
            is_handed_off: false,
        }
    }

    /// Side by side puts the card under the head pane, unless the lines are in the base.
    pub(crate) fn is_right(&self) -> bool {
        is_right(&self.thread_anchor)
    }

    /// The file the comment is written on, while the user writes it (not once it was sent).
    pub fn writing_path(&self) -> Option<&str> {
        (!self.is_handed_off).then_some(self.thread_anchor.path.as_str())
    }

    /// The draft key of the comment.
    pub(crate) fn key(&self) -> &str {
        &self.key
    }

    /// The thread was sent, and the threads loaded since show it: the loaded card takes over.
    pub(crate) fn has_landed(&self, threads: &ThreadsUi) -> bool {
        self.is_handed_off && threads.post_state(&self.key).is_none()
    }

    pub(crate) fn show(&mut self, ui: &mut Ui, threads: &mut ThreadsUi) -> ComposerOutcome {
        let state = threads.post_state(&self.key).cloned();
        if let Some(state) = state.as_ref().filter(|state| state.is_pending()) {
            self.is_handed_off = true;
            if let Some(thread) = threads.pending_thread(&self.key) {
                threads::show_pending_card(
                    ui,
                    &thread,
                    threads,
                    &CardOptions { show_location: false },
                    *state == PostState::Sending,
                );
            }
            return ComposerOutcome::Open;
        }
        if self.has_landed(threads) {
            return ComposerOutcome::Close;
        }
        self.is_handed_off = false;
        let palette = theme::palette(ui.ctx());
        let mut outcome = ComposerOutcome::Open;
        style::card_frame(&palette).inner_margin(Margin::same(16)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 10.0;
            header(ui, &self.thread_anchor);
            if let Some(note) = &self.moved_note {
                let label = ui.label(
                    RichText::new(format!("{}  {note}", theme::icons::WARNING))
                        .color(palette.warning),
                );
                glyph::speak_as(&label, note);
            }
            let suggestion = suggestion::suggestion_lines(ui.ctx(), &self.thread_anchor);
            let has_text = threads.has_draft(&self.key);
            let options = markdown_editor::Options {
                hint: "Leave a comment…",
                submit_label: "Comment",
                grab_focus: self.needs_focus,
                min_rows: 3,
                suggestion: suggestion.as_deref(),
                platform: Some(threads.context.platform),
                footer_hint: Some(if has_text {
                    "Draft saved"
                } else {
                    concat!(shortcut!(command Enter), " to comment")
                }),
                leading_buttons: true,
                ..markdown_editor::Options::default()
            };
            let id = egui::Id::unique(("diff_composer", &self.key));
            let editor = markdown_editor::show(ui, id, threads.text_mut(&self.key), &options);
            self.needs_focus = false;
            if editor == Some(Outcome::Cancelled) {
                threads.discard_draft(&self.key);
                outcome = ComposerOutcome::Close;
                return;
            }
            threads.keep_draft(&self.key, Some(&self.thread_anchor));
            if let Some(PostState::Failed { message, needs_token }) = &state {
                threads.failure_actions(ui, &self.key, message, *needs_token);
            }
            if matches!(editor, Some(Outcome::Submitted { .. })) {
                // A range picked dragging up had its card above the lines; the posted thread sits
                // under them, so the card on its way goes there now (the view keeps it in place).
                self.anchor.above = false;
                let body = threads.text_mut(&self.key).clone();
                outcome = ComposerOutcome::Create {
                    key: self.key.clone(),
                    thread: NewThread { body, anchor: Some(self.thread_anchor.clone()) },
                };
            }
        });
        outcome
    }

    /// The diff of the file was replaced (a live update): the draft follows its line into `new`,
    /// found by text, and says so when the line moved or changed. The text is never touched; its
    /// draft moves to the key of the new lines.
    pub(crate) fn follow(&mut self, old: &FileDiff, new: &FileDiff, threads: &mut ThreadsUi) {
        let anchor =
            line_anchor::for_numbers(old, self.thread_anchor.old_line, self.thread_anchor.new_line);
        let Some(target) = line_anchor::locate(&anchor, new) else {
            return;
        };
        let before = suggestion::lines_label(&self.thread_anchor);
        let shift = |from: Option<u32>, to: Option<u32>| match (from, to) {
            (Some(from), Some(to)) => i64::from(to) - i64::from(from),
            _ => 0,
        };
        let old_shift = shift(self.thread_anchor.old_line, target.old_line);
        let new_shift = shift(self.thread_anchor.new_line, target.new_line);
        let moved =
            |line: Option<u32>, by: i64| line.map(|line| (i64::from(line) + by).max(1) as u32);
        if let Some(range) = self.thread_anchor.range.as_mut() {
            range.start_old_line = moved(range.start_old_line, old_shift);
            range.start_new_line = moved(range.start_new_line, new_shift);
            if let Some(detail) = range.detail.as_mut() {
                for positions in [&mut detail.start, &mut detail.end] {
                    positions.old = moved(Some(positions.old), old_shift).unwrap_or(positions.old);
                    positions.new = moved(Some(positions.new), new_shift).unwrap_or(positions.new);
                }
            }
        }
        self.thread_anchor.old_line = target.old_line;
        self.thread_anchor.new_line = target.new_line;
        self.anchor = ComposerAnchor {
            old_line: target.old_line,
            new_line: target.new_line,
            range: self.thread_anchor.range.clone(),
            above: self.anchor.above,
        };
        self.key = threads.move_draft(&self.key, &self.thread_anchor);
        let after = suggestion::lines_label(&self.thread_anchor);
        self.moved_note = match (target.is_exact, before == after) {
            (true, true) => self.moved_note.take(),
            (true, false) => Some(format!("The latest push moved {before} to {after}.")),
            (false, _) => Some(format!(
                "The latest push changed {before}; this comment now goes on {after}. Check it before posting."
            )),
        };
    }
}

/// "Lines 84–87" as a pill and the file name; read out as "Comment on lines 84–87".
fn header(ui: &mut Ui, anchor: &ThreadAnchor) {
    let palette = theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = style::SPACE_S;
        let title = suggestion::lines_title(anchor);
        let font = theme::fonts::semibold_font(style::SMALL_SIZE);
        let galley = ui.painter().layout_no_wrap(title, font, palette.accent_text);
        let (rect, response) =
            ui.allocate_exact_size(galley.size() + vec2(20.0, 10.0), egui::Sense::hover());
        let label = format!("Comment on {}", suggestion::lines_label(anchor));
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &label));
        ui.painter().rect_filled(rect, rect.height() / 2.0, palette.selection);
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, palette.accent_text);
        let name = anchor.path.rsplit('/').next().unwrap_or(&anchor.path);
        ui.add(
            egui::Label::new(
                RichText::new(name).size(style::SMALL_SIZE + 1.0).color(palette.text_secondary),
            )
            .truncate(),
        )
        .on_hover_text(&anchor.path);
    });
}

/// Whether a thread's card belongs under the head pane: its lines are in the head file.
pub fn is_right(anchor: &ThreadAnchor) -> bool {
    match &anchor.range {
        Some(range) => range.side == DiffSide::New,
        None => anchor.new_line.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use review_core::LineRange;

    use super::*;

    #[test]
    fn base_lines_keep_the_card_under_the_base_pane() {
        let mut anchor = ThreadAnchor {
            path: "a.rs".into(),
            old_path: None,
            old_line: Some(7),
            new_line: Some(9),
            range: None,
        };
        assert!(is_right(&anchor));
        anchor.range = Some(LineRange {
            start_old_line: Some(3),
            start_new_line: Some(5),
            side: DiffSide::Old,
            detail: None,
        });
        assert!(!is_right(&anchor));
    }
}
