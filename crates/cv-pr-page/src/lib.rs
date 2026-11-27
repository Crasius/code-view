//! One pull request: header with review actions, and the Overview, Files, Commits and Pipelines tabs.
//! It opens where the user left it (tab and open file, saved with the viewed marks), the first
//! time on the tab Settings names.
#![forbid(unsafe_code)]
// RS-LINT-02: egui lays out in f32 points while counts are usize and durations f64.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "egui pixel math converts between f32 points, usize counts and f64 seconds"
)]

mod band;
pub mod checkout;
mod commits;
mod header;
mod header_reveal;
mod hibernate;
mod live;
mod merge_button;
mod overview;
#[cfg(test)]
mod page_harness;
#[cfg(test)]
mod pipelines_follow_check;
#[cfg(test)]
mod update_place_walkthrough;
mod vote_comment;

use std::collections::HashSet;
use std::sync::Arc;

use review_store::{LastTab, PullRequestStart};

use self::checkout::CheckoutButton;
use self::header_reveal::HeaderReveal;
use self::live::{Activity, Drafts, LiveAction, LiveUpdates};
use self::merge_button::MergeButton;
use self::overview::{LinkedAction, OverviewTab, PipelineRequest};
use self::vote_comment::VoteComposer;
use cv_base::theme::{icons, motion, style};
use cv_editor::markdown_editor::{Person, PullRequestContext};
use cv_pr_assistant::{Assistant, citations};
use cv_pr_files::FilesTab;
use cv_pr_pipelines::PipelinesTab;
use cv_pr_session::actions::{ActionRunner, Refresh};
use cv_pr_session::assistant_link;
use cv_pr_session::images::PageImages;
use cv_pr_session::links::PageLinks;
use cv_pr_session::session::{CompareRange, PullRequestSession};
use cv_pr_threads::ThreadsUi;
use cv_runtime::intents;
use cv_runtime::links::link_target::Here;
use cv_runtime::links::pull_request_link;
use cv_runtime::load::Live;
use cv_runtime::route::{Navigator, PullRequestKey, PullRequestTab as Tab, Route};
use cv_services::Services;
use cv_shell::{focus_mode, top_bar};
use cv_widgets::states::{self, Action};
use cv_widgets::{provider_icon, skeleton};
use review_core::http::Api;

pub struct PullRequestPage {
    key: PullRequestKey,
    session: Result<PullRequestSession, String>,
    tab: Tab,
    /// Input time the current tab was opened; its content fades in from there.
    tab_entered_at: Option<f64>,
    actions: ActionRunner,
    overview: OverviewTab,
    files: FilesTab,
    pipelines: PipelinesTab,
    /// The header's merge button and its choices, while the pull request is open.
    merge: Option<MergeButton>,
    /// The header's checkout button, its popover and menu.
    checkout: CheckoutButton,
    /// People the comment editors offer for @mentions, rebuilt when the session changes.
    mentionable: Option<Mentionable>,
    /// How this page's Markdown resolves links, and a click waiting for the platform's answer.
    links: PageLinks,
    /// Where this page's Markdown images point and how they load.
    images: PageImages,
    /// Drafts, read marks, open cards and posts of this pull request's threads, for every tab.
    threads: ThreadsUi,
    /// New pushes, comments and votes while the page is open.
    live: LiveUpdates,
    /// Questions about the pull request to the user's agent CLI.
    assistant: Assistant,
    /// How far the header slid away with the scrolling.
    header_reveal: HeaderReveal,
    /// The vote composer under the header and its unsent text.
    vote: VoteComposer,
}

impl PullRequestPage {
    pub fn new(key: PullRequestKey, services: &mut Services) -> Self {
        let files = FilesTab::new(&key, services);
        let kind = services
            .connection(&key.connection_id)
            .map_or(review_core::ProviderKind::GitHub, |connection| connection.kind);
        let threads = ThreadsUi::new(&key, kind, services);
        let assistant = Assistant::new(&key, services);
        let session = open_session(&key, services);
        let tab = files.last_tab().map_or_else(|| start_tab(services), tab_of);
        Self {
            key,
            session,
            tab,
            tab_entered_at: None,
            actions: ActionRunner::default(),
            overview: OverviewTab::default(),
            files,
            pipelines: PipelinesTab::default(),
            merge: None,
            checkout: CheckoutButton::default(),
            mentionable: None,
            links: PageLinks::default(),
            images: PageImages::default(),
            threads,
            live: LiveUpdates::default(),
            assistant,
            header_reveal: HeaderReveal::default(),
            vote: VoteComposer::default(),
        }
    }

