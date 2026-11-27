//! Overview tab: merge status, description and discussion on the left, reviewers, labels and the
//! assistant on the right, as in Azure DevOps. The expanded assistant takes the whole right side.
//! Turned off in Settings, the assistant is not part of the side column at all.

mod comment_composer;
mod labels;
mod linked;
mod merge_status;
mod reviewers;
mod timeline;

use std::sync::Arc;

use egui::{Id, RichText, Ui};
use review_core::{ItemRef, Provider, ProviderKind, PullRequestState, ReviewVote};
use review_store::PaneSizes;

pub(crate) use self::linked::LinkedAction;
use cv_base::platform;
use cv_base::theme::{self, icons, motion, style};
use cv_markdown::{self as markdown, RenderOptions};
use cv_pr_assistant::Assistant;
use cv_pr_session::actions::ActionRunner;
use cv_pr_session::links::{self, LinkedEntry, Sources};
use cv_pr_session::session::PullRequestSession;
use cv_pr_threads::ThreadsUi;
use cv_runtime::background::Background;
use cv_runtime::load::Live;
use cv_services::Services;
use cv_services::linked::LinkedCache;
use cv_services::self_review;
use cv_widgets::splitter::Splitter;
use cv_widgets::states;
use cv_widgets::{buttons, scroll, skeleton};

const COLUMN_GAP: f32 = 24.0;
/// Space around the two columns; the scroll gutter comes on top of it on the right.
const PAGE_PADDING: i8 = 16;
/// Inner padding of every card.
const CARD_PADDING: i8 = 16;

/// The side column keeps at least 240 points and can grow until only the description's grab strip is left.
fn side_width_range(available: f32) -> std::ops::RangeInclusive<f32> {
    cv_widgets::splitter::pane_range(240.0, available - COLUMN_GAP)
}

/// The boundary between the main and the side column, one persisted pane in both layouts. The
/// side column grows when the boundary moves left.
fn side_splitter(range: std::ops::RangeInclusive<f32>) -> Splitter {
    Splitter::new("overview_side_splitter", "Resize side column")
        .default_value(PaneSizes::DEFAULT.overview_side)
        .range(range)
        .per_point(-1.0)
        .draws_line(false)
}

#[derive(Default)]
pub(crate) struct OverviewTab {
    reviewers: reviewers::ReviewerEditor,
    labels: labels::LabelEditor,
    linked: LinkedRows,
    linked_action: Option<LinkedAction>,
    merge_action: Option<merge_status::MergeStatusAction>,
    pipeline_request: Option<PipelineRequest>,
    /// The merge button's strategy for the merge-status card, while the pull request is open.
    pub merge_type: Option<super::merge_button::MergeTypeLine>,
    /// How far the page's header slid away this frame; the page lends that height to the tab.
    pub header_hidden: f32,
}

/// Where the merge-status card asks the Pipelines tab to go.
pub(crate) enum PipelineRequest {
    Checks,
    Run(String),
}

/// What the Linked card's rows were built from: the answers of the detail, the threads and the
/// commits, the platform's own links and the connections' revision.
type LinkedKey = (u64, u64, u64, usize, u64);

/// A new answer of any part rebuilds the rows, also one with as many threads (a reply came in) or
/// commits (a force push) as before, the detail that arrives after a refresh started, and an
/// edited connection.
fn linked_key<D, T, C>(
    detail: &Live<D>,
    threads: &Live<T>,
    commits: &Live<C>,
    native: usize,
    connections: &review_store::ConnectionStore,
) -> LinkedKey {
    (detail.version(), threads.version(), commits.version(), native, connections.revision())
}

/// The Linked card's rows, rebuilt when the texts they come from changed.
#[derive(Default)]
struct LinkedRows {
    key: Option<LinkedKey>,
    entries: Vec<LinkedEntry>,
}

impl OverviewTab {
    /// Something the user is writing or editing here that a reload must not pull away.
    pub(crate) fn has_unsent_input(&self) -> bool {
        // Thread replies and the general comment are drafts of [`ThreadsUi`], which answers for them.
        self.reviewers.has_unsent_input() || self.labels.is_editing()
    }

