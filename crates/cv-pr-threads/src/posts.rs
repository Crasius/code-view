//! Comments on their way to the platform. Each post keeps its text until the platform has it and
//! the reloaded threads show it: a failure leaves the text, the reason and a retry on screen
//! instead of a toast that takes the text with it. Once the platform answered, the post keeps the
//! copy it sent back, so the pending comment shows the platform's ids and metadata until the
//! reloaded threads take over.

use std::collections::HashMap;

use cv_pr_session::session::PullRequestSession;
use cv_runtime::load::Load;
use cv_services::Services;
use cv_services::connection::access_issues::AccessIssues;
use review_core::{NewThread, ReviewThread, ThreadComment, ThreadStatus};
use review_store::ThreadStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostState {
    Sending,
    /// `needs_token`: the platform refused the token, so "Fix token" is the way out.
    Failed {
        message: String,
        needs_token: bool,
    },
    /// The platform has it; shown until the threads loaded after `after_version` include it.
    Sent {
        after_version: u64,
    },
}

impl PostState {
    pub fn is_pending(&self) -> bool {
        matches!(self, PostState::Sending | PostState::Sent { .. })
    }
}

#[derive(Clone)]
pub enum PostKind {
    Reply { thread_id: String, body: String, resolve: bool },
    Create(NewThread),
}

impl PostKind {
    fn body(&self) -> &str {
        match self {
            PostKind::Reply { body, .. } => body,
            PostKind::Create(thread) => &thread.body,
        }
    }

    /// The confirmation once the platform has it.
    fn done(&self) -> &'static str {
        match self {
            PostKind::Reply { .. } => "Reply posted.",
            PostKind::Create(thread) if thread.anchor.is_some() => "Comment added.",
            PostKind::Create(_) => "Comment posted.",
        }
    }

    /// The thread whose card changes with this post: the one replied to; `None` for a new thread,
    /// whose card is known by its draft key until the platform names it.
    pub fn thread_id(&self) -> Option<&str> {
        match self {
            PostKind::Reply { thread_id, .. } => Some(thread_id),
            PostKind::Create(_) => None,
        }
    }
}

/// What the platform sent back for a post.
#[derive(Default)]
pub struct Delivery {
    /// The new thread as the platform stored it.
    pub thread: Option<ReviewThread>,
    /// The reply as the platform stored it.
    pub comment: Option<ThreadComment>,
    /// The reply went out but resolving the thread failed, with the reason.
    pub warning: Option<String>,
}

/// What finished posts have to say: confirmations, warnings for replies whose thread could not
/// be resolved afterwards, and the posts that changed state (by draft key).
#[derive(Default)]
pub struct PostNews {
    pub done: Vec<&'static str>,
    pub warnings: Vec<String>,
    pub delivered: Vec<String>,
    pub failed: Vec<String>,
}

/// A post the reloaded threads now show: its draft key, and the thread it created or answered.
pub struct Landed {
    pub key: String,
    pub thread_id: Option<String>,
}

struct Post {
    kind: PostKind,
    state: PostState,
    load: Load<Delivery>,
    /// When the user sent it, for the pending comment's time.
    sent_at: time::OffsetDateTime,
    /// The platform's copy, once it answered.
    delivery: Option<Delivery>,
}

#[derive(Default)]
pub struct Posts {
    by_key: HashMap<String, Post>,
}

impl Posts {
    pub fn start(
        &mut self,
        key: &str,
        kind: PostKind,
        session: &PullRequestSession,
        services: &Services,
    ) {
        let mut load = Load::Idle;
        load.start(&services.background, send(kind.clone(), session));
        self.insert(key, kind, load);
    }

    fn insert(&mut self, key: &str, kind: PostKind, load: Load<Delivery>) {
        let post = Post {
            kind,
            state: PostState::Sending,
            load,
            sent_at: time::OffsetDateTime::now_utc(),
            delivery: None,
        };
        self.by_key.insert(key.to_owned(), post);
    }

