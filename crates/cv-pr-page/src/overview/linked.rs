//! The Linked card: pull requests, issues, work items and tickets the pull request's texts point
//! at, like Azure DevOps' work items and related pull requests, each with its current state.
//!
//! Rows are laid out from the references alone, at a fixed height; titles and states fill in
//! when the platform answers, so nothing moves when they arrive.

use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Ui, pos2, vec2};
use review_core::{ItemKind, LinkedItem, LinkedState};

use cv_base::platform::shortcut;
use cv_base::theme::{self, Palette, icons, style};
use cv_pr_session::links::{self, LinkedEntry};
use cv_runtime::links::link_target::{Here, Target};
use cv_services::linked::{LinkedCache, Status};
use cv_widgets::{badges, buttons, elide, glyph, provider_icon, row, skeleton};

const ROW_HEIGHT: f32 = 30.0;
const ICON_SIZE: f32 = 15.0;

pub enum LinkedAction {
    Open { target: Target, browser: bool },
    OpenUrl(String),
    Retry,
}

/// Shows the rows; returns what the user picked.
pub fn show(
    ui: &mut Ui,
    entries: &[LinkedEntry],
    cache: &LinkedCache,
    here: &Here,
    connections: &[review_core::Connection],
    native_error: Option<&str>,
) -> Option<LinkedAction> {
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    let mut failure: Option<String> = native_error.map(str::to_owned);
    for (index, entry) in entries.iter().enumerate() {
        let picked = match entry {
            LinkedEntry::Item(target) => {
                let status = cache.status(&target.connection_id, &target.item);
                if let Status::Failed(message) = status {
                    failure.get_or_insert_with(|| message.to_owned());
                }
                item_row(ui, index, target, status, here, connections, &palette)
            }
            LinkedEntry::Ticket { key, url } => ticket_row(ui, key, url, &palette),
        };
        action = action.or(picked);
    }
    if let Some(message) = failure {
        ui.add_space(style::SPACE_XS);
        ui.horizontal(|ui| {
            glyph::decorative(ui, badges::icon_text(icons::WARNING, palette.warning, 14.0))
                .on_hover_text(&message);
            ui.label(
                RichText::new("Some states could not be loaded.")
                    .size(style::SMALL_SIZE)
                    .color(palette.text_secondary),
            )
            .on_hover_text(&message);
            if buttons::subtle(ui, icons::ARROW_CLOCKWISE, "Retry").clicked() {
                action = Some(LinkedAction::Retry);
            }
        });
    }
    action
}

/// The row's frame: hover, pressed and focus states, tooltip and context menu.
fn row_frame(ui: &mut Ui, label: &str) -> (Rect, egui::Response) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, label));
    row::card_background(ui, &response, rect, Color32::TRANSPARENT, style::RADIUS_CONTROL);
    cv_widgets::focus::ring(ui, &response, rect);
    (rect, response)
}

