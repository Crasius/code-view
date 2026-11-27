//! The bar at the top of the window: the back and forward arrows and the breadcrumbs on the left,
//! the pull request search (Ctrl+K) in the middle, the chips of running checkouts on the right.
//!
//! Where the platform can integrate it, it is the window's title bar: breadcrumbs and search in one
//! 40 px row across the full width, around the window's own parts that [`platform::Caption`] lays
//! out (on Windows the app icon and Windows 11 caption buttons with the native frame kept for every
//! window behavior, like Windows Terminal or VS Code; on macOS the room for the native traffic
//! lights, like Safari or Xcode). Elsewhere, and when the window could not be
//! integrated, the system draws the title bar and this is a 44 px row beside the navigation rail.

mod arrows;
mod checkouts;
mod jump;

use egui::{Rect, Stroke, Ui, pos2, vec2};

use cv_base::platform;
use cv_base::theme::{self, style};
use cv_lists::pull_request_list::PullRequestListPage;
use cv_runtime::history::History;
use cv_runtime::route::{Navigator, Route};
use cv_runtime::trail::Trail;
use cv_services::Services;
use cv_shell::breadcrumbs;

pub use self::jump::JumpAction;
use self::jump::JumpBox;

/// The integrated title bar; the caption buttons span its full height.
const CAPTION_HEIGHT: f32 = 40.0;
/// The in-app row below a system title bar.
const BAR_HEIGHT: f32 = 44.0;
/// The trail keeps at least this much room while the search box rests beside it at its narrowest;
/// with less, the box rests hidden (Ctrl+K still opens it) and the trail takes its room too.
const TRAIL_MIN_WIDTH: f32 = 240.0;

/// The page shown with the way to it, and the ways from it: the history the arrows step through,
/// and the navigator the arrows and the trail ask.
pub struct Navigation<'a> {
    pub trail: &'a Trail,
    pub history: &'a History,
    pub navigator: &'a mut Navigator,
}

pub struct TitleBar {
    is_integrated: bool,
    crumbs: breadcrumbs::Breadcrumbs,
    jump: JumpBox,
    /// The window theme last sent, so the native frame (border, system menu) follows the app's.
    window_theme: Option<egui::SystemTheme>,
}

impl TitleBar {
    pub fn new(creation: &eframe::CreationContext<'_>) -> Self {
        Self {
            is_integrated: platform::integrate_title_bar(creation),
            crumbs: breadcrumbs::Breadcrumbs::default(),
            jump: JumpBox::default(),
            window_theme: None,
        }
    }

    /// True when this bar is the window's title bar (and so spans the full width above the rail).
    pub fn is_integrated(&self) -> bool {
        self.is_integrated
    }

    /// The breadcrumbs as last shown.
    #[cfg(test)]
    pub fn crumbs(&self) -> &breadcrumbs::Breadcrumbs {
        &self.crumbs
    }

    /// Draws the bar as the title bar, as on a real window, without a window behind it.
    #[cfg(test)]
    pub fn integrate_for_test(&mut self) {
        self.is_integrated = true;
    }

