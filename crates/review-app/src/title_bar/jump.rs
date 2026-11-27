//! The title bar's search box (Ctrl+K, or the search button of a list's header): jumps to a pull
//! request from the lists loaded so far, by number, title, author, repository or label. Every
//! match is listed (the results scroll), a pull request URL opens that pull request, a number in a
//! repository opens it there, and a search that finds nothing offers the full list, so it never
//! ends without a way forward.

use cv_base::platform::shortcut;
use cv_base::theme::{self, Palette, icons, style};
use cv_lists::pull_request_list::{LoadedPullRequest, PullRequestListPage};
use cv_runtime::intents;
use cv_runtime::route::{PullRequestKey, Route};
use cv_services::Services;
use cv_shell::focus_mode;
use cv_widgets::numbers::number_prefix;
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align2, Color32, FontId, Id, Key, Modifiers, Order, Rect, Sense, Stroke, StrokeKind, TextEdit,
    Ui, pos2, vec2,
};
use review_core::PullRequestSummary;

pub const BOX_HEIGHT: f32 = 28.0;
pub const MAX_WIDTH: f32 = 480.0;
pub const MIN_WIDTH: f32 = 200.0;
/// Rows the results show at most before they scroll; fewer where the window ends sooner.
const MAX_VISIBLE_RESULTS: usize = 12;
const ROW_HEIGHT: f32 = 32.0;
/// The results' frame: inner margin above and below the rows, and its hairline.
const POPUP_MARGIN: i8 = 4;
const POPUP_STROKE: f32 = 1.0;
const NUMBER_WIDTH: f32 = 56.0;
const PADDING: f32 = 10.0;
const TEXT_NUDGE: f32 = 1.5;
const LABEL: &str = "Search pull requests";
const HINT: &str = "Search pull requests…";
const SHORT_HINT: &str = "Search…";
/// The key caps painted in the idle box, and the shortcut they spell.
const SHORTCUT_CAPS: [&str; 2] = [shortcut!(command), "K"];
const SHORTCUT: &str = shortcut!(command "K");
const FOCUS_COMMAND: &str = "Toggle focus mode";

/// What picking a result asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum JumpAction {
    /// A pull request. `listed` is its loaded list row, which the page's header paints until the
    /// detail is in; none for a number typed in a repository whose row no list loaded.
    Open { key: PullRequestKey, listed: Option<Box<PullRequestSummary>> },
    /// A pasted pull request URL.
    Link(String),
    /// Show the list of the user's pull requests.
    ShowList,
    /// Switch focus mode (the diff or the log takes over the window).
    ToggleFocus,
}

#[derive(Default)]
pub struct JumpBox {
    query: String,
    /// The highlighted result.
    selected: usize,
    /// The query `selected` belongs to.
    selected_for: String,
    /// Where the results were painted last frame; a press there keeps them open.
    popup_rect: Option<Rect>,
    /// The box had focus last frame.
    had_focus: bool,
}

/// One result row.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    /// "#42", empty for actions.
    badge: String,
    title: String,
    detail: String,
    action: JumpAction,
}

impl JumpBox {
    pub fn id() -> Id {
        Id::unique("title_bar_jump")
    }

    /// The open results, for the title bar's hit test (they start inside the caption).
    pub fn popup_rect(&self) -> Option<Rect> {
        self.popup_rect
    }

