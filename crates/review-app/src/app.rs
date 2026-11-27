use std::sync::Arc;

use egui::{Key, Modifiers, PointerButton};
use review_core::{Connection, Provider};
use review_store::{AppPaths, PullRequestTab};

use cv_base::platform::{self, pointer};
use cv_base::theme::{self, motion};
use cv_editor::markdown_editor;
use cv_lists::pull_request_list::{ListSource, PullRequestListPage};
use cv_lists::repositories::RepositoriesPage;
use cv_pr_page::PullRequestPage;
use cv_pr_session::session::hand_over_row;
use cv_runtime::history::swipe::{Swipe, SwipeFrame};
use cv_runtime::history::{History, Place, Step, Visit};
use cv_runtime::links::pull_request_link;
use cv_runtime::route::{Landing, Navigator, Origin, PullRequestKey, Request, Route};
use cv_runtime::toasts::{ToastAction, WaysOut};
use cv_runtime::trail::Trail;
use cv_services::Services;
use cv_settings::onboarding;
use cv_settings::settings::SettingsPage;
use cv_shell::attention::{Attention, Cue};
use cv_shell::browser::passes;
use cv_shell::focus_mode;
use cv_shell::frame_pacing::FrameLimiter;
use cv_shell::frame_stats::FrameStats;
use cv_shell::input::autoscroll::Autoscroll;
use cv_shell::input::smooth_wheel::{self, SmoothWheel};
use cv_shell::links::clipboard_links::{
    self, ClipboardSource, ClipboardWatch, Request as ClipboardRequest,
};
use cv_shell::single_instance::{Forwarded, ForwardedLinks};
use cv_shell::window_placement::WindowKeeper;
use cv_shell::zoom::Zoom;
use cv_shell::{nav_rail, rate_limit_banner};
use cv_widgets::{focus, surfaces};

use crate::title_bar::{JumpAction, Navigation, TitleBar};

