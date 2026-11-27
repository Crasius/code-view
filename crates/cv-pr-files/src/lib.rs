//! Files tab: changed-file tree with category filters and viewed marks, and the diff or preview of one file.
#![forbid(unsafe_code)]
// RS-LINT-02: the shipped code converts no numbers; the tests' synthetic diffs and pixel probes
// convert between f32 points, line numbers and indices.
#![cfg_attr(
    test,
    expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        reason = "test fixtures convert between f32 points, line numbers and indices"
    )
)]

#[cfg(test)]
mod comment_handoff_check;
#[cfg(test)]
mod diff_selection_check;
#[cfg(test)]
mod file_header_check;
mod focus_strip;
#[cfg(test)]
mod frame_budget;
#[cfg(test)]
mod instant_colors_check;
#[cfg(test)]
mod loading_walkthrough;
mod origin_view;
mod prefetch;
mod tab;
#[cfg(test)]
mod threads_shots;

#[cfg(any(test, feature = "test-support"))]
pub use self::tab::probe;
use std::collections::HashMap;
use std::sync::Arc;

use review_core::{DiffSide, ProviderKind};
use review_git::{ChangedFile, FileDiff};
use review_store::{DiffLayout, FileFilters, FilesStart, PullRequestTab};

use self::origin_view::OriginView;
use self::tab::choices::{
    file_layout, new_only_toggled, toggle_ignore_whitespace, toggles_new_only, whole_file_side,
};
use self::tab::download::Download;
use self::tab::file_diffs::{FileDiffs, threads_on};
use self::tab::layout::panels::{self, PanelContext};
use self::tab::notices::{self, NoChanges};
use self::tab::origin::{self, Shown};
use self::tab::resume::{self, Resume};
use self::tab::review::stepping::{self, Stepping};
use self::tab::review::viewed::{ViewedMarks, ViewedToggle};
use self::tab::start::{self, Start};
use self::tab::summary_pane::SummaryPane;
use self::tab::{layout, rendered};
use cv_code::diff_rows::Layout;
use cv_editor::markdown_editor;
use cv_pr_diff::diff_view::{ChangePosition, DiffEvent, DiffInputs, DiffView, LineRequest};
use cv_pr_diff::{find, whitespace_notice};
use cv_pr_rendered::file_header::{self, HeaderAction};
use cv_pr_rendered::preview::Preview;
use cv_pr_session::actions::ActionRunner;
use cv_pr_session::session::PullRequestSession;
use cv_pr_session::web_link;
use cv_pr_summary::SummaryEvent;
use cv_pr_threads::{Jump, ThreadsUi};
use cv_pr_tree::{FileFilter, FileTree, TreeEvent};
use cv_runtime::route::PullRequestKey;
use cv_services::Services;

pub struct FilesTab {
    viewed: ViewedMarks,
    tree: FileTree,
    selected_path: Option<String>,
    diffs: FileDiffs,
    view: DiffView,
    preview: Preview,
    /// The file of the thread the user jumped to, opened once the files show.
    pending_reveal: Option<String>,
    download: Download,
    /// The layout Alt+N returns to when it leaves New only.
    layout_before_new_only: DiffLayout,
    /// The shown file on its platform, for the pull request header's platform mark menu.
    shown_link: Option<web_link::WebLink>,
    /// Position of the open file in the tree's order, to stay near it when it leaves the list.
    selected_rank: usize,
    /// The file that was open when an update took it out of the changed files.
    vanished: Option<String>,
    /// Shown while no file is open.
    summary: SummaryPane,
    /// "Show original version": the version an outdated thread was written on, while shown.
    origin: Option<OriginView>,
    /// The thread whose original version opens next frame.
    pending_origin: Option<String>,
    /// What the user did in the file tree, which is laid out after the diff, with the file list
    /// it was done on: carried out at the start of the next frame.
    pending_tree_event: Option<(Arc<Vec<ChangedFile>>, TreeEvent)>,
    /// Whether the tab already chose what it opens on (see [`start::open_start_file`]).
    start: Start,
    /// Lines of the open file the rendered Markdown diff handed to the text diff: carried out
    /// once that file's diff shows.
    line_request: Option<(String, LineRequest)>,
    /// The thread of a file its text or rendered diff scrolls to: handed over once that file's
    /// diff shows, never to the file open before it (opening another file starts a new view).
    thread_request: Option<(String, String)>,
    /// Next unviewed file, go to file, and N or P on into the next file.
    stepping: Stepping,
    /// The file the user left open and the settings' start file ([`tab::resume`]).
    resume: Resume,
    thread_counts: layout::ThreadCounts,
}

