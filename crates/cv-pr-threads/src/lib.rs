//! Comment threads: the inline card in the diff and the overview, its states, the gutter markers,
//! the all-threads panel beside the diff, and what the app keeps about threads on this machine
//! (read marks, unsent drafts, posts in flight, the user's comments being edited or deleted).
//!
//! Everything here reads threads generically from the session: a reload or a live merge that
//! brings new comments only changes what [`ThreadsUi::new_comments`] reports, nothing else.

#![forbid(unsafe_code)]
// RS-LINT-02: egui lays out in f32 points while rows, columns and line numbers are usize or u32.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "egui pixel math converts between f32 points and usize indices"
)]

mod card;
mod context;
mod drafts;
mod edits;
mod event;
pub mod hover;
pub mod lazy_list;
mod links;
pub mod markers;
mod panel;
mod posts;
mod reading;
#[cfg(any(test, feature = "test-support"))]
pub mod samples;
pub mod suggestion;

use std::collections::{HashMap, HashSet};

use cv_pr_session::actions::{ActionRunner, Refresh};
use cv_pr_session::session::PullRequestSession;
use cv_runtime::load::Load;
use cv_runtime::route::PullRequestKey;
use cv_runtime::toasts::ToastAction;
use cv_services::Services;
use cv_services::writes::Effect;
use review_core::{NewThread, ProviderKind, ReviewThread, ThreadStatus, UserIdentity};
use review_store::ThreadStore;

use self::card::NewComments;
pub use self::card::{CardOptions, show as show_card, show_pending as show_pending_card};
pub use self::context::ThreadContext;
pub use self::drafts::{GENERAL_KEY, new_thread_key};
pub use self::event::{Jump, ThreadEvent};
pub use self::links::thread_url;
pub use self::panel::{
    HIDE_HINT, PanelEvent, PanelInputs, SHOW_HINT, panel_toggle, show as show_panel, toggle_counts,
};
#[cfg(any(test, feature = "test-support"))]
pub use self::posts::Delivery;
pub use self::posts::PostState;
use self::posts::{PostKind, PostNews, Posts};

/// Drafts are written at most this often while the user types (and always when the page closes).
const SAVE_INTERVAL_SECONDS: f64 = 1.0;
/// How long after a hand-off (a post sent, answered, failed or shown by the reloaded threads) a
/// card's height change counts as caused by it and eases instead of jumping: the change shows a
/// frame or two later, and the ease itself takes [`cv_widgets::settle::DURATION`].
const SETTLE_WINDOW_SECONDS: f64 = 0.5;

/// The id a new thread's card has while it is on its way.
pub fn pending_id(key: &str) -> String {
    format!("pending:{key}")
}

/// Per pull request: drafts, open composers, collapsed cards, read marks and posts.
pub struct ThreadsUi {
    store: ThreadStore,
    pub context: ThreadContext,
    me: Load<UserIdentity>,
    wants_me: bool,
    /// Working copies of drafts, by draft key; the store keeps them on disk.
    texts: HashMap<String, String>,
    /// Threads whose reply composer is open.
    pub reply_open: HashSet<String>,
    /// The composer that takes the keyboard focus next frame.
    focus: Option<String>,
    /// Threads the user expanded or collapsed (resolved threads start collapsed).
    collapsed: HashMap<String, bool>,
    /// (thread, comment index) the card scrolls to once: "Jump to first new".
    jump_to_new: Option<(String, usize)>,
    /// A failed post whose Retry was clicked, started at the next frame.
    retry_requested: Option<String>,
    posts: Posts,
    edits: edits::CommentEdits,
    /// Cards (by thread id or draft key) whose height changes ease until this time: a post of
    /// theirs was handed on.
    settling: HashMap<String, f64>,
    /// New threads the threads loaded last show, by the draft key they were written under: their
    /// cards take the place of the pending ones.
    landed: HashMap<String, String>,
    reading: reading::Reading,
    /// Threads the user answered: read up to the reply once it is loaded.
    read_after_reload: HashSet<String>,
    threads_version: u64,
    last_save: f64,
    now: f64,
    pub panel: panel::PanelState,
    /// The heights of the overview's cards, for laying out only those near the view.
    pub timeline_heights: lazy_list::Heights,
    code: card::code_context::CodeContexts,
}