// Test modules carry a plain `#[cfg(test)]` (other conditions in an attribute of their own):
// clippy's test exemptions (`allow-*-in-tests`) recognize only that form.
#[cfg(test)]
mod a11y_check;
#[cfg(test)]
mod action_e2e;
#[cfg(test)]
mod actions_tests;
#[cfg(test)]
mod activity_walkthrough;
#[cfg(test)]
mod assistant_script;
#[cfg(test)]
mod assistant_walkthrough;
#[cfg(test)]
mod background_window_walkthrough;
#[cfg(test)]
mod branch_copy_walkthrough;
#[cfg(test)]
#[cfg(windows)]
mod breadcrumb_overflow;
#[cfg(test)]
mod browser_pass_walkthrough;
#[cfg(test)]
mod change_breakdown_walkthrough;
#[cfg(test)]
mod checkout_finish_walkthrough;
#[cfg(test)]
mod checkout_walkthrough;
#[cfg(test)]
mod cite_flash_walkthrough;
#[cfg(test)]
#[cfg(debug_assertions)]
mod clip_check;
#[cfg(test)]
mod clipboard_walkthrough;
#[cfg(test)]
mod comment_edit_walkthrough;
#[cfg(test)]
mod comment_origin_walkthrough;
#[cfg(test)]
mod count_priority_walkthrough;
#[cfg(test)]
mod crumb_drag_walkthrough;
#[cfg(test)]
mod crumb_trail_walkthrough;
#[cfg(test)]
mod diff_find_walkthrough;
#[cfg(test)]
mod favorites_walkthrough;
#[cfg(test)]
mod files_keyboard_triage_walkthrough;
#[cfg(test)]
mod files_review_loop_walkthrough;
#[cfg(test)]
#[cfg(windows)]
mod focus_mode_walkthrough;
#[cfg(test)]
mod forwarded_link_walkthrough;
#[cfg(test)]
mod frame_rate_walkthrough;
#[cfg(test)]
mod github_mock;
#[cfg(test)]
mod gitlab_mock;
#[cfg(test)]
mod hide_header_walkthrough;
#[cfg(test)]
mod image_copy_walkthrough;
#[cfg(test)]
mod keyboard_walkthrough;
// egui's debug options (`show_expand_width`) exist only with debug assertions; release test
// builds (`cargo test --release`, for the live performance checks) leave the layout audit and its
// `clip_check` out.
#[cfg(test)]
#[cfg(debug_assertions)]
mod layout_audit_walkthrough;
#[cfg(test)]
mod layout_walkthrough;
#[cfg(test)]
mod leave_page_e2e;
#[cfg(test)]
mod list_warm_walkthrough;
#[cfg(test)]
mod live_check;
#[cfg(test)]
mod live_updates_e2e;
#[cfg(test)]
mod live_writes;
#[cfg(test)]
mod memory_walkthrough;
#[cfg(test)]
mod merge_button_walkthrough;
#[cfg(test)]
mod mini_chat_walkthrough;
#[cfg(test)]
mod mock_host;
#[cfg(test)]
mod model_chip_walkthrough;
#[cfg(test)]
mod motion_check;
#[cfg(test)]
mod navigation_walkthrough;
#[cfg(test)]
mod open_path_walkthrough;
#[cfg(test)]
mod open_resume_walkthrough;
#[cfg(test)]
mod overview_walkthrough;
#[cfg(test)]
mod own_vote_walkthrough;
#[cfg(test)]
mod pipelines_walkthrough;
#[cfg(test)]
mod pr_list_status_walkthrough;
#[cfg(test)]
mod prefetch_walkthrough;
#[cfg(test)]
mod private_image_e2e;
#[cfg(test)]
mod provider_icons_walkthrough;
#[cfg(test)]
mod recording;
#[cfg(test)]
mod remove_connection_e2e;
#[cfg(test)]
mod request_budget;
#[cfg(test)]
mod restart_walkthrough;
#[cfg(test)]
mod review_ways_out_walkthrough;
#[cfg(test)]
mod scripted_platforms;
#[cfg(test)]
mod self_review_walkthrough;
#[cfg(test)]
mod settings_walkthrough;
#[cfg(test)]
mod setup_walkthrough;
#[cfg(test)]
mod startup;
#[cfg(test)]
mod storage_walkthrough;
#[cfg(test)]
mod thread_code_walkthrough;
#[cfg(test)]
mod thread_jump_walkthrough;
#[cfg(test)]
#[cfg(windows)]
mod title_bar_walkthrough;
#[cfg(test)]
mod tree_side_walkthrough;
#[cfg(test)]
mod walkthrough;
#[cfg(test)]
mod whitespace_walkthrough;

/// The system clipboard for copied pull request links; tests start without one and set a fake.
fn system_clipboard() -> Option<std::sync::Arc<dyn ClipboardSource>> {
    if cfg!(test) { None } else { Some(std::sync::Arc::new(clipboard_links::SystemClipboard)) }
}

/// Pull request pages kept alive for instant back/forward; older ones are dropped to free memory.
const OPEN_PULL_REQUESTS: usize = 3;