    /// Returns where a thread card asked the Files tab to go.
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        session: &mut PullRequestSession,
        services: &mut Services,
        actions: &mut ActionRunner,
        threads: &mut ThreadsUi,
        assistant: &mut Assistant,
    ) -> Option<cv_pr_threads::Jump> {
        // The header shows the error and its actions; skeletons here would wait forever.
        if session.detail.error().is_some() {
            states::message(
                ui,
                icons::WARNING,
                "Pull request unavailable",
                "Check the number and whether this connection can read the repository.",
            );
            return None;
        }
        if assistant.is_expanded() && services.settings.assistant.enabled {
            return self.show_with_assistant(ui, session, services, actions, threads, assistant);
        }
        let mut jump = None;
        let mut is_resized = false;
        scroll::page("overview", egui::Margin::same(PAGE_PADDING)).show(ui, |ui| {
            let range = side_width_range(ui.available_width());
            let side_width =
                services.settings.panes.overview_side.clamp(*range.start(), *range.end());
            let main_width = ui.available_width() - side_width - COLUMN_GAP;
            let row = ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(main_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(main_width);
                        jump = self.show_main(ui, session, services, actions, threads);
                    },
                );
                // The row already put its item spacing after the main column; without this the
                // side column would overrun the page padding (and the scroll gutter) by as much.
                ui.add_space(COLUMN_GAP - ui.spacing().item_spacing.x);
                ui.allocate_ui_with_layout(
                    egui::vec2(side_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(side_width);
                        self.show_side(ui, session, services, actions, assistant);
                    },
                );
            });
            let boundary = row.response.rect.left() + main_width + COLUMN_GAP / 2.0;
            let resize = side_splitter(range).show(
                ui,
                boundary,
                row.response.rect.y_range(),
                &mut services.settings.panes.overview_side,
            );
            is_resized = resize.committed;
        });
        if is_resized {
            services.save_settings();
        }
        self.run_merge_action(ui, session, services, actions);
        jump
    }

    /// The expanded assistant: the side column without its other cards, the assistant filling its
    /// visible height, beside the main column that scrolls on its own. The column keeps its width
    /// and its splitter, so expanding never moves the boundary.
    fn show_with_assistant(
        &mut self,
        ui: &mut Ui,
        session: &mut PullRequestSession,
        services: &mut Services,
        actions: &mut ActionRunner,
        threads: &mut ThreadsUi,
        assistant: &mut Assistant,
    ) -> Option<cv_pr_threads::Jump> {
        // The panel is as tall as the window shows, whatever room the page leaves below it; its
        // edges are the side column's on the scrolling page (padding and scroll gutter included).
        let full = ui.available_rect_before_wrap().intersect(ui.clip_rect());
        let inner = full - scroll::with_gutter(egui::Margin::same(PAGE_PADDING));
        let range = side_width_range(inner.width());
        let width = services.settings.panes.overview_side.clamp(*range.start(), *range.end());
        let panel_rect =
            egui::Rect::from_x_y_ranges(inner.right() - width..=inner.right(), inner.y_range());
        // The main column's page padding and gutter fill the gap up to the panel.
        let main_right = panel_rect.left() - COLUMN_GAP + full.right() - inner.right();
        let main_rect = egui::Rect::from_min_max(full.min, egui::pos2(main_right, full.bottom()));
        let mut jump = None;
        let mut main_ui =
            ui.new_child(egui::UiBuilder::new().id_salt("overview_main").max_rect(main_rect));
        scroll::page("overview", egui::Margin::same(PAGE_PADDING)).show(&mut main_ui, |ui| {
            jump = self.show_main(ui, session, services, actions, threads);
        });
        let mut panel_ui =
            ui.new_child(egui::UiBuilder::new().id_salt("overview_assistant").max_rect(panel_rect));
        cv_pr_assistant::panel(&mut panel_ui, assistant, session, services);
        let resize = side_splitter(range).show(
            ui,
            panel_rect.left() - COLUMN_GAP / 2.0,
            inner.y_range(),
            &mut services.settings.panes.overview_side,
        );
        ui.advance_cursor_after_rect(full);
        if resize.committed {
            services.save_settings();
        }
        self.run_merge_action(ui, session, services, actions);
        jump
    }

    fn run_merge_action(
        &mut self,
        ui: &Ui,
        session: &mut PullRequestSession,
        services: &mut Services,
        actions: &mut ActionRunner,
    ) {
        use merge_status::MergeStatusAction;
        match self.merge_action.take() {
            Some(MergeStatusAction::Toggle) => {
                let pipelines = &mut services.settings.pipelines;
                pipelines.is_overview_expanded = !pipelines.is_overview_expanded;
                services.save_settings();
            }
            Some(MergeStatusAction::OpenRun(run_id)) => {
                self.pipeline_request = Some(PipelineRequest::Run(run_id));
            }
            Some(MergeStatusAction::OpenChecks) => {
                self.pipeline_request = Some(PipelineRequest::Checks);
            }
            Some(MergeStatusAction::ReloadRuns) => session.reload_runs(services),
            Some(MergeStatusAction::Refresh) => session.refresh(services),
            Some(MergeStatusAction::Approve) => {
                let approve = cv_pr_session::lifecycle::LifecycleAction::Vote {
                    vote: ReviewVote::Approved,
                    comment: None,
                };
                // A write that did not start (another one runs) is visible as the busy header.
                approve.start(session, services, actions);
            }
            Some(MergeStatusAction::OpenOnPlatform) => {
                if let Some(detail) = session.detail.ready() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(&detail.summary.web_url));
                }
            }
            None => {}
        }
    }

    /// Where the merge-status card asked the Pipelines tab to go this frame, once.
    pub(crate) fn take_pipeline_request(&mut self) -> Option<PipelineRequest> {
        self.pipeline_request.take()
    }

    fn show_main(
        &mut self,
        ui: &mut Ui,
        session: &mut PullRequestSession,
        services: &Services,
        actions: &mut ActionRunner,
        threads: &mut ThreadsUi,
    ) -> Option<cv_pr_threads::Jump> {
        const TITLE: &str = "Description";
        let me = services.identities.known(&session.key.connection_id);
        let can_approve = session.has_token
            && session.capabilities.votes.contains(&ReviewVote::Approved)
            && session.detail.ready().is_some_and(|detail| {
                let standing = self_review::standing(&detail.summary, me.as_ref());
                detail.summary.state == PullRequestState::Open
                    && cv_pr_session::lifecycle::author_refusal(session.kind, standing).is_none()
            });
        let card = merge_status::CardInput {
            is_expanded: services.settings.pipelines.is_overview_expanded,
            can_approve,
            services,
            merge_type: self.merge_type.as_ref(),
        };
        if let Some(action) = merge_status::show(ui, session, &card) {
            self.merge_action = Some(action);
        }
        ui.add_space(style::SPACE_L);
        let opacity = motion::reveal(
            ui.ctx(),
            Id::unique(("description", &session.key)),
            session.detail.ready().is_some(),
        );
        let body = session
            .detail
            .ready()
            .map(|detail| without_leading_heading(&detail.description, TITLE));
        section_card(ui, TITLE, |ui| match body {
            Some(body) if body.trim().is_empty() => {
                ui.multiply_opacity(opacity);
                ui.label(
                    RichText::new("No description provided.")
                        .color(theme::palette(ui.ctx()).text_secondary),
                );
            }
            Some(body) => {
                ui.multiply_opacity(opacity);
                markdown::render(
                    ui,
                    "description",
                    body,
                    RenderOptions::for_platform(session.kind),
                );
            }
            None => {
                for width in [0.9, 0.75, 0.82, 0.4] {
                    let full = ui.available_width();
                    skeleton::bar(ui, full * width, 12.0);
                    ui.add_space(6.0);
                }
            }
        });
        ui.add_space(style::SPACE_L);
        if session.has_token {
            comment_composer::show(ui, session, services, threads);
            ui.add_space(style::SPACE_L);
        }
        timeline::show(ui, session, services, actions, threads)
    }

    /// What the user picked in the Linked card this frame, once.
    pub(crate) fn take_linked_action(&mut self) -> Option<LinkedAction> {
        self.linked_action.take()
    }

    fn show_side(
        &mut self,
        ui: &mut Ui,
        session: &mut PullRequestSession,
        services: &mut Services,
        actions: &mut ActionRunner,
        assistant: &mut Assistant,
    ) {
        // Where the column's visible room ends with the page at the top and the header shown
        // whole; the assistant fills it. The height the sliding header lends does not count, so
        // the card keeps its height while the header comes and goes and follows only the window.
        let room = ui.clip_rect().height() - self.header_hidden - 2.0 * f32::from(PAGE_PADDING);
        let column_bottom = ui.max_rect().top() + room;
        // Card actions sit in the title line, so every input below spans the full card width.
        let can_edit = session.has_token && session.detail.ready().is_some();
        let reviewers_action =
            |ui: &mut Ui| can_edit && buttons::icon(ui, icons::USER_PLUS, "Add reviewer").clicked();
        section_card_with_action(ui, "Reviewers", reviewers_action, |ui, add_clicked| {
            self.reviewers.show(ui, session, services, actions, add_clicked);
        });
        ui.add_space(style::SPACE_M);
        let is_editing = self.labels.is_editing();
        let is_busy = actions.is_busy();
        let labels_action = |ui: &mut Ui| {
            if can_edit {
                labels::card_actions(ui, is_editing, is_busy)
            } else {
                labels::CardAction::None
            }
        };
        section_card_with_action(ui, "Labels", labels_action, |ui, action| {
            self.labels.show(ui, session, services, actions, action);
        });
        self.show_linked(ui, session, services);
        if !services.settings.assistant.enabled {
            return;
        }
        ui.add_space(style::SPACE_M);
        let line = assistant.title_line(services);
        let title_line = |ui: &mut Ui| cv_pr_assistant::card_action(ui, &line);
        let card = section_card_with_action(ui, cv_pr_assistant::TITLE, title_line, |ui, title| {
            cv_pr_assistant::card(ui, assistant, session, services, &title, column_bottom);
        });
        cv_pr_assistant::card_drawn(ui, assistant, card.rect);
    }

    /// The Linked card, shown once the texts name anything. States load when the card is on
    /// screen and the window is active, one batch per connection.
    fn show_linked(&mut self, ui: &mut Ui, session: &PullRequestSession, services: &mut Services) {
        let Some(detail) = session.detail.ready() else {
            return;
        };
        let is_active = ui.ctx().input(|input| platform::is_window_active(input.viewport()));
        let provider = (session.kind == ProviderKind::AzureDevOps && is_active)
            .then(|| services.provider(&session.key.connection_id))
            .flatten();
        let (native, native_error) =
            native_links(session, &mut services.linked, &services.background, provider);
        let threads = session.threads.ready();
        let commits = session.commits.ready();
        let key = linked_key(
            &session.detail,
            &session.threads,
            &session.commits,
            native.len(),
            &services.connections,
        );
        let here = session.here();
        if self.linked.key != Some(key) {
            let sources = Sources {
                title: &detail.summary.title,
                description: &detail.description,
                source_branch: &detail.summary.source_branch,
                comments: threads
                    .into_iter()
                    .flatten()
                    .filter(|thread| !thread.is_system)
                    .flat_map(|thread| thread.comments.iter().map(|comment| comment.body.as_str()))
                    .collect(),
                commit_subjects: commits
                    .into_iter()
                    .flatten()
                    .map(|commit| commit.subject.as_str())
                    .collect(),
                native,
            };
            self.linked.entries =
                links::collect(&sources, services.connections.all(), &here, session.key.number);
            self.linked.key = Some(key);
        }
        if self.linked.entries.is_empty() && native_error.is_none() {
            return;
        }
        ui.add_space(style::SPACE_M);
        let mut action = None;
        let entries = &self.linked.entries;
        let card = section_card(ui, "Linked", |ui| {
            action = linked::show(
                ui,
                entries,
                &services.linked,
                &here,
                services.connections.all(),
                native_error.as_deref(),
            );
        });
        let retry = matches!(action, Some(LinkedAction::Retry));
        if retry && native_error.is_some() {
            services.linked.forget_native(&session.key.storage_key());
        }
        if (retry || ui.is_rect_visible(card.rect)) && is_active {
            let now = ui.input(|input| input.time);
            for (connection_id, items) in links::by_connection(&self.linked.entries) {
                if let Some(provider) = services.provider(&connection_id) {
                    services.linked.request(
                        &services.background,
                        provider,
                        &connection_id,
                        items,
                        now,
                        retry,
                    );
                }
            }
        }
        if !retry {
            self.linked_action = action;
        }
    }
}