impl ThreadsUi {
    pub fn new(key: &PullRequestKey, kind: ProviderKind, services: &mut Services) -> Self {
        let loaded = ThreadStore::load(&services.paths, &key.connection_id, &key.storage_key());
        let store = match loaded {
            Ok(store) => store,
            Err(error) => {
                let text = format!("Could not read saved drafts and read marks: {error}");
                let folder = ToastAction::open_folder(services.paths.threads_dir());
                services.toasts.error(text, folder);
                ThreadStore::in_memory()
            }
        };
        Self::with_store(store, ThreadContext::new(kind, &key.connection_id))
    }

    fn with_store(store: ThreadStore, context: ThreadContext) -> Self {
        Self {
            store,
            context,
            me: Load::Idle,
            wants_me: false,
            texts: HashMap::new(),
            reply_open: HashSet::new(),
            focus: None,
            collapsed: HashMap::new(),
            jump_to_new: None,
            retry_requested: None,
            posts: Posts::default(),
            edits: edits::CommentEdits::default(),
            settling: HashMap::new(),
            landed: HashMap::new(),
            reading: reading::Reading::default(),
            read_after_reload: HashSet::new(),
            threads_version: 0,
            last_save: 0.0,
            now: 0.0,
            panel: panel::PanelState::default(),
            timeline_heights: lazy_list::Heights::default(),
            code: card::code_context::CodeContexts::default(),
        }
    }