pub struct ReviewApp {
    route: Route,
    /// The way to the route as the breadcrumbs show it; it ends on the route.
    trail: Trail,
    /// The pull request tab of the current place as last recorded; a page that switched its tab
    /// since makes that a step of the history.
    shown_tab: Option<PullRequestTab>,
    history: History,
    /// The trackpad gesture under way, which may become a step through the history.
    swipe: Swipe,
    navigator: Navigator,
    my_pull_requests: PullRequestListPage,
    repositories: RepositoriesPage,
    repository_pull_requests: Option<PullRequestListPage>,
    settings: SettingsPage,
    pull_requests: Vec<(PullRequestKey, PullRequestPage)>,
    /// The UI scale last handed to egui; `services.settings.ui_scale` is the source of truth.
    applied_scale: Option<f32>,
    smooth_wheel: SmoothWheel,
    autoscroll: Autoscroll,
    zoom: Zoom,
    /// Input time the current page was entered; its content fades in from there.
    page_entered_at: Option<f64>,
    frame_stats: FrameStats,
    frame_limiter: FrameLimiter,
    title_bar: TitleBar,
    window: WindowKeeper,
    clipboard: ClipboardWatch,
    forwarded: ForwardedLinks,
    /// Pull requests the frame opened in the browser, which the extension then leaves there.
    browser_passes: passes::Recorder,
    /// The user's returns after a pause and their writes, which say what to refresh ahead.
    attention: Attention,
    /// Last, so it drops after the pages: their last writes start on its runtime, whose drop
    /// waits for them ([`Background`](cv_runtime::background::Background)).
    services: Services,
}

impl ReviewApp {
    /// `factory` builds the provider of each connection: the hosts' providers in `main`, a
    /// closure over a scripted one in a walkthrough.
    pub fn new(
        creation: &eframe::CreationContext<'_>,
        paths: AppPaths,
        factory: impl Fn(Connection, reqwest::Client) -> Arc<dyn Provider> + Send + Sync + 'static,
    ) -> Self {
        let ctx = creation.egui_ctx.clone();
        theme::fonts::install(&ctx);
        #[cfg(test)]
        a11y_check::install(&ctx);
        ctx.options_mut(|options| {
            options.input_options.line_scroll_speed = smooth_wheel::NOTCH;
            // Ctrl+Plus/Minus/0 are handled by `cv_shell::zoom`, which follows focus and saves the
            // level.
            options.zoom_with_keyboard = false;
        });
        let services = Services::new(ctx, paths, factory);
        let has_connections = !services.connections.all().is_empty();
        let route = if has_connections { Route::MyPullRequests } else { Route::Settings };
        let saved_window = services.settings.window;
        let browser_passes = passes::Recorder::new(&services.paths, services.background.spawner());
        Self {
            services,
            trail: Trail::structural(&route),
            route,
            shown_tab: None,
            history: History::default(),
            swipe: Swipe::default(),
            navigator: Navigator::default(),
            my_pull_requests: PullRequestListPage::new(ListSource::Mine),
            repositories: RepositoriesPage::default(),
            repository_pull_requests: None,
            settings: SettingsPage::default(),
            pull_requests: Vec::new(),
            applied_scale: None,
            smooth_wheel: SmoothWheel::default(),
            autoscroll: Autoscroll::default(),
            zoom: Zoom::default(),
            page_entered_at: None,
            frame_stats: FrameStats::new(creation),
            frame_limiter: FrameLimiter::default(),
            title_bar: TitleBar::new(creation),
            // After the title bar: integrating it changes the window's size, the placement sets
            // the final one.
            window: WindowKeeper::restore(creation, saved_window),
            clipboard: ClipboardWatch::new(system_clipboard()),
            forwarded: ForwardedLinks::default(),
            browser_passes,
            attention: Attention::default(),
        }
    }

    /// The title bar with its search box; the pages' loaded rows are what the search finds.
    fn show_title_bar(&mut self, ui: &mut egui::Ui) {
        let mine = &self.my_pull_requests;
        let both;
        let lists: &[&PullRequestListPage] = match &self.repository_pull_requests {
            Some(repository) => {
                both = [mine, repository];
                &both
            }
            None => std::slice::from_ref(&mine),
        };
        let navigation = Navigation {
            trail: &self.trail,
            history: &self.history,
            navigator: &mut self.navigator,
        };
        let action = self.title_bar.show(ui, navigation, &self.services, lists);
        match action {
            Some(JumpAction::Open { key, listed }) => {
                // As a click in the list does: the page opens at the end of this frame and its
                // header paints the row from the page cache until the detail is in.
                if let Some(summary) = listed {
                    hand_over_row(&mut self.services, &key, *summary);
                }
                self.navigator.jump(Route::PullRequest(key));
            }
            Some(JumpAction::Link(url)) => self.open_link(&url),
            Some(JumpAction::ShowList) => self.navigator.jump(Route::MyPullRequests),
            Some(JumpAction::ToggleFocus) => focus_mode::request_toggle(ui.ctx()),
            None => {}
        }
    }