    /// The text a failed post goes out with on Retry: its composer stays editable after the
    /// failure, so the text there is the one to send.
    pub fn set_failed_body(&mut self, key: &str, text: &str) {
        let Some(post) =
            self.by_key.get_mut(key).filter(|post| matches!(post.state, PostState::Failed { .. }))
        else {
            return;
        };
        let body = match &mut post.kind {
            PostKind::Reply { body, .. } => body,
            PostKind::Create(thread) => &mut thread.body,
        };
        text.clone_into(body);
    }

    pub fn retry(&mut self, key: &str, session: &PullRequestSession, services: &Services) {
        let Some(post) =
            self.by_key.get_mut(key).filter(|post| matches!(post.state, PostState::Failed { .. }))
        else {
            return;
        };
        post.load.start(&services.background, send(post.kind.clone(), session));
        post.state = PostState::Sending;
        post.sent_at = time::OffsetDateTime::now_utc();
    }

    /// Moves finished posts on. Returns true when one reached the platform (the threads need a
    /// reload); `news` gets a confirmation per delivered post and the keys that changed state.
    pub fn poll(
        &mut self,
        store: &mut ThreadStore,
        texts: &mut HashMap<String, String>,
        issues: &AccessIssues,
        connection_id: &str,
        version: u64,
        news: &mut PostNews,
    ) -> bool {
        let mut delivered = false;
        for (key, post) in &mut self.by_key {
            match post.load.take_finished() {
                Some(Ok(delivery)) => {
                    deliver(key, post, delivery, version, news);
                    store.remove_draft(key);
                    texts.remove(key);
                    delivered = true;
                }
                Some(Err(message)) => {
                    fail(key, post, message, issues.get(connection_id).is_some(), news);
                }
                None => {}
            }
        }
        delivered
    }

    /// Drops sent posts the threads loaded at `version` show, and says which they were.
    pub fn forget_delivered(&mut self, version: u64) -> Vec<Landed> {
        let has_landed = |post: &Post| matches!(post.state, PostState::Sent { after_version } if version > after_version);
        let landed: Vec<String> = self
            .by_key
            .iter()
            .filter(|(_, post)| has_landed(post))
            .map(|(key, _)| key.clone())
            .collect();
        landed
            .into_iter()
            .filter_map(|key| {
                let post = self.by_key.remove(&key)?;
                let created =
                    post.delivery.and_then(|delivery| delivery.thread).map(|thread| thread.id);
                let thread_id = post.kind.thread_id().map(str::to_owned).or(created);
                Some(Landed { key, thread_id })
            })
            .collect()
    }

    /// Drops a failed post; its draft stays for the user to edit.
    pub fn forget(&mut self, key: &str) {
        self.by_key.remove(key);
    }

    pub fn state(&self, key: &str) -> Option<&PostState> {
        self.by_key.get(key).map(|post| &post.state)
    }

    pub fn kind(&self, key: &str) -> Option<&PostKind> {
        self.by_key.get(key).map(|post| &post.kind)
    }

    /// Whether a post is on its way to the platform.
    pub fn is_sending(&self) -> bool {
        self.by_key.values().any(|post| post.state == PostState::Sending)
    }

    pub fn body(&self, key: &str) -> Option<&str> {
        self.by_key.get(key).map(|post| post.kind.body())
    }

    pub fn sent_at(&self, key: &str) -> Option<time::OffsetDateTime> {
        self.by_key.get(key).map(|post| post.sent_at)
    }

    /// The new thread as the platform stored it, once it answered.
    pub fn created(&self, key: &str) -> Option<&ReviewThread> {
        self.by_key.get(key)?.delivery.as_ref()?.thread.as_ref()
    }

