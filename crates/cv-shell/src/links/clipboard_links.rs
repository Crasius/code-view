//! Opens a pull request whose link was copied in another app. When the window comes to the front
//! (and once at start) the clipboard is read off the UI thread; a link a saved connection serves
//! is checked first ([`super::link_access`]): one read of the pull request's detail with that
//! connection's token. A readable one opens like any navigation, so Back returns, and its page
//! reuses that read. One the token cannot read (401, 403, 404) stays closed, and a toast says why
//! with the ways out: the page on its platform and the connection's token setup. A host without a
//! clear answer (5xx, no answer in time) opens the page as before, which reports the failure
//! itself. Nothing reads while the window is in the background.
//!
//! Each copy is looked at once: opened, refused or offered, it is not looked at again, so a
//! refused link does not toast on every return to the window.
//!
//! Privacy: the clipboard's text is never logged or kept. Only a hash of the last text seen stays
//! in memory, and the canonical URL of the last pull request looked at is saved in the settings,
//! so a restart does not open it again.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use egui::Id;
use review_core::{Connection, ProviderKind};

use cv_base::mailbox;
use cv_base::theme::icons;
use cv_runtime::background::{Task, TaskPoll};
use cv_runtime::intents::AddRequest;
use cv_runtime::route::{PullRequestKey, Route};
use cv_runtime::toasts::{ToastAction, Toasts, WaysOut};
use cv_services::Services;

use super::link_access::{self, AccessCheck, Denial, Target, Verdict};

/// Longer texts are no link someone copied to open; they are not looked at.
pub const MAX_TEXT: usize = 2048;

const REQUEST_ID: &str = "review.clipboard_links.request";

/// Where copied text comes from: the system clipboard, or a fake in tests.
pub trait ClipboardSource: Send + Sync {
    fn read_text(&self) -> Option<String>;
}

/// The system clipboard. A new handle per read: reads happen only on focus changes.
pub struct SystemClipboard;

impl ClipboardSource for SystemClipboard {
    fn read_text(&self) -> Option<String> {
        arboard::Clipboard::new().ok()?.get_text().ok()
    }
}

/// What one copied URL names. Anything else (several words, texts over [`MAX_TEXT`]) is
/// [`Target::NotAPullRequest`].
pub fn classify(text: &str, connections: &[Connection]) -> Target {
    let url = text.trim();
    let is_one_url = (url.starts_with("https://") || url.starts_with("http://"))
        && !url.contains(char::is_whitespace);
    if text.len() > MAX_TEXT || !is_one_url {
        return Target::NotAPullRequest;
    }
    link_access::locate(url, connections)
}

/// The host a setup for `request` connects to, for the toast: `github.com`, `dev.azure.com/contoso`.
pub fn host_label(request: &AddRequest) -> String {
    let base = request.kind.default_base_url();
    let host = base.split_once("://").map_or(base, |(_, host)| host);
    match (&request.organization, request.kind) {
        (Some(organization), ProviderKind::AzureDevOps) => format!("{host}/{organization}"),
        _ => host.to_owned(),
    }
}

/// What a toast button about a copied link asks the app for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Open(PullRequestKey),
    Back,
    TurnOff,
    Settings,
}

/// Leaves `request` for the app, which takes it before the next page runs.
pub fn request(ctx: &egui::Context, request: Request) {
    mailbox::post(ctx, Id::unique(REQUEST_ID), request);
}

/// The pending [`request`], at most once.
pub fn take_request(ctx: &egui::Context) -> Option<Request> {
    mailbox::take::<Request>(ctx, Id::unique(REQUEST_ID))
}

/// "Opened … from the clipboard", with Back and Turn off; the app opens `key` with it.
pub fn announce_opened(toasts: &mut Toasts, key: &PullRequestKey) {
    let back = ToastAction::new(icons::ARROW_LEFT, "Back", |ctx| request(ctx, Request::Back));
    let turn_off =
        ToastAction::new(icons::PROHIBIT, "Turn off", |ctx| request(ctx, Request::TurnOff));
    toasts.info(format!("Opened {} from the clipboard", short_name(key)), vec![back, turn_off]);
}

/// The page has input that leaving would lose: `key` waits behind an Open button.
pub fn offer_later(toasts: &mut Toasts, key: PullRequestKey) {
    let text =
        format!("You copied a link to {}. Open it once you are done here.", short_name(&key));
    let open =
        ToastAction::new(icons::ARROW_RIGHT, "Open", move |ctx| request(ctx, Request::Open(key)));
    toasts.info(text, vec![open]);
}

/// Saves the choice to turn copied links off, and says where it turns on again.
pub fn turn_off(services: &mut Services) {
    services.settings.open_clipboard_links = false;
    services.save_settings();
    let settings = ToastAction::new(icons::GEAR, "Settings", |ctx| request(ctx, Request::Settings));
    services.toasts.info(
        "Copied pull request links no longer open. Settings → Links turns it back on.",
        vec![settings],
    );
}

/// `widgets #7`: the repository's last segment and the number.
fn short_name(key: &PullRequestKey) -> String {
    format!("{} #{}", key.repository_key.rsplit('/').next().unwrap_or_default(), key.number)
}

