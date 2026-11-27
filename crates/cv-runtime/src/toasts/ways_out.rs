//! The buttons of toasts, and the ways out every error toast must carry ([`WaysOut`], never
//! empty, so an error that leaves the user nowhere to go does not compile).
//!
//! One kind per way out, the same everywhere:
//! - [`ToastAction::retry`] asks the failed work's owner to run it once more: the owner takes the
//!   request with [`take_retry`] on its next frame and starts exactly what failed.
//! - [`ToastAction::fix_token`] opens the connection setup at its token step (401/403), and
//!   [`ToastAction::add_connection`] the setup of a new connection a link needs.
//! - [`ToastAction::open_on_platform`] and [`ToastAction::open_in_browser`] lead to the web page
//!   when the platform can do what the app could not.
//! - [`ToastAction::open_folder`] shows the folder a disk failure happened in.
//! - [`ToastAction::copy`] hands over a path or a command to use elsewhere.

use std::path::PathBuf;

use cv_base::mailbox;
use cv_base::theme::icons;
use cv_widgets::{provider_icon, states};
use egui::Id;
use review_core::{Connection, ProviderError, ProviderKind};

use crate::intents::{self, AddRequest};
use crate::links::pull_request_link;
use crate::route::PullRequestKey;

/// A button in a toast. Clicking it runs it once and dismisses the toast; actions that need more
/// than the context leave a request there for their owner, like [`take_retry`].
pub struct ToastAction {
    pub(super) icon: &'static str,
    pub(super) label: String,
    pub(super) run: Run,
}

/// What a toast's button does.
pub(super) enum Run {
    Context(Box<dyn FnOnce(&egui::Context)>),
    /// Opened by the toasts, which report when the folder does not open.
    OpenFolder(PathBuf),
}

/// What [`ToastAction::retry`] leaves in the context; its own type keeps the id's slot apart.
#[derive(Clone, Copy)]
struct RetryAsked;

impl ToastAction {
    pub fn new(
        icon: &'static str,
        label: impl Into<String>,
        run: impl FnOnce(&egui::Context) + 'static,
    ) -> Self {
        Self { icon, label: label.into(), run: Run::Context(Box::new(run)) }
    }

    /// "Retry": leaves `id` for the owner of the failed work, which runs it once more when it
    /// takes the request ([`take_retry`]).
    pub fn retry(id: Id) -> Self {
        Self::new(icons::ARROW_CLOCKWISE, "Retry", move |ctx| mailbox::post(ctx, id, RetryAsked))
    }

    /// "Fix token": the connection setup of `connection_id` at its token step.
    pub fn fix_token(connection_id: impl Into<String>) -> Self {
        let connection_id = connection_id.into();
        Self::new(icons::KEY, "Fix token", move |ctx| {
            intents::request_fix(ctx, &connection_id);
        })
    }

    /// "Add connection": the connection setup for a platform's cloud host, prefilled.
    pub fn add_connection(request: AddRequest) -> Self {
        Self::new(icons::PLUS, "Add connection", move |ctx| intents::request_add(ctx, request))
    }

    /// "Open on GitHub" and the like: `url` on its platform.
    pub fn open_on_platform(kind: ProviderKind, url: impl Into<String>) -> Self {
        let url = url.into();
        Self::new(provider_icon::glyph(kind), states::open_on_platform(kind), move |ctx| {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        })
    }

    /// The pull request `key` on its platform: at `known_url` when the page has it, else at the
    /// address its `connection` gives; `None` once the connection is gone (callers pass
    /// `services.connection(&key.connection_id)`).
    pub fn open_pull_request(
        connection: Option<&Connection>,
        key: &PullRequestKey,
        known_url: Option<&str>,
    ) -> Option<Self> {
        let connection = connection?;
        let url = known_url.map_or_else(
            || pull_request_link::web_url(connection, &key.repository_key, key.number),
            str::to_owned,
        );
        Some(Self::open_on_platform(connection.kind, url))
    }

    /// "Open in browser": `url` on a host no connection names a platform for.
    pub fn open_in_browser(url: impl Into<String>) -> Self {
        let url = url.into();
        Self::new(icons::ARROW_SQUARE_OUT, "Open in browser", move |ctx| {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        })
    }

