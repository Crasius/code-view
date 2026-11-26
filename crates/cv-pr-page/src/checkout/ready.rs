//! A done checkout: the split button opens the tool used last; its menu says where the checkout
//! is and opens the terminal or an agent there, updates it when it could not follow the pull
//! request by itself, shows the folder, copies its path, removes the worktree, and shows the log.

use egui::epaint::CornerRadius;
use egui::{Align2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use review_store::checkouts::{CheckoutRecord, OpenWith};

use super::{Act, CARET_WIDTH, View, is_removable};
use cv_base::theme::{self, icons, style};
use cv_services::checkouts::{self, Status, reason};
use cv_widgets::{buttons, focus, progress_log, row};

const MENU_WIDTH: f32 = 320.0;
const LOG_HEIGHT: f32 = 260.0;
const ROW_HEIGHT: f32 = 40.0;

/// The tool the main part opens: the checkout's last, else the app's last, else the terminal; an
/// agent that is not installed falls back to the terminal.
fn main_tool(view: &View<'_>, record: &CheckoutRecord) -> OpenWith {
    let last = match (record.last_opened, view.settings.last_opened) {
        (OpenWith::Nothing, OpenWith::Nothing) => OpenWith::Terminal,
        (OpenWith::Nothing, last) | (last, _) => last,
    };
    match last {
        OpenWith::Agent(kind) if !is_installed(view, kind) => OpenWith::Terminal,
        last => last,
    }
}

fn is_installed(view: &View<'_>, kind: review_core::AgentKind) -> bool {
    view.agents.iter().any(|(agent, is_installed)| *agent == kind && *is_installed)
}

fn tool_icon(with: OpenWith) -> &'static str {
    match with {
        OpenWith::Agent(_) => icons::ROBOT,
        OpenWith::Nothing | OpenWith::Terminal => icons::TERMINAL_WINDOW,
    }
}

/// "Open Claude Code ▾": returns the button's rect for the popover and what was asked for. A
/// `compact` button shows only the tool's icon, its name as the tooltip.
pub(super) fn button(
    ui: &mut Ui,
    view: &View<'_>,
    record: &CheckoutRecord,
    status: Option<&Status>,
    compact: bool,
) -> (Rect, Option<Act>) {
    let palette = theme::palette(ui.ctx());
    let tool = main_tool(view, record);
    let name = match tool {
        OpenWith::Terminal => "terminal".to_owned(),
        other => checkouts::tool_name(other, view.settings),
    };
    let label = format!("Open {name}");
    let galley = ui.painter().layout_no_wrap(
        if compact { String::new() } else { label.clone() },
        egui::FontId::proportional(style::BODY_SIZE),
        palette.text,
    );
    let gap = if compact { 0.0 } else { 6.0 };
    let main_width = style::SPACE_M + icons::SIZE + gap + galley.size().x + style::SPACE_M;
    let (rect, _) = ui
        .allocate_exact_size(vec2(main_width + CARET_WIDTH, style::CONTROL_HEIGHT), Sense::hover());
    let main_rect = Rect::from_min_size(rect.min, vec2(main_width, rect.height()));
    let caret_rect = Rect::from_min_max(pos2(main_rect.right(), rect.top()), rect.max);
    let id = ui.make_persistent_id(("checkout_open", view.key.number));
    let main = ui.interact(main_rect, id, Sense::click());
    let caret = ui.interact(caret_rect, id.with("caret"), Sense::click());
    main.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    let main = if compact { main.on_hover_text(&label) } else { main };
    caret.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Checkout options"));
    let is_open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&caret));
    let radius = style::RADIUS_CONTROL;
    for (response, corners) in [
        (&main, CornerRadius { nw: radius, sw: radius, ne: 0, se: 0 }),
        (&caret, CornerRadius { nw: 0, sw: 0, ne: radius, se: radius }),
    ] {
        let fill = row::state_fill(ui, response, palette.surface);
        ui.painter().rect(
            response.rect,
            corners,
            fill,
            Stroke::new(1.0, palette.border_strong),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().text(
        pos2(main_rect.left() + style::SPACE_M + icons::SIZE / 2.0, rect.center().y),
        Align2::CENTER_CENTER,
        tool_icon(tool),
        icons::rest_font(&palette, icons::SIZE),
        palette.text,
    );
    let text_left = main_rect.left() + style::SPACE_M + icons::SIZE + 6.0;
    ui.painter().galley(
        pos2(text_left, rect.center().y - galley.size().y / 2.0),
        galley,
        palette.text,
    );
    ui.painter().text(
        caret_rect.center(),
        Align2::CENTER_CENTER,
        if is_open { icons::CARET_UP } else { icons::CARET_DOWN },
        icons::rest_font(&palette, icons::SIZE_SMALL),
        palette.text_secondary,
    );
    focus::ring(ui, &main, main_rect);
    focus::ring(ui, &caret, caret_rect);
    let mut act = main.clicked().then_some(Act::Open(tool));
    // A menu: the caret toggles it, a pick closes it.
    let shown = egui::Popup::menu(&caret)
        .align(egui::RectAlign::BOTTOM_END)
        .gap(style::SPACE_XS)
        .width(MENU_WIDTH)
        .show(|ui| {
            ui.set_width(MENU_WIDTH);
            menu(ui, view, record, status, tool)
        });
    if let Some(picked) = shown.and_then(|shown| shown.inner) {
        act = Some(picked);
    }
    (rect, act)
}

fn menu(
    ui: &mut Ui,
    view: &View<'_>,
    record: &CheckoutRecord,
    status: Option<&Status>,
    main: OpenWith,
) -> Option<Act> {
    let palette = theme::palette(ui.ctx());
    let mut act = None;
    ui.label(RichText::new("Checked out in").size(style::SMALL_SIZE).color(palette.text_secondary));
    ui.add(
        egui::Label::new(
            RichText::new(record.path.display().to_string())
                .font(theme::fonts::code_font(style::SMALL_SIZE)),
        )
        .truncate(),
    )
    .on_hover_text(record.path.display().to_string());
    ui.separator();
    let terminal = checkouts::terminal_name(view.settings);
    if entry(
        ui,
        icons::TERMINAL_WINDOW,
        "Terminal",
        Some(&terminal),
        marker(main, OpenWith::Terminal),
        true,
    )
    .clicked()
    {
        act = Some(Act::Open(OpenWith::Terminal));
    }
    for (kind, is_installed) in view.agents {
        let with = OpenWith::Agent(kind);
        let subtitle = (!is_installed).then_some("Not installed");
        let name = checkouts::agent_name(kind);
        if entry(ui, icons::ROBOT, name, subtitle, marker(main, with), is_installed).clicked() {
            act = Some(Act::Open(with));
        }
    }
    if let Some(Status { blocked: Some(blocked), behind, .. }) = status
        && *behind > 0
    {
        ui.separator();
        let subtitle = format!("{behind} behind · {}", reason(blocked, *behind));
        if entry(ui, icons::ARROWS_CLOCKWISE, "Update checkout", Some(&subtitle), None, true)
            .clicked()
        {
            act = Some(Act::UpdateNow);
        }
    }
    ui.separator();
    if buttons::menu_item(ui, icons::FOLDER_OPEN, "Open folder", None, None).clicked() {
        act = Some(Act::OpenFolder(record.path.clone()));
    }
    if buttons::menu_item(ui, icons::COPY, "Copy path", None, None).clicked() {
        act = Some(Act::CopyPath(record.path.clone()));
    }
    if is_removable(record)
        && buttons::menu_item(ui, icons::TRASH, "Remove worktree…", None, Some(palette.error))
            .clicked()
    {
        act = Some(Act::AskRemove);
    }
    ui.separator();
    ui.horizontal(|ui| {
        let took = std::time::Duration::from_millis(record.took_ms);
        let text = if record.took_ms > 0 {
            format!("Checked out in {}", progress_log::format_duration(took))
        } else {
            "Opened from your clone".to_owned()
        };
        ui.label(RichText::new(text).size(style::SMALL_SIZE).color(palette.text_secondary));
        if buttons::text_link(ui, "Show log", "The checkout's log and its updates").clicked() {
            act = Some(Act::ShowLog);
        }
    });
    act
}

fn marker(main: OpenWith, with: OpenWith) -> Option<&'static str> {
    (main == with).then_some("last used")
}

