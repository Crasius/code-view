use std::sync::Arc;

use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Color32, Context, FontId, Galley, Id, Key, Popup, Rect, Response, Sense, Ui, pos2, vec2,
};
use review_core::ProviderKind;

use cv_base::theme::{self, icons, motion, style};
use cv_base::{mailbox, platform};
use cv_runtime::route::{Navigator, Route};
use cv_runtime::trail::Trail;
use cv_services::Services;
use cv_widgets::atlas_watch::AtlasWatch;
use cv_widgets::crumb_fit::{self, CrumbWidth, Spacing};
use cv_widgets::numbers::number_prefix;
use cv_widgets::{focus, provider_icon, row};

const CRUMB_PADDING: egui::Vec2 = egui::vec2(6.0, 4.0);
/// Longer names (Azure DevOps `org/project/repo`) end in an ellipsis; the tooltip has them whole.
const CRUMB_MAX_WIDTH: f32 = 280.0;
/// Where the trail runs short of room, middle crumbs are cut to this before any folds into "…".
const CRUMB_SHORT_WIDTH: f32 = 160.0;
/// The platform mark's slot before a connection's name, and its gap to the name.
const MARK_SIZE: f32 = icons::SIZE;
const MARK_GAP: f32 = 6.0;
const SEPARATOR_SIZE: f32 = 11.0;
/// Between a crumb and the separator caret on either side.
const SEPARATOR_GAP: f32 = 2.0;
/// The "…" crumb is at least this wide, so it is an easy target.
const OVERFLOW_MIN_WIDTH: f32 = 28.0;
const OVERFLOW_LABEL: &str = "Show hidden levels";
const HIDDEN_LIST_LABEL: &str = "Hidden levels";
/// Rows of the hidden levels' list.
const ROW_HEIGHT: f32 = 28.0;
const ROW_MIN_WIDTH: f32 = 200.0;
/// Names longer than this end in an ellipsis in the list; the tooltip has them whole.
const ROW_TEXT_MAX_WIDTH: f32 = 420.0;

/// One level of the trail.
struct Level {
    text: String,
    /// The level this crumb stands for; the crumb of the current page leads nowhere.
    route: Route,
    is_root: bool,
    /// The connection's platform and web root: the connection crumb leads with its mark.
    platform: Option<(ProviderKind, String)>,
    /// A level the page added below its route (see [`publish_trail`]), with its tooltip.
    from_page: Option<(usize, &'static str)>,
}

impl Level {
    fn mark_width(&self) -> f32 {
        if self.platform.is_some() { MARK_SIZE + MARK_GAP } else { 0.0 }
    }

    /// Where the crumb leads, for its tooltip.
    fn hint(&self) -> &'static str {
        self.from_page.map_or_else(|| hint(&self.route), |(_, hint)| hint)
    }
}

