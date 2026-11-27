//! Step 5: which organizations, groups or collections to show, the connection's name, and save.

use cv_base::theme::{self, icons};
use cv_runtime::load::Load;
use cv_services::Services;
use cv_widgets::{buttons, checkbox, states};
use egui::{RichText, TextEdit, Ui};
use review_core::{ProviderKind, ScopeCandidate};

use super::{Step, VaultCheck, Wizard, footer};
use crate::onboarding::host::Hosting;
use crate::onboarding::parts::{self, Tone};

/// Returns the saved connection's name.
pub(crate) fn show(wizard: &mut Wizard, ui: &mut Ui, services: &mut Services) -> Option<String> {
    let Ok(host) = wizard.resolved() else {
        wizard.step = Step::Host;
        return None;
    };
    wizard.start_discovery(services);
    let (noun, title, detail) = words(wizard.kind, wizard.hosting);
    parts::headline(ui, title, detail);

    let can_include_everything = wizard.kind != ProviderKind::AzureDevOps;
    if can_include_everything {
        everything_choice(ui, wizard, noun);
    }
    let mut wants_retry = false;
    if !can_include_everything || !wizard.includes_everything {
        wants_retry = scope_list(ui, wizard, noun);
        typed_scopes(ui, wizard, noun);
    }
    name_field(ui, wizard);

    let organizations = wizard.organizations(&host);
    let problem = if wizard.name.trim().is_empty() {
        Some("Give the connection a name.".to_owned())
    } else if !can_include_everything && organizations.is_empty() {
        Some(format!("Choose at least one of your {noun}."))
    } else if !can_include_everything || wizard.includes_everything || !organizations.is_empty() {
        None
    } else {
        Some(format!("Choose at least one of your {noun}, or show everything."))
    };

    let mut saved = None;
    ui.add_space(theme::style::SPACE_M);
    let vault = wizard.vault_message(ui, problem.is_none());
    if vault.wants_recheck {
        wizard.vault = VaultCheck::Idle;
    }
    if vault.wants_tokenless_save {
        saved = wizard.save(services, true);
    }
    let mut wants_save_retry = false;
    if let Some(message) = &wizard.save_error {
        parts::callout(ui, Tone::Error, "The connection could not be saved", message, |ui| {
            if buttons::secondary_icon(ui, icons::ARROW_CLOCKWISE, "Try again").clicked() {
                wants_save_retry = true;
            }
        });
    }
    if wants_save_retry {
        saved = wizard.save(services, false);
    }
    let mut goes_back = false;
    footer(ui, Some(&mut goes_back), |ui| {
        let save = ui.add_enabled_ui(problem.is_none(), |ui| {
            buttons::primary_icon(ui, icons::FLOPPY_DISK, "Save connection")
        });
        if save.inner.clicked() {
            saved = wizard.save(services, false);
        }
        if let Some(problem) = &problem {
            parts::small(ui, problem);
        }
    });
    if goes_back {
        wizard.step = Step::Access;
    }
    if wants_retry {
        wizard.discovered_for = None;
        wizard.start_discovery(services);
    }
    saved
}

/// The platform's noun for a scope, the step's title and its detail line.
fn words(kind: ProviderKind, hosting: Hosting) -> (&'static str, &'static str, &'static str) {
    match (kind, hosting) {
        (ProviderKind::GitLab, _) => {
            ("groups", "Choose groups", "Merge requests from these groups and their subgroups.")
        }
        (ProviderKind::AzureDevOps, Hosting::SelfHosted) => (
            "collections",
            "Choose collections",
            "Pull requests from every project in these collections.",
        ),
        (ProviderKind::AzureDevOps, Hosting::Cloud) => (
            "organizations",
            "Choose organizations",
            "Pull requests from every project in these organizations.",
        ),
        (ProviderKind::GitHub, _) => (
            "organizations",
            "Choose organizations",
            "Pull requests from these accounts and organizations.",
        ),
    }
}

