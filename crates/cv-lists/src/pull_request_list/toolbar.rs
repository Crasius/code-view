//! The list page's header pieces: how its row fits the width, the drop-down pickers, and the
//! refresh, search, Favorites lane, repository sections and refresh-note tools.

use cv_base::theme::{icons, style};
use cv_runtime::intents;
use cv_widgets::{buttons, elide};

use super::SEARCH_TOOLTIP;

use cv_widgets::refresh_status::{self, RefreshStatus};

/// The overview's star tool: on while the Favorites lane is shown. The Settings row (`cv-settings`,
/// `appearance.rs`) repeats this text.
pub const FAVORITES_LANE: &str = "Show the Favorites lane";
/// The overview's sections tool: on while its cards group their rows by repository.
pub const GROUP_BY_REPOSITORY: &str = "Group by repository";
/// The overview's own tools beside refresh and search.
const OVERVIEW_TOOLS: f32 = 2.0;

/// A drop-down exactly `width` wide whose choice is cut with an ellipsis; `name` is its
/// accessible name. Its list is at most [`MENU_MAX_WIDTH`] wide (and never wider than the window):
/// longer choices are cut too, see [`choice`]. True when `menu` reports a change.
pub fn picker(
    ui: &mut egui::Ui,
    id: &str,
    selected: &str,
    name: &str,
    width: f32,
    menu: impl FnOnce(&mut egui::Ui) -> bool,
) -> bool {
    let size = egui::vec2(width, HEADER_ROW);
    let menu_width =
        MENU_MAX_WIDTH.max(width).min(ui.ctx().content_rect().width() - 2.0 * style::SPACE_M);
    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        let combo = egui::ComboBox::from_id_salt(id)
            .selected_text(selected)
            .width(width)
            .truncate()
            .show_ui(ui, |ui| {
                ui.set_max_width(menu_width);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                menu(ui)
            });
        combo.response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, name));
        if combo.response.hovered() && combo.inner.is_none() {
            combo.response.on_hover_text(selected);
        }
        combo.inner.unwrap_or(false)
    })
    .inner
}

/// One choice of a [`picker`]'s list; one cut to the list's width reads in full on hover.
pub fn choice<T: PartialEq>(
    ui: &mut egui::Ui,
    current: &mut T,
    value: T,
    label: &str,
) -> egui::Response {
    let font = egui::FontId::proportional(style::BODY_SIZE);
    let full = elide::text_width(ui, label, &font) + 2.0 * ui.spacing().button_padding.x;
    let room = ui.available_width();
    let response = ui.selectable_value(current, value, label);
    if full > room { response.on_hover_text(label) } else { response }
}

/// The header's single row: tall enough for the pivot tabs, so the list below never moves.
pub const HEADER_ROW: f32 = 40.0;
/// Gap after the title.
pub const TITLE_GAP: f32 = style::SPACE_L;
/// A longer title (a repository's key) is cut here, its full text in the tooltip.
const TITLE_MAX: f32 = 320.0;
/// A picker's list: room for a long repository key and its connection.
const MENU_MAX_WIDTH: f32 = 480.0;
/// The repository picker gives way first as the window narrows, down to its minimum: at the
/// narrowest window it cuts "All repositories" (the full choice on hover), so the overview's
/// tools and filters still share one row.
const REPOSITORY_MAX: f32 = 240.0;
const REPOSITORY_MIN: f32 = 108.0;
/// How the header's row fits the width, decided before anything is drawn so it never flickers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeaderLayout {
    pub title_width: f32,
    /// The overview's repository picker.
    pub repository_width: f32,
    /// The refresh note shrinks to an icon before the filters leave the row.
    pub compact_status: bool,
    /// Last resort: the filters move to a second row of the same height.
    pub filters_wrap: bool,
}