    /// Takes Ctrl+K and [`intents::open_search`] requests, and Esc; true while the box is open
    /// (has focus), so the bar can make room for it even where it is left out at rest. Runs every
    /// frame.
    pub fn take_open_request(&mut self, ctx: &egui::Context) -> bool {
        let id = Self::id();
        // Esc closes the box and drops its text. egui takes focus away on Esc before any widget
        // runs, so a box that had focus last frame counts as open here.
        let has_focus = ctx.memory(|memory| memory.has_focus(id));
        if (has_focus || self.had_focus)
            && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.query.clear();
            ctx.memory_mut(|memory| memory.surrender_focus(id));
        }
        let is_requested = intents::take_search_request(ctx);
        // Ctrl+K from anywhere but another text field (the comment editor's Ctrl+K makes a link).
        let is_shortcut = !ctx.text_edit_focused()
            && ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::K));
        if is_requested || is_shortcut {
            ctx.memory_mut(|memory| memory.request_focus(id));
        }
        self.had_focus = ctx.memory(|memory| memory.has_focus(id));
        self.had_focus
    }

    /// Paints the box into `rect` and its results below while it has focus. Call
    /// [`Self::take_open_request`] first in the frame.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        lists: &[&PullRequestListPage],
        route: &Route,
        services: &Services,
    ) -> Option<JumpAction> {
        let ctx = ui.ctx().clone();
        let id = Self::id();
        let has_focus = ctx.memory(|memory| memory.has_focus(id));
        let (mut wants_open, mut moves) = (false, 0_i32);
        if has_focus {
            ctx.input_mut(|input| {
                moves += i32::from(input.consume_key(Modifiers::NONE, Key::ArrowDown));
                moves -= i32::from(input.consume_key(Modifiers::NONE, Key::ArrowUp));
                wants_open = input.consume_key(Modifiers::NONE, Key::Enter);
            });
        }

        let palette = theme::palette(&ctx);
        self.paint_box(ui, rect, has_focus, &palette);

        let is_held = self.popup_rect.is_some_and(|popup| {
            ctx.input(|input| {
                input.pointer.any_down()
                    && input.pointer.interact_pos().is_some_and(|pos| popup.contains(pos))
            })
        });
        if !(has_focus || is_held) {
            self.popup_rect = None;
            return None;
        }
        let entries = entries(&self.query, lists, route, services, focus_mode::is_available(&ctx));
        // Every opening starts at the first result, scrolled to the top, whatever was highlighted
        // or scrolled to when the results closed.
        let is_opening = self.popup_rect.is_none();
        if is_opening || self.selected_for != self.query {
            self.selected = 0;
            self.selected_for.clone_from(&self.query);
        }
        if !entries.is_empty() {
            self.selected =
                (self.selected as i32 + moves).rem_euclid(entries.len() as i32) as usize;
        }
        let entered = wants_open
            .then(|| entries.get(self.selected).map(|entry| entry.action.clone()))
            .flatten();
        let clicked = self.show_results(&ctx, rect, &entries, is_opening, moves, &palette);
        let picked = clicked.or(entered);
        if picked.is_some() {
            self.query.clear();
            self.popup_rect = None;
            ctx.memory_mut(|memory| memory.surrender_focus(id));
        }
        picked
    }

    /// The box itself: frame, magnifier, the text field and, at rest, the shortcut's key caps.
    fn paint_box(&mut self, ui: &mut Ui, rect: Rect, has_focus: bool, palette: &Palette) {
        let hovered = ui.rect_contains_pointer(rect);
        let border = if has_focus {
            Stroke::new(2.0, palette.accent)
        } else {
            Stroke::new(1.0, if hovered { palette.border_strong } else { palette.border })
        };
        let fill = if has_focus { palette.surface } else { palette.surface_subtle };
        ui.painter().rect(rect, style::RADIUS_CONTROL, fill, border, StrokeKind::Inside);
        let icon_center = pos2(rect.left() + PADDING + 7.0, rect.center().y);
        ui.painter().text(
            icon_center,
            Align2::CENTER_CENTER,
            icons::MAGNIFYING_GLASS,
            icons::rest_font(palette, icons::SIZE_SMALL),
            palette.text_secondary,
        );
        let shortcut_width =
            if has_focus || !self.query.is_empty() { 0.0 } else { paint_shortcut(ui, rect) };
        // Segoe UI's line box sits low; a point and a half up centers the text on the icon.
        let text_rect = Rect::from_min_max(
            pos2(icon_center.x + 12.0, rect.top()),
            pos2(rect.right() - PADDING - shortcut_width, rect.bottom()),
        )
        .translate(vec2(0.0, -TEXT_NUDGE));
        // A narrow window gets the short hint instead of a cut one.
        let hint_width = ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(
                    HINT.to_owned(),
                    egui::FontId::proportional(style::BODY_SIZE),
                    palette.text,
                )
                .size()
                .x
        });
        let hint = if hint_width <= text_rect.width() { HINT } else { SHORT_HINT };
        let edit = TextEdit::singleline(&mut self.query)
            .id(Self::id())
            .frame(egui::Frame::NONE)
            .hint_text(hint)
            .desired_width(text_rect.width())
            .return_key(None)
            .align(egui::Align2::LEFT_CENTER);
        let response = ui.put(text_rect, edit);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::SearchInput,
                true,
                format!("{LABEL} ({SHORTCUT})"),
            )
        });
    }

    /// The results below the box; the one clicked, if any.
    fn show_results(
        &mut self,
        ctx: &egui::Context,
        rect: Rect,
        entries: &[Entry],
        is_opening: bool,
        moves: i32,
        palette: &Palette,
    ) -> Option<JumpAction> {
        let top = rect.bottom() + style::SPACE_XS;
        let height =
            visible_rows(entries.len(), ctx.content_rect().bottom() - top) as f32 * ROW_HEIGHT;
        let popup = egui::Area::new(Self::id().with("results"))
            .order(Order::Foreground)
            .fixed_pos(pos2(rect.left(), top))
            // The height already fits the window; constraining by the size remembered from the
            // last opening would push the results up over the box.
            .constrain(false)
            .show(ctx, |ui| {
                popup_frame(palette)
                    .show(ui, |ui| {
                        ui.set_width(rect.width() - 2.0);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        // While results are scrolled out of view the bar shows at rest too.
                        let scroll = &mut ui.spacing_mut().scroll;
                        scroll.dormant_handle_opacity = scroll.active_handle_opacity;
                        // Every match is listed; past a screenful the results scroll, and the
                        // arrow keys keep the highlighted one in view. The height is set for this
                        // frame's results: an area hands its content the size it had last frame,
                        // so a height left to the space available would stay at whatever the
                        // results first opened with (two rows after an opening with one).
                        let mut results = egui::ScrollArea::vertical()
                            .id_salt("title_bar_jump_results")
                            .min_scrolled_height(height)
                            .max_height(height);
                        if is_opening {
                            results = results.vertical_scroll_offset(0.0);
                        }
                        results.show(ui, |ui| self.rows(ui, entries, moves)).inner
                    })
                    .inner
            });
        self.popup_rect = Some(popup.response.rect);
        popup.inner
    }

    /// The result rows: a moving pointer highlights, the arrow keys scroll the highlighted one into
    /// view; the one clicked, if any.
    fn rows(&mut self, ui: &mut Ui, entries: &[Entry], moves: i32) -> Option<JumpAction> {
        let is_pointer_moving = ui.ctx().input(|input| input.pointer.delta() != egui::Vec2::ZERO);
        let mut clicked = None;
        for (index, entry) in entries.iter().enumerate() {
            let row = result_row(ui, entry, index == self.selected);
            if row.hovered() && is_pointer_moving {
                self.selected = index;
            }
            if moves != 0 && index == self.selected {
                row.scroll_to_me(None);
            }
            if row.clicked() {
                clicked = Some(entry.action.clone());
            }
        }
        clicked
    }
}