    /// Call once per frame before any card is shown.
    pub fn begin_frame(
        &mut self,
        ctx: &egui::Context,
        session: &mut PullRequestSession,
        services: &mut Services,
    ) {
        self.now = ctx.input(|input| input.time);
        self.context.platform = session.kind;
        self.context.can_write = session.has_token;
        self.context.provider = Some(session.provider.clone());
        self.context.author_id =
            session.detail.ready().map(|detail| detail.summary.author.id.clone());
        self.context.web_url = session.detail.ready().map(|detail| detail.summary.web_url.clone());
        self.me.poll();
        if self.wants_me && self.me.is_idle() {
            let provider = session.provider.clone();
            self.me.start(&services.background, async move { provider.current_user().await });
        }
        if let Some(key) = self.take_retry() {
            self.posts.retry(&key, session, services);
            self.on_retry(&key);
        }
        let issues = services.providers.issues().clone();
        let mut news = PostNews::default();
        if self.posts.poll(
            &mut self.store,
            &mut self.texts,
            &issues,
            &self.context.connection_id,
            session.threads.version(),
            &mut news,
        ) {
            session.reload_threads(services);
        }
        self.on_post_news(&news);
        let version = session.threads.version();
        if self.edits.poll(&issues, &self.context.connection_id, version, &mut services.toasts) {
            session.reload_threads(services);
        }
        for done in news.done {
            services.toasts.success(done);
        }
        for warning in news.warnings {
            let known_url = self.context.web_url.as_deref();
            let connection = services.connection(&session.key.connection_id);
            let page = ToastAction::open_pull_request(connection, &session.key, known_url);
            let ways_out =
                page.unwrap_or_else(|| ToastAction::copy("Copy message", warning.clone()));
            services.toasts.error(warning, ways_out);
        }
        let threads = session.threads.ready().map(Vec::as_slice);
        self.track_frame(threads, session.threads.version());
        self.code.begin_frame(ctx, session, services);
        let is_due = self.now - self.last_save >= SAVE_INTERVAL_SECONDS;
        if self.store.is_dirty() && is_due {
            self.last_save = self.now;
            if let Err(error) = self.store.save() {
                let folder = ToastAction::open_folder(services.paths.threads_dir());
                let text = format!("Could not save drafts and read marks: {error}");
                services.toasts.error(text, folder);
            }
        }
        if self.store.is_dirty() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(SAVE_INTERVAL_SECONDS));
        }
    }

    /// The threads-only part of [`Self::begin_frame`]: the threads loaded at `version` (once they
    /// are there), the hand-offs that stop easing, and the cards that were read.
    fn track_frame(&mut self, threads: Option<&[ReviewThread]>, version: u64) {
        if let Some(threads) = threads {
            self.on_threads(threads, version);
        }
        self.settling.retain(|_, until| *until > self.now);
        self.reading.track(self.now, &mut self.store);
    }

    fn on_threads(&mut self, threads: &[ReviewThread], version: u64) {
        if !self.store.has_visited() {
            let now = time::OffsetDateTime::now_utc().unix_timestamp();
            self.store.start_visit(
                now,
                threads.iter().filter_map(|thread| Some((thread.id.clone(), newest(thread)?))),
            );
        }
        if version == self.threads_version {
            return;
        }
        self.threads_version = version;
        self.edits.forget_landed(version);
        // The pending cards of posts these threads show give way to the loaded ones, in place.
        self.landed.clear();
        for landed in self.posts.forget_delivered(version) {
            self.settle(&landed.key);
            if let Some(thread_id) = landed.thread_id {
                self.settle(&thread_id);
                self.read_after_reload.insert(thread_id.clone());
                if !landed.key.starts_with(drafts::REPLY_PREFIX) {
                    self.landed.insert(landed.key, thread_id);
                }
            }
        }
        for thread in threads.iter().filter(|thread| self.read_after_reload.contains(&thread.id)) {
            if let Some(newest) = newest(thread) {
                self.store.mark_seen(&thread.id, newest);
            }
        }
        self.read_after_reload.clear();
    }

    /// Posts that reached the platform or failed hand their card on: its height eases to the new
    /// content. A failed reply opens its composer again, with the text, the reason and Retry.
    fn on_post_news(&mut self, news: &PostNews) {
        for key in news.delivered.iter().chain(&news.failed) {
            self.settle(key);
            if let Some(thread_id) =
                self.posts.kind(key).and_then(PostKind::thread_id).map(str::to_owned)
            {
                self.settle(&thread_id);
            }
            if let Some(created) = self.posts.created(key).cloned() {
                // The user wrote it: it is not news to them, whoever the platform says they are.
                if let Some(newest) = newest(&created) {
                    self.store.mark_seen(&created.id, newest);
                }
                self.settle(&created.id);
            }
        }
        for key in &news.failed {
            if let Some(PostKind::Reply { thread_id, .. }) = self.posts.kind(key) {
                self.reply_open.insert(thread_id.clone());
            }
        }
        if news.delivered.iter().any(|key| key == GENERAL_KEY) {
            // The panel's general comment is posted: its composer stays folded.
            self.panel.is_writing_general = false;
        }
    }

    /// The card of `id` (a thread id or a draft key) changes height because of a hand-off.
    fn settle(&mut self, id: &str) {
        self.settling.insert(id.to_owned(), self.now + SETTLE_WINDOW_SECONDS);
    }

    /// Whether a height change of the card of `id` (a thread id or a draft key) comes from a
    /// hand-off and eases instead of jumping.
    pub fn is_settling(&self, id: &str) -> bool {
        self.settling.get(id).is_some_and(|until| self.now < *until)
    }

    /// The thread the threads loaded last show for the new thread drafted under `key`: its card
    /// takes the place of the pending one.
    pub fn landed_thread(&self, key: &str) -> Option<&str> {
        self.landed.get(key).map(String::as_str)
    }

    /// A stable id for the place of `thread`'s card: the pending card's while the loaded thread
    /// takes its place, so a height kept for the place carries over.
    pub fn card_slot(&self, thread: &ReviewThread) -> String {
        match self.landed.iter().find(|(_, id)| **id == thread.id) {
            Some((key, _)) => pending_id(key),
            None => thread.id.clone(),
        }
    }

    /// The signed-in user, once known. Asking starts the one request that finds out.
    pub(crate) fn me(&mut self) -> Option<&UserIdentity> {
        self.request_me();
        self.me.ready()
    }

    /// Asks for the signed-in user: the next frame starts the one request that finds out.
    pub(crate) fn request_me(&mut self) {
        self.wants_me = true;
    }

    fn known_me(&self) -> Option<&UserIdentity> {
        self.me.ready()
    }

    pub(crate) fn is_collapsed(&self, thread: &ReviewThread) -> bool {
        self.collapsed.get(&thread.id).copied().unwrap_or(thread.status == ThreadStatus::Resolved)
    }

    pub(crate) fn set_collapsed(&mut self, thread_id: &str, is_collapsed: bool) {
        self.collapsed.insert(thread_id.to_owned(), is_collapsed);
    }

    pub(crate) fn toggle_collapsed(&mut self, thread: &ReviewThread) {
        self.set_collapsed(&thread.id, !self.is_collapsed(thread));
    }

    /// Comments of other people the user has not seen yet; `None` when there are none.
    pub(crate) fn new_comments(&self, thread: &ReviewThread) -> Option<NewComments> {
        let me = self.known_me().map(|me| me.id.as_str());
        NewComments::of(thread, reading::news(&self.store, me, thread)?)
    }

    pub fn is_unread(&self, thread: &ReviewThread) -> bool {
        let me = self.known_me().map(|me| me.id.as_str());
        reading::news(&self.store, me, thread)
            .is_some_and(|is_new| thread.comments.iter().any(is_new))
    }

    /// Marks every comment of `thread` as read now.
    pub(crate) fn mark_read(&mut self, thread: &ReviewThread) {
        if let Some(newest) = newest(thread) {
            self.store.mark_seen(&thread.id, newest);
        }
    }

    /// The draft text under `key`, loaded from disk the first time.
    pub fn text_mut(&mut self, key: &str) -> &mut String {
        let store = &self.store;
        self.texts
            .entry(key.to_owned())
            .or_insert_with(|| store.draft(key).map(|draft| draft.text.clone()).unwrap_or_default())
    }

    pub fn has_draft(&self, key: &str) -> bool {
        self.texts
            .get(key)
            .map_or_else(|| self.store.draft(key).is_some(), |text| !text.trim().is_empty())
    }

    /// Writes the working copy of `key` to the store (on disk with the next save).
    pub fn keep_draft(&mut self, key: &str, anchor: Option<&review_core::ThreadAnchor>) {
        let text = self.texts.get(key).map_or("", String::as_str);
        if !drafts::is_kept(&self.store, key, text, anchor) {
            let draft = review_store::Draft {
                text: text.to_owned(),
                anchor: anchor.map(drafts::to_stored),
            };
            self.store.set_draft(key, draft);
        }
    }

    /// Drops the draft under `key` for good (the user cancelled, or it was posted).
    pub fn discard_draft(&mut self, key: &str) {
        self.texts.remove(key);
        self.store.remove_draft(key);
    }

    /// A new-thread draft moved to other lines (a push shifted them): its text goes along to the
    /// key of the new lines. A draft kept on those lines already stays, with the moved text after
    /// it, so the one composer there shows both. A draft that is being posted, or that would land
    /// on lines whose draft is being posted, keeps its key until the post is through.
    pub fn move_draft(&mut self, from: &str, to: &review_core::ThreadAnchor) -> String {
        let key = drafts::new_thread_key(to);
        if key == from || self.posts.state(from).is_some() || self.posts.state(&key).is_some() {
            return from.to_owned();
        }
        let moved = self.text_mut(from).clone();
        self.discard_draft(from);
        let text = self.text_mut(&key);
        if text.trim().is_empty() {
            *text = moved;
        } else if !moved.trim().is_empty() {
            text.push_str("\n\n");
            text.push_str(&moved);
        }
        self.keep_draft(&key, Some(to));
        key
    }

    /// Whether the user is writing here: an open reply, the general comment with text or a post on
    /// its way (the diff's new-thread composer is the Files tab's to report). Live updates hold
    /// back what would move it; drafts merely kept on disk do not, they survive any reload.
    pub fn has_unsent_input(&self) -> bool {
        let is_writing_general = self.panel.is_writing_general
            || self.texts.get(GENERAL_KEY).is_some_and(|text| !text.trim().is_empty());
        let is_busy = self.posts.is_sending() || self.edits.is_editing();
        !self.reply_open.is_empty() || is_writing_general || is_busy
    }

    /// Unsent new-thread drafts on `path`, with the lines they were written on.
    pub fn new_thread_drafts(&self, path: &str) -> Vec<review_core::ThreadAnchor> {
        self.store
            .drafts()
            .filter_map(|(_, draft)| draft.anchor.as_ref())
            .filter(|anchor| anchor.path == path)
            .map(drafts::from_stored)
            .collect()
    }

    pub fn post_state(&self, key: &str) -> Option<&PostState> {
        self.posts.state(key)
    }

    /// Posts a new inline thread written in the composer under `key`.
    pub fn post_new_thread(
        &mut self,
        key: &str,
        thread: NewThread,
        session: &PullRequestSession,
        services: &Services,
    ) {
        self.settle(key);
        self.posts.start(key, PostKind::Create(thread), session, services);
    }

    /// Posts the general comment drafted under [`GENERAL_KEY`].
    pub fn post_general(&mut self, session: &PullRequestSession, services: &Services) {
        let body = self.text_mut(GENERAL_KEY).clone();
        if body.trim().is_empty() {
            return;
        }
        self.settle(GENERAL_KEY);
        self.posts.start(
            GENERAL_KEY,
            PostKind::Create(NewThread { body, anchor: None }),
            session,
            services,
        );
    }

    /// The signed-in user, or "You" until the platform said who that is.
    fn me_or_you(&mut self) -> UserIdentity {
        self.me().cloned().unwrap_or_else(|| UserIdentity {
            id: String::new(),
            display_name: "You".into(),
            login: None,
            avatar_url: None,
        })
    }

    /// The new thread drafted under `key` while it is on its way, as its card shows it: the copy
    /// the platform sent back once it answered (its ids and metadata), else the thread as it will
    /// most likely come back. `None` when nothing is on its way there, or it failed.
    pub fn pending_thread(&mut self, key: &str) -> Option<ReviewThread> {
        let Some(PostKind::Create(new)) = self
            .posts
            .kind(key)
            .filter(|_| self.posts.state(key).is_some_and(PostState::is_pending))
            .cloned()
        else {
            return None;
        };
        if let Some(created) = self.posts.created(key) {
            return Some(created.clone());
        }
        let author = self.me_or_you();
        let created_at = self.posts.sent_at(key).unwrap_or_else(time::OffsetDateTime::now_utc);
        // GitHub's pull request comments (no lines) take no replies and no status.
        let can_answer = new.anchor.is_some() || self.context.platform != ProviderKind::GitHub;
        Some(ReviewThread {
            id: pending_id(key),
            status: ThreadStatus::Active,
            anchor: new.anchor,
            is_outdated: false,
            origin: None,
            is_system: false,
            can_reply: can_answer,
            can_resolve: can_answer,
            comments: vec![review_core::ThreadComment {
                id: pending_id(key),
                author,
                body: new.body,
                created_at,
                updated_at: None,
            }],
        })
    }

    /// The new threads on their way, oldest first, as their cards show them.
    pub fn pending_threads(&mut self) -> Vec<(String, ReviewThread)> {
        let keys: Vec<String> =
            self.posts.pending_creates().into_iter().map(str::to_owned).collect();
        keys.into_iter().filter_map(|key| Some((key.clone(), self.pending_thread(&key)?))).collect()
    }

    /// The reply to `thread_id` while it is on its way: the platform's copy once it answered,
    /// else the comment as it will come back. `None` when none is on its way, or it failed.
    pub(crate) fn pending_reply(&mut self, thread_id: &str) -> Option<review_core::ThreadComment> {
        let key = drafts::reply_key(thread_id);
        if !self.posts.state(&key).is_some_and(PostState::is_pending) {
            return None;
        }
        if let Some(comment) = self.posts.replied(&key) {
            return Some(comment.clone());
        }
        let body = self.posts.body(&key)?.to_owned();
        let created_at = self.posts.sent_at(&key).unwrap_or_else(time::OffsetDateTime::now_utc);
        let author = self.me_or_you();
        Some(review_core::ThreadComment {
            id: pending_id(&key),
            author,
            body,
            created_at,
            updated_at: None,
        })
    }

    /// Sends a reply, a status change, an edit or a deletion. Returns where to go for `Reveal` and `ShowOrigin`.
    pub fn run(
        &mut self,
        event: ThreadEvent,
        session: &PullRequestSession,
        services: &Services,
        actions: &mut ActionRunner,
    ) -> Option<Jump> {
        match event {
            ThreadEvent::Reply { thread_id, body, resolve } => {
                let key = drafts::reply_key(&thread_id);
                self.note_reply(&thread_id);
                self.posts.start(
                    &key,
                    PostKind::Reply { thread_id, body, resolve },
                    session,
                    services,
                );
                None
            }
            ThreadEvent::SetStatus { thread_id, status } => {
                let (provider, repository, number) = (
                    session.provider.clone(),
                    session.key.repository_key.clone(),
                    session.key.number,
                );
                let done = match status {
                    ThreadStatus::Resolved => "Thread resolved.",
                    ThreadStatus::Active => "Thread reopened.",
                };
                if status == ThreadStatus::Resolved {
                    self.collapsed.remove(&thread_id);
                }
                let effect =
                    (&session.key, Effect::ThreadStatus { thread_id: thread_id.clone(), status });
                actions.run(services, done, Refresh::Threads, effect, async move {
                    provider.set_thread_status(&repository, number, &thread_id, status).await
                });
                None
            }
            ThreadEvent::Reveal { thread_id } => {
                let all = session.threads.ready().map_or(&[][..], Vec::as_slice);
                let anchor = all
                    .iter()
                    .find(|thread| thread.id == thread_id)
                    .and_then(|thread| thread.anchor.as_ref())?;
                Some(Jump::Reveal { path: anchor.path.clone(), thread_id })
            }
            ThreadEvent::ShowOrigin { thread_id } => Some(Jump::Origin { thread_id }),
            ThreadEvent::EditComment { thread_id, comment_id, body } => {
                self.edits.save(&thread_id, &comment_id, body, session, services);
                None
            }
            ThreadEvent::DeleteComment { thread_id, comment_id } => {
                self.edits.delete(&thread_id, &comment_id, session, services);
                None
            }
        }
    }

    /// A reply to `thread_id` goes out: its composer folds into the pending comment, and the
    /// thread is read up to the reply once the reloaded threads carry it.
    fn note_reply(&mut self, thread_id: &str) {
        self.reply_open.remove(thread_id);
        self.read_after_reload.insert(thread_id.to_owned());
        self.settle(thread_id);
    }

    /// The failed post whose Retry was clicked, now carrying the text its composer shows: the user
    /// may have changed it after the failure. A composer emptied meanwhile sends the kept text.
    fn take_retry(&mut self) -> Option<String> {
        let key = self.retry_requested.take()?;
        if let Some(text) = self.texts.get(&key).filter(|text| !text.trim().is_empty()) {
            self.posts.set_failed_body(&key, text);
        }
        Some(key)
    }

    /// A failed post goes out again: its composer collapses into the pending comment once more.
    fn on_retry(&mut self, key: &str) {
        if !self.posts.state(key).is_some_and(PostState::is_pending) {
            return;
        }
        self.settle(key);
        if let Some(thread_id) =
            self.posts.kind(key).and_then(PostKind::thread_id).map(str::to_owned)
        {
            self.reply_open.remove(&thread_id);
            self.settle(&thread_id);
        }
    }

    /// Opens the reply composer of `thread_id` and gives it the keyboard focus.
    fn open_reply(&mut self, thread_id: &str) {
        self.reply_open.insert(thread_id.to_owned());
        self.focus = Some(drafts::reply_key(thread_id));
    }

    /// The failure row under a composer whose text stays in the editor: Retry sends it again, Edit
    /// clears the error to change the text, Fix token opens the connection's token fix.
    pub fn failure_actions(
        &mut self,
        ui: &mut egui::Ui,
        key: &str,
        message: &str,
        needs_token: bool,
    ) {
        match card::failure_row(ui, message, needs_token) {
            Some(card::FailureAction::Retry) => self.retry_requested = Some(key.to_owned()),
            Some(card::FailureAction::Edit) => {
                self.settle(key);
                if let Some(thread_id) = key.strip_prefix(drafts::REPLY_PREFIX) {
                    self.settle(thread_id);
                }
                self.posts.forget(key);
            }
            Some(card::FailureAction::FixToken) => {
                cv_runtime::intents::request_fix(ui.ctx(), &self.context.connection_id);
            }
            None => {}
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    /// A state without disk, for tests and benchmarks.
    pub fn for_tests(platform: ProviderKind) -> Self {
        let mut ui =
            Self::with_store(ThreadStore::in_memory(), ThreadContext::new(platform, "test"));
        ui.context.can_write = true;
        ui
    }
    #[cfg(any(test, feature = "test-support"))]
    /// The first visit was at `unix`: later comments are new.
    pub(crate) fn visited_at(&mut self, unix: i64) {
        self.store.start_visit(unix, std::iter::empty());
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_me(&mut self, me: UserIdentity) {
        self.me = Load::Ready(me);
    }
    #[cfg(test)]
    pub(crate) fn set_post_state(&mut self, key: &str, state: PostState, body: &str) {
        self.posts.insert_for_tests(key, state, body);
    }
    #[cfg(any(test, feature = "test-support"))]
    /// The start of a frame without a session: the clock, and the threads loaded at `version`.
    pub fn begin_frame_for_tests(
        &mut self,
        ctx: &egui::Context,
        threads: &[ReviewThread],
        version: u64,
    ) {
        self.now = ctx.input(|input| input.time);
        self.track_frame(Some(threads), version);
    }
    #[cfg(any(test, feature = "test-support"))]
    /// What the files tab does with a new thread the diff asked to post, without a platform.
    pub fn post_new_thread_for_tests(&mut self, key: &str, thread: NewThread) {
        self.settle(key);
        self.posts.start_for_tests(key, PostKind::Create(thread));
    }
    #[cfg(any(test, feature = "test-support"))]
    /// What a reply does, without a platform.
    pub fn reply_for_tests(&mut self, thread_id: &str, body: &str) {
        self.note_reply(thread_id);
        let kind = PostKind::Reply {
            thread_id: thread_id.to_owned(),
            body: body.to_owned(),
            resolve: false,
        };
        self.posts.start_for_tests(&drafts::reply_key(thread_id), kind);
    }
    #[cfg(any(test, feature = "test-support"))]
    /// The platform answered the post under `key` while the threads were at `version`.
    pub fn deliver_for_tests(&mut self, key: &str, delivery: posts::Delivery, version: u64) {
        let mut news = PostNews::default();
        self.posts.deliver_for_tests(key, delivery, version, &mut news);
        self.store.remove_draft(key);
        self.texts.remove(key);
        self.on_post_news(&news);
    }
    #[cfg(any(test, feature = "test-support"))]
    /// The platform refused the post under `key`.
    pub fn fail_for_tests(&mut self, key: &str, message: &str) {
        let mut news = PostNews::default();
        self.posts.fail_for_tests(key, message, &mut news);
        self.on_post_news(&news);
    }
    #[cfg(any(test, feature = "test-support"))]
    /// Retry was clicked on the failed post under `key` and the next frame sent it again.
    pub fn retry_for_tests(&mut self, key: &str) {
        self.retry_requested = Some(key.to_owned());
        self.take_retry();
        if matches!(self.posts.state(key), Some(PostState::Failed { .. }))
            && let Some(kind) = self.posts.kind(key).cloned()
        {
            self.posts.start_for_tests(key, kind);
            self.on_retry(key);
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    /// The code of `thread`'s excerpt loaded: `diff` and `text` of its file at ("base", "head").
    pub(crate) fn insert_code_for_tests(&mut self, thread: &ReviewThread, diff: &str, text: &str) {
        self.code.insert_for_tests(thread, "base", "head", diff, text);
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn open_reply_for_tests(&mut self, thread_id: &str, text: &str) {
        text.clone_into(self.text_mut(&drafts::reply_key(thread_id)));
        self.reply_open.insert(thread_id.to_owned());
    }
}

impl Drop for ThreadsUi {
    fn drop(&mut self) {
        // Nothing to report to: the page is gone. The next save attempt would show the error.
        let _unreported = self.store.save();
    }
}

/// Unix seconds of the newest comment.
fn newest(thread: &ReviewThread) -> Option<i64> {
    thread.comments.iter().map(|comment| comment.created_at.unix_timestamp()).max()
}

#[cfg(test)]
mod tests {
    use review_core::{ThreadComment, UserIdentity};

    use super::samples::{self, comment_at, lines, person};
    use super::*;

    fn comment(id: &str, author: &UserIdentity, unix: i64) -> ThreadComment {
        comment_at(id, author, unix, "text")
    }

    #[test]
    fn new_comments_are_from_other_people_after_the_last_visit() {
        let (jonas, mara, me) =
            (person("1", "Jonas Weber"), person("2", "Mara Lind"), person("3", "Erika"));
        let mut ui = ThreadsUi::for_tests(ProviderKind::GitHub);
        let read =
            samples::thread("t1", ThreadStatus::Active, None, vec![comment("a", &jonas, 100)]);
        assert_eq!(ui.new_comments(&read), None, "nothing is new before the first visit");
        assert!(!ui.is_unread(&read));
        ui.visited_at(150);
        ui.set_me(me.clone());
        let replied = samples::thread(
            "t1",
            ThreadStatus::Active,
            None,
            vec![
                comment("a", &jonas, 100),
                comment("b", &me, 160),
                comment("c", &mara, 170),
                comment("d", &jonas, 180),
            ],
        );
        let new = ui.new_comments(&replied).expect("two new replies");
        assert_eq!(
            (new.first, new.count, new.authors.clone(), new.is_new_thread()),
            (2, 2, vec!["Mara".to_owned(), "Jonas".to_owned()], false),
            "my own reply is not new"
        );
        assert!(ui.is_unread(&replied));
        let mine = samples::thread(
            "t1",
            ThreadStatus::Active,
            None,
            vec![comment("a", &jonas, 100), comment("b", &me, 160)],
        );
        assert!(!ui.is_unread(&mine), "my own reply alone is no news");
        ui.mark_read(&replied);
        assert_eq!(ui.new_comments(&replied), None);
        assert!(!ui.is_unread(&replied));
    }

    #[test]
    fn resolved_threads_start_collapsed_until_the_user_opens_them() {
        let mut ui = ThreadsUi::for_tests(ProviderKind::GitLab);
        let resolved = samples::thread("t1", ThreadStatus::Resolved, None, Vec::new());
        assert!(ui.is_collapsed(&resolved));
        ui.toggle_collapsed(&resolved);
        assert!(!ui.is_collapsed(&resolved));
        let active = samples::thread("t1", ThreadStatus::Active, None, Vec::new());
        assert!(!ui.is_collapsed(&active), "active threads start open");
    }

    #[test]
    fn drafts_are_kept_until_discarded() {
        let mut ui = ThreadsUi::for_tests(ProviderKind::AzureDevOps);
        ui.text_mut("reply:t1").push_str("Good point");
        ui.keep_draft("reply:t1", None);
        assert!(ui.has_draft("reply:t1"));
        ui.texts.clear();
        assert_eq!(ui.text_mut("reply:t1"), "Good point", "the store gives the text back");
        ui.discard_draft("reply:t1");
        assert!(!ui.has_draft("reply:t1"));
    }

    #[test]
    fn retry_sends_the_text_the_user_changed_after_the_failure() {
        let mut ui = ThreadsUi::for_tests(ProviderKind::GitLab);
        let reply = drafts::reply_key("t1");
        ui.open_reply_for_tests("t1", "Typo hree");
        ui.reply_for_tests("t1", "Typo hree");
        ui.fail_for_tests(&reply, "Server error");
        *ui.text_mut(&reply) = "Typo here".into();
        ui.retry_for_tests(&reply);
        assert_eq!(
            ui.pending_reply("t1").map(|comment| comment.body),
            Some("Typo here".to_owned()),
            "the reply as the composer shows it"
        );

        let key = new_thread_key(&lines("src/a.rs", 4, 4));
        *ui.text_mut(&key) = "Frist".into();
        ui.post_new_thread_for_tests(&key, NewThread { body: "Frist".into(), anchor: None });
        ui.fail_for_tests(&key, "Server error");
        *ui.text_mut(&key) = "First".into();
        ui.retry_for_tests(&key);
        assert_eq!(
            ui.pending_thread(&key).map(|thread| thread.comments[0].body.clone()),
            Some("First".to_owned()),
            "the new thread as its composer shows it"
        );
    }

    #[test]
    fn a_draft_moved_by_a_push_keeps_its_text_under_the_new_lines() {
        let mut ui = ThreadsUi::for_tests(ProviderKind::GitHub);
        let before = lines("src/a.rs", 12, 12);
        let after = review_core::ThreadAnchor { new_line: Some(15), ..before.clone() };
        let key = new_thread_key(&before);
        ui.text_mut(&key).push_str("Why not a slice?");
        ui.keep_draft(&key, Some(&before));
        assert!(
            !ui.has_unsent_input(),
            "a kept new-thread draft alone does not hold back live updates"
        );
        let moved = ui.move_draft(&key, &after);
        assert_eq!(moved, new_thread_key(&after));
        assert!(!ui.has_draft(&key), "no copy stays under the old lines");
        assert_eq!(ui.text_mut(&moved), "Why not a slice?");
        assert_eq!(ui.new_thread_drafts("src/a.rs"), vec![after]);
    }

    #[test]
    fn a_draft_moved_onto_another_kept_draft_keeps_both_texts() {
        let mut ui = ThreadsUi::for_tests(ProviderKind::GitHub);
        let before = lines("src/a.rs", 12, 12);
        let after = review_core::ThreadAnchor { new_line: Some(15), ..before.clone() };
        let kept = new_thread_key(&after);
        ui.text_mut(&kept).push_str("Rename this.");
        ui.keep_draft(&kept, Some(&after));
        ui.texts.remove(&kept);
        let open = new_thread_key(&before);
        ui.text_mut(&open).push_str("Why not a slice?");
        ui.keep_draft(&open, Some(&before));
        let moved = ui.move_draft(&open, &after);
        assert_eq!(moved, kept);
        assert_eq!(ui.text_mut(&moved), "Rename this.\n\nWhy not a slice?");
        assert_eq!(ui.new_thread_drafts("src/a.rs"), vec![after]);
    }
}