    /// The page is shown again: what it shows stays, a background refresh brings newer data.
    pub fn revisit(&mut self, services: &mut Services) {
        if let Ok(session) = &mut self.session {
            session.revisit(services);
        }
        self.header_reveal.reset();
    }

    /// The languages whose compiled grammars the page keeps.
    #[cfg(any(test, feature = "test-support"))]
    pub fn held_languages(&self) -> Vec<String> {
        self.files.held_languages()
    }

    /// The detailed file list (line counts, renames) is still loading.
    #[cfg(any(test, feature = "test-support"))]
    pub fn is_counting_lines(&self) -> bool {
        self.session.as_ref().is_ok_and(PullRequestSession::is_counting_lines)
    }

    /// Whether the Files tab has a diff of `path` cached (shown before or loaded ahead).
    #[cfg(any(test, feature = "test-support"))]
    pub fn has_cached_diff(&self, path: &str) -> bool {
        self.files.has_cached_diff(path)
    }

    /// A comment, reply, reviewer or label edit is open, a post is on its way, or the merge
    /// button is in use: nothing should take the page away or reload it underneath.
    pub fn has_unsent_input(&self) -> bool {
        self.vote.has_unsent_text()
            || has_unsent_input(&self.overview, &self.files, &self.threads, self.merge.as_ref())
    }

    /// The tab shown; the app records every change of it as a step of the history.
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// Switches to `tab`: the list's Files button, or a step back or forward through the history.
    pub fn show_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.tab_entered_at = None;
        }
    }

    /// While the connection's REST requests wait out a rate-limit pause, up to an hour, what the
    /// page loads waits with them: its placeholders hold still, as the banner above says why.
    pub fn show(&mut self, ui: &mut egui::Ui, services: &mut Services, navigator: &mut Navigator) {
        if is_held(services, &self.key) {
            skeleton::still(ui, |ui| self.show_page(ui, services, navigator));
        } else {
            self.show_page(ui, services, navigator);
        }
    }

    fn show_page(&mut self, ui: &mut egui::Ui, services: &mut Services, navigator: &mut Navigator) {
        let Self {
            key,
            session,
            tab,
            tab_entered_at,
            actions,
            overview,
            files,
            pipelines,
            merge,
            mentionable,
            links,
            images,
            threads,
            live,
            assistant,
            header_reveal,
            vote,
            checkout,
        } = self;
        let session = match session {
            Ok(session) => session,
            Err(message) => {
                if failed_page(ui, services, key, message) {
                    *session = open_session(key, services);
                }
                return;
            }
        };
        let ctx = ui.ctx().clone();
        let now = ui.input(|input| input.time);
        session.poll(services);
        let is_assistant_on = update_assistant(&ctx, assistant, session, services);
        files.prepare(session, services);
        threads.begin_frame(&ctx, session, services);
        update_merge(&ctx, session, services, merge, overview);
        live.update(&ctx, session, services, &activity(files, threads, actions));
        checkout::follow(session, services);
        services.linked.poll(now);
        services.merge_policies.poll(now);
        let here = session.here();
        links.publish(&ctx, services, &here);
        images.publish(&ctx, services, session);
        reload_after_write(actions, session, services);
        publish_mentions(&ctx, session, mentionable);

        // Header and tabs form one band; content spacing resumes below its divider. Focus mode
        // leaves the band out: the view's strip stands in for it, and no header is hidden.
        let is_focused = focus_mode::is_shown(&ctx);
        // Before the band, so its tab bar already shows the tab the key picked.
        let picked = band::tab_key(&ctx).map(|picked| *tab = picked).is_some();
        let item_spacing = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        let parts =
            band::Parts { session, services, actions, threads, files, merge, vote, checkout };
        let clicked = !is_focused && band::show(ui, parts, header_reveal, tab);
        if picked || clicked {
            *tab_entered_at = Some(now);
        }
        threads_panel_key(ui, services, threads, *tab);
        overview.header_hidden = if is_focused { 0.0 } else { header_reveal.drawn_hidden() };
        let (card_top, card_width) = live_card_place(ui, is_focused);
        ui.spacing_mut().item_spacing.y = item_spacing;
        let opacity = tab_entered_at.map_or(1.0, |entered| motion::entrance(&ctx, now - entered));
        ui.multiply_opacity(opacity);

        let previous_tab = *tab;
        let mut parts = TabParts { overview, files, pipelines, actions, threads, assistant };
        parts.follow_trail_click(&ctx, tab);
        header_reveal.show_content(ui, !is_focused, |ui| {
            parts.show(ui, session, services, tab, is_focused);
        });
        parts.publish_trail(&ctx, key, *tab, session);
        let cited = follow_links(&ctx, links, parts.overview, services, navigator, &here);
        parts.follow_assistant(&ctx, tab, cited, is_assistant_on);
        if *tab != previous_tab {
            *tab_entered_at = Some(now);
        }

        if let Some(LiveAction::ShowFiles) =
            live.show(&ctx, card_top, card_width, session, services)
        {
            *tab = Tab::Files;
            *tab_entered_at = Some(now);
        }
        files.remember_place(services, stored_tab(*tab));
    }
}