/// The "Ctrl K" (macOS: "⌘ K") key caps at the right end of the idle box; returns their width.
fn paint_shortcut(ui: &Ui, rect: Rect) -> f32 {
    let palette = theme::palette(ui.ctx());
    let font = FontId::proportional(style::SMALL_SIZE - 1.0);
    let mut right = rect.right() - PADDING + 4.0;
    for key in SHORTCUT_CAPS.iter().rev() {
        let galley =
            ui.painter().layout_no_wrap((*key).to_owned(), font.clone(), palette.text_secondary);
        let cap = Rect::from_min_max(
            pos2(right - galley.size().x - 10.0, rect.center().y - 9.0),
            pos2(right, rect.center().y + 9.0),
        );
        ui.painter().rect(
            cap,
            3,
            palette.surface,
            Stroke::new(1.0, palette.border),
            StrokeKind::Inside,
        );
        ui.painter().galley(cap.center() - galley.size() / 2.0, galley, palette.text_secondary);
        right = cap.left() - 3.0;
    }
    rect.right() - PADDING - right
}

/// Height the results' frame adds around the rows.
fn popup_chrome() -> f32 {
    2.0 * (f32::from(POPUP_MARGIN) + POPUP_STROKE)
}

/// How many of `count` results show at once: all of them up to [`MAX_VISIBLE_RESULTS`], and no
/// more than fit in `room` (from the results' top to the window's bottom, which keeps a small gap).
fn visible_rows(count: usize, room: f32) -> usize {
    let fitting = ((room - popup_chrome() - style::SPACE_S) / ROW_HEIGHT).floor().max(1.0) as usize;
    count.min(fitting).min(MAX_VISIBLE_RESULTS)
}