impl HeaderLayout {
    /// `fixed_filters` is the filters' width without the repository picker (or the pivot tabs'),
    /// `is_overview` whether the picker is part of them and the overview's tools of the tools.
    pub fn plan(available: f32, title: f32, fixed_filters: f32, is_overview: bool) -> Self {
        let title_width = title.min(TITLE_MAX);
        let own = if is_overview { OVERVIEW_TOOLS } else { 0.0 };
        let icons = (buttons::icon_width() + style::SPACE_XS) * (2.0 + own);
        let tools = |compact: bool| {
            style::SPACE_M
                + icons
                + if compact { refresh_status::COMPACT_WIDTH } else { refresh_status::WIDTH }
        };
        let room =
            |compact: bool| available - title_width - TITLE_GAP - tools(compact) - fixed_filters;
        let fits = |room: f32| if is_overview { room >= REPOSITORY_MIN } else { room >= 0.0 };
        let repository = |room: f32| {
            if is_overview { room.clamp(REPOSITORY_MIN, REPOSITORY_MAX) } else { 0.0 }
        };
        if fits(room(false)) {
            return Self {
                title_width,
                repository_width: repository(room(false)),
                compact_status: false,
                filters_wrap: false,
            };
        }
        if fits(room(true)) {
            return Self {
                title_width,
                repository_width: repository(room(true)),
                compact_status: true,
                filters_wrap: false,
            };
        }
        // On a row of their own the filters have the full width; the title keeps what the tools leave.
        let title_width = title_width.min((available - tools(false) - TITLE_GAP).max(0.0));
        Self {
            title_width,
            repository_width: repository(available - fixed_filters),
            compact_status: false,
            filters_wrap: true,
        }
    }
}

/// What the header asked for, carried out once it is drawn.
#[derive(Default)]
pub struct HeaderAsks {
    pub wants_refresh: bool,
    pub filters_changed: bool,
    /// A star of the header was clicked.
    pub star: Option<HeaderStar>,
    /// The overview's sections tool was clicked: group by repository, or stop.
    pub toggles_sections: bool,
}

/// The overview's tools and whether each is on.
#[derive(Clone, Copy)]
pub struct OverviewTools {
    pub is_lane_shown: bool,
    pub is_grouped: bool,
}

/// The header's star: a repository's list has the repository's, the overview the lane tool.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HeaderStar {
    /// Star or unstar the repository.
    Repository,
    /// Show or hide the Favorites lane.
    Lane,
}

/// Refresh (F5), search, the overview's Favorites lane and sections tools and the refresh note,
/// laid out from the right edge; fills in what they asked for.
pub fn header_tools(
    ui: &mut egui::Ui,
    layout: HeaderLayout,
    status: RefreshStatus<'_>,
    overview: Option<OverviewTools>,
    asks: &mut HeaderAsks,
) {
    asks.wants_refresh = buttons::icon(ui, icons::ARROW_CLOCKWISE, "Refresh (F5)").clicked()
        || ui.input(|input| input.key_pressed(egui::Key::F5));
    ui.add_space(style::SPACE_XS);
    if buttons::icon(ui, icons::MAGNIFYING_GLASS, SEARCH_TOOLTIP).clicked() {
        intents::open_search(ui.ctx());
    }
    ui.add_space(style::SPACE_XS);
    if let Some(OverviewTools { is_lane_shown, is_grouped }) = overview {
        if buttons::icon_toggle(ui, icons::STAR, icons::STAR, is_lane_shown, FAVORITES_LANE)
            .clicked()
        {
            asks.star = Some(HeaderStar::Lane);
        }
        ui.add_space(style::SPACE_XS);
        asks.toggles_sections =
            buttons::icon_toggle(ui, icons::BOOKS, icons::BOOKS, is_grouped, GROUP_BY_REPOSITORY)
                .clicked();
        ui.add_space(style::SPACE_XS);
    }
    let retry = if layout.compact_status {
        refresh_status::show_compact(ui, status)
    } else {
        refresh_status::show(ui, status)
    };
    asks.wants_refresh |= retry;
}

#[cfg(test)]
mod tests {
    use super::{HeaderLayout, REPOSITORY_MAX, REPOSITORY_MIN};

    #[test]
    fn the_header_compacts_the_note_before_it_wraps_the_filters() {
        let (title, filters) = (180.0, 360.0);
        let mut phases: Vec<&str> = Vec::new();
        let mut available = 1600.0;
        while available >= 300.0 {
            let layout = HeaderLayout::plan(available, title, filters, true);
            assert!(
                (REPOSITORY_MIN..=REPOSITORY_MAX).contains(&layout.repository_width),
                "the repository picker keeps its bounds at {available}: {layout:?}"
            );
            let phase = match (layout.compact_status, layout.filters_wrap) {
                (false, false) => "full note",
                (true, false) => "compact note",
                (false, true) => "filters wrapped",
                (true, true) => "compact note and filters wrapped",
            };
            if phases.last() != Some(&phase) {
                phases.push(phase);
            }
            available -= 4.0;
        }
        assert_eq!(
            phases,
            ["full note", "compact note", "filters wrapped"],
            "as the window narrows"
        );

        let repository = HeaderLayout::plan(300.0, title, filters, false);
        assert!(repository.filters_wrap, "{repository:?}");
        assert_eq!(repository.repository_width, 0.0, "a repository's list has no picker");
    }
}