fn item_row(
    ui: &mut Ui,
    index: usize,
    target: &Target,
    status: Status<'_>,
    here: &Here,
    connections: &[review_core::Connection],
    palette: &Palette,
) -> Option<LinkedAction> {
    let label = target.label(Some(here));
    let found = match status {
        Status::Found(item) => Some(item),
        _ => None,
    };
    let accessible = match found {
        Some(item) => format!("{label} {} ({})", item.title, item.state_label),
        None => label.clone(),
    };
    let (rect, response) = row_frame(ui, &accessible);
    let painter = ui.painter_at(rect);
    let (icon, color) = kind_icon(target.item.kind, found, palette);
    let icon_center = pos2(rect.left() + style::SPACE_S + ICON_SIZE / 2.0, rect.center().y);
    painter.text(icon_center, Align2::CENTER_CENTER, icon, FontId::proportional(ICON_SIZE), color);

    // Right side first: state pill and checks; the reference and the title share what is left.
    let mut right = rect.right() - style::SPACE_S;
    if let Some(item) = found {
        right = paint_state(&painter, item, right, rect.center().y, palette);
    }
    let label_left = icon_center.x + ICON_SIZE / 2.0 + style::SPACE_S;
    let slots = text_slots(ui, rect, label_left, right, &label);
    paint_text(&painter, slots.label_rect, &slots.label, palette.text_secondary);
    let mut title_rect = slots.title_rect;
    // An item on another connection names its platform with the mark after its reference.
    let elsewhere = (target.connection_id != here.connection_id)
        .then(|| connections.iter().find(|connection| connection.id == target.connection_id))
        .flatten();
    if let Some(connection) = elsewhere
        && title_rect.width() >= ICON_SIZE
    {
        let mark = Rect::from_min_size(
            pos2(title_rect.left(), rect.center().y - ICON_SIZE / 2.0),
            vec2(ICON_SIZE, ICON_SIZE),
        );
        provider_icon::paint(&painter, palette, mark.center(), ICON_SIZE, connection.kind);
        provider_icon::interact(
            ui,
            mark,
            response.id.with("platform"),
            connection.kind,
            connection.base_url(),
        );
        title_rect.min.x = (mark.right() + style::SPACE_S).min(title_rect.max.x);
    }
    paint_item_title(ui, title_rect, status, index, palette);

    let fallback = links::fallback_url(target, connections);
    let web_url = found.map_or(fallback, |item| item.web_url.clone());
    let opens_in_app = match found {
        Some(item) => item.kind == ItemKind::PullRequest,
        None => matches!(target.item.kind, ItemKind::PullRequest | ItemKind::IssueOrPullRequest),
    };
    let mut action = item_menu(&response, target, opens_in_app, &web_url);
    let hint = open_hint(opens_in_app, found, target.item.kind);
    let response = response.on_hover_text(item_tooltip(found, &label, &web_url, hint));
    if response.clicked() {
        let browser = ui.input(|input| input.modifiers.command);
        action = Some(LinkedAction::Open { target: target.clone(), browser });
    }
    action
}

/// The title once it is known, a note when it cannot be, a skeleton bar while it loads.
fn paint_item_title(
    ui: &Ui,
    title_rect: Rect,
    status: Status<'_>,
    index: usize,
    palette: &Palette,
) {
    match status {
        Status::Found(item) => paint_title(ui, title_rect, &item.title, palette.text),
        Status::Missing => {
            paint_title(ui, title_rect, "No details for this connection", palette.text_disabled);
        }
        Status::Failed(_) => {
            paint_title(ui, title_rect, "State unavailable", palette.text_disabled);
        }
        Status::Unknown | Status::Loading => {
            let width = (title_rect.width() * (0.55 + 0.1 * (index % 3) as f32)).max(0.0);
            let bar = Rect::from_min_size(
                pos2(title_rect.left(), title_rect.center().y - 5.0),
                vec2(width, 10.0),
            );
            skeleton::paint_bar(ui, bar);
        }
    }
}

/// The row's context menu: open in the app or the browser, copy the link.
fn item_menu(
    response: &egui::Response,
    target: &Target,
    opens_in_app: bool,
    web_url: &str,
) -> Option<LinkedAction> {
    let mut action = None;
    response.context_menu(|ui| {
        if opens_in_app && glyph::button(ui, icons::GIT_PULL_REQUEST, "Open in Code View").clicked()
        {
            action = Some(LinkedAction::Open { target: target.clone(), browser: false });
            ui.close();
        }
        if glyph::button(ui, icons::ARROW_SQUARE_OUT, "Open in browser").clicked() {
            action = Some(LinkedAction::Open { target: target.clone(), browser: true });
            ui.close();
        }
        if glyph::button(ui, icons::LINK, "Copy link").clicked() {
            ui.ctx().copy_text(web_url.to_owned());
            ui.close();
        }
    });
    action
}

/// What a click does, as the last line of the tooltip.
fn open_hint(opens_in_app: bool, found: Option<&LinkedItem>, kind: ItemKind) -> &'static str {
    match (opens_in_app, found) {
        (true, None) if kind == ItemKind::IssueOrPullRequest => {
            concat!(
                "Opens in Code View if it is a pull request (",
                shortcut!(command click),
                ": browser)"
            )
        }
        (true, _) => concat!("Open in Code View (", shortcut!(command click), ": browser)"),
        (false, _) => "Open in browser",
    }
}

