//! Steps 1 and 2: which platform, and which host of it.

use cv_base::theme::{self, icons};
use cv_widgets::buttons;
use cv_widgets::provider_icon;
use egui::{RichText, TextEdit, Ui};
use review_core::ProviderKind;

use super::{Step, Wizard, footer};
use crate::onboarding::host::{self, Hosting};
use crate::onboarding::parts;

const PLATFORMS: [(ProviderKind, &str); 3] = [
    (ProviderKind::AzureDevOps, "Azure DevOps Services or Azure DevOps Server"),
    (ProviderKind::GitHub, "GitHub.com or GitHub Enterprise Server"),
    (ProviderKind::GitLab, "GitLab.com or self-managed GitLab"),
];

pub fn platform(wizard: &mut Wizard, ui: &mut Ui) {
    parts::headline(
        ui,
        "Where do your pull requests live?",
        "You can add more connections later, one per account and host.",
    );
    for (kind, detail) in PLATFORMS {
        let is_selected = wizard.existing.is_some() && wizard.kind == kind;
        if parts::choice_row(
            ui,
            provider_icon::glyph(kind),
            kind.label(),
            detail,
            is_selected,
            true,
        )
        .clicked()
        {
            wizard.select_kind(kind);
        }
        ui.add_space(8.0);
    }
}

pub fn host(wizard: &mut Wizard, ui: &mut Ui) {
    let kind = wizard.kind;
    parts::headline(ui, &format!("Which {}?", kind.label()), "");
    for (hosting, title, detail) in [
        (Hosting::Cloud, host::cloud_label(kind), host::cloud_detail(kind)),
        (Hosting::SelfHosted, host::self_hosted_label(kind), host::self_hosted_detail(kind)),
    ] {
        let icon = if hosting == Hosting::Cloud {
            cv_base::theme::icons::LINK
        } else {
            cv_base::theme::icons::FOLDER
        };
        if parts::choice_row(ui, icon, title, detail, wizard.hosting == hosting, false).clicked()
            && wizard.hosting != hosting
        {
            wizard.hosting = hosting;
            wizard.host_input.clear();
            wizard.reset_token();
        }
        ui.add_space(8.0);
    }

    let resolved = wizard.resolved();
    if host::needs_input(kind, wizard.hosting) {
        ui.add_space(8.0);
        let (label, example) = host::field(kind, wizard.hosting);
        let label = ui
            .label(RichText::new(label).font(theme::fonts::semibold_font(theme::style::BODY_SIZE)));
        let response = ui
            .add(
                TextEdit::singleline(&mut wizard.host_input)
                    .id_salt("onboarding_host")
                    .hint_text(example)
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(label.id);
        if response.lost_focus()
            && ui.input(|input| input.key_pressed(egui::Key::Enter))
            && resolved.is_ok()
        {
            wizard.go(Step::Token);
        }
        if response.changed() {
            wizard.reset_token();
        }
        let palette = theme::palette(ui.ctx());
        match &resolved {
            Ok(host) => {
                ui.label(
                    RichText::new(format!("Connects to {}", host.display()))
                        .size(theme::style::SMALL_SIZE)
                        .color(palette.text_secondary),
                );
            }
            Err(problem) if !wizard.host_input.trim().is_empty() => {
                ui.label(
                    RichText::new(problem).size(theme::style::SMALL_SIZE).color(palette.error),
                );
            }
            Err(_) => {
                parts::small(ui, "You can paste any address from your browser's address bar.");
            }
        }
    }

    let mut goes_back = false;
    footer(ui, Some(&mut goes_back), |ui| {
        let can_continue = resolved.is_ok();
        let next = ui.add_enabled_ui(can_continue, |ui| {
            buttons::primary_icon(ui, icons::ARROW_RIGHT, "Continue")
        });
        if next.inner.clicked() {
            wizard.go(Step::Token);
        }
    });
    if goes_back {
        wizard.go(Step::Platform);
    }
}