    /// The banners above the page (none in focus mode), then the page.
    fn show_content(&mut self, ui: &mut egui::Ui, is_focused: bool) {
        if !is_focused {
            rate_limit_banner::show(ui, &self.services);
            if self.route != Route::Settings {
                onboarding::show_banner(ui, &mut self.services);
            }
        }
        // Under an id of its own: a banner that opens or closes above the page moves its widgets
        // but never renames them, so a click pressed before it still counts.
        let page = egui::UiBuilder::new().scope_id(egui::Id::unique("route_page"));
        ui.scope_builder(page, |ui| self.show_route(ui));
    }

    fn show_route(&mut self, ui: &mut egui::Ui) {
        let services = &mut self.services;
        let navigator = &mut self.navigator;
        match &self.route {
            Route::MyPullRequests => self.my_pull_requests.show(ui, services, navigator),
            Route::Repositories | Route::Connection { .. } => {
                self.repositories.show(ui, services, navigator);
            }
            Route::RepositoryPullRequests { .. } => {
                if let Some(page) = self.repository_pull_requests.as_mut() {
                    page.show(ui, services, navigator);
                }
            }
            Route::PullRequest(key) => {
                if let Some((_, page)) = self.pull_requests.iter_mut().find(|(open, _)| open == key)
                {
                    page.show(ui, services, navigator);
                }
            }
            Route::Settings => self.settings.show(ui, services, navigator),
        }
    }

    fn apply_scale(&mut self, ctx: &egui::Context) {
        let scale = self.services.settings.ui_scale;
        if self.applied_scale != Some(scale) {
            ctx.set_zoom_factor(scale);
            self.applied_scale = Some(scale);
        }
    }

    /// Opens a pull request from its web URL (a typed or pasted link, or a forwarded one that no
    /// connection serves).
    fn open_link(&mut self, url: &str) {
        if let Some(key) = pull_request_link::resolve(url, self.services.connections.all()) {
            self.navigator.jump(Route::PullRequest(key));
            return;
        }
        let message = format!(
            "No saved connection can open {}. Add a connection for its host here.",
            url.trim()
        );
        self.services.toasts.error(message, unopened_link_ways_out(url));
        self.navigator.jump(Route::Settings);
    }

    /// A pull request link copied in another app, read when the window comes to the front, and
    /// the buttons of the toasts about one. A readable link opens like a click would, unless the
    /// page has input that leaving would lose.
    fn check_clipboard(&mut self, ctx: &egui::Context) {
        match clipboard_links::take_request(ctx) {
            Some(ClipboardRequest::Open(key)) => self.navigator.jump(Route::PullRequest(key)),
            Some(ClipboardRequest::Back) => self.navigator.back(),
            Some(ClipboardRequest::Settings) => self.navigator.jump(Route::Settings),
            Some(ClipboardRequest::TurnOff) => clipboard_links::turn_off(&mut self.services),
            None => {}
        }
        let Some(key) = self.clipboard.update(ctx, &mut self.services, &self.route) else {
            return;
        };
        if self.has_unsent_input() {
            clipboard_links::offer_later(&mut self.services.toasts, key);
        } else {
            clipboard_links::announce_opened(&mut self.services.toasts, &key);
            self.navigator.jump(Route::PullRequest(key));
        }
    }