/// The connection of `key` has its REST requests, which load the page, held back by a rate-limit
/// pause ([`review_core::http::rate_limits`]).
pub(crate) fn is_held(services: &Services, key: &PullRequestKey) -> bool {
    services
        .rate_limited()
        .iter()
        .any(|limited| limited.connection.id == key.connection_id && limited.api == Api::Rest)
}

/// The tab a pull request the user never left anywhere opens on: Settings' "Pull requests open on".
fn start_tab(services: &Services) -> Tab {
    match services.settings.pull_request_start {
        PullRequestStart::Overview => Tab::Overview,
        PullRequestStart::Files => Tab::Files,
    }
}

/// `tab` as the pull request's saved place keeps it.
fn stored_tab(tab: Tab) -> LastTab {
    match tab {
        Tab::Overview => LastTab::Overview,
        Tab::Files => LastTab::Files,
        Tab::Commits => LastTab::Commits,
        Tab::Pipelines => LastTab::Pipelines,
    }
}

/// The tab of a saved place.
fn tab_of(tab: LastTab) -> Tab {
    match tab {
        LastTab::Overview => Tab::Overview,
        LastTab::Files => Tab::Files,
        LastTab::Commits => Tab::Commits,
        LastTab::Pipelines => Tab::Pipelines,
    }
}

/// Advances the assistant, or keeps it off as Settings says; returns whether it is on. The views
/// of this frame learn it too.
fn update_assistant(
    ctx: &egui::Context,
    assistant: &mut Assistant,
    session: &PullRequestSession,
    services: &mut Services,
) -> bool {
    let is_on = services.settings.assistant.enabled;
    assistant_link::publish_enabled(ctx, is_on);
    if is_on {
        assistant.update(session, services);
    } else {
        assistant.turn_off(services);
    }
    is_on
}

/// The tabs' views and what they share, borrowed from the page while the session is.
struct TabParts<'a> {
    overview: &'a mut OverviewTab,
    files: &'a mut FilesTab,
    pipelines: &'a mut PipelinesTab,
    actions: &'a mut ActionRunner,
    threads: &'a mut ThreadsUi,
    assistant: &'a mut Assistant,
}