/// Levels a page shows below its route's crumb, published by the page whenever they change.
struct PageTrail {
    route: Route,
    /// The tooltip of the route's own crumb while it leads to the page's top level.
    root_hint: &'static str,
    levels: Vec<(String, &'static str)>,
    /// Counts up with every change, so the bar sees one without comparing the levels.
    generation: u64,
}

fn trail_id() -> Id {
    Id::unique("top_bar_page_trail")
}

fn click_id() -> Id {
    Id::unique("top_bar_page_trail_click")
}

/// The page on `route` names levels below it, each with the tooltip of its link (the pipelines
/// tab: "Pipelines", the run, the job). The last one is the current page; the route's own crumb
/// (with `root_hint`) and the levels above the last become links reported by [`take_trail_click`].
/// Without levels, a `root_hint` alone makes the route's own crumb a link to the page's top level
/// (the files tab: back to the summary of all files).
pub fn publish_trail(
    ctx: &Context,
    route: &Route,
    root_hint: &'static str,
    levels: Vec<(String, &'static str)>,
) {
    let old = ctx.data(|data| data.get_temp::<Arc<PageTrail>>(trail_id()));
    let is_same = old.as_ref().is_some_and(|old| {
        old.route == *route && old.root_hint == root_hint && old.levels == levels
    });
    if is_same {
        return;
    }
    let generation = old.map_or(0, |old| old.generation + 1);
    let trail = PageTrail { route: route.clone(), root_hint, levels, generation };
    ctx.data_mut(|data| data.insert_temp(trail_id(), Arc::new(trail)));
    // The bar is drawn before the page; show the new trail in the next frame.
    ctx.request_repaint();
}

/// The page level clicked in the breadcrumb this frame: 0 for the route's own crumb, then the
/// published levels from 1.
pub fn take_trail_click(ctx: &Context) -> Option<usize> {
    mailbox::take::<usize>(ctx, click_id())
}

/// Breadcrumb trail: where you are and the way you came ([`Trail`]), every level above it one
/// click away. Going back and forward is the title bar's arrows, Alt+Left/Right or the mouse's
/// side buttons. The title bar gives it its room; a trail longer than that keeps its first and last
/// crumbs and the page's own route, cuts long names and folds the levels between into a "…" crumb
/// whose list leads to each of them ([`crumb_fit`]).
///
/// Measured and placed only when the trail, the page's levels, the connections, the scale or the
/// room change; a frame without changes paints the cached galleys at the cached positions.
#[derive(Default)]
pub struct Breadcrumbs {
    model: Option<Model>,
    placement: Option<Placement>,
    /// egui rebuilds the font atlas (and with it the glyphs the cached galleys point into) when it
    /// runs full or its options change; cached galleys would then paint fragments of other glyphs.
    atlas: AtlasWatch,
    /// Where the hidden levels' list was painted, for the title bar's hit test.
    popup_rect: Option<Rect>,
}

/// The trail's crumbs with their galleys, for one trail, page trail, set of connections and scale.
struct Model {
    trail: Trail,
    generation: Option<u64>,
    /// Each level's connection: its name, platform and web root the crumbs were made with.
    connections: Vec<Option<(String, ProviderKind, String)>>,
    pixels_per_point: f32,
    crumbs: Vec<Crumb>,
    /// The route's own crumb (the pull request, the list): of the middle crumbs, the last to fold.
    anchor: usize,
    separator: Arc<Galley>,
    ellipsis: Arc<Galley>,
    height: f32,
}

struct Crumb {
    level: Level,
    is_current: bool,
    /// Semibold for the app's name and the current page.
    font: FontId,
    /// Cut at [`CRUMB_MAX_WIDTH`], and at [`CRUMB_SHORT_WIDTH`] for a middle crumb.
    full: Arc<Galley>,
    short: Arc<Galley>,
}

impl Crumb {
    fn width(&self, galley: &Galley) -> f32 {
        galley.size().x + CRUMB_PADDING.x * 2.0 + self.level.mark_width()
    }
}

/// Where each shown crumb sits for one width of room, relative to the trail's left edge.
struct Placement {
    budget: f32,
    items: Vec<Item>,
    /// Indices of the crumbs folded into "…", in trail order.
    hidden: Vec<usize>,
    width: f32,
}

struct Item {
    x: f32,
    width: f32,
    kind: ItemKind,
}

enum ItemKind {
    Crumb { index: usize, galley: Arc<Galley> },
    Overflow,
}

impl Breadcrumbs {
    /// The hidden levels' list while it is open, for the title bar's hit test.
    pub fn popup_rect(&self) -> Option<Rect> {
        self.popup_rect
    }

    /// Lays the trail out in `area` (never past its right edge), vertically centered; returns the
    /// rect it covers.
    #[expect(clippy::float_cmp, reason = "a cache key: the same value again means nothing changed")]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        area: Rect,
        trail: &Trail,
        services: &Services,
        navigator: &mut Navigator,
    ) -> Rect {
        let ctx = ui.ctx().clone();
        self.refresh(&ctx, trail, services);
        let Some(model) = &self.model else {
            return Rect::NOTHING;
        };
        let budget = area.width().max(0.0);
        let kept = self.placement.take().filter(|placement| placement.budget == budget);
        let placement = &*self.placement.insert(kept.unwrap_or_else(|| place(&ctx, model, budget)));

        let palette = theme::palette(&ctx);
        let top = area.center().y - model.height / 2.0;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
        child.set_clip_rect(area.intersect(ui.clip_rect()));
        let mut picked = None;
        self.popup_rect = None;
        for (position, item) in placement.items.iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(area.left() + item.x, top),
                vec2(item.width, model.height),
            );
            if position > 0 {
                let separator = &model.separator;
                let pos = pos2(
                    rect.left() - SEPARATOR_GAP - separator.size().x,
                    rect.center().y - separator.size().y / 2.0,
                );
                child.painter().galley(pos, separator.clone(), palette.text_disabled);
            }
            match &item.kind {
                ItemKind::Crumb { index, galley } => {
                    if crumb_link(&mut child, crumb_id(*index), &model.crumbs[*index], galley, rect)
                    {
                        picked = Some(*index);
                    }
                }
                ItemKind::Overflow => {
                    let (list_rect, pick) = overflow(&mut child, rect, model, &placement.hidden);
                    self.popup_rect = list_rect;
                    picked = picked.or(pick);
                }
            }
        }
        if let Some(index) = picked {
            activate(&ctx, &model.crumbs[index].level, navigator);
        }
        Rect::from_min_size(pos2(area.left(), top), vec2(placement.width, model.height))
    }

    /// Every level shown, with the route it leads to, from the app's name down to the page.
    #[cfg(any(test, feature = "test-support"))]
    pub fn levels(&self) -> Vec<(String, Route)> {
        self.model.as_ref().map_or_else(Vec::new, |model| {
            model
                .crumbs
                .iter()
                .map(|crumb| (crumb.level.text.clone(), crumb.level.route.clone()))
                .collect()
        })
    }

    /// Measures the trail again when its levels, page levels, connections or scale changed, or
    /// when egui rebuilt the font atlas.
    #[expect(clippy::float_cmp, reason = "a cache key: the same value again means nothing changed")]
    fn refresh(&mut self, ctx: &Context, trail: &Trail, services: &Services) {
        let is_atlas_cleared = self.atlas.is_rebuilt(ctx);
        let page = ctx
            .data(|data| data.get_temp::<Arc<PageTrail>>(trail_id()))
            .filter(|page| page.route == *trail.route());
        let generation = page.as_ref().map(|page| page.generation);
        let pixels_per_point = ctx.pixels_per_point();
        let is_fresh = !is_atlas_cleared
            && self.model.as_ref().is_some_and(|model| {
                model.trail == *trail
                    && model.generation == generation
                    && model.pixels_per_point == pixels_per_point
                    && model
                        .connections
                        .iter()
                        .map(|stamp| {
                            stamp
                                .as_ref()
                                .map(|(name, kind, url)| (name.as_str(), *kind, url.as_str()))
                        })
                        .eq(connection_stamps(trail, services))
            });
        if is_fresh {
            return;
        }
        self.model = Some(Model::build(ctx, trail, services, page.as_deref()));
        self.placement = None;
    }
}

