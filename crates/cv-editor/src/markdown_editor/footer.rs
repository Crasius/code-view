//! The composer's footer (shortcut hint, submit and cancel buttons, the discard question) and how
//! its buttons and the keys end a frame.

use cv_base::theme::{self, icons, style};
use cv_widgets::buttons;
use egui::{RichText, Ui};

use super::{Options, SUBMIT_SHORTCUT};

/// A footer button clicked this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FooterAction {
    Submit,
    /// The outlined second submit button ([`Options::secondary_label`]).
    Secondary,
    Cancel,
    /// Confirms discarding the draft.
    Discard,
    /// Answers the discard question with "no".
    KeepEditing,
}

/// The footer: the discard question while it shows, else the shortcut hint and the submit and
/// cancel buttons. Returns the button clicked.
pub(super) fn show(
    ui: &mut Ui,
    options: &Options<'_>,
    is_confirming_discard: bool,
    has_text: bool,
) -> Option<FooterAction> {
    let palette = theme::palette(ui.ctx());
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = style::SPACE_S;
        if is_confirming_discard {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if buttons::danger(ui, icons::TRASH, "Discard").clicked() {
                    clicked = Some(FooterAction::Discard);
                }
                if buttons::secondary_icon(ui, icons::PENCIL_SIMPLE, "Keep editing")
                    .on_hover_text("Esc")
                    .clicked()
                {
                    clicked = Some(FooterAction::KeepEditing);
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(RichText::new("Discard draft?").color(palette.text))
                            .wrap(),
                    );
                });
            });
            return;
        }
        let verb = options.submit_label.to_lowercase();
        let hint = match options.footer_hint {
            Some(hint) => hint.to_owned(),
            None if options.can_cancel => format!("{SUBMIT_SHORTCUT} to {verb}, Esc to cancel"),
            None => format!("{SUBMIT_SHORTCUT} to {verb}"),
        };
        if options.leading_buttons {
            clicked = leading(ui, options, has_text, &hint);
            return;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if submit_button(ui, options, has_text) {
                clicked = Some(FooterAction::Submit);
            }
            if options.can_cancel
                && buttons::secondary_icon(ui, icons::X, "Cancel").on_hover_text("Esc").clicked()
            {
                clicked = Some(FooterAction::Cancel);
            }
            // The hint takes what the buttons leave and shortens itself in narrow composers.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                hint_label(ui, &hint);
            });
        });
    });
    clicked
}

/// The primary button; while posting it shows a spinner in its icon slot, so nothing moves.
/// Returns whether it was clicked.
fn submit_button(ui: &mut Ui, options: &Options<'_>, has_text: bool) -> bool {
    let submit = ui.add_enabled_ui(has_text || options.is_busy, |ui| {
        buttons::primary_busy(ui, icons::PAPER_PLANE_RIGHT, options.submit_label, options.is_busy)
    });
    let hover = if options.is_busy { "Posting…" } else { SUBMIT_SHORTCUT };
    submit.inner.on_hover_text(hover).clicked()
}

fn hint_label(ui: &mut Ui, hint: &str) {
    let palette = theme::palette(ui.ctx());
    let text = RichText::new(hint).size(style::SMALL_SIZE).color(palette.text_secondary);
    ui.add(egui::Label::new(text).truncate());
}

/// Thread composers: primary, secondary and Cancel buttons from the left, the hint at the right end.
fn leading(ui: &mut Ui, options: &Options<'_>, has_text: bool, hint: &str) -> Option<FooterAction> {
    let mut clicked = None;
    if submit_button(ui, options, has_text) {
        clicked = Some(FooterAction::Submit);
    }
    if let Some(label) = options.secondary_label {
        let secondary =
            ui.add_enabled_ui(has_text && !options.is_busy, |ui| buttons::secondary(ui, label));
        if secondary.inner.clicked() {
            clicked = Some(FooterAction::Secondary);
        }
    }
    if options.can_cancel && buttons::secondary(ui, "Cancel").on_hover_text("Esc").clicked() {
        clicked = Some(FooterAction::Cancel);
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| hint_label(ui, hint));
    clicked
}

