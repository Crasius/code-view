//! Links that reach the window from outside: a later start's link (the browser extension's native
//! host, `CodeView <url>`) and this start's command-line link. A pull request link of a saved
//! connection opens only once its token read it, the same check a copied link takes
//! ([`link_access`]); a refused one opens no page and says why, with the page on its platform and
//! the token setup ([`link_access::announce_refusal`]). Any other link opens as a typed one
//! does, whose error toast offers the connection setup or the browser.

use cv_runtime::route::PullRequestKey;
use cv_services::Services;

use crate::links::link_access::{self, AccessCheck, Target, Verdict};

/// What the app does with a forwarded link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Forwarded {
    /// A pull request its connection may open; with no clear answer from the host it opens too,
    /// and its page shows the failure with its own Retry.
    Open(PullRequestKey),
    /// Not a pull request of a saved connection: opened as a typed link.
    Elsewhere(String),
}

/// The access checks of the forwarded pull request links still under way.
#[derive(Default)]
pub struct ForwardedLinks {
    checks: Vec<AccessCheck>,
}

impl ForwardedLinks {
    /// Takes the links queued since the last frame and polls the checks under way; returns what
    /// to open, oldest first. Refused links get their toast here.
    pub fn update(&mut self, ctx: &egui::Context, services: &mut Services) -> Vec<Forwarded> {
        let mut ready = Vec::new();
        for url in super::take_links(ctx) {
            match link_access::locate(&url, services.connections.all()) {
                Target::PullRequest { key, .. } => {
                    self.checks.push(AccessCheck::start(services, key));
                }
                Target::NotConnected(_) | Target::NotAPullRequest => {
                    ready.push(Forwarded::Elsewhere(url));
                }
            }
        }
        let mut pending = Vec::with_capacity(self.checks.len());
        for mut check in self.checks.drain(..) {
            match check.poll(services) {
                None => pending.push(check),
                Some(Verdict::Open(key) | Verdict::Unknown(key)) => {
                    ready.push(Forwarded::Open(key));
                }
                Some(Verdict::NoAccess { key, denial }) => {
                    link_access::announce_refusal(services, &key, denial);
                    // The toast is drawn from the next frame.
                    ctx.request_repaint();
                }
            }
        }
        self.checks = pending;
        ready
    }
}