    /// Links from outside: the command line, a second start, the browser extension. A pull
    /// request opens once its token read it (`cv_shell::single_instance::ForwardedLinks`).
    fn open_forwarded_links(&mut self, ctx: &egui::Context) {
        for link in self.forwarded.update(ctx, &mut self.services) {
            match link {
                Forwarded::Open(key) => self.navigator.jump(Route::PullRequest(key)),
                Forwarded::Elsewhere(url) => self.open_link(&url),
            }
        }
    }

    /// The current page holds input that leaving would lose: a draft, a post on its way, a
    /// connection setup.
    fn has_unsent_input(&self) -> bool {
        match &self.route {
            Route::PullRequest(key) => {
                self.pull_requests.iter().any(|(open, page)| open == key && page.has_unsent_input())
            }
            Route::Settings => self.settings.is_editing(),
            _ => false,
        }
    }

    /// After the page: the frame's back and forward input and pasted links, then every request of
    /// the frame. A swipe reads what the page left of the trackpad's travel, so it runs after it.
    fn apply_navigation(&mut self, ctx: &egui::Context) {
        let swipe = self.swipe.observe(ctx.input(SwipeFrame::read), platform::page_swipe);
        match history_input(ctx).or(swipe) {
            Some(Step::Back) => self.navigator.back(),
            Some(Step::Forward) => self.navigator.forward(),
            None => {}
        }
        let is_typing = ctx.memory(|memory| memory.focused().is_some());
        let pasted_link = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Paste(text) if text.trim().starts_with("https://") => {
                    Some(text.clone())
                }
                _ => None,
            })
        });
        if let Some(url) = pasted_link.filter(|_| !is_typing) {
            self.open_link(&url);
        }
        self.enter_requested(ctx);
    }

    /// Enters the page asked for. Before the page runs it takes what the rail, the title bar or
    /// anything between frames asked for: the page being left never runs once more after the
    /// request, so nothing it has due (a live-update probe, a pipeline poll) goes out for a page
    /// nobody sees any more. Asked for from the page, the new page's trail continues the one
    /// left ([`Trail::entered`]).
    fn enter_requested(&mut self, ctx: &egui::Context) {
        self.record_tab_switch();
        let (requested, step) = self.navigator.take();
        let now = ctx.input(|input| input.time);
        if let Some(step) = step {
            if let Some(target) = self.history.step(step, self.visit()) {
                self.restore(target, now);
            }
            return;
        }
        let Some(Request { route, landing, origin }) = requested else {
            return;
        };
        if route == self.route {
            return;
        }
        let trail = match origin {
            Origin::Page => self.trail.entered(&route),
            Origin::Outside => Trail::structural(&route),
        };
        let left = self.visit();
        self.enter(route, landing, trail);
        self.history.record(left, &self.place());
        self.page_entered_at = Some(now);
    }

    /// Before the page runs: the page asked for, the answers of the lists not shown, then what to
    /// refresh ahead of the user.
    fn prepare_page(&mut self, ctx: &egui::Context) {
        self.enter_requested(ctx);
        self.land_hidden_lists();
        self.anticipate(ctx);
    }

    /// The pull request lists not shown take in their answers in the frame they arrive
    /// ([`PullRequestListPage::land`]), so a revisit judges their age right; the shown list does
    /// in its own frame.
    fn land_hidden_lists(&mut self) {
        let services = &mut self.services;
        if self.route != Route::MyPullRequests {
            self.my_pull_requests.land(services);
        }
        let is_repository_shown = matches!(self.route, Route::RepositoryPullRequests { .. });
        if let Some(page) = self.repository_pull_requests.as_mut()
            && !is_repository_shown
        {
            page.land(services);
        }
    }

    /// The user came back after a pause or to the window, or wrote to a pull request
    /// ([`Attention`]): the page shown refreshes now (through the scheduler's wake: on a pull
    /// request page its live probe and running pipelines, on a pull request list its cards on
    /// screen with their rows' checks and votes), and the lists the user likely goes to next warm
    /// in the background.
    fn anticipate(&mut self, ctx: &egui::Context) {
        let Some(cue) = self.attention.update(ctx, self.services.writes.last()) else {
            return;
        };
        let services = &mut self.services;
        if cue == Cue::Returned {
            services.refresh.wake(ctx);
            if matches!(self.route, Route::Repositories | Route::Connection { .. }) {
                self.repositories.revisit(services);
            }
        }
        // After reading or acting on a pull request the way leads back to a list: the overview,
        // or the repository's list it was opened from.
        let Route::PullRequest(key) = &self.route else {
            return;
        };
        self.my_pull_requests.warm(services);
        let its_repository = ListSource::Repository {
            connection_id: key.connection_id.clone(),
            repository_key: key.repository_key.clone(),
        };
        if let Some(page) =
            self.repository_pull_requests.as_mut().filter(|page| page.source() == &its_repository)
        {
            page.warm(services);
        }
    }

    /// Where the user is: the route and, on a pull request, the tab it shows.
    fn place(&self) -> Place {
        Place { route: self.route.clone(), tab: self.current_tab() }
    }

    /// The place with the trail it shows, as the history keeps it.
    fn visit(&self) -> Visit {
        Visit { place: self.place(), trail: self.trail.clone() }
    }

    fn current_tab(&self) -> Option<PullRequestTab> {
        let Route::PullRequest(key) = &self.route else {
            return None;
        };
        self.pull_requests.iter().find(|(open, _)| open == key).map(|(_, page)| page.tab())
    }

    /// A tab the pull request page switched to itself (its tab bar, a link to a file or a check)
    /// is a step of the history like a new page.
    fn record_tab_switch(&mut self) {
        let tab = self.current_tab();
        if tab == self.shown_tab {
            return;
        }
        let place = Place { route: self.route.clone(), tab: self.shown_tab };
        let left = Visit { place, trail: self.trail.clone() };
        self.shown_tab = tab;
        self.history.record(left, &self.place());
    }

    /// Goes back or forward to `visit`: its page with its trail, and on a pull request the tab
    /// it showed.
    fn restore(&mut self, visit: Visit, now: f64) {
        let Visit { place: Place { route, tab }, trail } = visit;
        if route == self.route {
            self.trail = trail;
        } else {
            self.enter(route, Landing::Default, trail);
            self.page_entered_at = Some(now);
        }
        if let (Some(tab), Route::PullRequest(key)) = (tab, &self.route)
            && let Some((_, page)) = self.pull_requests.iter_mut().find(|(open, _)| open == key)
        {
            page.show_tab(tab);
        }
        self.shown_tab = self.current_tab();
    }

    /// Shows `route` with `trail`, which ends on it.
    fn enter(&mut self, route: Route, landing: Landing, trail: Trail) {
        // A pull request of a connection that is gone (an old link) opens nothing: no page, and
        // nothing written for it.
        let (route, trail) = match route {
            Route::PullRequest(key)
                if self.services.connections.find(&key.connection_id).is_none() =>
            {
                (Route::MyPullRequests, Trail::structural(&Route::MyPullRequests))
            }
            route => (route, trail),
        };
        // The page left stays for a quick return, without what loads again
        // (`cv_pr_files::tab::hibernate`).
        if let Route::PullRequest(left) = &self.route
            && route != self.route
            && let Some((_, page)) = self.pull_requests.iter_mut().find(|(open, _)| open == left)
        {
            page.hibernate();
        }
        match &route {
            Route::RepositoryPullRequests { connection_id, repository_key } => {
                let source = ListSource::Repository {
                    connection_id: connection_id.clone(),
                    repository_key: repository_key.clone(),
                };
                match self.repository_pull_requests.as_mut() {
                    Some(page) if page.source() == &source => page.revisit(&mut self.services),
                    // A new page paints the remembered rows of its last visit from the page cache.
                    _ => self.repository_pull_requests = Some(PullRequestListPage::new(source)),
                }
            }
            Route::PullRequest(key) => self.open_pull_request(key.clone(), landing),
            // Revisits show the known data at once and refresh it in the background.
            Route::MyPullRequests => self.my_pull_requests.revisit(&mut self.services),
            Route::Repositories => self.repositories.revisit(&mut self.services),
            Route::Connection { connection_id } => {
                self.repositories.select_connection(connection_id);
                self.repositories.revisit(&mut self.services);
            }
            Route::Settings => {}
        }
        self.route = route;
        self.trail = trail;
        self.shown_tab = self.current_tab();
    }

    /// After Settings removed `connection_id`: its pull request pages close first (their last
    /// saves land now) and back and forward lead to none of its pages, then retention deletes its
    /// data, so nothing of it comes back.
    fn forget_connection(&mut self, connection_id: &str) {
        self.pull_requests.retain(|(key, _)| key.connection_id != connection_id);
        self.history.forget_connection(connection_id);
        self.trail.forget_connection(connection_id);
        self.services.retention.connection_removed();
    }

    fn open_pull_request(&mut self, key: PullRequestKey, landing: Landing) {
        let position = self.pull_requests.iter().position(|(open, _)| *open == key);
        let mut entry = match position {
            Some(index) => {
                let mut entry = self.pull_requests.remove(index);
                entry.1.revisit(&mut self.services);
                entry
            }
            None => (key.clone(), PullRequestPage::new(key, &mut self.services)),
        };
        if landing == Landing::Files {
            entry.1.show_tab(PullRequestTab::Files);
        }
        self.pull_requests.push(entry);
        if self.pull_requests.len() > OPEN_PULL_REQUESTS {
            self.pull_requests.remove(0);
        }
    }
}

