//! Settings → Merging: a favourite merge type per platform, and the repositories' remembered ones.
//! The merge button preselects a favourite only where the target branch allows it; the preview
//! ([`super::preview_merge`]) shows the button for the platform last chosen here.

use cv_base::theme::{self, icons, style};
use cv_pr_session::merge_choice;
use cv_services::Services;
use cv_widgets::{buttons, provider_icon, tabs};
use egui::{RichText, Ui};
use review_core::{MergeStrategy, ProviderKind};

use super::rows::MergingRow;

/// Platforms of the saved connections, each once, in the app's platform order.
pub fn connected_kinds(services: &Services) -> Vec<ProviderKind> {
    ProviderKind::ALL
        .into_iter()
        .filter(|kind| services.connections.all().iter().any(|connection| connection.kind == *kind))
        .collect()
}

/// The strategies a favourite can name on `kind`: everything its API can merge with.
pub fn choices(kind: ProviderKind) -> &'static [MergeStrategy] {
    match kind {
        ProviderKind::GitHub => {
            &[MergeStrategy::Merge, MergeStrategy::Squash, MergeStrategy::Rebase]
        }
        ProviderKind::AzureDevOps | ProviderKind::GitLab => &MergeStrategy::ALL,
    }
}

/// Short segment labels; the tooltip carries the platform's full name.
fn short(strategy: MergeStrategy, kind: ProviderKind) -> &'static str {
    match (strategy, kind) {
        (MergeStrategy::Merge, _) => "Merge commit",
        (MergeStrategy::Squash, _) => "Squash",
        (MergeStrategy::Rebase, ProviderKind::GitLab) => "Fast-forward",
        (MergeStrategy::Rebase, _) => "Rebase",
        (MergeStrategy::SemiLinear, _) => "Semi-linear",
    }
}

/// The control of Merging row `id`; `previewed` follows the platform the user works on.
pub fn control(
    ui: &mut Ui,
    id: MergingRow,
    services: &mut Services,
    previewed: &mut Option<ProviderKind>,
) {
    let changed = match id {
        MergingRow::Favourites => favourites(ui, services, previewed),
        MergingRow::Remembered => remembered(ui, services),
    };
    if changed {
        services.save_settings();
    }
}

fn favourites(ui: &mut Ui, services: &mut Services, previewed: &mut Option<ProviderKind>) -> bool {
    let palette = theme::palette(ui.ctx());
    let mut changed = false;
    for (index, kind) in connected_kinds(services).into_iter().enumerate() {
        if index > 0 {
            ui.add_space(style::SPACE_M);
        }
        // The platform above its choices: Azure DevOps' five choices need the row's whole width.
        let row = ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = style::SPACE_S;
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(icons::SIZE, icons::SIZE),
                    egui::Sense::hover(),
                );
                provider_icon::paint(ui.painter(), &palette, rect.center(), icons::SIZE, kind);
                ui.label(kind.label());
            });
            let mut favourite = services.settings.merge.platform_favourite(kind);
            let mut options: Vec<(Option<MergeStrategy>, &str, &str)> =
                vec![(None, "No favourite", "The platform's default, else the first allowed type")];
            options.extend(choices(kind).iter().map(|strategy| {
                (Some(*strategy), short(*strategy, kind), merge_choice::name(*strategy, kind))
            }));
            if tabs::segmented_with_hints(ui, &mut favourite, &options) {
                services.settings.merge.set_platform_favourite(kind, favourite);
                changed = true;
                *previewed = Some(kind);
            }
        });
        if row.response.contains_pointer() {
            *previewed = Some(kind);
        }
    }
    changed
}

fn remembered(ui: &mut Ui, services: &mut Services) -> bool {
    let palette = theme::palette(ui.ctx());
    let remembered = services.settings.merge.remembered_repositories();
    let forget = ui.add_enabled_ui(remembered > 0, |ui| {
        buttons::secondary_icon(ui, icons::ARROW_COUNTER_CLOCKWISE, "Forget")
    });
    let text = match remembered {
        0 => "None remembered yet".to_owned(),
        1 => "1 repository".to_owned(),
        count => format!("{count} repositories"),
    };
    ui.label(RichText::new(text).color(palette.text_secondary));
    if forget
        .inner
        .on_hover_text(
            "Forget every repository's favourite, last used merge type and source branch choice",
        )
        .clicked()
    {
        services.settings.merge.forget_repositories();
        return true;
    }
    false
}