fn popup_frame(palette: &theme::Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(POPUP_STROKE, palette.border))
        .corner_radius(style::RADIUS_POPUP)
        .shadow(style::popup_shadow(palette))
        .inner_margin(egui::Margin::symmetric(0, POPUP_MARGIN))
}

fn result_row(ui: &mut Ui, entry: &Entry, is_selected: bool) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let name = if entry.badge.is_empty() {
        entry.title.clone()
    } else {
        format!("{} {}", entry.badge, entry.title)
    };
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, is_selected, &name));
    let fill = if response.is_pointer_button_down_on() {
        palette.pressed
    } else if is_selected {
        palette.hover
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect.shrink2(vec2(4.0, 1.0)), style::RADIUS_CONTROL, fill);
    }
    let left = rect.left() + PADDING + 2.0;
    let body = FontId::proportional(style::BODY_SIZE);
    let small = FontId::proportional(style::SMALL_SIZE);
    let title_left = if entry.badge.is_empty() {
        ui.painter().text(
            pos2(left + 7.0, rect.center().y),
            Align2::CENTER_CENTER,
            icons::ARROW_RIGHT,
            icons::rest_font(&palette, icons::SIZE_SMALL),
            palette.accent_text,
        );
        left + 22.0
    } else {
        ui.painter().text(
            pos2(left, rect.center().y),
            Align2::LEFT_CENTER,
            &entry.badge,
            small.clone(),
            palette.text_secondary,
        );
        left + NUMBER_WIDTH
    };
    let detail_width = if entry.detail.is_empty() {
        0.0
    } else {
        let mut job =
            LayoutJob::simple_singleline(entry.detail.clone(), small, palette.text_secondary);
        job.wrap = TextWrapping::truncate_at_width(rect.width() * 0.4);
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let width = galley.size().x;
        ui.painter().galley(
            pos2(rect.right() - PADDING - width, rect.center().y - galley.size().y / 2.0),
            galley,
            palette.text_secondary,
        );
        width + style::SPACE_M
    };
    let title_color = if entry.badge.is_empty() { palette.accent_text } else { palette.text };
    let mut job = LayoutJob::simple_singleline(entry.title.clone(), body, title_color);
    job.wrap = TextWrapping::truncate_at_width(
        (rect.right() - PADDING - detail_width - title_left).max(0.0),
    );
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.painter().galley(
        pos2(title_left, rect.center().y - galley.size().y / 2.0),
        galley,
        title_color,
    );
    response
}