impl FilesTab {
    pub fn new(key: &PullRequestKey, services: &mut Services) -> Self {
        let viewed = ViewedMarks::load(key, services);
        let resume = Resume::new(&viewed, &services.settings);
        let mut tab = Self {
            viewed,
            tree: FileTree::default(),
            selected_path: None,
            diffs: FileDiffs::default(),
            view: DiffView::default(),
            preview: Preview::default(),
            pending_reveal: None,
            download: Download::default(),
            layout_before_new_only: DiffLayout::SideBySide,
            shown_link: None,
            selected_rank: 0,
            vanished: None,
            summary: SummaryPane::default(),
            origin: None,
            pending_origin: None,
            pending_tree_event: None,
            start: Start::Undecided,
            line_request: None,
            thread_request: None,
            stepping: Stepping::new(key),
            resume,
            thread_counts: layout::ThreadCounts::default(),
        };
        tab.adopt_filters(&services.settings.file_filters);
        tab
    }

    /// The tab the user left the pull request on, if they ever left it.
    pub fn last_tab(&self) -> Option<PullRequestTab> {
        resume::last_tab(self)
    }

    /// Saves where the user is (see [`resume::remember_place`]).
    pub fn remember_place(&mut self, services: &mut Services, tab: PullRequestTab) {
        resume::remember_place(self, services, tab);
    }

    /// Every frame, on every tab: once the changed files are known (the cached list right when the
    /// pull request opens), their grammars compile in the background, so a file opened right away
    /// colors without compiling first.
    pub fn prepare(&mut self, session: &PullRequestSession, services: &Services) {
        self.diffs.poll_colors();
        if let Some(files) = session.files().cloned() {
            resume::warm_up_grammars(self, &files, services.settings.files_start);
        }
    }

    /// The languages whose compiled grammars the pull request keeps.
    #[cfg(any(test, feature = "test-support"))]
    pub fn held_languages(&self) -> Vec<String> {
        self.diffs.held_languages()
    }

    /// Whether a diff of `path` is cached (shown before or loaded ahead).
    #[cfg(any(test, feature = "test-support"))]
    pub fn has_cached_diff(&self, path: &str) -> bool {
        self.diffs.has_cached_diff(path)
    }

    /// The page went out of sight: see [`tab::hibernate`].
    pub fn hibernate(&mut self) {
        tab::hibernate::hibernate(self);
    }

    /// The progress line or a toast asked for the Overview since the last call.
    pub fn take_overview_request(&mut self) -> bool {
        self.stepping.take_overview_request()
    }

    /// The shown file's page on its platform, while a file is on screen.
    pub fn shown_link(&self) -> Option<&web_link::WebLink> {
        self.shown_link.as_ref()
    }

    /// The file a new thread is being written on, until it is sent; a reload must not pull it away
    /// (replies, and the comment once sent, are [`ThreadsUi`]'s to report).
    pub fn unsent_comment_path(&self) -> Option<&str> {
        self.view.composer().and_then(|composer| composer.writing_path())
    }

    /// Opens `path` and scrolls to the thread once the diff is there, showing it while comments
    /// are hidden in the diff. A rendered Markdown diff shows it under its block; an SVG file shows
    /// its text diff for that, where the comment sits, and previews stay on for the other files.
    fn reveal(&mut self, path: &str, thread_id: &str) {
        self.mark_asked();
        self.origin = None;
        if !self.preview.shows_rendered(path) {
            self.preview.show_source(path);
        }
        self.thread_request = Some((path.to_owned(), thread_id.to_owned()));
        self.pending_reveal = Some(path.to_owned());
    }

    /// Shows `path` as its text diff and scrolls to the thread there, once its diff shows.
    fn show_in_text_diff(&mut self, path: &str, thread_id: &str) {
        self.preview.show_source(path);
        self.thread_request = Some((path.to_owned(), thread_id.to_owned()));
    }

    /// Goes where a thread card asked: the thread in its file, or the version it was written on.
    pub fn jump(&mut self, jump: Jump) {
        match jump {
            Jump::Reveal { path, thread_id } => self.reveal(&path, &thread_id),
            Jump::Origin { thread_id } => {
                self.mark_asked();
                self.pending_origin = Some(thread_id);
            }
        }
    }

    /// Opens `path`'s text diff at head lines `first` to `last` the assistant cited; they flash.
    pub fn show_lines(&mut self, path: &str, first: u32, last: u32) {
        self.mark_asked();
        self.origin = None;
        self.preview.show_source(path);
        self.line_request = Some((path.to_owned(), LineRequest::head(first..=last)));
        self.pending_reveal = Some(path.to_owned());
    }

    /// Something particular was asked for: the tab does not open on its default any more.
    fn mark_asked(&mut self) {
        if self.start == Start::Undecided {
            self.start = Start::Asked;
        }
    }