/// The keys of this frame that act like footer buttons.
#[derive(Clone, Copy, Default)]
pub(super) struct Keys {
    /// Ctrl+Enter.
    pub submit: bool,
    /// Esc.
    pub cancel: bool,
}

/// What the text holds against where the composer started ([`Options::initial`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Content {
    /// Blank: nothing to post or to lose.
    #[default]
    Empty,
    /// The text the composer started with: something to post, nothing to lose.
    AsStarted,
    /// Text the user wrote or changed.
    Edited,
}

impl Content {
    pub fn of(text: &str, initial: Option<&str>) -> Self {
        if text.trim().is_empty() {
            Self::Empty
        } else if initial == Some(text) {
            Self::AsStarted
        } else {
            Self::Edited
        }
    }

    pub fn has_text(self) -> bool {
        self != Self::Empty
    }
}

/// The draft as the frame found and left it.
#[derive(Clone, Copy, Default)]
pub(super) struct Draft {
    pub content: Content,
    /// The discard question showed when the frame began.
    pub was_confirming: bool,
    /// The text changed this frame (typing, an edit, an accepted mention).
    pub is_changed: bool,
}

/// How a frame of the composer ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A non-empty comment is posted, from the primary button or Ctrl+Enter, or from the
    /// secondary button.
    Submitted { with_secondary: bool },
    /// The draft is given up: it was empty, or its discarding was confirmed.
    Cancelled,
    /// Esc in an editor that cannot cancel: the caller closes it and keeps the text.
    Dismissed,
}

/// What the footer and the keys decided this frame.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Resolution {
    pub outcome: Option<Outcome>,
    /// Whether the discard question shows next frame.
    pub is_confirming_discard: bool,
    /// The user chose to keep editing: the text edit takes the focus back.
    pub keeps_editing: bool,
}

/// Resolves the footer's button and the keys into the frame's outcome, with the discard guard: a
/// cancel of edited text asks first (text as it started closes at once); Esc while it asks means "keep editing", never a second cancel;
/// typing answers it with "keep editing" too.
pub(super) fn resolve(
    action: Option<FooterAction>,
    keys: Keys,
    draft: Draft,
    options: &Options<'_>,
) -> Resolution {
    let wants_submit = (matches!(action, Some(FooterAction::Submit | FooterAction::Secondary))
        || keys.submit)
        && draft.content.has_text()
        && !options.is_busy;
    let keeps_editing =
        action == Some(FooterAction::KeepEditing) || (draft.was_confirming && keys.cancel);
    let wants_cancel =
        action == Some(FooterAction::Cancel) || (keys.cancel && !draft.was_confirming);
    let mut is_confirming = draft.was_confirming;
    let outcome = if wants_submit {
        is_confirming = false;
        Some(Outcome::Submitted { with_secondary: action == Some(FooterAction::Secondary) })
    } else if action == Some(FooterAction::Discard) {
        is_confirming = false;
        Some(Outcome::Cancelled)
    } else if wants_cancel && options.can_cancel {
        if draft.content == Content::Edited {
            is_confirming = true;
            None
        } else {
            Some(Outcome::Cancelled)
        }
    } else if wants_cancel {
        Some(Outcome::Dismissed)
    } else {
        None
    };
    let is_finished = matches!(outcome, Some(Outcome::Submitted { .. } | Outcome::Cancelled));
    if keeps_editing || (draft.is_changed && !is_finished) {
        is_confirming = false;
    }
    Resolution { outcome, is_confirming_discard: is_confirming, keeps_editing }
}

#[cfg(test)]
mod tests {
    use super::{Content, Draft, FooterAction, Keys, Outcome, Resolution, resolve};
    use crate::markdown_editor::Options;