/// A menu entry with a second line and a marker on the right ("last used").
fn entry(
    ui: &mut Ui,
    icon: &str,
    title: &str,
    subtitle: Option<&str>,
    marker: Option<&str>,
    is_enabled: bool,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), ROW_HEIGHT),
        if is_enabled { Sense::click() } else { Sense::hover() },
    );
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::Role::MenuItem, is_enabled, title);
        // The second line and the marker reach a screen reader as the entry's value.
        let details: Vec<&str> = subtitle.into_iter().chain(marker).collect();
        info.current_text_value = (!details.is_empty()).then(|| details.join(", "));
        info
    });
    if is_enabled {
        let fill = row::state_fill(ui, &response, egui::Color32::TRANSPARENT);
        ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
    }
    let color = if is_enabled { palette.text } else { palette.text_disabled };
    let icon_center = pos2(rect.left() + style::SPACE_S + icons::SIZE / 2.0, rect.center().y);
    ui.painter().text(
        icon_center,
        Align2::CENTER_CENTER,
        icon,
        icons::rest_font(&palette, icons::SIZE),
        color,
    );
    let left = icon_center.x + icons::SIZE / 2.0 + style::SPACE_S;
    let (title_y, subtitle_y) = match subtitle {
        Some(_) => (rect.center().y - 8.0, rect.center().y + 9.0),
        None => (rect.center().y, rect.center().y),
    };
    ui.painter().text(
        pos2(left, title_y),
        Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(style::BODY_SIZE),
        color,
    );
    if let Some(subtitle) = subtitle {
        ui.painter().text(
            pos2(left, subtitle_y),
            Align2::LEFT_CENTER,
            subtitle,
            egui::FontId::proportional(style::SMALL_SIZE),
            palette.text_secondary,
        );
    }
    if let Some(marker) = marker {
        ui.painter().text(
            pos2(rect.right() - style::SPACE_S, rect.center().y),
            Align2::RIGHT_CENTER,
            marker,
            egui::FontId::proportional(style::SMALL_SIZE),
            palette.text_secondary,
        );
    }
    focus::ring(ui, &response, rect);
    response
}

/// The checkout's saved log: the checkout itself, then each update.
pub(super) fn log(ui: &mut Ui, record: &CheckoutRecord) -> Option<Act> {
    let palette = theme::palette(ui.ctx());
    let mut act = None;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Checkout log").font(theme::fonts::semibold_font(style::SECTION_SIZE)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            progress_log::copy_text_button(ui, || record.log.join("\n"));
        });
    });
    let frame = egui::Frame::new()
        .fill(palette.surface_subtle)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(style::RADIUS_CARD)
        .inner_margin(egui::Margin::symmetric(10, 8));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::both()
            .id_salt("checkout_saved_log")
            .max_height(LOG_HEIGHT)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in &record.log {
                    ui.label(
                        RichText::new(progress_log::redact(line))
                            .font(theme::fonts::code_font(style::SMALL_SIZE)),
                    );
                }
            });
    });
    if buttons::secondary(ui, "Close").clicked() {
        act = Some(Act::Close);
    }
    act
}