    /// The reply as the platform stored it, once it answered.
    pub fn replied(&self, key: &str) -> Option<&ThreadComment> {
        self.by_key.get(key)?.delivery.as_ref()?.comment.as_ref()
    }

    /// Keys of the new threads on their way (not failed), in the order they were sent.
    pub fn pending_creates(&self) -> Vec<&str> {
        let mut keys: Vec<(&str, time::OffsetDateTime)> = self
            .by_key
            .iter()
            .filter(|(_, post)| post.state.is_pending() && matches!(post.kind, PostKind::Create(_)))
            .map(|(key, post)| (key.as_str(), post.sent_at))
            .collect();
        keys.sort_by_key(|(key, sent_at)| (*sent_at, *key));
        keys.into_iter().map(|(key, _)| key).collect()
    }

    #[cfg(any(test, feature = "test-support"))]
    /// A post on its way, without a platform behind it.
    pub fn start_for_tests(&mut self, key: &str, kind: PostKind) {
        self.insert(key, kind, Load::Idle);
    }

    #[cfg(any(test, feature = "test-support"))]
    /// The platform answered the post under `key` at threads `version`.
    pub fn deliver_for_tests(
        &mut self,
        key: &str,
        delivery: Delivery,
        version: u64,
        news: &mut PostNews,
    ) {
        if let Some(post) = self.by_key.get_mut(key) {
            deliver(key, post, delivery, version, news);
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    /// The platform refused the post under `key`.
    pub fn fail_for_tests(&mut self, key: &str, message: &str, news: &mut PostNews) {
        if let Some(post) = self.by_key.get_mut(key) {
            fail(key, post, message.to_owned(), false, news);
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn insert_for_tests(&mut self, key: &str, state: PostState, body: &str) {
        let kind = PostKind::Reply {
            thread_id: key.strip_prefix(super::drafts::REPLY_PREFIX).unwrap_or_default().to_owned(),
            body: body.to_owned(),
            resolve: false,
        };
        self.insert(key, kind, Load::Idle);
        if let Some(post) = self.by_key.get_mut(key) {
            post.state = state;
        }
    }
}

/// The platform has the post: its copy is kept for the pending comment.
fn deliver(key: &str, post: &mut Post, delivery: Delivery, version: u64, news: &mut PostNews) {
    match &delivery.warning {
        Some(reason) => news
            .warnings
            .push(format!("Reply posted, but the thread could not be resolved: {reason}")),
        None => news.done.push(post.kind.done()),
    }
    news.delivered.push(key.to_owned());
    post.state = PostState::Sent { after_version: version };
    post.delivery = Some(delivery);
}

/// The platform refused the post: its text stays with the reason and Retry.
fn fail(key: &str, post: &mut Post, message: String, needs_token: bool, news: &mut PostNews) {
    post.state = PostState::Failed { message, needs_token };
    news.failed.push(key.to_owned());
}

fn send(
    kind: PostKind,
    session: &PullRequestSession,
) -> impl Future<Output = review_core::Result<Delivery>> + Send + 'static {
    let provider = session.provider.clone();
    let repository = session.key.repository_key.clone();
    let number = session.key.number;
    async move {
        match kind {
            PostKind::Reply { thread_id, body, resolve } => {
                let comment = provider.reply(&repository, number, &thread_id, &body).await?;
                if !resolve {
                    return Ok(Delivery { comment: Some(comment), ..Delivery::default() });
                }
                // The reply is out: a failed resolve must not make a retry post it twice.
                let resolved = provider
                    .set_thread_status(&repository, number, &thread_id, ThreadStatus::Resolved)
                    .await;
                Ok(Delivery {
                    comment: Some(comment),
                    warning: resolved.err().map(|error| error.to_string()),
                    ..Delivery::default()
                })
            }
            PostKind::Create(thread) => {
                let created = provider.create_thread(&repository, number, &thread).await?;
                Ok(Delivery { thread: Some(created), ..Delivery::default() })
            }
        }
    }
}
