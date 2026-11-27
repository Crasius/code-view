use egui::{Align2, Rect, Sense, Ui, pos2, vec2};

use cv_base::theme::icons;
use cv_base::theme::{self, motion, style};
use cv_runtime::intents;
use cv_runtime::route::{Navigator, Route, Section};
use cv_widgets::row;

const RAIL_WIDTH: f32 = 48.0;
const ITEM_SIZE: f32 = 40.0;
const ITEM_GAP: f32 = 4.0;
const INDICATOR_HEIGHT: f32 = 18.0;
/// Icon weight at rest for the regular items, and for the support item, which stays quieter so it
/// is easy to find yet easy to ignore.
const REST: f32 = 0.75;
const QUIET_REST: f32 = 0.4;

/// Where the support item leads: the author's Buy Me a Coffee page.
const SUPPORT_URL: &str = "https://buymeacoffee.com/northbyte";
/// The support item's tooltip and accessible name.
const SUPPORT_LABEL: &str = "Support Code View: buy me a coffee";

/// The icon rail on the left, like the Azure DevOps hub bar. The selection indicator slides
/// between items instead of jumping. At the bottom, a quiet coffee button opens the support page in
/// the browser, the keyboard button opens the shortcut list at the page's shortcuts, as `?` does,
/// and Settings closes the rail.
pub fn show(ui: &mut Ui, current: Section, navigator: &mut Navigator) {
    let palette = theme::palette(ui.ctx());
    egui::Panel::left("nav_rail")
        .exact_size(RAIL_WIDTH)
        .resizable(false)
        .frame(egui::Frame::new().fill(palette.rail).stroke(egui::Stroke::new(
            1.0,
            if palette.is_dark { palette.rail } else { palette.border },
        )))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = ITEM_GAP;
            ui.add_space(style::SPACE_S);
            let mut selected_rect = None;
            let items: [(&str, &str, Section, Route); 2] = [
                (
                    icons::GIT_PULL_REQUEST,
                    "Pull requests",
                    Section::PullRequests,
                    Route::MyPullRequests,
                ),
                (icons::BOOKS, "Repositories", Section::Repositories, Route::Repositories),
            ];
            for (icon, label, section, route) in items {
                let rect =
                    rail_item(ui, icon, label, current == section, REST, || navigator.jump(route));
                if current == section {
                    selected_rect = Some(rect);
                }
            }
            let remaining =
                ui.available_height() - 3.0 * ITEM_SIZE - 2.0 * ITEM_GAP - style::SPACE_S;
            ui.add_space(remaining.max(0.0));
            let ctx = ui.ctx().clone();
            rail_item(ui, icons::COFFEE, SUPPORT_LABEL, false, QUIET_REST, || {
                ctx.open_url(egui::OpenUrl::new_tab(SUPPORT_URL));
            });
            rail_item(ui, icons::KEYBOARD, "Keyboard shortcuts (?)", false, REST, || {
                intents::request_shortcuts(&ctx);
            });
            let settings = rail_item(
                ui,
                icons::GEAR,
                "Settings and connections",
                current == Section::Settings,
                REST,
                || navigator.jump(Route::Settings),
            );
            if current == Section::Settings {
                selected_rect = Some(settings);
            }
            if let Some(rect) = selected_rect {
                let id = ui.scope_id().with("rail_indicator");
                let y = motion::glide(ui.ctx(), id, rect.center().y, motion::MEDIUM);
                let indicator = Rect::from_center_size(
                    pos2(ui.max_rect().left() + 1.5, y),
                    vec2(3.0, INDICATOR_HEIGHT),
                );
                ui.painter().rect_filled(indicator, 2, palette.accent);
            }
        });
}

/// One rail button; returns its rect so the indicator can find it. `rest` is the icon's weight
/// when the pointer is elsewhere; hover always brings it to full strength.
fn rail_item(
    ui: &mut Ui,
    icon: &str,
    label: &str,
    is_selected: bool,
    rest: f32,
    on_click: impl FnOnce(),
) -> Rect {
    let palette = theme::palette(ui.ctx());
    let (slot, _) = ui.allocate_exact_size(vec2(RAIL_WIDTH, ITEM_SIZE), Sense::hover());
    let rect = Rect::from_center_size(slot.center(), vec2(ITEM_SIZE, ITEM_SIZE));
    let response = ui.interact(rect, ui.scope_id().with(("rail_item", label)), Sense::click());
    let hover = row::hover_amount(ui, &response);
    let selected =
        motion::toggle(ui.ctx(), response.id.with("rail_selected"), is_selected, motion::MEDIUM);
    let tint = palette.rail_text;
    // Hover and press tint neutrally; the current section takes an accent wash, the accent colour
    // and the Fill weight, so it reads as "you are here" even without the side indicator.
    let neutral = 0.06 * hover * (1.0 - selected)
        + if response.is_pointer_button_down_on() { 0.06 } else { 0.0 };
    if neutral > 0.0 {
        ui.painter().rect_filled(rect, style::RADIUS_CONTROL, tint.gamma_multiply(neutral));
    }
    if selected > 0.0 {
        let wash = if palette.is_dark { 0.22 } else { 0.12 };
        ui.painter().rect_filled(
            rect,
            style::RADIUS_CONTROL,
            palette.accent.gamma_multiply(wash * selected),
        );
    }
    let rest = motion::mix(tint.gamma_multiply(rest), tint, hover);
    let color = motion::mix(rest, palette.accent_text, selected);
    let font = icons::state_font(&palette, icons::SIZE_LARGE, is_selected);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, font, color);
    cv_widgets::focus::ring(ui, &response, rect);
    let response = response.on_hover_text(label);
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, is_selected, label));
    if response.clicked() {
        on_click();
    }
    slot
}

#[cfg(test)]
mod tests {
    use cv_runtime::route::{Navigator, Section};
    use cv_testkit::Clicks;
    use egui::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::{SUPPORT_LABEL, SUPPORT_URL};

    #[test]
    fn support_button_opens_the_coffee_page_and_leaves_the_route_alone() {
        let mut harness = Harness::builder()
            .with_size([400.0, 600.0])
            .with_context_setup(cv_base::theme::fonts::install)
            .build_ui_state(
                |ui, navigator: &mut Navigator| super::show(ui, Section::PullRequests, navigator),
                Navigator::default(),
            );
        harness.run();
        let button = harness.get_by_role_and_label(Role::Button, SUPPORT_LABEL);
        let settings = harness.get_by_role_and_label(Role::Button, "Settings and connections");
        let (button_bottom, settings_top) = (button.rect().bottom(), settings.rect().top());
        assert!(button_bottom <= settings_top, "the coffee button sits above Settings");
        harness.click_at(button.rect());
        harness.step();
        let opened: Vec<String> = harness
            .output()
            .platform_output
            .commands
            .iter()
            .filter_map(|command| match command {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(opened, [SUPPORT_URL], "a click opens the support page once");
        assert!(harness.state_mut().take().0.is_none(), "the route stays where it was");
    }
}