impl Model {
    fn build(ctx: &Context, trail: &Trail, services: &Services, page: Option<&PageTrail>) -> Self {
        let route = trail.route();
        let mut levels = levels(trail, services);
        let anchor = levels.len() - 1;
        if let Some(page) =
            page.filter(|page| !page.levels.is_empty() || !page.root_hint.is_empty())
        {
            levels[anchor].from_page = Some((0, page.root_hint));
            for (index, (text, hint)) in page.levels.iter().enumerate() {
                levels.push(Level {
                    text: text.clone(),
                    route: route.clone(),
                    is_root: false,
                    platform: None,
                    from_page: Some((index + 1, hint)),
                });
            }
        }
        let last = levels.len() - 1;
        let crumbs: Vec<Crumb> = ctx.fonts_mut(|fonts| {
            levels
                .into_iter()
                .enumerate()
                .map(|(index, level)| {
                    // The app name on the pull request list is that page itself, not a link to it;
                    // the route's own crumb that leads to the page's top level is a link, even last.
                    let leads_up = matches!(level.from_page, Some((0, _)));
                    let is_current =
                        (index == last && !leads_up) || (index == 0 && level.route == *route);
                    let font = if level.is_root || is_current {
                        theme::fonts::semibold_font(style::BODY_SIZE)
                    } else {
                        FontId::proportional(style::BODY_SIZE)
                    };
                    let full =
                        fonts.layout_job(cut_job(&level.text, font.clone(), CRUMB_MAX_WIDTH));
                    let is_middle = index > 0 && index < last;
                    let short = if is_middle && full.size().x > CRUMB_SHORT_WIDTH {
                        fonts.layout_job(cut_job(&level.text, font.clone(), CRUMB_SHORT_WIDTH))
                    } else {
                        full.clone()
                    };
                    Crumb { level, is_current, font, full, short }
                })
                .collect()
        });
        let (separator, ellipsis) = ctx.fonts_mut(|fonts| {
            let separator = fonts.layout_job(LayoutJob::simple_singleline(
                icons::CARET_RIGHT.to_owned(),
                FontId::proportional(SEPARATOR_SIZE),
                Color32::PLACEHOLDER,
            ));
            let ellipsis = fonts.layout_job(LayoutJob::simple_singleline(
                "\u{2026}".to_owned(),
                FontId::proportional(style::BODY_SIZE),
                Color32::PLACEHOLDER,
            ));
            (separator, ellipsis)
        });
        let text_height =
            crumbs.iter().map(|crumb| crumb.full.size().y).fold(ellipsis.size().y, f32::max);
        Self {
            trail: trail.clone(),
            generation: page.map(|page| page.generation),
            connections: connection_stamps(trail, services)
                .map(|stamp| stamp.map(|(name, kind, url)| (name.to_owned(), kind, url.to_owned())))
                .collect(),
            pixels_per_point: ctx.pixels_per_point(),
            crumbs,
            anchor,
            separator,
            ellipsis,
            height: text_height + CRUMB_PADDING.y * 2.0,
        }
    }

