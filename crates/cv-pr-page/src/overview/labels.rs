use std::collections::HashSet;

use egui::{RichText, TextEdit, Ui};
use review_core::Label;

use cv_base::theme::{self, icons, style};
use cv_pr_session::actions::{ActionRunner, Refresh, Ticket};
use cv_pr_session::session::PullRequestSession;
use cv_runtime::load::Load;
use cv_services::Services;
use cv_services::writes::Effect;
use cv_widgets::{badges, buttons, skeleton};

/// Label chips plus an editor: a catalog picker (GitHub, GitLab) or free-text tags (Azure DevOps).
/// Editing works on a draft: Apply sends it once (only when it differs), Cancel or Esc drops it.
#[derive(Default)]
pub struct LabelEditor {
    /// The labels being edited; `None` while not editing.
    draft: Option<Vec<Label>>,
    catalog: Load<Vec<Label>>,
    search: String,
}

/// What the card's title-line buttons asked for this frame.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CardAction {
    None,
    Edit,
    Apply,
    Cancel,
}

/// The card's title-line buttons: Edit, or Apply and Cancel while editing. Apply waits while
/// another write runs (`is_busy`).
pub fn card_actions(ui: &mut Ui, is_editing: bool, is_busy: bool) -> CardAction {
    if !is_editing {
        return if buttons::icon(ui, icons::PENCIL_SIMPLE, "Edit labels").clicked() {
            CardAction::Edit
        } else {
            CardAction::None
        };
    }
    // Right to left: Apply ends the line, Cancel sits before it.
    let mut action = CardAction::None;
    if ui.add_enabled_ui(!is_busy, |ui| buttons::icon(ui, icons::CHECK, "Apply")).inner.clicked() {
        action = CardAction::Apply;
    }
    if buttons::icon(ui, icons::X, "Cancel (Esc)").clicked() {
        action = CardAction::Cancel;
    }
    action
}

impl LabelEditor {
    /// The editor is open: the user is changing the labels.
    pub fn is_editing(&self) -> bool {
        self.draft.is_some()
    }

    /// `action`: what the card's title-line buttons asked for this frame.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        session: &PullRequestSession,
        services: &Services,
        actions: &mut ActionRunner,
        action: CardAction,
    ) {
        let palette = theme::palette(ui.ctx());
        self.catalog.poll();
        let Some(detail) = session.detail.ready() else {
            ui.horizontal(|ui| {
                skeleton::bar(ui, 60.0, 18.0);
                skeleton::bar(ui, 80.0, 18.0);
            });
            return;
        };
        let wants_cancel = self.draft.is_some()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        match action {
            CardAction::Edit => {
                self.draft = Some(detail.summary.labels.clone());
                self.search.clear();
                if session.capabilities.has_label_catalog && wants_catalog(&self.catalog) {
                    self.start_catalog(session, services);
                }
            }
            CardAction::Apply => {
                if let Some(draft) = self.draft.take() {
                    let provider = session.provider.clone();
                    let repository = session.key.repository_key.clone();
                    let number = session.key.number;
                    self.draft = apply(&detail.summary.labels, draft, |names, labels| {
                        let effect = (&session.key, Effect::Labels(labels.to_vec()));
                        let text = "Labels updated.";
                        actions.run(services, text, Refresh::PullRequest, effect, async move {
                            provider.set_labels(&repository, number, &names).await
                        })
                    });
                }
            }
            CardAction::Cancel => self.draft = None,
            CardAction::None if wants_cancel => self.draft = None,
            CardAction::None => {}
        }
        let Some(mut draft) = self.draft.take() else {
            chip_rows(ui, |ui| {
                if detail.summary.labels.is_empty() {
                    ui.label(RichText::new("No labels.").color(palette.text_secondary));
                }
                for label in &detail.summary.labels {
                    badges::label_chip(ui, label);
                }
            });
            return;
        };
        let mut removed = None;
        chip_rows(ui, |ui| {
            if draft.is_empty() {
                ui.label(RichText::new("No labels.").color(palette.text_secondary));
            }
            for (index, label) in draft.iter().enumerate() {
                if badges::label_chip(ui, label)
                    .interact(egui::Sense::click())
                    .on_hover_text("Click to remove")
                    .clicked()
                {
                    removed = Some(index);
                }
            }
        });
        if let Some(index) = removed {
            draft.remove(index);
        }
        ui.add_space(style::SPACE_S);
        if let Some(added) = self.picker(ui, session, &draft) {
            draft.push(added);
        }
        self.draft = Some(draft);
    }

    fn start_catalog(&mut self, session: &PullRequestSession, services: &Services) {
        let provider = session.provider.clone();
        let repository = session.key.repository_key.clone();
        self.catalog.start(&services.background, async move {
            provider.available_labels(&repository).await
        });
    }

    /// Returns a label to add to the draft.
    fn picker(
        &mut self,
        ui: &mut Ui,
        session: &PullRequestSession,
        draft: &[Label],
    ) -> Option<Label> {
        let palette = theme::palette(ui.ctx());
        let field = ui.add(
            TextEdit::singleline(&mut self.search)
                .hint_text(if session.capabilities.has_label_catalog {
                    "Search labels"
                } else {
                    "New tag, Enter to add"
                })
                .desired_width(f32::INFINITY),
        );
        let needle = self.search.trim().to_lowercase();
        let taken: HashSet<String> = draft.iter().map(|label| label.name.to_lowercase()).collect();
        if !session.capabilities.has_label_catalog {
            let wants_add =
                field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if wants_add && !needle.is_empty() && !taken.contains(&needle) {
                let name = std::mem::take(&mut self.search).trim().to_owned();
                field.request_focus();
                return Some(Label { name, color: None, description: None });
            }
            return None;
        }
        let catalog = match &self.catalog {
            Load::Ready(catalog) => catalog,
            Load::Failed(message) => {
                ui.label(RichText::new(message).color(palette.error));
                return None;
            }
            _ => {
                for _ in 0..4 {
                    skeleton::bar(ui, 160.0, 14.0);
                    ui.add_space(style::SPACE_XS);
                }
                return None;
            }
        };
        let mut added = None;
        ui.add_space(style::SPACE_S);
        egui::ScrollArea::vertical().id_salt("label_catalog").max_height(220.0).show(ui, |ui| {
            chip_rows(ui, |ui| {
                let candidates = catalog.iter().filter(|label| {
                    let name = label.name.to_lowercase();
                    !taken.contains(&name) && (needle.is_empty() || name.contains(&needle))
                });
                for label in candidates {
                    let chip = badges::label_chip(ui, label)
                        .interact(egui::Sense::click())
                        .on_hover_text("Click to add");
                    if chip.clicked() {
                        added = Some(label.clone());
                    }
                }
            });
        });
        added
    }
}