/// A step through the history asked for this frame: Alt+Left/Right (also Cmd+[ and Cmd+], and
/// Cmd+Left/Right as in Chrome, on macOS) or the mouse's side buttons, each press once. On macOS
/// Option+arrows move the cursor by words and Cmd+arrows to the line's ends, so they leave a
/// focused text field alone there.
fn history_input(ctx: &egui::Context) -> Option<Step> {
    let is_mac = ctx.os() == egui::os::OperatingSystem::Mac;
    let has_arrows = !(is_mac && ctx.text_edit_focused());
    let has_command_arrows = is_mac && has_arrows;
    ctx.input_mut(|input| {
        let mut key = |modifiers, key| input.consume_key(modifiers, key);
        let back = (has_arrows && key(Modifiers::ALT, Key::ArrowLeft))
            || (has_command_arrows && key(Modifiers::COMMAND, Key::ArrowLeft))
            || (is_mac && key(Modifiers::COMMAND, Key::OpenBracket));
        let forward = (has_arrows && key(Modifiers::ALT, Key::ArrowRight))
            || (has_command_arrows && key(Modifiers::COMMAND, Key::ArrowRight))
            || (is_mac && key(Modifiers::COMMAND, Key::CloseBracket));
        if back || input.pointer.button_pressed(PointerButton::Extra1) {
            Some(Step::Back)
        } else if forward || input.pointer.button_pressed(PointerButton::Extra2) {
            Some(Step::Forward)
        } else {
            None
        }
    })
}