    /// "Open folder": `folder` in the system's file manager.
    pub fn open_folder(folder: impl Into<PathBuf>) -> Self {
        Self {
            icon: icons::FOLDER_OPEN,
            label: "Open folder".to_owned(),
            run: Run::OpenFolder(folder.into()),
        }
    }

    /// "Copy path", "Copy command": `text` on the clipboard.
    pub fn copy(label: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        Self::new(icons::COPY, label, move |ctx| ctx.copy_text(text))
    }
}

/// Whether a toast's Retry asked for the work behind `id` since the last call; true once per
/// click.
pub fn take_retry(ctx: &egui::Context, id: Id) -> bool {
    mailbox::take::<RetryAsked>(ctx, id).is_some()
}

/// The buttons of an error toast: at least one, the most direct way out first.
pub struct WaysOut(Vec<ToastAction>);

impl WaysOut {
    pub fn new(first: ToastAction) -> Self {
        Self(vec![first])
    }

    #[must_use]
    pub fn and(mut self, action: ToastAction) -> Self {
        self.0.push(action);
        self
    }

    /// Adds `action` when there is one.
    #[must_use]
    pub fn and_some(self, action: Option<ToastAction>) -> Self {
        match action {
            Some(action) => self.and(action),
            None => self,
        }
    }

    /// The ways out in `actions`, `None` when there is none.
    pub(crate) fn from_vec(actions: Vec<ToastAction>) -> Option<Self> {
        (!actions.is_empty()).then_some(Self(actions))
    }

    pub(super) fn into_actions(self) -> Vec<ToastAction> {
        self.0
    }
}

impl From<ToastAction> for WaysOut {
    fn from(action: ToastAction) -> Self {
        Self::new(action)
    }
}

/// Why a request to a platform failed, and whether the platform refused the token (401/403),
/// which offers "Fix token".
#[derive(Debug, Clone)]
pub struct ProviderFailure {
    pub message: String,
    pub needs_token: bool,
}

impl From<ProviderError> for ProviderFailure {
    fn from(error: ProviderError) -> Self {
        Self { needs_token: matches!(error.status(), Some(401 | 403)), message: error.to_string() }
    }
}

impl ProviderFailure {
    /// A failure the platform did not answer, e.g. a background task that stopped.
    pub fn local(message: String) -> Self {
        Self { message, needs_token: false }
    }

    /// The ways out of this failure on a platform: `retry` when the request can run again, "Fix
    /// token" for `connection_id` when the token was refused, and `page` (the item on its
    /// platform). `None` when none of them applies.
    pub fn ways_out(
        &self,
        retry: Option<ToastAction>,
        connection_id: &str,
        page: Option<ToastAction>,
    ) -> Option<WaysOut> {
        let fix = self.fix_token(connection_id);
        WaysOut::from_vec(retry.into_iter().chain(fix).chain(page).collect())
    }

    /// "Fix token" for `connection_id` when the platform refused the token.
    pub fn fix_token(&self, connection_id: &str) -> Option<ToastAction> {
        self.needs_token.then(|| ToastAction::fix_token(connection_id))
    }
}

#[cfg(test)]
mod tests {
    use review_core::ProviderError;

    use super::{ProviderFailure, ToastAction, WaysOut};

    fn labels(ways_out: &WaysOut) -> Vec<&str> {
        ways_out.0.iter().map(|action| action.label.as_str()).collect()
    }

    #[test]
    fn provider_failure_offers_fix_token_only_when_refused() {
        let refused = ProviderFailure::from(ProviderError::Status {
            status: 403,
            path: "/repos".into(),
            body: "Resource not accessible".into(),
        });
        let ways_out = refused
            .ways_out(Some(ToastAction::retry(egui::Id::unique("r"))), "gh", None)
            .expect("Retry and Fix token");
        assert_eq!(labels(&ways_out), ["Retry", "Fix token"]);

        let lost = ProviderFailure::local("stopped".into());
        assert!(lost.ways_out(None, "gh", None).is_none(), "no way out without retry or page");
        let page = ToastAction::open_on_platform(review_core::ProviderKind::GitHub, "https://x");
        let ways_out = lost.ways_out(None, "gh", Some(page)).expect("the page");
        assert_eq!(labels(&ways_out), ["Open on GitHub"]);
    }
}