fn item_tooltip(found: Option<&LinkedItem>, label: &str, web_url: &str, hint: &str) -> String {
    match found {
        Some(item) => {
            let checks = item
                .checks
                .map(|checks| format!("\nChecks: {}", badges::run_status_label(checks)))
                .unwrap_or_default();
            let kind = item.item_type.as_deref().map(|kind| format!("{kind} ")).unwrap_or_default();
            format!("{kind}{label} {}\n{}{checks}\n{hint}", item.title, item.state_label)
        }
        None => format!("{web_url}\n{hint}"),
    }
}

fn ticket_row(ui: &mut Ui, key: &str, url: &str, palette: &Palette) -> Option<LinkedAction> {
    let (rect, response) = row_frame(ui, key);
    let painter = ui.painter_at(rect);
    let icon_center = pos2(rect.left() + style::SPACE_S + ICON_SIZE / 2.0, rect.center().y);
    painter.text(
        icon_center,
        Align2::CENTER_CENTER,
        icons::TICKET,
        FontId::proportional(ICON_SIZE),
        palette.text_secondary,
    );
    let left = icon_center.x + ICON_SIZE / 2.0 + style::SPACE_S;
    let slots = text_slots(ui, rect, left, rect.right() - style::SPACE_S, key);
    paint_text(&painter, slots.label_rect, &slots.label, palette.accent);
    let host = url.split('/').nth(2).unwrap_or(url);
    paint_title(ui, slots.title_rect, host, palette.text_secondary);
    let mut action = None;
    response.context_menu(|ui| {
        if glyph::button(ui, icons::ARROW_SQUARE_OUT, "Open in browser").clicked() {
            action = Some(LinkedAction::OpenUrl(url.to_owned()));
            ui.close();
        }
        if glyph::button(ui, icons::LINK, "Copy link").clicked() {
            ui.ctx().copy_text(url.to_owned());
            ui.close();
        }
    });
    if response.on_hover_text(format!("{url}\nOpen in browser")).clicked() {
        action = Some(LinkedAction::OpenUrl(url.to_owned()));
    }
    action
}

/// Where a row's reference and title go between `left` and `right` (the left edge of the
/// trailing pill): the reference first, cut in the middle so its number stays, the title after it.
struct TextSlots {
    label: String,
    label_rect: Rect,
    title_rect: Rect,
}

fn text_slots(ui: &Ui, rect: Rect, left: f32, right: f32, label: &str) -> TextSlots {
    let font = FontId::proportional(style::BODY_SIZE);
    let room = (right - style::SPACE_S - left).max(0.0);
    let label = elide::reference(ui, label, &font, room);
    let width = ui
        .fonts_mut(|fonts| fonts.layout_no_wrap(label.clone(), font, Color32::PLACEHOLDER).size().x)
        .min(room);
    let label_rect = Rect::from_min_max(pos2(left, rect.top()), pos2(left + width, rect.bottom()));
    let title_left = label_rect.right() + style::SPACE_S;
    let title_rect = Rect::from_min_max(
        pos2(title_left, rect.top()),
        pos2((right - style::SPACE_S).max(title_left), rect.bottom()),
    );
    TextSlots { label, label_rect, title_rect }
}