    /// The filters and view choices are the same for every pull request and stay over restarts:
    /// the tab takes the saved ones (another pull request may have changed them) and saves its own
    /// once the user changed one.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut PullRequestSession,
        services: &mut Services,
        actions: &mut ActionRunner,
        threads_ui: &mut ThreadsUi,
    ) {
        self.adopt_filters(&services.settings.file_filters);
        self.shown_link = None;
        layout::show(self, ui, &mut Env { session, services, actions, threads_ui });
        self.save_filters(services);
    }

    /// Takes the saved filters where they differ from the tab's; the search stays this tab's own.
    fn adopt_filters(&mut self, saved: &FileFilters) {
        let filter = &mut self.tree.filter;
        if filter.categories != saved.categories {
            filter.categories.clone_from(&saved.categories);
        }
        filter.hide_viewed = saved.hide_viewed;
        self.preview.restore(saved.markdown_preview, saved.image_compare);
    }

    /// Saves the filters when the user changed one this frame.
    fn save_filters(&self, services: &mut Services) {
        let filter = &self.tree.filter;
        let saved = &services.settings.file_filters;
        let is_saved = filter.categories == saved.categories
            && filter.hide_viewed == saved.hide_viewed
            && self.preview.is_enabled() == saved.markdown_preview
            && self.preview.compare() == saved.image_compare;
        if is_saved {
            return;
        }
        services.settings.file_filters = FileFilters {
            categories: filter.categories.clone(),
            hide_viewed: filter.hide_viewed,
            markdown_preview: self.preview.is_enabled(),
            image_compare: self.preview.compare(),
            ..saved.clone()
        };
        services.save_settings();
    }

    /// Takes in what finished loading since the last frame.
    fn poll_loads(&mut self, services: &mut Services) {
        self.diffs.poll();
        self.download.poll(services);
    }

    /// Follows the changed files: the file a thread jump asked for, the original version asked
    /// for, the tree, the file the tab opens on and the one at the place of a file that left.
    fn follow_files(
        &mut self,
        session: &PullRequestSession,
        choice: FilesStart,
        files: &Arc<Vec<ChangedFile>>,
    ) {
        // The view of the thread's file scrolls to it: see `reveal`.
        if let Some(path) = self.pending_reveal.take() {
            if let Some(index) = files.iter().position(|file| file.path == path) {
                self.tree.scroll_to(index);
            }
            self.selected_path = Some(path);
        }
        resume::open_left_file(self, files);
        origin::open_pending(&mut self.origin, &mut self.pending_origin, session);
        let viewed = &self.viewed;
        self.tree.keep_open(self.selected_path.as_deref());
        self.tree.refresh(files, &|file| viewed.is_viewed(file), viewed.version());
        let choice = self.resume.choice(choice);
        start::open_start_file(
            &mut self.start,
            &mut self.tree,
            &mut self.selected_path,
            files,
            choice,
        );
        if let Some(path) = start::stay_near_vanished_file(
            &mut self.tree,
            &mut self.selected_path,
            self.selected_rank,
            files,
        ) {
            self.vanished = Some(path);
        }
    }

    /// The all-threads panel while it is open; a thread picked in it shows.
    fn show_threads_panel(&mut self, ui: &mut egui::Ui, env: &mut Env<'_>, files: &[ChangedFile]) {
        if !env.services.settings.threads_panel.is_open {
            return;
        }
        let context = PanelContext { tree: &self.tree, files, session: env.session };
        if let Some(jump) =
            panels::show_threads_panel(ui, &context, env.services, env.actions, env.threads_ui)
        {
            self.jump(jump);
            ui.ctx().request_repaint();
        }
    }

    /// Opens what the tree or a key picked. Returns the viewed mark that was toggled (a file's or a
    /// folder's), which the caller applies once it is done reading the marks.
    fn apply_tree_event(
        &mut self,
        event: Option<TreeEvent>,
        files: &[ChangedFile],
    ) -> Option<ViewedToggle> {
        match event? {
            TreeEvent::Select(index) => {
                self.selected_path = Some(files[index].path.clone());
                self.vanished = None;
                self.origin = None;
                None
            }
            TreeEvent::SelectFolder(folder) => {
                self.selected_path = None;
                self.vanished = None;
                self.summary.set_folder(folder);
                self.origin = None;
                None
            }
            TreeEvent::ToggleViewed(index) => Some(ViewedToggle::File(index)),
            TreeEvent::ToggleFolderViewed(folder) => Some(ViewedToggle::Folder(folder)),
            // The frame carries these out as the tree reports them (`tab::layout`).
            TreeEvent::FlipSide | TreeEvent::Review(_) => None,
        }
    }

    /// The open file: its header, then its rendered diff or its text diff.
    fn show_file(
        &mut self,
        ui: &mut egui::Ui,
        env: &mut Env<'_>,
        files: &Arc<Vec<ChangedFile>>,
        index: usize,
        opened: Opened,
    ) {
        self.summary.hide();
        let file = &files[index];
        let state = self.file_state(file, env.services);
        let web_link = web_link::for_file(env.session, file);
        self.shown_link.clone_from(&web_link);
        if self.run_header(ui, env.services, files, index, state, opened) {
            self.show_rendered(ui, env, files, index);
        } else {
            self.show_text_diff(ui, env, files, index, web_link.as_ref());
        }
    }

    fn file_state(&self, file: &ChangedFile, services: &Services) -> FileState {
        let is_previewing = self.preview.is_active(file);
        let diff = self.diffs.ready().filter(|_| !is_previewing);
        let shows_diff = diff.is_some_and(|diff| !diff.is_binary && !diff.is_empty());
        let rendered_changes = self.preview.change_position(file);
        FileState {
            is_previewing,
            shows_diff,
            changes: shows_diff.then(|| self.view.change_position()).or(rendered_changes),
            rendered_changes,
            whole_file: whole_file_side(file),
            hides_whitespace: services.settings.ignore_whitespace
                && diff.is_some_and(|diff| !diff.is_binary),
        }
    }

    /// The file header (or what the strip did in its place): the viewed mark, the layout, Alt+N
    /// and the change arrows. Returns whether the rendered diff shows now.
    fn run_header(
        &mut self,
        ui: &mut egui::Ui,
        services: &mut Services,
        files: &[ChangedFile],
        index: usize,
        state: FileState,
        opened: Opened,
    ) -> bool {
        let file = &files[index];
        let header_inputs = file_header::HeaderInputs {
            file,
            is_viewed: self.viewed.is_viewed(file),
            changes: state.changes,
            is_rendered: state.rendered_changes.is_some(),
            whole_file: state.whole_file,
            hides_whitespace: state.hides_whitespace,
            edge_hint: stepping::edge_hint(self, ui, index, &file.path),
        };
        let header_action = match opened.header {
            Some(header_action) => header_action,
            None => file_header::show(ui, &header_inputs, services, &mut self.preview),
        };
        let toggled = if header_action.toggle_viewed {
            Some(ViewedToggle::File(index))
        } else {
            opened.toggled
        };
        if let Some(toggled) = &toggled {
            self.viewed.apply(services, files, toggled);
        }
        if header_action.layout_changed {
            self.remember_layout(opened.layout_before);
            self.view.relayout();
        }
        // A whole added or deleted file has no layouts to switch between.
        if state.changes.is_some() && state.whole_file.is_none() && toggles_new_only(ui) {
            let target =
                new_only_toggled(services.settings.diff_layout, self.layout_before_new_only);
            self.switch_layout(services, target);
        }
        let previews_now = self.preview.is_active(file);
        if let Some(direction) = header_action.navigate {
            if previews_now {
                self.preview.go_to_change(direction);
            } else {
                self.view.go_to_change(direction);
            }
        }
        // Switching between the rendered and the text diff keeps the place: the line at the top.
        match (state.is_previewing, previews_now) {
            (true, false) => {
                self.line_request = self
                    .preview
                    .top_line()
                    .map(|(side, line)| (file.path.clone(), LineRequest::Top { side, line }));
            }
            (false, true) if state.shows_diff => self.preview.reveal_line(self.view.top_line()),
            _ => {}
        }
        previews_now
    }

    /// The open file's rendered diff (Markdown, images) with its comments.
    fn show_rendered(
        &mut self,
        ui: &mut egui::Ui,
        env: &mut Env<'_>,
        files: &[ChangedFile],
        index: usize,
    ) {
        let file = &files[index];
        if self.preview.shows_rendered(&file.path)
            && let Some((_, thread_id)) = self.thread_request.take_if(|asked| asked.0 == file.path)
        {
            self.preview.reveal_thread(&thread_id);
        }
        self.load_ahead(env, files, Some(index));
        let file_threads = threads_on(env.session, &file.path);
        let event =
            self.preview.show(ui, env.session, env.services, file, &file_threads, env.threads_ui);
        if let Some(event) = event {
            rendered::apply(self, env, &file.path, event);
        }
    }

    /// The open file's text diff, or the notice or skeleton in its place.
    fn show_text_diff(
        &mut self,
        ui: &mut egui::Ui,
        env: &mut Env<'_>,
        files: &[ChangedFile],
        index: usize,
        web_link: Option<&web_link::WebLink>,
    ) {
        let file = &files[index];
        if self.diffs.ensure(ui.ctx(), env.session, env.services, file) {
            self.view.reset();
        }
        // After the new view for another file.
        self.take_requests(&file.path);
        self.load_ahead(env, files, Some(index));
        let layout = file_layout(whole_file_side(file), env.services.settings.diff_layout);
        let font_size = env.services.settings.code_font_size;
        let Some(diff) = self.diffs.diff_to_show(ui, &file.path, layout, font_size, web_link)
        else {
            return;
        };
        if diff.is_binary {
            if notices::binary(ui, self.download.is_loading(), web_link) {
                self.download.start(env.session, env.services, file);
            }
            return;
        }
        if diff.is_empty() {
            let ignores_whitespace = env.services.settings.ignore_whitespace;
            match notices::without_changes(ui, file, ignores_whitespace, web_link) {
                Some(NoChanges::ShowWhitespace) => toggle_ignore_whitespace(env.services),
                Some(NoChanges::Reload) => {
                    env.session.refresh(env.services);
                    self.diffs.reload();
                }
                None => {}
            }
            return;
        }
        self.diffs.ensure_contents(file, env.session, env.services);
        markdown_editor::FileContext { path: file.path.clone(), diff: diff.clone() }
            .publish(ui.ctx());
        if let Some(summary) = diff.whitespace()
            && whitespace_notice::banner(ui, summary, env.services.settings.ignore_whitespace)
        {
            toggle_ignore_whitespace(env.services);
        }
        let event = self.show_diff_view(ui, env, file, &diff, layout);
        self.apply_diff_event(event, env, file);
    }

    /// Carries out the lines and the thread asked for `path` once its text diff shows.
    fn take_requests(&mut self, path: &str) {
        if let Some((_, request)) = self.line_request.take_if(|asked| asked.0 == path) {
            self.view.request_lines(request);
        }
        if let Some((_, thread_id)) = self.thread_request.take_if(|asked| asked.0 == path) {
            self.view.reveal_thread(&thread_id);
        }
    }

    /// The text diff with the file's threads and its syntax colors.
    fn show_diff_view(
        &mut self,
        ui: &mut egui::Ui,
        env: &mut Env<'_>,
        file: &ChangedFile,
        diff: &Arc<FileDiff>,
        layout: Layout,
    ) -> Option<DiffEvent> {
        let (contents, line_spans) = self.diffs.coloring(diff);
        let file_threads = threads_on(env.session, &file.path);
        let settings = &env.services.settings;
        let inputs = DiffInputs {
            diff,
            contents,
            threads: &file_threads,
            layout,
            font_size: settings.code_font_size,
            wrap: settings.wrap_lines,
            can_comment: env.session.has_token,
            can_comment_outside_hunks: env.session.kind != ProviderKind::GitHub,
            path: &file.path,
            old_path: file.old_path.as_deref(),
            line_spans,
            show_threads: settings.show_comments_in_diff,
            find: find::FindInputs::of(settings),
            past_end: settings.scroll_past_end,
        };
        self.view.show(ui, &inputs, &mut env.services.settings.panes.diff_split, env.threads_ui)
    }

    fn apply_diff_event(
        &mut self,
        event: Option<DiffEvent>,
        env: &mut Env<'_>,
        file: &ChangedFile,
    ) {
        match event {
            Some(DiffEvent::ColorAnyway) => self.diffs.force_colors(file),
            Some(DiffEvent::RetryContents) => self.diffs.retry_contents(),
            Some(DiffEvent::SwitchLayout(layout)) => {
                let target = match layout {
                    Layout::SideBySide | Layout::WholeFile(_) | Layout::CompactSplit(_) => {
                        DiffLayout::SideBySide
                    }
                    Layout::Inline | Layout::Compact(_) => DiffLayout::Inline,
                    Layout::NewOnly => DiffLayout::NewOnly,
                };
                self.switch_layout(env.services, target);
            }
            Some(DiffEvent::SplitResized) => env.services.save_settings(),
            Some(DiffEvent::SwitchPastEnd(is_on)) => rendered::switch_past_end(env.services, is_on),
            Some(DiffEvent::Create { key, thread }) => {
                env.threads_ui.post_new_thread(&key, thread, env.session, env.services);
            }
            Some(DiffEvent::Thread(event)) => {
                if let Some(jump) =
                    env.threads_ui.run(event, env.session, env.services, env.actions)
                {
                    self.jump(jump);
                }
            }
            None => {}
        }
    }

    /// The original version of a thread, while one is open (or was asked for): true when it took
    /// the place of the file view this frame.
    fn show_origin(&mut self, ui: &mut egui::Ui, env: &mut Env<'_>) -> bool {
        let Shown::Yes(jump) = origin::show(&mut self.origin, ui, env) else { return false };
        if let Some(jump) = jump {
            self.jump(jump);
        }
        true
    }

    /// Closes the open file and shows the summary of `folder` (`""` for every changed file).
    fn open_summary(&mut self, folder: String) {
        self.mark_asked();
        self.selected_path = None;
        self.vanished = None;
        self.summary.set_folder(folder);
    }

    /// A file or a folder below the top is shown. The breadcrumb ends at the pull request either
    /// way (the summary header and the tree name the folders); its crumb then leads back to the
    /// summary of every changed file.
    pub fn is_below_top(&self) -> bool {
        self.selected_path.is_some() || !self.summary.folder().is_empty()
    }

    /// The pull request's crumb was clicked: the summary of every changed file.
    pub fn show_all_files(&mut self) {
        self.open_summary(String::new());
    }

    /// The folder summary while no file is open; a click on View opens that file. Comments are
    /// written and answered in its cards as in the file view.
    fn show_summary(
        &mut self,
        ui: &mut egui::Ui,
        env: &mut Env<'_>,
        files: &Arc<Vec<ChangedFile>>,
        thread_counts: &HashMap<String, usize>,
    ) {
        self.load_ahead(env, files, None);
        let event = self.summary.show(ui, env, files, thread_counts, &self.tree, &mut self.diffs);
        self.apply_summary_event(ui, env, files, event);
    }

    fn apply_summary_event(
        &mut self,
        ui: &egui::Ui,
        env: &mut Env<'_>,
        files: &[ChangedFile],
        event: Option<SummaryEvent>,
    ) {
        match event {
            Some(SummaryEvent::View(index)) => {
                self.selected_path = Some(files[index].path.clone());
                self.tree.scroll_to(index);
                ui.ctx().request_repaint();
            }
            Some(SummaryEvent::OpenFolder(folder)) => self.open_summary(folder),
            Some(SummaryEvent::ClearFilters) => self.tree.filter = FileFilter::default(),
            Some(SummaryEvent::Create { key, thread }) => {
                env.threads_ui.post_new_thread(&key, thread, env.session, env.services);
            }
            Some(SummaryEvent::Thread(event)) => {
                env.threads_ui.run(event, env.session, env.services, env.actions);
            }
            Some(SummaryEvent::SwitchLayout(layout)) => self.switch_layout(env.services, layout),
            Some(SummaryEvent::SplitResized) => env.services.save_settings(),
            Some(SummaryEvent::SwitchPreview(is_on)) => {
                env.services.settings.file_filters.summary_preview = is_on;
                env.services.save_settings();
            }
            Some(SummaryEvent::Rendered { file, event }) => {
                rendered::apply_from_summary(self, ui, env, files, file, event);
            }
            None => {}
        }
    }

    /// Changes and saves the diff layout, keeping the rows the user revealed.
    fn switch_layout(&mut self, services: &mut Services, layout: DiffLayout) {
        if services.settings.diff_layout == layout {
            return;
        }
        self.remember_layout(services.settings.diff_layout);
        services.settings.diff_layout = layout;
        services.save_settings();
        self.view.relayout();
    }

    /// Notes the layout New only was entered from, for Alt+N to return to.
    fn remember_layout(&mut self, previous: DiffLayout) {
        if previous != DiffLayout::NewOnly {
            self.layout_before_new_only = previous;
        }
    }

    /// Loads the diffs of the file the pointer rests on in the tree and of the files around the
    /// open one (`None` while the summary shows) ahead, from the local mirror only.
    fn load_ahead(&mut self, env: &Env<'_>, files: &[ChangedFile], open: Option<usize>) {
        self.diffs.load_ahead(env.session, env.services, files, open, &self.tree, &self.preview);
    }

    fn selected_index(&self, files: &[ChangedFile]) -> Option<usize> {
        files.iter().position(|file| self.selected_path.as_ref() == Some(&file.path))
    }
}