/// Between chips, across and between rows.
const CHIP_GAP: f32 = 6.0;

/// Chips left to right, wrapping, each row only as tall as a chip: a wrapping layout in a taller
/// space would centre the rows in it and leave gaps above and below.
fn chip_rows(ui: &mut Ui, add_chips: impl FnOnce(&mut Ui)) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(CHIP_GAP, CHIP_GAP);
        ui.spacing_mut().interact_size.y = chip_height(ui);
        ui.horizontal_wrapped(add_chips);
    });
}

/// Height of a label chip: one line of small text plus its vertical padding.
fn chip_height(ui: &Ui) -> f32 {
    let font = egui::FontId::proportional(style::SMALL_SIZE);
    ui.fonts_mut(|fonts| fonts.row_height(&font)) + 4.0
}

/// The catalog loads when the editor opens: the first time, and again after it failed.
fn wants_catalog(catalog: &Load<Vec<Label>>) -> bool {
    catalog.is_idle() || catalog.error().is_some()
}

/// Sends the draft through `send` (its names, and the labels themselves) when it names other
/// labels than the pull request has. Gives the draft back when `send` did not start (another write
/// still runs), so the edit is not lost.
fn apply(
    current: &[Label],
    draft: Vec<Label>,
    send: impl FnOnce(Vec<String>, &[Label]) -> Option<Ticket>,
) -> Option<Vec<Label>> {
    let names = |labels: &[Label]| {
        let mut names: Vec<String> = labels.iter().map(|label| label.name.clone()).collect();
        names.sort();
        names
    };
    if names(current) == names(&draft) {
        return None;
    }
    let labels: Vec<String> = draft.iter().map(|label| label.name.clone()).collect();
    match send(labels, &draft) {
        Some(_) => None,
        None => Some(draft),
    }
}

#[cfg(test)]
mod tests {
    use review_core::Label;

    use super::{apply, wants_catalog};
    use cv_runtime::load::Load;

    fn label(name: &str) -> Label {
        Label { name: name.to_owned(), color: None, description: None }
    }

    #[test]
    fn applying_labels_while_another_write_runs_keeps_the_edit() {
        let current = [label("bug")];
        let kept = apply(&current, vec![label("bug"), label("ui")], |_, _| None);
        let names: Option<Vec<&str>> =
            kept.as_ref().map(|draft| draft.iter().map(|label| label.name.as_str()).collect());
        assert_eq!(names, Some(vec!["bug", "ui"]), "the editor stays open with the edit");
        let mut sent = Vec::new();
        let unchanged = apply(&current, vec![label("bug")], |labels, _| {
            sent = labels;
            None
        });
        assert!(unchanged.is_none() && sent.is_empty(), "nothing to send, nothing kept");
    }

    #[test]
    fn a_label_catalog_that_failed_loads_again() {
        assert!(wants_catalog(&Load::Idle));
        assert!(wants_catalog(&Load::Failed("timed out".into())));
        assert!(!wants_catalog(&Load::Ready(Vec::new())));
    }
}