/// Work items Azure DevOps links to the pull request itself; asked once per pull request while
/// the window is active (`provider` is `None` otherwise). Other platforms have no such list. It
/// borrows only the linked cache, so the caller reads the connections beside it.
fn native_links<'a>(
    session: &PullRequestSession,
    linked: &'a mut LinkedCache,
    background: &Background,
    provider: Option<Arc<dyn Provider>>,
) -> (&'a [ItemRef], Option<String>) {
    let Some(provider) = provider else {
        return (&[], None);
    };
    let load = linked.native(
        background,
        provider,
        &session.key.storage_key(),
        &session.key.repository_key,
        session.key.number,
    );
    let error = load.error().map(|message| format!("Linked work items: {message}"));
    (load.ready().map_or(&[], Vec::as_slice), error)
}

/// Drops a first Markdown heading that only repeats the card title (PR templates often start with
/// "## Description"), so the card does not show the same word twice.
fn without_leading_heading<'a>(text: &'a str, title: &str) -> &'a str {
    let trimmed = text.trim_start();
    let (first_line, rest) = trimmed.split_once('\n').unwrap_or((trimmed, ""));
    let heading = first_line.trim_end();
    let words = heading.trim_start_matches('#');
    let is_heading =
        heading.len() - words.len() <= 6 && words.starts_with(' ') && heading.starts_with('#');
    let words = words.trim().trim_end_matches('#').trim().trim_end_matches(':');
    if is_heading && words.eq_ignore_ascii_case(title) { rest } else { text }
}