/// One line, vertically centred in `rect`, clipped to it.
fn paint_text(painter: &egui::Painter, rect: Rect, text: &str, color: Color32) {
    let galley =
        painter.layout_no_wrap(text.to_owned(), FontId::proportional(style::BODY_SIZE), color);
    painter.with_clip_rect(rect.intersect(painter.clip_rect())).galley(
        pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
}

/// One line of text, cut with an ellipsis where it does not fit.
fn paint_title(ui: &Ui, rect: Rect, text: &str, color: Color32) {
    if rect.width() <= 0.0 {
        return;
    }
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.to_owned(),
        FontId::proportional(style::BODY_SIZE),
        color,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width());
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.painter_at(rect).galley(
        pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
}

const PILL_PADDING: egui::Vec2 = vec2(8.0, 2.0);

/// The state pill's rect, right-aligned at `right`.
fn pill_rect(painter: &egui::Painter, state_label: &str, right: f32, center_y: f32) -> Rect {
    let galley = painter.layout_no_wrap(
        state_label.to_owned(),
        FontId::proportional(style::SMALL_SIZE),
        Color32::PLACEHOLDER,
    );
    let size = galley.size() + PILL_PADDING * 2.0;
    Rect::from_min_size(pos2(right - size.x, center_y - size.y / 2.0), size)
}

/// State pill and checks icon, right-aligned at `right`; returns the left edge of what it drew.
fn paint_state(
    painter: &egui::Painter,
    item: &LinkedItem,
    right: f32,
    center_y: f32,
    palette: &Palette,
) -> f32 {
    let (fill, text_color) = state_colors(item.state, palette);
    let pill = pill_rect(painter, &item.state_label, right, center_y);
    let galley = painter.layout_no_wrap(
        item.state_label.clone(),
        FontId::proportional(style::SMALL_SIZE),
        text_color,
    );
    painter.rect_filled(pill, 10, fill);
    painter.galley(pill.min + PILL_PADDING, galley, text_color);
    let mut left = pill.left();
    if let Some(checks) = item.checks {
        let (icon, color) = badges::run_status_icon(checks, palette);
        let center = pos2(left - style::SPACE_S - 8.0, center_y);
        painter.text(center, Align2::CENTER_CENTER, icon, FontId::proportional(16.0), color);
        left = center.x - 8.0;
    }
    left
}

/// Pills carry the state as text; the fill only repeats it.
fn state_colors(state: LinkedState, palette: &Palette) -> (Color32, Color32) {
    match state {
        LinkedState::Open => (palette.accent, palette.on_accent),
        LinkedState::Draft => (palette.text_secondary, palette.on_accent),
        LinkedState::Merged | LinkedState::Done => (palette.success, palette.on_accent),
        LinkedState::Closed => (palette.surface_subtle, palette.text_secondary),
    }
}

/// Kind glyph, colored by state once it is known.
fn kind_icon(
    kind: ItemKind,
    found: Option<&LinkedItem>,
    palette: &Palette,
) -> (&'static str, Color32) {
    let Some(item) = found else {
        let icon = match kind {
            ItemKind::PullRequest => icons::GIT_PULL_REQUEST,
            ItemKind::Issue => icons::TARGET,
            ItemKind::IssueOrPullRequest => icons::CIRCLE_DASHED,
            ItemKind::WorkItem => icons::SQUARE,
        };
        return (icon, palette.text_disabled);
    };
    let color = match item.state {
        LinkedState::Open => palette.accent,
        LinkedState::Draft | LinkedState::Closed => palette.text_secondary,
        LinkedState::Merged | LinkedState::Done => palette.success,
    };
    let icon = match (item.kind, item.state) {
        (ItemKind::PullRequest, LinkedState::Merged) => icons::GIT_MERGE,
        (ItemKind::PullRequest | ItemKind::IssueOrPullRequest, _) => icons::GIT_PULL_REQUEST,
        (ItemKind::Issue, _) => icons::TARGET,
        (ItemKind::WorkItem, LinkedState::Done) => icons::CHECK_SQUARE,
        (ItemKind::WorkItem, _) => icons::SQUARE,
    };
    (icon, color)
}

#[cfg(test)]
mod tests {
    use egui::{Rect, pos2, vec2};
    use egui_kittest::Harness;

    use super::{ROW_HEIGHT, pill_rect, text_slots};
    use cv_base::theme::style;

    /// A long reference in a narrow card: the reference is cut before the "Active" pill and keeps
    /// its number; the title gets no room rather than running under the pill.
    #[test]
    fn a_long_reference_stops_before_the_state_pill() {
        let mut harness = Harness::new_ui(|ui| {
            cv_base::theme::fonts::install(ui.ctx());
            let row = Rect::from_min_size(pos2(0.0, 0.0), vec2(208.0, ROW_HEIGHT));
            let pill =
                pill_rect(ui.painter(), "Active", row.right() - style::SPACE_S, row.center().y);
            let slots =
                text_slots(ui, row, 31.0, pill.left(), "contoso-platform-tools/data-loader#59");
            assert!(
                slots.label_rect.right() <= pill.left() - style::SPACE_S + 0.5,
                "{:?} runs into {pill:?}",
                slots.label_rect
            );
            assert!(!slots.label_rect.intersects(pill), "{:?} overlaps {pill:?}", slots.label_rect);
            assert!(slots.label.ends_with("#59"), "keeps the number: {}", slots.label);
            assert!(slots.label.contains('…'), "cut: {}", slots.label);
            assert!(
                slots.title_rect.right() <= pill.left(),
                "the title stays left of the pill too"
            );
        });
        harness.run();
    }
}