    fn spacing(&self) -> Spacing {
        Spacing {
            separator: self.separator.size().x + SEPARATOR_GAP * 2.0,
            overflow: self.overflow_width(),
        }
    }

    fn overflow_width(&self) -> f32 {
        (self.ellipsis.size().x + CRUMB_PADDING.x * 2.0).max(OVERFLOW_MIN_WIDTH)
    }
}

/// A single line of `text` in `font`, cut with "…" past `max_width`. Uncolored, so the painter
/// gives it the crumb's color of the moment and the galley serves both themes.
fn cut_job(text: &str, font: FontId, max_width: f32) -> LayoutJob {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, Color32::PLACEHOLDER);
    job.wrap = TextWrapping::truncate_at_width(max_width);
    job
}

/// Places the crumbs for `budget` points of room.
#[expect(clippy::float_cmp, reason = "the width is one of the measured widths, taken unchanged")]
fn place(ctx: &Context, model: &Model, budget: f32) -> Placement {
    let widths: Vec<CrumbWidth> = model
        .crumbs
        .iter()
        .map(|crumb| CrumbWidth {
            full: crumb.width(&crumb.full),
            short: crumb.width(&crumb.short),
        })
        .collect();
    let spacing = model.spacing();
    let shown = crumb_fit::fit(&widths, model.anchor, spacing, budget);
    let mut items = Vec::with_capacity(model.crumbs.len());
    let mut hidden = Vec::new();
    let mut x = 0.0;
    for (index, (crumb, (shown, measured))) in
        model.crumbs.iter().zip(shown.iter().zip(&widths)).enumerate()
    {
        let Some(shown) = *shown else {
            if hidden.is_empty() {
                items.push(Item { x, width: spacing.overflow, kind: ItemKind::Overflow });
                x += spacing.overflow + spacing.separator;
            }
            hidden.push(index);
            continue;
        };
        let galley = if shown == measured.full {
            crumb.full.clone()
        } else if shown == measured.short {
            crumb.short.clone()
        } else {
            let text_width = (shown - CRUMB_PADDING.x * 2.0 - crumb.level.mark_width()).max(0.0);
            ctx.fonts_mut(|fonts| {
                fonts.layout_job(cut_job(&crumb.level.text, crumb.font.clone(), text_width))
            })
        };
        let width = crumb.width(&galley);
        items.push(Item { x, width, kind: ItemKind::Crumb { index, galley } });
        x += width + spacing.separator;
    }
    let width = if items.is_empty() { 0.0 } else { x - spacing.separator };
    Placement { budget, items, hidden, width }
}

fn crumb_id(index: usize) -> Id {
    Id::unique("title_bar_crumb").with(index)
}

fn overflow_id() -> Id {
    Id::unique("title_bar_crumb_overflow")
}

/// Goes where a crumb leads: its route, or a level of the page (reported by [`take_trail_click`]).
fn activate(ctx: &Context, level: &Level, navigator: &mut Navigator) {
    match level.from_page {
        Some((page_level, _)) => mailbox::post(ctx, click_id(), page_level),
        None => navigator.go(level.route.clone()),
    }
}