/// "Everything this token can see" or "Only selected …".
fn everything_choice(ui: &mut Ui, wizard: &mut Wizard, noun: &str) {
    let everything = "Everything this token can see";
    let everything_detail = "Repositories you are a member of or have access to";
    if parts::choice_row(
        ui,
        icons::USERS,
        everything,
        everything_detail,
        wizard.includes_everything,
        parts::Mark::Radio,
    )
    .clicked()
    {
        wizard.includes_everything = true;
    }
    ui.add_space(theme::style::SPACE_S);
    let only = format!("Only selected {noun}");
    if parts::choice_row(
        ui,
        icons::FUNNEL,
        &only,
        "Pick from the list below",
        !wizard.includes_everything,
        parts::Mark::Radio,
    )
    .clicked()
    {
        wizard.includes_everything = false;
    }
    ui.add_space(theme::style::SPACE_M);
}

/// The scopes the token sees, each with a checkbox; true when the user asks to list them again.
fn scope_list(ui: &mut Ui, wizard: &mut Wizard, noun: &str) -> bool {
    let mut wants_retry = false;
    match &wizard.discovery {
        Load::Loading(_) | Load::Idle => {
            states::loading(ui, &format!("Looking up your {noun}…"));
        }
        Load::Failed(message) => {
            parts::callout(
                ui,
                Tone::Warning,
                &format!("Your {noun} could not be listed"),
                &format!("{message}\nType the names below instead."),
                |ui| {
                    wants_retry =
                        buttons::secondary_icon(ui, icons::ARROW_CLOCKWISE, "Try again").clicked();
                },
            );
        }
        Load::Ready(candidates) if candidates.is_empty() => {
            parts::small(ui, &format!("The token sees no {noun}. Type the names below."));
        }
        Load::Ready(candidates) => {
            for candidate in candidates {
                candidate_row(ui, &mut wizard.selected, candidate);
            }
        }
    }
    wants_retry
}

/// One scope's checkbox with its kind (and key, when it differs from the name).
fn candidate_row(ui: &mut Ui, selected: &mut Vec<String>, candidate: &ScopeCandidate) {
    let palette = theme::palette(ui.ctx());
    let mut is_selected = selected.iter().any(|key| key.eq_ignore_ascii_case(&candidate.key));
    ui.horizontal(|ui| {
        let name = RichText::new(&candidate.display_name)
            .font(theme::fonts::semibold_font(theme::style::BODY_SIZE));
        if checkbox::checkbox(ui, &mut is_selected, name).changed() {
            if is_selected {
                selected.push(candidate.key.clone());
            } else {
                selected.retain(|key| !key.eq_ignore_ascii_case(&candidate.key));
            }
        }
        let kind = if candidate.display_name == candidate.key {
            candidate.kind.label().to_owned()
        } else {
            format!("{}, {}", candidate.kind.label(), candidate.key)
        };
        ui.label(RichText::new(kind).size(theme::style::SMALL_SIZE).color(palette.text_secondary));
    });
}

/// Scopes typed by name, for ones the list does not show.
fn typed_scopes(ui: &mut Ui, wizard: &mut Wizard, noun: &str) {
    ui.add_space(theme::style::SPACE_S);
    let label = ui.label(
        RichText::new(format!("Add {noun} by name"))
            .font(theme::fonts::semibold_font(theme::style::BODY_SIZE)),
    );
    ui.add(
        TextEdit::singleline(&mut wizard.extra_scopes)
            .id_salt("onboarding_scopes")
            .hint_text("Comma-separated, for ones not listed above")
            .desired_width(f32::INFINITY),
    )
    .labelled_by(label.id);
    ui.add_space(theme::style::SPACE_M);
}

/// The connection's name; typing in it keeps it from following the host.
fn name_field(ui: &mut Ui, wizard: &mut Wizard) {
    let label = ui.label(
        RichText::new("Connection name").font(theme::fonts::semibold_font(theme::style::BODY_SIZE)),
    );
    let name = ui
        .add(
            TextEdit::singleline(&mut wizard.name)
                .id_salt("onboarding_name")
                .hint_text("Work, Open source, …")
                .desired_width(f32::INFINITY),
        )
        .labelled_by(label.id);
    if name.changed() {
        wizard.is_name_custom = true;
    }
}