impl TabParts<'_> {
    /// A line the assistant cited opens in the Files tab, with the mini chat when it is on. Alt+A
    /// (the diff takes it first while lines are selected) and "Ask about these lines" open the
    /// assistant on the Overview, unless it is turned off: then Alt+A stays free and a request is
    /// dropped.
    fn follow_assistant(
        &mut self,
        ctx: &egui::Context,
        tab: &mut Tab,
        cited: Option<(String, u32, u32)>,
        is_assistant_on: bool,
    ) {
        if let Some((path, start, end)) = cited {
            self.assistant.follow_citation(&path, start, end);
            self.files.show_lines(&path, start, end);
            *tab = Tab::Files;
        }
        let requested = assistant_link::take_request(ctx);
        if !is_assistant_on {
            return;
        }
        let request = requested.or_else(|| {
            ctx.input_mut(|input| input.consume_key(egui::Modifiers::ALT, egui::Key::A))
                .then_some(assistant_link::Request { quote: None })
        });
        if let Some(request) = request {
            self.assistant.open(request.quote);
            *tab = Tab::Overview;
        }
    }

    /// The trail is the shown tab's: its levels lead within that tab.
    fn follow_trail_click(&mut self, ctx: &egui::Context, tab: &mut Tab) {
        let Some(level) = top_bar::take_trail_click(ctx) else {
            return;
        };
        match tab {
            Tab::Files => self.files.show_all_files(),
            Tab::Overview | Tab::Commits | Tab::Pipelines => {
                *tab = Tab::Pipelines;
                self.pipelines.go_to_level(level);
            }
        }
    }

    /// The content of `tab`, which may switch to another tab for what the user asked.
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut PullRequestSession,
        services: &mut Services,
        tab: &mut Tab,
        is_focused: bool,
    ) {
        match tab {
            Tab::Overview => {
                if let Some(jump) = self.overview.show(
                    ui,
                    session,
                    services,
                    self.actions,
                    self.threads,
                    self.assistant,
                ) {
                    self.files.jump(jump);
                    *tab = Tab::Files;
                }
            }
            Tab::Files => {
                if !is_focused && live::range_bar(ui, &session.range) {
                    session.set_range(services, CompareRange::AllChanges);
                }
                self.files.show(ui, session, services, self.actions, self.threads);
                if self.files.take_overview_request() {
                    *tab = Tab::Overview;
                }
                if let Some((path, first, last)) = self.assistant.mini_chat(ui, session, services) {
                    self.files.show_lines(&path, first, last);
                }
            }
            Tab::Commits => match commits::show(ui, session) {
                Some(commits::CommitsAction::Show(range)) => {
                    session.set_range(services, range);
                    *tab = Tab::Files;
                }
                Some(commits::CommitsAction::Retry) => session.refresh(services),
                None => {}
            },
            Tab::Pipelines => self.pipelines.show(ui, session, services),
        }
        match self.overview.take_pipeline_request() {
            Some(PipelineRequest::Checks) => {
                self.pipelines.show_checks();
                *tab = Tab::Pipelines;
            }
            Some(PipelineRequest::Run(run_id)) => {
                self.pipelines.open_run(&run_id);
                *tab = Tab::Pipelines;
            }
            None => {}
        }
    }

    /// The top bar's trail for `tab`: the levels within the Pipelines tab. The Files tab names no
    /// levels below the pull request; its crumb leads back to the summary of all files.
    fn publish_trail(
        &self,
        ctx: &egui::Context,
        key: &PullRequestKey,
        tab: Tab,
        session: &PullRequestSession,
    ) {
        let (root_hint, trail) = match tab {
            Tab::Pipelines => ("Back to the checks", self.pipelines.trail(session)),
            Tab::Files if session.files().is_some() && self.files.is_below_top() => {
                ("Summary of all changed files", Vec::new())
            }
            Tab::Overview | Tab::Files | Tab::Commits => ("", Vec::new()),
        };
        top_bar::publish_trail(ctx, &Route::PullRequest(key.clone()), root_hint, trail);
    }
}

/// The session of the pull request, or why it could not be opened.
fn open_session(
    key: &PullRequestKey,
    services: &mut Services,
) -> Result<PullRequestSession, String> {
    PullRequestSession::open(key.clone(), services).map_err(|error| error.to_string())
}