/// A link no connection opens: on a platform's cloud host the connection setup for it and the page
/// on its platform, elsewhere the page in the browser.
fn unopened_link_ways_out(url: &str) -> WaysOut {
    let Some(request) = pull_request_link::unconnected_cloud_link(url) else {
        return ToastAction::open_in_browser(url.trim()).into();
    };
    let kind = request.kind;
    WaysOut::new(ToastAction::add_connection(request))
        .and(ToastAction::open_on_platform(kind, url.trim()))
}

impl eframe::App for ReviewApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // The test watchdog says whether a hung walkthrough was inside a frame or between two.
        #[cfg(test)]
        cv_testkit::frame_started(ui.ctx());
        let ui_started = std::time::Instant::now();
        let ctx = ui.ctx().clone();
        if self.zoom.handle_input(&ctx, &mut self.services.settings) {
            self.services.save_settings();
        }
        surfaces::begin_frame(&ctx);
        self.apply_scale(&ctx);
        focus::track_modality(&ctx);
        let palette = theme::activate(&ctx, self.services.settings.theme);
        if self.settings.update(&ctx, &mut self.services) {
            self.navigator.jump(Route::Settings);
        }
        if self.settings.open_shortcuts(&ctx, &self.route, self.current_tab()) {
            self.navigator.jump(Route::Settings);
        }
        // In focus mode the view's own strip stands in for all of the chrome.
        let is_focused = focus_mode::begin_frame(&ctx, self.title_bar.is_integrated());
        // The integrated title bar spans the window above the rail; a bar below a system title
        // bar sits beside the rail.
        if !is_focused {
            if self.title_bar.is_integrated() {
                self.show_title_bar(ui);
                nav_rail::show(ui, self.route.section(), &mut self.navigator);
            } else {
                nav_rail::show(ui, self.route.section(), &mut self.navigator);
                self.show_title_bar(ui);
            }
        }
        self.prepare_page(&ctx);
        markdown_editor::publish_editors(&ctx, self.services.settings.comment_editors);
        let now = ctx.input(|input| input.time);
        let opacity =
            self.page_entered_at.map_or(1.0, |entered| motion::entrance(&ctx, now - entered));
        let content = egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.ground))
            .show(ui, |ui| {
                ui.multiply_opacity(opacity);
                self.show_content(ui, is_focused);
            });
        self.autoscroll.set_content_rect(content.response.rect);
        if let Some(connection_id) = self.settings.take_removed_connection() {
            self.forget_connection(&connection_id);
        }
        // A composer's mode switch chose the editor its platform's comments open in.
        if let Some(editors) = markdown_editor::take_chosen_editors(&ctx) {
            self.services.settings.comment_editors = editors;
            self.services.save_settings();
        }
        self.services.end_frame(&ctx);
        self.services.image_clipboard.update(&ctx, &mut self.services.toasts);
        self.services.toasts.show(&ctx);
        cv_markdown::show_lightbox(&ctx);
        if self.zoom.show_overlay(&ctx, &mut self.services.settings) {
            self.services.save_settings();
        }
        self.open_forwarded_links(&ctx);
        self.check_clipboard(&ctx);
        self.apply_navigation(&ctx);
        self.clipboard.note_own_copies(&ctx);
        focus_mode::end_frame(&ctx);
        self.window.track(&ctx, &mut self.services);
        focus::paint_for_focused_widget(&ctx);
        if self.autoscroll.is_active() {
            // Direction cursor and origin marker; nothing under the frozen pointer is clickable now.
            self.autoscroll.show(&ctx);
        } else {
            pointer::pointing_hand_on_clickables(&ctx);
        }
        if self.smooth_wheel.is_active() {
            ctx.request_repaint();
        }
        // Last: every link of the frame is asked for by now, whichever widget asked.
        self.browser_passes.note_frame(&ctx, self.services.connections.all());
        self.frame_stats.finish(
            ui,
            frame,
            ui_started,
            self.frame_limiter.held_before(),
            &mut self.services,
        );
        #[cfg(test)]
        cv_testkit::frame_ended(self.services.background.running());
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // First, on the window's own input: holds a continuous frame back for its slot (input
        // frames pass at once) and moves its time past the wait, so the wheel glide and
        // autoscroll below step by the real interval and their synthetic events don't count as input.
        self.frame_limiter.pace(raw_input, self.services.settings.frame_rate);
        self.autoscroll.process(ctx, raw_input);
        self.smooth_wheel.process(raw_input, !motion::is_reduced(ctx));
    }
}