/// Card with a section title, the building block of the overview.
pub(crate) fn section_card(
    ui: &mut Ui,
    title: &str,
    content: impl FnOnce(&mut Ui),
) -> egui::Response {
    section_card_with_action(ui, title, |_| (), |ui, ()| content(ui))
}

/// [`section_card`] with a control at the right end of the title line (an edit toggle, say),
/// centred on the title. The control does not make the title line taller, so the content starts
/// at the same height in every card; its result goes to `content`.
pub(crate) fn section_card_with_action<A>(
    ui: &mut Ui,
    title: &str,
    action: impl FnOnce(&mut Ui) -> A,
    content: impl FnOnce(&mut Ui, A),
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    style::card_frame(&palette)
        .inner_margin(egui::Margin::same(CARD_PADDING))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let title_rect = ui
                .label(RichText::new(title).font(theme::fonts::semibold_font(style::SECTION_SIZE)))
                .rect;
            let slot = egui::Rect::from_x_y_ranges(
                title_rect.right()..=ui.max_rect().right(),
                egui::Rangef::point(title_rect.center().y).expand(buttons::SUBTLE_HEIGHT / 2.0),
            );
            let mut slot_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(slot)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            let picked = action(&mut slot_ui);
            ui.add_space(style::SPACE_S);
            content(ui, picked);
        })
        .response
}