/// One crumb at `rect`. Levels above the current page are links with an eased hover, a pressed
/// tint and a keyboard focus ring; the current page is plain text (sensing drags, no clicks). Held
/// still, any crumb moves the window instead ([`platform::long_press_moves_window`]), unlit.
fn crumb_link(ui: &mut Ui, id: Id, crumb: &Crumb, galley: &Arc<Galley>, rect: Rect) -> bool {
    let palette = theme::palette(ui.ctx());
    let level = &crumb.level;
    let is_cut = galley.elided;
    let sense = if crumb.is_current { Sense::DRAG } else { Sense::click() };
    let response = ui.interact(rect, id, sense);
    let is_moving_window = platform::long_press_moves_window(&response);
    let text_pos = pos2(
        rect.left() + CRUMB_PADDING.x + level.mark_width(),
        rect.center().y - galley.size().y / 2.0,
    );
    let paint_mark = |ui: &Ui| {
        if let Some((kind, _)) = &level.platform {
            let center = pos2(rect.left() + CRUMB_PADDING.x + MARK_SIZE / 2.0, rect.center().y);
            provider_icon::paint(ui.painter(), &palette, center, MARK_SIZE, *kind);
        }
    };
    if crumb.is_current {
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Label,
                true,
                format!("{} (current page)", level.text),
            )
        });
        paint_mark(ui);
        ui.painter().galley(text_pos, galley.clone(), palette.text);
        if is_cut || level.platform.is_some() {
            response.on_hover_ui(|ui| tooltip(ui, level, is_cut, None));
        }
        return false;
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &level.text));
    let hover = row::hover_amount(ui, &response);
    let rest = if level.is_root { palette.text } else { palette.text_secondary };
    let color = if is_moving_window {
        rest
    } else {
        row::card_background(ui, &response, rect, Color32::TRANSPARENT, style::RADIUS_CONTROL);
        motion::mix(rest, palette.accent_text, hover)
    };
    paint_mark(ui);
    ui.painter().galley(text_pos, galley.clone(), color);
    focus::ring(ui, &response, rect);
    let response = response.on_hover_ui(|ui| tooltip(ui, level, is_cut, Some(level.hint())));
    response.clicked() && !is_moving_window
}

/// Full name when cut, the platform and host, then where the crumb leads; built only while hovered.
fn tooltip(ui: &mut Ui, level: &Level, is_cut: bool, hint: Option<&str>) {
    if is_cut {
        ui.label(&level.text);
    }
    if let Some((kind, url)) = &level.platform {
        provider_icon::tooltip_line(ui, *kind, url);
    }
    if let Some(hint) = hint {
        ui.label(hint);
    }
}

/// The "…" crumb and, while open, the list of the levels it hides right below it: the first row
/// under the pointer, arrow keys between rows, Enter goes, Esc closes and returns to "…". Returns
/// the open list's rect and the level picked in it.
fn overflow(
    ui: &mut Ui,
    rect: Rect,
    model: &Model,
    hidden: &[usize],
) -> (Option<Rect>, Option<usize>) {
    let ctx = ui.ctx().clone();
    let palette = theme::palette(&ctx);
    let response = ui.interact(rect, overflow_id(), Sense::click());
    let popup_id = Popup::default_response_id(&response);
    let was_open = Popup::is_id_open(&ctx, popup_id);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, true, was_open, OVERFLOW_LABEL)
    });
    let hover = row::hover_amount(ui, &response);
    let rest = if was_open { palette.hover } else { Color32::TRANSPARENT };
    row::card_background(ui, &response, rect, rest, style::RADIUS_CONTROL);
    let color = motion::mix(palette.text_secondary, palette.accent_text, hover);
    let ellipsis = &model.ellipsis;
    ui.painter().galley(rect.center() - ellipsis.size() / 2.0, ellipsis.clone(), color);
    focus::ring(ui, &response, rect);
    let response = if was_open {
        response
    } else {
        response.on_hover_ui(|ui| {
            ui.label(format!("{} more levels", hidden.len()));
        })
    };

    let focus_first = response.clicked() && !was_open && focus::is_visible(&ctx);
    let shown = Popup::menu(&response)
        .gap(style::SPACE_XS)
        .accessible_name(HIDDEN_LIST_LABEL)
        .show(|ui| hidden_list(ui, model, hidden, focus_first));
    if was_open
        && !Popup::is_id_open(&ctx, popup_id)
        && ctx.input(|input| input.key_pressed(Key::Escape))
    {
        response.request_focus();
    }
    match shown {
        Some(shown) => (Some(shown.response.rect), shown.inner),
        None => (None, None),
    }
}