/// The results for `query`: loaded pull requests that match (exact number first, then titles
/// that start with the text), then what can still be done with the text. "Toggle focus mode"
/// leads while `can_focus` (a view that can take over the window is on screen) and the text names it.
fn entries(
    query: &str,
    lists: &[&PullRequestListPage],
    route: &Route,
    services: &Services,
    can_focus: bool,
) -> Vec<Entry> {
    let query = query.trim();
    if query.starts_with("https://") || query.starts_with("http://") {
        return vec![action("Open this link", JumpAction::Link(query.to_owned()))];
    }
    let mut found: Vec<LoadedPullRequest<'_>> = Vec::new();
    for list in lists {
        for loaded in list.loaded_matching(query) {
            let is_known = found.iter().any(|other| {
                other.connection_id == loaded.connection_id
                    && other.pull_request.repository_key == loaded.pull_request.repository_key
                    && other.pull_request.number == loaded.pull_request.number
            });
            if !is_known {
                found.push(loaded);
            }
        }
    }
    let number = query.trim_start_matches(['#', '!']).parse::<u64>().ok();
    let lower = query.to_lowercase();
    // Stable sort: list order (newest first) within each rank.
    found.sort_by_key(|loaded| {
        if Some(loaded.pull_request.number) == number {
            0
        } else if loaded.pull_request.title.to_lowercase().starts_with(&lower) {
            1
        } else {
            2
        }
    });
    let mut entries: Vec<Entry> = found.iter().map(loaded_entry).collect();
    if let (Some(number), Some((connection_id, repository_key))) = (number, repository_of(route)) {
        let is_listed = found.iter().any(|loaded| {
            loaded.pull_request.number == number
                && loaded.pull_request.repository_key == repository_key
        });
        if !is_listed {
            let prefix = services
                .connection(connection_id)
                .map_or("#", |connection| number_prefix(connection.kind));
            let key = PullRequestKey {
                connection_id: connection_id.to_owned(),
                repository_key: repository_key.to_owned(),
                number,
            };
            entries.insert(
                0,
                action(
                    &format!("Open pull request {prefix}{number} in {repository_key}"),
                    JumpAction::Open { key, listed: None },
                ),
            );
        }
    }
    if can_focus && lower.chars().count() >= 2 && FOCUS_COMMAND.to_lowercase().contains(&lower) {
        let command = Entry {
            detail: focus_mode::SHORTCUT.to_owned(),
            ..action(FOCUS_COMMAND, JumpAction::ToggleFocus)
        };
        entries.insert(0, command);
    }
    if entries.is_empty() {
        let (title, detail) = if query.is_empty() {
            ("Show your pull requests", String::new())
        } else {
            ("Show all pull requests", format!("Nothing loaded matches \u{201C}{query}\u{201D}"))
        };
        entries.push(Entry { detail, ..action(title, JumpAction::ShowList) });
    }
    entries
}

/// A loaded pull request's result: opening it hands its row to the page.
fn loaded_entry(loaded: &LoadedPullRequest<'_>) -> Entry {
    let pull_request = loaded.pull_request;
    Entry {
        badge: format!("{}{}", number_prefix(loaded.kind), pull_request.number),
        title: pull_request.title.clone(),
        detail: pull_request.repository_key.clone(),
        action: JumpAction::Open {
            key: PullRequestKey {
                connection_id: loaded.connection_id.to_owned(),
                repository_key: pull_request.repository_key.clone(),
                number: pull_request.number,
            },
            listed: Some(Box::new(pull_request.clone())),
        },
    }
}

fn action(title: &str, action: JumpAction) -> Entry {
    Entry { badge: String::new(), title: title.to_owned(), detail: String::new(), action }
}

