//! What the cards read about the pull request each frame: its platform, the user's rights on it,
//! its author and its web page. `ThreadsUi::begin_frame` fills it from the session.

use std::sync::Arc;

use review_core::{Provider, ProviderKind};

/// What the cards need to know about the pull request this frame.
#[derive(Clone)]
pub struct ThreadContext {
    pub platform: ProviderKind,
    pub can_write: bool,
    /// The pull request's author, who gets the "Author" badge.
    pub author_id: Option<String>,
    /// The pull request's web page, for thread links.
    pub web_url: Option<String>,
    pub connection_id: String,
    /// The pull request's platform, for thread links and what the user may do per comment.
    pub(crate) provider: Option<Arc<dyn Provider>>,
}

impl ThreadContext {
    pub(crate) fn new(platform: ProviderKind, connection_id: &str) -> Self {
        Self {
            platform,
            can_write: false,
            author_id: None,
            web_url: None,
            connection_id: connection_id.to_owned(),
            provider: None,
        }
    }
}