    /// Shows the bar; returns what a pick in the search results asks for.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        navigation: Navigation<'_>,
        services: &Services,
        lists: &[&PullRequestListPage],
    ) -> Option<JumpAction> {
        self.follow_theme(ui.ctx(), services);
        let palette = theme::palette(ui.ctx());
        let height = if self.is_integrated { CAPTION_HEIGHT } else { BAR_HEIGHT };
        // Integrated, the window border is the system's: only the hairline below the bar is ours.
        let stroke =
            if self.is_integrated { Stroke::NONE } else { Stroke::new(1.0, palette.border) };
        egui::Panel::top("title_bar")
            .exact_size(height)
            .resizable(false)
            .frame(egui::Frame::new().fill(palette.surface).stroke(stroke))
            .show(ui, |ui| {
                let bar = ui.max_rect();
                if self.is_integrated {
                    ui.painter().hline(
                        bar.x_range(),
                        bar.bottom() - 0.5,
                        Stroke::new(1.0, palette.border),
                    );
                }
                let Navigation { trail, history, navigator } = navigation;
                let caption = self.is_integrated.then(|| platform::Caption::show(ui, bar, true));
                let free = caption.as_ref().map_or(bar.x_range(), platform::Caption::free);
                let left = free.min + style::SPACE_S;
                let arrows = arrows::show(ui, bar, left, services, history, navigator);
                let trail_left = arrows.right();
                let shown =
                    self.show_trail(ui, bar, trail_left, free.max, trail, services, navigator);
                let (search, action) = self.show_jump(
                    ui,
                    bar,
                    shown.right(),
                    free.max,
                    trail.route(),
                    services,
                    lists,
                );
                let after = search.map_or(shown.right(), |search| search.right()) + style::SPACE_M;
                let (chips, chip) =
                    checkouts::show(ui, bar, after, free.max - style::SPACE_M, services);
                if let Some(caption) = caption {
                    // Trail and search box first: the caption checks read them by position.
                    let holes = [
                        shown,
                        search.unwrap_or(Rect::NOTHING),
                        self.jump.popup_rect().unwrap_or(Rect::NOTHING),
                        self.crumbs.popup_rect().unwrap_or(Rect::NOTHING),
                        arrows,
                        chips,
                    ];
                    caption.publish(ui.ctx(), &holes);
                }
                action.or(chip)
            })
            .inner
    }

    /// The breadcrumb trail from `left`, ending where the search box's narrowest width still fits
    /// before `before` (the caption buttons, or the bar's end); returns the rect it covers. Laid
    /// out before anything right of it, so the trail never runs under the box or the buttons.
    #[expect(
        clippy::too_many_arguments,
        reason = "the bar's layout bounds plus what the trail names"
    )]
    fn show_trail(
        &mut self,
        ui: &mut Ui,
        bar: Rect,
        left: f32,
        before: f32,
        trail: &Trail,
        services: &Services,
        navigator: &mut Navigator,
    ) -> Rect {
        let beside_search = before - style::SPACE_M - jump::MIN_WIDTH - style::SPACE_M;
        let right = if beside_search - left >= TRAIL_MIN_WIDTH {
            beside_search
        } else {
            before - style::SPACE_M
        };
        let area = Rect::from_min_max(pos2(left, bar.top()), pos2(right.max(left), bar.bottom()));
        self.crumbs.show(ui, area, trail, services, navigator)
    }

    /// The search box, centered on the window when the trail leaves room, else right after it.
    /// Narrower than [`jump::MIN_WIDTH`] it rests hidden; opened (Ctrl+K, a search button) it then
    /// lies over the end of the trail until it closes. Returns its rect and the picked action.
    #[expect(
        clippy::too_many_arguments,
        reason = "the bar's layout bounds plus the page state the results come from"
    )]
    fn show_jump(
        &mut self,
        ui: &mut Ui,
        bar: Rect,
        after: f32,
        before: f32,
        route: &Route,
        services: &Services,
        lists: &[&PullRequestListPage],
    ) -> (Option<Rect>, Option<JumpAction>) {
        let is_open = self.jump.take_open_request(ui.ctx());
        let (mut from, to) = (after + style::SPACE_M, before - style::SPACE_M);
        if to - from < jump::MIN_WIDTH {
            if !is_open {
                return (None, None);
            }
            from = bar.left() + style::SPACE_M;
        }
        let width = (to - from).min(jump::MAX_WIDTH);
        if width <= 0.0 {
            return (None, None);
        }
        let left = (bar.center().x - width / 2.0).max(from).min(to - width);
        let rect = Rect::from_min_size(
            pos2(left, bar.center().y - jump::BOX_HEIGHT / 2.0),
            vec2(width, jump::BOX_HEIGHT),
        );
        (Some(rect), self.jump.show(ui, rect, lists, route, services))
    }

    /// Light or dark window frame (Windows 11 border, system menu) to match the app's theme.
    fn follow_theme(&mut self, ctx: &egui::Context, services: &Services) {
        let wanted = match services.settings.theme {
            review_store::ThemePreference::Dark => egui::SystemTheme::Dark,
            review_store::ThemePreference::Light => egui::SystemTheme::Light,
            review_store::ThemePreference::System => egui::SystemTheme::SystemDefault,
        };
        if self.window_theme != Some(wanted) {
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(wanted));
            self.window_theme = Some(wanted);
        }
    }
}
