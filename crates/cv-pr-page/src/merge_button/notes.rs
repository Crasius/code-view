//! What the merge button says beyond its label: its tone, the main part's hover (a refused merge,
//! the auto-complete, conflicts, a policy still loading) and the warning while a merge is forced
//! past blocking policies.

use egui::{Rect, RectAlign, Response, Ui};
use review_core::{MergeStatus, MergeStrategy, ProviderKind};

use super::{MENU_WIDTH, Page};
use cv_base::theme::style;
use cv_pr_session::auto_complete::{self, Mode};
use cv_pr_session::merge_choice::Reason;
use cv_pr_session::merge_paint::Tone;
use cv_widgets::states::{self, Action};
use cv_widgets::{buttons, provider_icon};

/// The button's tone in `mode`, or after a refused merge.
pub(super) fn tone(mode: Mode, is_failed: bool) -> Tone {
    match mode {
        _ if is_failed => Tone::Failed,
        Mode::Now => Tone::Normal,
        Mode::SetAutoComplete | Mode::CancelAutoComplete => Tone::Auto,
    }
}

/// The main part's hover: a refused merge (`failure`), the auto-complete, the conflicts or the
/// policy that still loads.
pub(super) fn main_hover(
    main: Response,
    page: Page<'_>,
    failure: Option<&str>,
    choice: Option<(MergeStrategy, Option<Reason>)>,
    can_write: bool,
) -> Response {
    let Page { session, detail, mode } = page;
    let kind = session.kind;
    let strategy = choice.map(|(strategy, _)| strategy);
    if let Some(name) = failure {
        main.on_hover_text(format!("{name}. Click to try again."))
    } else if let Some(text) = auto_complete::hover(detail, mode, strategy, kind) {
        main.on_hover_text(text)
    } else if detail.merge_status == MergeStatus::Conflicts {
        main.on_hover_ui(|ui| conflict_tooltip(ui, kind, &detail.summary.web_url))
    } else if choice.is_none() && can_write {
        main.on_hover_text("Loading the merge types this branch allows…")
    } else {
        main
    }
}

fn conflict_tooltip(ui: &mut Ui, kind: ProviderKind, url: &str) {
    ui.label("This pull request has merge conflicts. Resolve them first.");
    let label = states::open_on_platform(kind);
    if buttons::secondary_icon(ui, provider_icon::glyph(kind), &label).clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

/// While armed on a blocked pull request: the platform may refuse, and where to look.
pub(super) fn blocked_warning(ui: &Ui, id: egui::Id, anchor: Rect, kind: ProviderKind, url: &str) {
    egui::Popup::new(id.with("blocked"), ui.ctx().clone(), anchor, ui.layer_id())
        .align(RectAlign::BOTTOM_END)
        .gap(style::SPACE_XS)
        .width(MENU_WIDTH)
        .show(|ui| {
            ui.set_width(MENU_WIDTH - 2.0 * style::SPACE_S);
            let label = states::open_on_platform(kind);
            let open = [Action::secondary(provider_icon::glyph(kind), &label)];
            if states::warning_with_actions(
                ui,
                "Policies or required reviews still block this merge.",
                &open,
            )
            .is_some()
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
        });
}