/// The page when the pull request could not be opened: the reason, Retry, the connection check
/// and the pull request on its platform; true when Retry was picked, to open it again.
fn failed_page(
    ui: &mut egui::Ui,
    services: &Services,
    key: &PullRequestKey,
    message: &str,
) -> bool {
    let web_url = services.connection(&key.connection_id).map(|connection| {
        (connection.kind, pull_request_link::web_url(connection, &key.repository_key, key.number))
    });
    let open_label =
        web_url.as_ref().map(|(kind, _)| states::open_on_platform(*kind)).unwrap_or_default();
    let mut actions = vec![
        Action::primary(icons::ARROW_CLOCKWISE, "Retry"),
        Action::secondary(icons::GEAR, "Check connection"),
    ];
    if let Some((kind, _)) = &web_url {
        actions.push(Action::secondary(provider_icon::glyph(*kind), &open_label));
    }
    let picked = egui::Frame::new()
        .inner_margin(egui::Margin::same(style::PAGE_MARGIN_X))
        .show(ui, |ui| states::error_with_actions(ui, message, &actions))
        .inner;
    match picked {
        Some(0) => return true,
        Some(1) => intents::request_fix(ui.ctx(), &key.connection_id),
        Some(_) => {
            if let Some((_, url)) = &web_url {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
        }
        None => {}
    }
    false
}

/// A finished write reloads what it changed.
fn reload_after_write(
    actions: &mut ActionRunner,
    session: &mut PullRequestSession,
    services: &mut Services,
) {
    match actions.poll(services, session) {
        Some(Refresh::PullRequest) => session.refresh(services),
        Some(Refresh::Threads) => session.reload_threads(services),
        None => {}
    }
}

/// What a live update has to respect: the comments being written where it could move them, and
/// a write of the user's on its way.
fn activity(files: &FilesTab, threads: &ThreadsUi, actions: &ActionRunner) -> Activity {
    Activity {
        drafts: Drafts {
            files: files.unsent_comment_path().map(str::to_owned).into_iter().collect(),
            replies: threads.reply_open.iter().cloned().collect(),
        },
        is_saving: actions.is_busy(),
    }
}

/// The merge button exists while the pull request is open; the overview shows its merge type.
fn update_merge(
    ctx: &egui::Context,
    session: &PullRequestSession,
    services: &mut Services,
    merge: &mut Option<MergeButton>,
    overview: &mut OverviewTab,
) {
    let open_detail = session
        .detail
        .ready()
        .filter(|detail| detail.summary.state == review_core::PullRequestState::Open);
    if let Some(detail) = open_detail {
        let merge = merge.get_or_insert_with(|| MergeButton::new(session, detail));
        merge.update(ctx, session, detail, services);
        overview.merge_type = Some(merge.line(session, detail, services));
    } else {
        *merge = None;
        overview.merge_type = None;
    }
}

/// The people offered for @mentions, with the [`mentions_key`] they were built at.
type Mentionable = ((u64, u64), Arc<Vec<Person>>);

/// The answers the mention list is built from: a new one of either rebuilds it, also a reloaded
/// thread with a new reply and the detail that arrives after the threads.
fn mentions_key<D, T>(detail: &Live<D>, threads: &Live<T>) -> (u64, u64) {
    (detail.version(), threads.version())
}

/// The people the comment editors offer for @mentions, rebuilt when the detail or the threads
/// changed.
fn publish_mentions(
    ctx: &egui::Context,
    session: &PullRequestSession,
    mentionable: &mut Option<Mentionable>,
) {
    let key = mentions_key(&session.detail, &session.threads);
    if mentionable.as_ref().is_none_or(|(built, _)| *built != key) {
        *mentionable = Some((key, Arc::new(mentionable_people(session))));
    }
    if let Some((_, people)) = mentionable {
        PullRequestContext { kind: session.kind, people: people.clone() }.publish(ctx);
    }
}

/// Alt+T shows or hides the threads panel, on the Files tab. Shown this way, the panel takes the
/// keys and selects a thread to start from; hidden, it gives them back to the diff.
fn threads_panel_key(ui: &egui::Ui, services: &mut Services, threads: &mut ThreadsUi, tab: Tab) {
    let is_pressed = || ui.input_mut(|input| input.consume_key(egui::Modifiers::ALT, egui::Key::T));
    if tab == Tab::Files && is_pressed() {
        let is_open = !services.settings.threads_panel.is_open;
        services.settings.threads_panel.is_open = is_open;
        services.save_settings();
        if is_open {
            threads.panel.take_keys();
        } else {
            threads.panel.release_keys();
        }
    }
}

/// Where the live-update cards float over the content: right below the tabs (or the focus
/// strip), centered, and how wide they may be.
fn live_card_place(ui: &egui::Ui, is_focused: bool) -> (egui::Pos2, f32) {
    let below = if is_focused { focus_mode::strip::HEIGHT } else { 0.0 };
    let top = egui::pos2(ui.max_rect().center().x, ui.cursor().top() + below + style::SPACE_S);
    (top, ui.max_rect().width() - 2.0 * style::SPACE_L)
}

/// Opens what the page's Markdown and the overview's linked items asked for; a link waiting for
/// the platform's answer shows the progress cursor. A citation in the assistant's answer is the
/// Files tab's to open: returned.
fn follow_links(
    ctx: &egui::Context,
    links: &mut PageLinks,
    overview: &mut OverviewTab,
    services: &mut Services,
    navigator: &mut Navigator,
    here: &Here,
) -> Option<(String, u32, u32)> {
    let mut cited = None;
    if let Some(text) = cv_markdown::take_in_app_request(ctx) {
        match citations::parse_target(&text) {
            Some(lines) => cited = Some(lines),
            None => links.open_text(&text, ctx, services, navigator, here),
        }
    }
    match overview.take_linked_action() {
        Some(LinkedAction::Open { target, browser }) => {
            links.open(target, browser, ctx, services, navigator);
        }
        Some(LinkedAction::OpenUrl(url)) => ctx.open_url(egui::OpenUrl::new_tab(url)),
        Some(LinkedAction::Retry) | None => {}
    }
    links.poll_pending(ctx, services, navigator);
    if links.is_pending() {
        ctx.set_cursor_icon(egui::CursorIcon::Progress);
    }
    cited
}

/// Author, reviewers (not groups) and everyone who commented, each once.
fn mentionable_people(session: &PullRequestSession) -> Vec<Person> {
    let mut seen = HashSet::new();
    let mut people = Vec::new();
    let mut add = |identity: &review_core::UserIdentity| {
        if identity.id.is_empty() || !seen.insert(identity.id.clone()) {
            return;
        }
        people.push(Person {
            id: identity.id.clone(),
            display_name: identity.display_name.clone(),
            login: identity.login.clone(),
        });
    };
    if let Some(detail) = session.detail.ready() {
        add(&detail.summary.author);
        for reviewer in detail.summary.reviewers.iter().filter(|reviewer| !reviewer.is_group) {
            add(&reviewer.identity);
        }
    }
    for thread in session.threads.ready().into_iter().flatten() {
        for comment in &thread.comments {
            add(&comment.author);
        }
    }
    people
}

/// [`PullRequestPage::has_unsent_input`] from the parts that know.
fn has_unsent_input(
    overview: &OverviewTab,
    files: &FilesTab,
    threads: &ThreadsUi,
    merge: Option<&MergeButton>,
) -> bool {
    overview.has_unsent_input()
        || files.unsent_comment_path().is_some()
        || threads.has_unsent_input()
        || merge.is_some_and(MergeButton::is_engaged)
}

#[cfg(test)]
mod tests {
    use super::{Live, mentions_key};

    #[test]
    fn mentions_follow_the_detail_that_arrives_after_the_threads() {
        let mut detail: Live<String> = Live::default();
        let mut threads: Live<Vec<String>> = Live::default();
        threads.set(vec!["review comment".into()]);
        let threads_in = mentions_key(&detail, &threads);
        detail.set("author and reviewers".into());
        assert_ne!(mentions_key(&detail, &threads), threads_in, "the detail adds the author");
        let detail_in = mentions_key(&detail, &threads);
        threads.set(vec!["review comment with a new reply".into()]);
        assert_ne!(mentions_key(&detail, &threads), detail_in, "as many threads as before");
    }
}