/// The pull request's state, the app's services and the thread machinery: what every part of the
/// tab's frame works with.
struct Env<'a> {
    session: &'a mut PullRequestSession,
    services: &'a mut Services,
    actions: &'a mut ActionRunner,
    threads_ui: &'a mut ThreadsUi,
}

/// What the frame decided before the open file shows.
struct Opened {
    /// What focus mode's strip did, in place of the file header.
    header: Option<HeaderAction>,
    /// The viewed mark the tree or a key toggled this frame.
    toggled: Option<ViewedToggle>,
    /// The layout when the frame started, for Alt+N to come back to.
    layout_before: DiffLayout,
}

/// What the open file shows this frame, before its header acts.
#[derive(Clone, Copy)]
struct FileState {
    is_previewing: bool,
    /// A text diff with changes is on screen.
    shows_diff: bool,
    /// The text diff's change position, else the rendered diff's (it steps through changes too).
    changes: Option<ChangePosition>,
    rendered_changes: Option<ChangePosition>,
    whole_file: Option<DiffSide>,
    /// "Ignore whitespace" left the file without changes.
    hides_whitespace: bool,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use review_core::FileCategory;
    use review_git::{ChangeKind, ChangedFile};
    use review_store::{FileFilters, ImageCompare};

    use super::FilesTab;
    use cv_runtime::route::PullRequestKey;
    use cv_services::Services;
    use cv_testkit::ScratchDir;