/// The hidden levels in trail order; a line parts the ones above a shown crumb (the pull request)
/// from the ones below it.
fn hidden_list(ui: &mut Ui, model: &Model, hidden: &[usize], focus_first: bool) -> Option<usize> {
    ui.set_min_width(ROW_MIN_WIDTH);
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut picked = None;
    let mut previous: Option<usize> = None;
    for &index in hidden {
        if previous.is_some_and(|previous| previous + 1 != index) {
            ui.separator();
        }
        let response = hidden_row(ui, &model.crumbs[index].level);
        if focus_first && previous.is_none() {
            response.request_focus();
        }
        if response.clicked() {
            picked = Some(index);
        }
        previous = Some(index);
    }
    picked
}

fn hidden_row(ui: &mut Ui, level: &Level) -> Response {
    let palette = theme::palette(ui.ctx());
    let galley = ui.fonts_mut(|fonts| {
        fonts.layout_job(cut_job(
            &level.text,
            FontId::proportional(style::BODY_SIZE),
            ROW_TEXT_MAX_WIDTH,
        ))
    });
    let mark_width = level.mark_width();
    let min_width = style::SPACE_S * 2.0 + mark_width + galley.size().x;
    let (rect, response) = ui
        .allocate_exact_size(vec2(ui.available_width().max(min_width), ROW_HEIGHT), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::MenuItem, true, &level.text));
    let fill = row::state_fill(ui, &response, Color32::TRANSPARENT);
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
    }
    let left = rect.left() + style::SPACE_S;
    if let Some((kind, _)) = &level.platform {
        provider_icon::paint(
            ui.painter(),
            &palette,
            pos2(left + MARK_SIZE / 2.0, rect.center().y),
            MARK_SIZE,
            *kind,
        );
    }
    let is_cut = galley.elided;
    ui.painter().galley(
        pos2(left + mark_width, rect.center().y - galley.size().y / 2.0),
        galley,
        palette.text,
    );
    focus::ring(ui, &response, rect);
    response.on_hover_ui(|ui| tooltip(ui, level, is_cut, Some(level.hint())))
}

fn hint(route: &Route) -> &'static str {
    match route {
        Route::MyPullRequests => "Your pull requests",
        Route::Repositories => "All repositories",
        Route::Connection { .. } => "This connection's repositories",
        Route::RepositoryPullRequests { .. } => "This repository's pull requests",
        Route::PullRequest(_) => "Back to this pull request",
        Route::Settings => "Settings",
    }
}

/// What the trail shows of each level's connection: its name, platform and web root.
fn connection_stamps<'a>(
    trail: &'a Trail,
    services: &'a Services,
) -> impl Iterator<Item = Option<(&'a str, ProviderKind, &'a str)>> {
    trail.levels().map(|level| {
        let connection = services.connection(level.connection_id()?)?;
        Some((connection.name.as_str(), connection.kind, connection.base_url()))
    })
}

/// The app's name, the root that leads to the pull request list, then the trail's levels.
fn levels(trail: &Trail, services: &Services) -> Vec<Level> {
    let root = Level {
        text: review_core::APP_NAME.to_owned(),
        route: Route::MyPullRequests,
        is_root: true,
        platform: None,
        from_page: None,
    };
    let below = trail.levels().map(|level| Level {
        text: crumb_text(level, services),
        platform: match level {
            Route::Connection { connection_id } => services
                .connection(connection_id)
                .map(|connection| (connection.kind, connection.base_url().to_owned())),
            _ => None,
        },
        route: level.clone(),
        is_root: false,
        from_page: None,
    });
    std::iter::once(root).chain(below).collect()
}

/// The name of `route`'s page, as its crumb and the title bar's arrows show it.
pub fn crumb_text(route: &Route, services: &Services) -> String {
    let connection_name = |id: &str| {
        services.connection(id).map_or_else(|| id.to_owned(), |connection| connection.name.clone())
    };
    match route {
        Route::MyPullRequests => "My pull requests".to_owned(),
        Route::Repositories => "Repositories".to_owned(),
        Route::Connection { connection_id } => connection_name(connection_id),
        Route::RepositoryPullRequests { repository_key, .. } => repository_key.clone(),
        Route::PullRequest(key) => {
            let prefix = services
                .connection(&key.connection_id)
                .map_or("#", |connection| number_prefix(connection.kind));
            format!("Pull request {prefix}{}", key.number)
        }
        Route::Settings => "Settings".to_owned(),
    }
}