/// Reads the clipboard when the window gains focus, checks a copied pull request link and hands
/// a readable one to the app.
pub struct ClipboardWatch {
    /// `None` reads nothing: tests start without a clipboard and set a fake one.
    source: Option<Arc<dyn ClipboardSource>>,
    was_focused: bool,
    reading: Option<Task<Option<String>>>,
    /// Hash of the last text seen, read or copied in the app; never the text itself.
    last_seen: Option<u64>,
    /// The access check of the last copied link; a newer copy replaces it.
    checking: Option<AccessCheck>,
}

impl ClipboardWatch {
    pub fn new(source: Option<Arc<dyn ClipboardSource>>) -> Self {
        Self { source, was_focused: false, reading: None, last_seen: None, checking: None }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_source(&mut self, source: Arc<dyn ClipboardSource>) {
        self.source = Some(source);
    }

    /// Looks at a newly copied text and polls the check of the last link: returns the pull request
    /// to open, once its check let it through and the app is not on it already. Toasts about links
    /// that do not open go out here.
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        services: &mut Services,
        route: &Route,
    ) -> Option<PullRequestKey> {
        let is_enabled = services.settings.open_clipboard_links;
        // Turned off while the clipboard was read: the text is dropped.
        if let Some(text) = self.poll(ctx, services, is_enabled)
            && is_enabled
        {
            self.look_at(&text, services, route);
            // The toasts of this frame are drawn already.
            ctx.request_repaint();
        }
        let access = self.checking.as_mut()?.poll(services)?;
        self.checking = None;
        ctx.request_repaint();
        let key = match access {
            Verdict::Open(key) | Verdict::Unknown(key) => key,
            Verdict::NoAccess { key, denial } => {
                refuse(services, &key, denial);
                return None;
            }
        };
        let is_open = *route == Route::PullRequest(key.clone());
        let is_wanted = services.settings.open_clipboard_links && !is_open;
        is_wanted.then_some(key)
    }

    /// A copied link to a pull request starts its check, unless it was looked at before or is open
    /// already; one to an unconnected cloud host offers the setup.
    fn look_at(&mut self, text: &str, services: &mut Services, route: &Route) {
        match classify(text, services.connections.all()) {
            Target::PullRequest { key, web_url } => {
                if services.settings.last_clipboard_link.as_deref() == Some(web_url.as_str()) {
                    return;
                }
                services.settings.last_clipboard_link = Some(web_url);
                services.save_settings();
                if *route != Route::PullRequest(key.clone()) {
                    self.checking = Some(AccessCheck::start(services, key));
                }
            }
            Target::NotConnected(request) => {
                let text = format!(
                    "The copied link is for {}, which has no connection yet.",
                    host_label(&request)
                );
                services.toasts.info(text, vec![ToastAction::add_connection(request)]);
            }
            Target::NotAPullRequest => {}
        }
    }

    /// Starts a read in the frame the window comes to the front, when `is_enabled`; returns the
    /// text once read, if it changed since the last look. One cheap comparison per frame otherwise.
    fn poll(
        &mut self,
        ctx: &egui::Context,
        services: &Services,
        is_enabled: bool,
    ) -> Option<String> {
        let is_focused = ctx.input(|input| input.focused);
        let has_gained_focus = is_focused && !self.was_focused;
        self.was_focused = is_focused;
        if has_gained_focus
            && is_enabled
            && self.reading.is_none()
            && let Some(source) = self.source.clone()
        {
            let read = services.background.spawn(async move {
                tokio::task::spawn_blocking(move || source.read_text()).await.ok().flatten()
            });
            self.reading = Some(read);
        }
        let text = match self.reading.as_mut()?.poll() {
            TaskPoll::Pending => return None,
            TaskPoll::Done(text) => text,
            TaskPoll::Lost => None,
        };
        self.reading = None;
        let text = text.filter(|text| text.len() <= MAX_TEXT)?;
        let hash = hash(&text);
        if self.last_seen == Some(hash) {
            return None;
        }
        self.last_seen = Some(hash);
        Some(text)
    }

    /// What the app itself copied this frame ("Copy link", Ctrl+C) counts as seen: it never
    /// opens anything when the window comes back.
    pub fn note_own_copies(&mut self, ctx: &egui::Context) {
        ctx.output(|output| {
            for command in &output.commands {
                if let egui::OutputCommand::CopyText(text) = command {
                    self.last_seen = Some(hash(text));
                }
            }
        });
    }
}