    /// The app's services on the data folders below `scratch`.
    pub(crate) fn services_in(scratch: &ScratchDir) -> Services {
        Services::new(egui::Context::default(), scratch.app_paths(), cv_providers::create_provider)
    }

    #[test]
    fn filters_stay_for_other_pull_requests_and_after_a_restart() {
        let scratch = ScratchDir::new("file-filters");
        let mut services = services_in(&scratch);
        let key = |number| PullRequestKey {
            connection_id: "ado".into(),
            repository_key: "Shop/web".into(),
            number,
        };
        let mut first = FilesTab::new(&key(1), &mut services);
        let mut already_open = FilesTab::new(&key(2), &mut services);

        first.tree.filter.categories = BTreeSet::from([FileCategory::Tests]);
        first.tree.filter.hide_viewed = true;
        first.tree.filter.search = "cart".into();
        first.preview.set_enabled(false);
        first.preview.restore(false, ImageCompare::Swipe);
        first.save_filters(&mut services);

        let chosen = FileFilters {
            categories: BTreeSet::from([FileCategory::Tests]),
            hide_viewed: true,
            markdown_preview: false,
            summary_preview: false,
            image_compare: ImageCompare::Swipe,
        };
        let shown = |tab: &FilesTab| {
            (
                tab.tree.filter.categories.clone(),
                tab.tree.filter.hide_viewed,
                tab.preview.is_enabled(),
                tab.preview.compare(),
            )
        };
        let expected = (chosen.categories.clone(), true, false, ImageCompare::Swipe);
        let opened = FilesTab::new(&key(3), &mut services);
        assert_eq!(shown(&opened), expected, "a pull request opened next shows the same filters");
        assert!(opened.tree.filter.search.is_empty(), "the search stays with its pull request");
        // A pull request open from before takes them the next time it shows.
        already_open.adopt_filters(&services.settings.file_filters);
        assert_eq!(shown(&already_open), expected);

        let restarted = services_in(&scratch);
        assert_eq!(restarted.settings.file_filters, chosen, "the filters were saved");
    }