#[cfg(test)]
mod tests {
    use review_core::{Connection, ProviderKind};
    use review_store::ConnectionStore;

    use super::{linked_key, without_leading_heading};
    use cv_runtime::load::Live;
    use cv_testkit::{ScratchDir, samples};

    #[test]
    fn linked_rows_follow_a_reply_in_a_reloaded_thread() {
        let detail: Live<u8> = Live::seeded(Some(1));
        let mut threads: Live<Vec<String>> = Live::seeded(Some(vec!["See #3".into()]));
        let commits: Live<Vec<String>> = Live::default();
        let scratch = ScratchDir::new("linked-reloaded-thread");
        let store = ConnectionStore::open(&scratch.app_paths()).expect("empty store");
        let before = linked_key(&detail, &threads, &commits, 0, &store);
        threads.set(vec!["See #3, and #4 too".into()]);
        assert_ne!(linked_key(&detail, &threads, &commits, 0, &store), before, "as many threads");
    }

    /// An edited connection (a new base URL) keeps the number of connections, and the rows it
    /// resolves links against must still be rebuilt.
    #[test]
    fn linked_rows_follow_an_edited_connection() {
        let scratch = ScratchDir::new("linked-edited-connection");
        let mut store = ConnectionStore::open(&scratch.app_paths()).expect("empty store");
        let connection =
            samples::connection("work", ProviderKind::GitLab, "https://gitlab.example");
        store.upsert(connection.clone()).expect("add the connection");
        let detail: Live<u8> = Live::seeded(Some(1));
        let threads: Live<Vec<String>> = Live::default();
        let commits: Live<Vec<String>> = Live::default();
        let before = linked_key(&detail, &threads, &commits, 0, &store);
        let moved = Connection { base_url: "https://gitlab.other.example".into(), ..connection };
        store.upsert(moved).expect("edit the connection");
        assert_eq!(store.all().len(), 1, "an edit adds no connection");
        let after = linked_key(&detail, &threads, &commits, 0, &store);
        assert_ne!(after, before, "the edited connection rebuilds the rows");
    }

    #[test]
    fn a_heading_that_repeats_the_title_is_dropped() {
        assert_eq!(
            without_leading_heading("## Description\nFixes the cache.", "Description"),
            "Fixes the cache."
        );
        assert_eq!(without_leading_heading("\n# description:\r\nBody", "Description"), "Body");
        assert_eq!(without_leading_heading("### Description ###", "Description"), "");
    }

    #[test]
    fn other_first_lines_stay() {
        let text = "## Summary\nDescription follows.";
        assert_eq!(without_leading_heading(text, "Description"), text);
        assert_eq!(
            without_leading_heading("#Description\nBody", "Description"),
            "#Description\nBody"
        );
        assert_eq!(
            without_leading_heading("Description of the change", "Description"),
            "Description of the change"
        );
    }
}