/// The repository the current page belongs to: (connection id, repository key).
fn repository_of(route: &Route) -> Option<(&str, &str)> {
    match route {
        Route::RepositoryPullRequests { connection_id, repository_key } => {
            Some((connection_id, repository_key))
        }
        Route::PullRequest(key) => Some((&key.connection_id, &key.repository_key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use egui::accesskit::Toggled;
    use egui::{Key, Rect, Role, pos2, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use review_core::{PullRequestState, PullRequestSummary, UserIdentity};
    use review_store::ThemePreference;

    use cv_base::theme::{self, style};
    use cv_lists::pull_request_list::PullRequestListPage;
    use cv_runtime::intents::open_search;
    use cv_runtime::route::Route;
    use cv_services::Services;
    use cv_testkit::ScratchDir;

    use super::{BOX_HEIGHT, JumpAction, JumpBox, MAX_VISIBLE_RESULTS, ROW_HEIGHT, popup_chrome};

    struct Scene {
        jump: JumpBox,
        page: PullRequestListPage,
        services: Services,
        picked: Option<JumpAction>,
        /// The services' data folders; declared last so it drops after them.
        _scratch: ScratchDir,
    }

    fn box_rect() -> Rect {
        Rect::from_min_size(pos2(120.0, 6.0), vec2(420.0, BOX_HEIGHT))
    }

    fn row(number: u64) -> PullRequestSummary {
        PullRequestSummary {
            repository_key: "acme/shop".into(),
            repository_name: "shop".into(),
            number,
            title: format!("Change {number}"),
            author: UserIdentity::default(),
            state: PullRequestState::Open,
            is_draft: false,
            source_branch: "feature".into(),
            target_branch: "main".into(),
            created_at: time::macros::datetime!(2026-10-16 09:30 UTC),
            updated_at: None,
            web_url: String::new(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            checks: None,
            diff_stat: review_core::DiffStat::default(),
            head_sha: None,
            target_sha: None,
        }
    }

    /// `count` loaded rows, newest (highest number) first like the lists.
    fn page(count: u64) -> PullRequestListPage {
        PullRequestListPage::loaded_for_test(
            "gh",
            (0..count).map(|index| row(500 - index)).collect(),
        )
    }

    fn scene(name: &str, rows: u64, height: f32) -> Harness<'static, Scene> {
        let scratch = ScratchDir::new(&format!("jump-{name}"));
        let services = Services::new(
            egui::Context::default(),
            scratch.app_paths(),
            cv_providers::create_provider,
        );
        let scene = Scene {
            jump: JumpBox::default(),
            page: page(rows),
            services,
            picked: None,
            _scratch: scratch,
        };
        let builder = Harness::builder()
            .with_size([900.0, height])
            .with_max_steps(240)
            .with_context_setup(theme::fonts::install);
        let mut harness = builder.build_ui_state(
            |ui, scene: &mut Scene| {
                theme::activate(ui.ctx(), ThemePreference::Light);
                scene.jump.take_open_request(ui.ctx());
                let lists = [&scene.page];
                if let Some(action) =
                    scene.jump.show(ui, box_rect(), &lists, &Route::MyPullRequests, &scene.services)
                {
                    scene.picked = Some(action);
                }
            },
            scene,
        );
        harness.run();
        harness
    }

    fn open_box(harness: &mut Harness<'_, Scene>) {
        open_search(&harness.ctx);
        harness.run();
        assert!(harness.state().jump.popup_rect().is_some(), "the results are open");
    }

    fn close_box(harness: &mut Harness<'_, Scene>) {
        harness.key_press(Key::Escape);
        harness.run();
        assert!(harness.state().jump.popup_rect().is_none(), "Esc closed the results");
    }

    /// The result rows, in order: (label, rect, is selected).
    fn rows(harness: &Harness<'_, Scene>) -> Vec<(String, Rect, bool)> {
        harness
            .query_all_by_role(Role::Button)
            .filter_map(|node| {
                let label = node.accesskit_node().label()?;
                // egui reports a selected row as toggled on.
                label.starts_with('#').then(|| {
                    (label, node.rect(), node.accesskit_node().toggled() == Some(Toggled::True))
                })
            })
            .collect()
    }

    /// Labels of the rows that show whole inside the results.
    fn visible(harness: &Harness<'_, Scene>) -> Vec<String> {
        let popup = harness.state().jump.popup_rect().expect("the results are open");
        rows(harness)
            .into_iter()
            .filter(|(_, rect, _)| popup.expand(0.5).contains_rect(*rect))
            .map(|(label, ..)| label)
            .collect()
    }

    fn selected(harness: &Harness<'_, Scene>) -> String {
        rows(harness)
            .into_iter()
            .find(|(.., is_selected)| *is_selected)
            .map(|(label, ..)| label)
            .expect("a selected row")
    }

    /// The results are exactly `count` rows tall: no blank space, no cut row.
    fn assert_height(harness: &Harness<'_, Scene>, count: usize) {
        let popup = harness.state().jump.popup_rect().expect("the results are open");
        let expected = count as f32 * ROW_HEIGHT + popup_chrome();
        assert!(
            (popup.height() - expected).abs() < 0.5,
            "the results are {} points tall, {count} rows take {expected}",
            popup.height()
        );
    }

    #[test]
    fn jump_results_reopened_after_fewer_rows_show_every_row() {
        // First opened before the list loaded: one row, "Show your pull requests".
        let mut harness = scene("reopen", 0, 900.0);
        open_box(&mut harness);
        assert_height(&harness, 1);
        close_box(&mut harness);

        // The list loaded three pull requests meanwhile; all three show, the first one selected.
        harness.state_mut().page = page(3);
        open_box(&mut harness);
        assert_eq!(visible(&harness), ["#500 Change 500", "#499 Change 499", "#498 Change 498"]);
        assert_eq!(selected(&harness), "#500 Change 500");
        assert_height(&harness, 3);
        assert_eq!(harness.state().picked, None);
    }

    #[test]
    fn jump_results_grow_in_the_frame_rows_arrive() {
        let mut harness = scene("grow", 2, 900.0);
        open_box(&mut harness);
        assert_height(&harness, 2);

        harness.state_mut().page = page(30);
        harness.step();
        assert_height(&harness, MAX_VISIBLE_RESULTS);
        let shown = visible(&harness);
        assert_eq!(shown.len(), MAX_VISIBLE_RESULTS, "a full screen of rows: {shown:?}");
        assert_eq!(shown.first().map(String::as_str), Some("#500 Change 500"), "the newest leads");
        assert_eq!(selected(&harness), "#500 Change 500");
        // Nothing moves afterwards.
        let popup = harness.state().jump.popup_rect();
        harness.run();
        assert_eq!(harness.state().jump.popup_rect(), popup);
    }

    #[test]
    fn jump_results_reopen_at_the_top_with_the_first_row_selected() {
        let mut harness = scene("top", 20, 900.0);
        open_box(&mut harness);
        // Up from the first row wraps to the last one and scrolls it into view.
        harness.key_press(Key::ArrowUp);
        harness.run();
        assert_eq!(selected(&harness), "#481 Change 481");
        assert!(
            visible(&harness).contains(&"#481 Change 481".to_owned()),
            "the selected row is in view"
        );
        close_box(&mut harness);

        open_box(&mut harness);
        let shown = visible(&harness);
        assert_eq!(
            shown.first().map(String::as_str),
            Some("#500 Change 500"),
            "reopened at the top: {shown:?}"
        );
        assert_eq!(selected(&harness), "#500 Change 500");
        assert_height(&harness, MAX_VISIBLE_RESULTS);
    }

    #[test]
    fn jump_results_list_a_pull_request_once_when_favorites_list_it_too() {
        let mut harness = scene("favorites", 0, 900.0);
        // #499 is in a starred repository: the Favorites card and the connection's card both list it.
        let favorites = vec![row(499), row(42)];
        harness.state_mut().page = PullRequestListPage::loaded_with_favorites_for_test(
            "gh",
            vec![row(500), row(499)],
            favorites,
        );
        open_box(&mut harness);
        let labels: Vec<String> = rows(&harness).into_iter().map(|(label, ..)| label).collect();
        assert_eq!(
            labels,
            ["#499 Change 499", "#42 Change 42", "#500 Change 500"],
            "the Favorites card's rows lead, each pull request once"
        );
        assert_height(&harness, 3);
    }

    /// Self-hosted platforms on plain http are pasted too; their links open, not a failed search.
    #[test]
    fn jump_offers_to_open_pasted_http_and_https_links() {
        let harness = scene("links", 0, 900.0);
        let lists = [&harness.state().page];
        for url in [
            "https://gitlab.local/a/b/-/merge_requests/1",
            "http://gitlab.local/a/b/-/merge_requests/1",
        ] {
            let found = super::entries(
                url,
                &lists,
                &Route::MyPullRequests,
                &harness.state().services,
                false,
            );
            let actions: Vec<&JumpAction> = found.iter().map(|entry| &entry.action).collect();
            assert_eq!(actions, [&JumpAction::Link(url.to_owned())], "{url}");
        }
    }

    #[test]
    fn jump_results_stop_at_the_window_bottom() {
        let height = 300.0;
        let mut harness = scene("short", 20, height);
        open_box(&mut harness);
        let popup = harness.state().jump.popup_rect().expect("the results are open");
        assert!(
            popup.bottom() <= height - style::SPACE_S,
            "the results end at {} in a window {height} tall",
            popup.bottom()
        );
        let room = ((height - style::SPACE_S - popup.top() - popup_chrome()) / ROW_HEIGHT).floor()
            as usize;
        assert!(room < MAX_VISIBLE_RESULTS);
        assert_height(&harness, room);
        assert_eq!(visible(&harness).len(), room);
    }
}