    /// Regression: a click in the tree was dropped when a newer file list (the line counts of the
    /// last chunk, a live update) replaced the one it was made on before the next frame carried
    /// it out. The clicked file opens, and a viewed mark lands on its file, by path.
    #[test]
    fn a_tree_click_survives_a_new_file_list_before_the_next_frame() {
        use std::sync::Arc;

        use super::TreeEvent;
        use super::tab::layout::apply_pending_tree_event;

        let scratch = ScratchDir::new("tree-click-list");
        let mut services = services_in(&scratch);
        let key = PullRequestKey {
            connection_id: "gh".into(),
            repository_key: "acme/widgets".into(),
            number: 7,
        };
        let mut tab = FilesTab::new(&key, &mut services);
        let listed = Arc::new(vec![
            markdown_file("a/first.md"),
            markdown_file("src/lib.md"),
            markdown_file("z/last.md"),
        ]);
        // The same files counted, in another order.
        let counted = Arc::new(vec![
            markdown_file("z/last.md"),
            markdown_file("a/first.md"),
            markdown_file("src/lib.md"),
        ]);

        tab.pending_tree_event = Some((Arc::clone(&listed), TreeEvent::Select(2)));
        apply_pending_tree_event(&mut tab, &mut services, &counted);
        assert_eq!(tab.selected_path.as_deref(), Some("z/last.md"), "the clicked file opens");

        tab.pending_tree_event = Some((Arc::clone(&listed), TreeEvent::ToggleViewed(1)));
        apply_pending_tree_event(&mut tab, &mut services, &counted);
        let viewed: Vec<&str> = counted
            .iter()
            .filter(|file| tab.viewed.is_viewed(file))
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(viewed, ["src/lib.md"], "the clicked file is marked viewed");

        let without = Arc::new(vec![markdown_file("a/first.md")]);
        tab.pending_tree_event = Some((Arc::clone(&listed), TreeEvent::Select(1)));
        apply_pending_tree_event(&mut tab, &mut services, &without);
        assert_eq!(
            tab.selected_path.as_deref(),
            Some("z/last.md"),
            "a file that left the list opens nothing"
        );
    }