    const ESC: Keys = Keys { submit: false, cancel: true };
    const CTRL_ENTER: Keys = Keys { submit: true, cancel: false };
    const WRITTEN: Draft =
        Draft { content: Content::Edited, was_confirming: false, is_changed: false };
    const ASKING: Draft =
        Draft { content: Content::Edited, was_confirming: true, is_changed: false };

    fn outcome(action: Option<FooterAction>, keys: Keys, draft: Draft) -> Option<Outcome> {
        resolve(action, keys, draft, &Options::default()).outcome
    }

    #[test]
    fn cancelling_a_written_draft_asks_first() {
        for (action, keys) in [(Some(FooterAction::Cancel), Keys::default()), (None, ESC)] {
            assert_eq!(
                resolve(action, keys, WRITTEN, &Options::default()),
                Resolution { outcome: None, is_confirming_discard: true, keeps_editing: false },
            );
        }
        assert_eq!(
            outcome(Some(FooterAction::Discard), Keys::default(), ASKING),
            Some(Outcome::Cancelled)
        );
        let empty = Draft::default();
        assert_eq!(outcome(None, ESC, empty), Some(Outcome::Cancelled), "nothing to lose");
        let unchanged = Draft { content: Content::AsStarted, ..WRITTEN };
        assert_eq!(
            outcome(Some(FooterAction::Cancel), Keys::default(), unchanged),
            Some(Outcome::Cancelled),
            "an edit that changed nothing closes at once"
        );
    }

    #[test]
    fn content_tells_an_unchanged_edit_from_new_text() {
        assert_eq!(Content::of("  \n", None), Content::Empty);
        assert_eq!(Content::of("LGTM", Some("LGTM")), Content::AsStarted);
        assert_eq!(Content::of("LGTM!", Some("LGTM")), Content::Edited);
        assert_eq!(Content::of("draft", None), Content::Edited, "a new comment starts empty");
    }

    #[test]
    fn esc_or_typing_while_asking_keeps_editing() {
        let esc = resolve(None, ESC, ASKING, &Options::default());
        assert_eq!(
            esc,
            Resolution { outcome: None, is_confirming_discard: false, keeps_editing: true },
            "never a second cancel"
        );
        let typed = Draft { is_changed: true, ..ASKING };
        assert!(!resolve(None, Keys::default(), typed, &Options::default()).is_confirming_discard);
        assert!(resolve(None, Keys::default(), ASKING, &Options::default()).is_confirming_discard);
    }

    #[test]
    fn esc_dismisses_an_editor_that_cannot_cancel() {
        let options = Options { can_cancel: false, ..Options::default() };
        let resolution = resolve(None, ESC, WRITTEN, &options);
        assert_eq!(resolution.outcome, Some(Outcome::Dismissed));
        assert!(!resolution.is_confirming_discard, "nothing to discard: the text stays");
    }

    #[test]
    fn submits_only_written_drafts_and_never_while_posting() {
        let submitted = Some(Outcome::Submitted { with_secondary: false });
        assert_eq!(outcome(Some(FooterAction::Submit), Keys::default(), WRITTEN), submitted);
        assert_eq!(outcome(None, CTRL_ENTER, WRITTEN), submitted);
        assert_eq!(
            outcome(Some(FooterAction::Secondary), Keys::default(), WRITTEN),
            Some(Outcome::Submitted { with_secondary: true })
        );
        let empty = Draft::default();
        assert_eq!(outcome(None, CTRL_ENTER, empty), None);
        let busy = Options { is_busy: true, ..Options::default() };
        assert_eq!(resolve(None, CTRL_ENTER, WRITTEN, &busy).outcome, None);
        let typed_then_sent = Draft { is_changed: true, ..ASKING };
        let resolution = resolve(None, CTRL_ENTER, typed_then_sent, &Options::default());
        assert_eq!(resolution.outcome, submitted);
        assert!(!resolution.is_confirming_discard, "a submit answers the question");
    }
}
