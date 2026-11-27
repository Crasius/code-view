//! The dropdown's lines about the choice: why a strategy is preselected, "Always use for
//! `<repo>`", and the way to the branch's policies.

use egui::{RichText, Ui};
use review_core::{MergePolicy, MergeStrategy, ProviderKind};

use super::{Page, scope};
use cv_base::theme::{self, style};
use cv_pr_session::merge_choice::{self, Reason};
use cv_services::Services;
use cv_widgets::{buttons, checkbox};

/// Why `current` is preselected, and (where there is a choice) "Always use for `<repo>`".
pub(crate) fn favourite(
    ui: &mut Ui,
    page: Page<'_>,
    services: &mut Services,
    policy: &MergePolicy,
    (current, reason): (MergeStrategy, Option<Reason>),
) {
    let palette = theme::palette(ui.ctx());
    let Page { session, detail, .. } = page;
    let kind = session.kind;
    let target = &detail.summary.target_branch;
    let scope = scope(session);
    let favourite =
        services.settings.merge.repository_favourite(scope.connection_id, scope.repository);
    ui.label(
        RichText::new(reason_line(current, reason, favourite == Some(current), kind, target))
            .size(style::SMALL_SIZE)
            .color(palette.text_secondary),
    );
    if !merge_choice::is_choice(policy) {
        return;
    }
    let repository = merge_choice::repository_name(&session.key.repository_key);
    let mut always = favourite == Some(current);
    let response = checkbox::checkbox(ui, &mut always, format!("Always use for {repository}"));
    if response
        .on_hover_text(format!("A favourite for all of {} is in Settings, Merging", kind.label()))
        .changed()
    {
        let preferences = &mut services.settings.merge;
        preferences.set_repository_favourite(
            scope.connection_id,
            scope.repository,
            always.then_some(current),
        );
        if always {
            // Last used comes first in the preselection; the favourite takes effect now.
            preferences.remember_last_used(scope.connection_id, scope.repository, current);
        }
        services.save_settings();
    }
}

/// "Squash merge: last used in this repository".
fn reason_line(
    strategy: MergeStrategy,
    reason: Option<Reason>,
    is_favourite: bool,
    kind: ProviderKind,
    target: &str,
) -> String {
    let name = merge_choice::button_label(strategy, kind);
    let why = match reason {
        _ if is_favourite => "always used for this repository".to_owned(),
        None => "picked for this pull request".to_owned(),
        Some(Reason::PlatformDefault) => format!("the default on {target}"),
        Some(Reason::FirstAllowed) => format!("the first type {target} allows"),
        Some(reason) => reason.explain(kind).map(|text| lower_first(&text)).unwrap_or_default(),
    };
    format!("{name}: {why}")
}

fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_lowercase().chain(chars).collect()).unwrap_or_default()
}

pub(crate) fn policies_link(ui: &mut Ui, policy: &MergePolicy) {
    if let Some(url) = &policy.settings_url
        && buttons::text_link(
            ui,
            "Branch policies",
            "Open the branch's merge settings on the platform",
        )
        .clicked()
    {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}