    fn markdown_file(path: &str) -> ChangedFile {
        ChangedFile {
            additions: Some(2),
            deletions: Some(1),
            category: FileCategory::Docs,
            ..ChangedFile::sample(path, ChangeKind::Modified)
        }
    }

    /// A rendered Markdown diff shows its comments, so a jump to one stays there; an SVG preview
    /// does not, so its text diff shows, as does a Markdown file's when its card asks for it.
    #[test]
    fn jumping_to_a_comment_in_a_previewed_file_shows_it_where_it_shows() {
        let scratch = ScratchDir::new("reveal-source");
        let mut services = services_in(&scratch);
        let key = PullRequestKey {
            connection_id: "ado".into(),
            repository_key: "Shop/web".into(),
            number: 7,
        };
        let mut tab = FilesTab::new(&key, &mut services);
        let (readme, guide, logo) = (
            markdown_file("docs/README.md"),
            markdown_file("docs/guide.md"),
            markdown_file("docs/logo.svg"),
        );
        assert!(tab.preview.is_active(&readme), "Markdown opens as a preview");

        tab.reveal(&readme.path, "thread-1");
        assert!(
            tab.preview.is_active(&readme),
            "the rendered diff shows the thread under its block"
        );
        assert_eq!(tab.pending_reveal.as_deref(), Some(readme.path.as_str()), "the file opens");
        assert_eq!(
            tab.thread_request,
            Some((readme.path.clone(), "thread-1".to_owned())),
            "the request waits for the file's rendered diff"
        );

        tab.reveal(&logo.path, "thread-2");
        assert!(
            !tab.preview.is_active(&logo),
            "the commented SVG shows its text diff, where the thread sits"
        );
        assert!(tab.preview.is_active(&guide), "other Markdown files keep their preview");
        assert_eq!(
            tab.thread_request,
            Some((logo.path.clone(), "thread-2".to_owned())),
            "the text diff scrolls to the thread"
        );
        // The next frames keep the source and leave the saved choice alone.
        tab.save_filters(&mut services);
        tab.adopt_filters(&services.settings.file_filters);
        assert!(services.settings.file_filters.markdown_preview);
        assert!(!tab.preview.is_active(&logo));

        // A card's "Show in text diff" in the rendered diff.
        tab.show_in_text_diff(&readme.path, "thread-1");
        assert!(!tab.preview.is_active(&readme));
        assert_eq!(tab.thread_request, Some((readme.path.clone(), "thread-1".to_owned())));

        // The header's Preview switch brings the preview back.
        tab.preview.set_enabled(true);
        assert!(tab.preview.is_active(&readme) && tab.preview.is_active(&logo));
    }

