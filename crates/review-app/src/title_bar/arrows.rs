//! The back and forward arrows before the breadcrumb trail, like a browser's. Each names where it
//! leads and its shortcut; with nothing that way it rests disabled.

use egui::{Align, Layout, Rect, Ui, UiBuilder, pos2};

use cv_base::platform;
use cv_base::theme::icons;
use cv_runtime::history::{History, Place, Step};
use cv_runtime::route::Navigator;
use cv_services::Services;
use cv_shell::breadcrumbs;
use cv_widgets::buttons;

/// The room both arrows take.
fn width() -> f32 {
    buttons::icon_width() * 2.0
}

/// Both arrows from `left`, vertically centered in `bar`; returns the rect they cover.
pub(super) fn show(
    ui: &mut Ui,
    bar: Rect,
    left: f32,
    services: &Services,
    history: &History,
    navigator: &mut Navigator,
) -> Rect {
    let area = Rect::from_min_max(pos2(left, bar.top()), pos2(left + width(), bar.bottom()));
    let mut child =
        ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::left_to_right(Align::Center)));
    child.spacing_mut().item_spacing.x = 0.0;
    for step in [Step::Back, Step::Forward] {
        if arrow(&mut child, step, history.target(step), services) {
            match step {
                Step::Back => navigator.back(),
                Step::Forward => navigator.forward(),
            }
        }
    }
    area
}

/// One arrow; true when clicked.
fn arrow(ui: &mut Ui, step: Step, target: Option<&Place>, services: &Services) -> bool {
    let (icon, verb) = match step {
        Step::Back => (icons::ARROW_LEFT, "Back"),
        Step::Forward => (icons::ARROW_RIGHT, "Forward"),
    };
    let shortcut = shortcut(step);
    let tooltip = match target {
        Some(place) => format!("{verb} to {} ({shortcut})", place_name(place, services)),
        None => format!("{verb} ({shortcut})"),
    };
    ui.add_enabled_ui(target.is_some(), |ui| buttons::icon(ui, icon, &tooltip)).inner.clicked()
}

/// The shortcut the system's browsers use for `step`.
fn shortcut(step: Step) -> &'static str {
    match step {
        Step::Back => platform::BACK_SHORTCUT,
        Step::Forward => platform::FORWARD_SHORTCUT,
    }
}

/// "Repositories", or "Pull request #7, Files tab" for a pull request.
fn place_name(place: &Place, services: &Services) -> String {
    let page = breadcrumbs::crumb_text(&place.route, services);
    match place.tab {
        Some(tab) => format!("{page}, {} tab", tab.name()),
        None => page,
    }
}