/// The toast for a copied link the connection's token cannot read: why, then the page on its
/// platform and the token setup of that connection.
pub(crate) fn refuse(services: &mut Services, key: &PullRequestKey, denial: Denial) {
    let Some(connection) = services.connection(&key.connection_id) else {
        return;
    };
    let (name, connection) = (short_name(key), &connection.name);
    let text = match denial {
        Denial::NotFound => format!(
            "{name} did not open: connection {connection} cannot find it. It does not exist, or \
             the token cannot see its repository."
        ),
        Denial::Forbidden => {
            format!(
                "{name} did not open: the token of connection {connection} has no access to it."
            )
        }
        Denial::TokenRefused => {
            format!("{name} did not open: the host refused the token of connection {connection}.")
        }
    };
    let fix = ToastAction::fix_token(&key.connection_id);
    let ways_out =
        match ToastAction::open_pull_request(services.connection(&key.connection_id), key, None) {
            Some(page) => WaysOut::new(page).and(fix),
            None => fix.into(),
        };
    services.toasts.error(text, ways_out);
}

fn hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use review_core::{Connection, ProviderKind};

    use super::{MAX_TEXT, classify, host_label};
    use cv_runtime::intents::AddRequest;
    use cv_testkit::samples::connection;

    use crate::links::link_access::Target;

    fn connections() -> Vec<Connection> {
        vec![
            connection("ghe", ProviderKind::GitHub, "https://git.corp.example"),
            connection("gl-self", ProviderKind::GitLab, "https://gitlab.corp.example"),
            connection("ado", ProviderKind::AzureDevOps, "https://dev.azure.com"),
            connection("ado-server", ProviderKind::AzureDevOps, "https://tfs.corp.example/tfs"),
        ]
    }

    fn opened(text: &str) -> Option<(String, String, u64, String)> {
        match classify(text, &connections()) {
            Target::PullRequest { key, web_url } => {
                Some((key.connection_id, key.repository_key, key.number, web_url))
            }
            Target::NotConnected(_) | Target::NotAPullRequest => None,
        }
    }

    #[test]
    fn copied_links_of_every_platform_open_on_their_connection() {
        let cases = [
            (
                "https://git.corp.example/team/app/pull/8",
                "ghe",
                "team/app",
                8,
                "https://git.corp.example/team/app/pull/8",
            ),
            (
                "  https://git.corp.example/team/app/pull/8/files#diff-1\n",
                "ghe",
                "team/app",
                8,
                "https://git.corp.example/team/app/pull/8",
            ),
            (
                "https://gitlab.corp.example/a/b/c/-/merge_requests/4/diffs",
                "gl-self",
                "a/b/c",
                4,
                "https://gitlab.corp.example/a/b/c/-/merge_requests/4",
            ),
            (
                "https://dev.azure.com/org/Proj/_git/repo/pullrequest/41?_a=files",
                "ado",
                "org/Proj/repo",
                41,
                "https://dev.azure.com/org/Proj/_git/repo/pullrequest/41",
            ),
            (
                "https://org.visualstudio.com/Proj/_git/repo/pullrequest/42",
                "ado",
                "org/Proj/repo",
                42,
                "https://dev.azure.com/org/Proj/_git/repo/pullrequest/42",
            ),
            (
                "https://tfs.corp.example/tfs/Coll/Proj/_git/repo/pullrequest/43",
                "ado-server",
                "Coll/Proj/repo",
                43,
                "https://tfs.corp.example/tfs/Coll/Proj/_git/repo/pullrequest/43",
            ),
        ];
        for (text, connection_id, repository_key, number, web_url) in cases {
            assert_eq!(
                opened(text),
                Some((
                    connection_id.to_owned(),
                    repository_key.to_owned(),
                    number,
                    web_url.to_owned()
                )),
                "{text}"
            );
        }
    }

    #[test]
    fn other_texts_open_nothing() {
        let long = format!("https://git.corp.example/team/app/pull/8?{}", "a".repeat(MAX_TEXT));
        for text in [
            "",
            "git.corp.example/team/app/pull/8",
            "see https://git.corp.example/team/app/pull/8",
            "https://git.corp.example/team/app/pull/8 https://git.corp.example/team/app/pull/9",
            "https://git.corp.example/team/app/issues/8",
            "https://example.org/team/app/pull/8",
            "https://other.example/a/b/-/merge_requests/1",
            &long,
        ] {
            assert_eq!(classify(text, &connections()), Target::NotAPullRequest, "{text}");
        }
    }

    #[test]
    fn cloud_links_without_a_connection_offer_one() {
        let cases = [
            ("https://github.com/cli/cli/pull/3", ProviderKind::GitHub, None, "github.com"),
            (
                "https://gitlab.com/group/app/-/merge_requests/4",
                ProviderKind::GitLab,
                None,
                "gitlab.com",
            ),
        ];
        for (text, kind, organization, host) in cases {
            let request = AddRequest { kind, organization };
            assert_eq!(
                classify(text, &connections()),
                Target::NotConnected(request.clone()),
                "{text}"
            );
            assert_eq!(host_label(&request), host);
        }
        let only_github = [connection("gh", ProviderKind::GitHub, "https://github.com")];
        let azure =
            classify("https://contoso.visualstudio.com/Web/_git/site/pullrequest/6", &only_github);
        let request = AddRequest {
            kind: ProviderKind::AzureDevOps,
            organization: Some("contoso".to_owned()),
        };
        assert_eq!(azure, Target::NotConnected(request.clone()));
        assert_eq!(host_label(&request), "dev.azure.com/contoso");
    }
}