    /// "First file" opens the first file of the tree once, when the files first show; a file, a
    /// comment or a folder asked for before that wins, and "All files" keeps the folder view.
    #[test]
    fn the_files_tab_opens_on_the_first_file_only_when_nothing_else_was_asked_for() {
        use std::sync::Arc;

        use review_store::FilesStart;

        use super::FileTree;
        use super::tab::start::{Start, open_start_file};

        let files = Arc::new(vec![
            markdown_file("docs/b.md"),
            markdown_file("README.md"),
            markdown_file("docs/a.md"),
        ]);
        let mut tree = FileTree::default();
        tree.refresh(&files, &|_| false, 0);
        let first = files[tree.file_order().next().expect("a file")].path.clone();
        let open = |start: Start, selected: Option<&str>, choice: FilesStart| {
            let (mut start, mut tree, mut selected) =
                (start, FileTree::default(), selected.map(str::to_owned));
            tree.refresh(&files, &|_| false, 0);
            open_start_file(&mut start, &mut tree, &mut selected, &files, choice);
            // Later frames never choose again.
            let again = selected.clone();
            open_start_file(&mut start, &mut tree, &mut selected, &files, choice);
            assert_eq!(selected, again, "decided once");
            (start, selected)
        };
        assert_eq!(
            open(Start::Undecided, None, FilesStart::FirstFile),
            (Start::Decided, Some(first)),
            "the tree's first file"
        );
        assert_eq!(
            open(Start::Undecided, None, FilesStart::AllFiles).1,
            None,
            "the folder view stays the default"
        );
        assert_eq!(
            open(Start::Undecided, Some("docs/a.md"), FilesStart::FirstFile).1.as_deref(),
            Some("docs/a.md"),
            "a file link wins"
        );
        assert_eq!(
            open(Start::Asked, None, FilesStart::FirstFile).1,
            None,
            "a folder or a comment's version asked for wins"
        );
    }
}
