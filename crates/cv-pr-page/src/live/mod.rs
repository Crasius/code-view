//! Live updates of the open pull request.
//!
//! While the page is shown, a cheap probe ([`review_core::Provider::revision`]) runs every
//! [`INTERVAL`] (anonymous connections: [`ANONYMOUS_INTERVAL`]) through the refresh scheduler, so
//! nothing is asked while the window is inactive and focus brings one check at once. Failures
//! double the interval up to [`MAX_BACKOFF_STEPS`] times. Pipelines are not part of the revision;
//! the screens that show them poll them ([`cv_pr_pipelines::follow_running`]). When the probe
//! differs from what the page shows, the newer data is read ahead off screen ([`stage`]); then
//!
//! * it would move a comment the user is writing ([`hold`]: a new comment in a file whose diff
//!   changes, a reply whose thread changes or sits in such a file): nothing moves; a card offers
//!   Reload or Later. Once that draft is posted or closed, the update goes on screen by itself;
//! * otherwise it is applied at once, without a card: the views keep their place (tab, file,
//!   line, folders, layout, open log), drafts elsewhere and the open editors keep their text.
//!
//! Comments that arrived this way show as new through the read marks of
//! [`cv_pr_threads::ThreadsUi`], like any comment the user has not seen yet. Only a Reload the
//! user clicked answers with an "Updated" card, which offers what was pushed.

mod banner;
mod hold;
mod range_bar;
mod stage;
mod summary;

use std::time::Duration;

use egui::{Id, Pos2};
use review_core::PullRequestRevision;

use self::banner::{BannerAction, Content, Look};
use self::hold::Reach;
use self::stage::Staged;
use self::summary::{ChangeSummary, PageData};
use cv_base::theme::motion;
use cv_pr_session::session::{CompareRange, PullRequestSession};
use cv_runtime::load::Load;
use cv_runtime::refresh::WhenInactive;
use cv_services::Services;

pub(crate) use self::hold::Drafts;
pub(crate) use self::range_bar::show as range_bar;

/// Probe interval with a token (GitHub answers unchanged probes with free 304s).
pub(crate) const INTERVAL: Duration = Duration::from_secs(30);
/// Anonymous probes count against small hourly limits (GitHub: 60).
pub(crate) const ANONYMOUS_INTERVAL: Duration = Duration::from_secs(120);
const MAX_BACKOFF_STEPS: u32 = 3;
/// How long the "Updated" card after a Reload stays when the pointer is not on it.
const NOTICE_SECONDS: f64 = 6.0;
const CARD_MOTION: f32 = 0.18;

/// What the user is doing that an update has to respect.
#[derive(Debug, Clone, Default)]
pub(crate) struct Activity {
    /// Comments being written where an update could move them.
    pub drafts: Drafts,
    /// One of the user's own writes runs; its own refresh follows, so updates wait for it.
    pub is_saving: bool,
}

/// What the page has to do after the user picked an action on a card.
pub(crate) enum LiveAction {
    /// Open the Files tab: the range already switched to what was pushed.
    ShowFiles,
}

#[derive(Default)]
pub(crate) struct LiveUpdates {
    probe: Load<PullRequestRevision>,
    /// The revision of what the page shows; `None` until the first probe after opening or a refresh.
    shown: Option<PullRequestRevision>,
    /// The probe right after the page's own load went out (once per load, even when it fails).
    has_baseline_probe: bool,
    staging: Load<Staged>,
    /// Read ahead, waiting for the user to finish writing.
    pending: Option<Pending>,
    /// The revision the user chose "Later" for; its card stays away.
    postponed: Option<PullRequestRevision>,
    failures: u32,
    /// The session's generation after this module's last change; another value is a refresh.
    generation: Option<u64>,
    notice: Option<Notice>,
    /// What the card showed last, so it can fade out.
    card: Option<Content>,
    /// The pending update would move a draft, so it waits; the card says so.
    is_held: bool,
}

struct Pending {
    staged: Staged,
    summary: ChangeSummary,
    reach: Reach,
}

struct Notice {
    content: Content,
    since: Option<CompareRange>,
    shown_at: f64,
}

impl LiveUpdates {
    /// Where the probe stands, for a walkthrough's failure message.
    #[cfg(test)]
    pub fn describe(&self) -> String {
        format!(
            "probe loading {} ({:?}), staging {} ({:?}), pending {}, held {}, failures {}",
            self.probe.is_loading(),
            self.probe.error(),
            self.staging.is_loading(),
            self.staging.error(),
            self.pending.is_some(),
            self.is_held,
            self.failures,
        )
    }

    /// Each frame the page is shown: polls the probe and the read-ahead, applies what may be
    /// applied, and asks the scheduler whether the next probe is due.
    pub(crate) fn update(
        &mut self,
        ctx: &egui::Context,
        session: &mut PullRequestSession,
        services: &mut Services,
        activity: &Activity,
    ) {
        if self.generation != Some(session.generation) {
            // Opened or refreshed by hand: that load is the news; a probe right after it is the
            // baseline, so a comment that lands a few seconds later is not taken for the loaded state.
            self.generation = Some(session.generation);
            self.shown = None;
            self.has_baseline_probe = false;
            self.pending = None;
            self.staging = Load::Idle;
            self.postponed = None;
        }
        let is_loaded = !session.detail.is_loading()
            && !session.threads.is_loading()
            && session.detail.ready().is_some()
            && session.threads.ready().is_some();
        if !self.has_baseline_probe && is_loaded && !self.probe.is_loading() {
            self.has_baseline_probe = true;
            self.start_probe(session, services);
        }
        if self.probe.poll() {
            match &self.probe {
                Load::Ready(revision) => {
                    self.failures = 0;
                    let revision = revision.clone();
                    self.observe(revision, session, services);
                }
                Load::Failed(_) => self.failures = (self.failures + 1).min(MAX_BACKOFF_STEPS),
                Load::Idle | Load::Loading(_) => {}
            }
        }
        if self.staging.poll() {
            match std::mem::take(&mut self.staging) {
                Load::Ready(mut staged) => {
                    let summary = describe(session, &staged);
                    let shown = session.threads.ready().map_or(&[][..], Vec::as_slice);
                    let reach = Reach::new(staged.changed_files.take(), shown, &staged.threads);
                    self.pending = Some(Pending { staged, summary, reach });
                }
                Load::Failed(_) => self.failures = (self.failures + 1).min(MAX_BACKOFF_STEPS),
                Load::Idle | Load::Loading(_) => {}
            }
        }
        let shown = session.threads.ready().map_or(&[][..], Vec::as_slice);
        self.is_held = self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.reach.disturbs(&activity.drafts, shown));
        if self.pending.is_some() && !self.is_held && !activity.is_saving {
            self.apply(session, services);
        }
        let key = format!("live|{}", session.key.storage_key());
        let interval = self.interval(session.has_token);
        let is_busy =
            self.probe.is_loading() || self.staging.is_loading() || session.detail.is_loading();
        if !is_busy && services.refresh.due(ctx, &key, interval, WhenInactive::Pause) {
            self.start_probe(session, services);
        }
    }

    fn interval(&self, has_token: bool) -> Duration {
        let base = if has_token { INTERVAL } else { ANONYMOUS_INTERVAL };
        base * 2u32.pow(self.failures)
    }

    fn start_probe(&mut self, session: &PullRequestSession, services: &Services) {
        let provider = session.provider.clone();
        let repository = session.key.repository_key.clone();
        let number = session.key.number;
        self.probe.start(&services.background, async move {
            provider.revision(&repository, number).await
        });
    }

    fn observe(
        &mut self,
        revision: PullRequestRevision,
        session: &PullRequestSession,
        services: &Services,
    ) {
        let Some(shown) = &self.shown else {
            // The probe after a load only sets the baseline, unless the head already moved past
            // the loaded commits (a push during the load is never missed).
            let has_moved =
                session.detail.ready().is_some_and(|detail| detail.head_sha != revision.head_sha);
            self.shown = Some(revision.clone());
            if has_moved && !self.staging.is_loading() {
                stage::start(&mut self.staging, session, services, revision);
            }
            return;
        };
        let is_known = *shown == revision
            || self.pending.as_ref().is_some_and(|pending| pending.staged.revision == revision);
        if !is_known && !self.staging.is_loading() {
            stage::start(&mut self.staging, session, services, revision);
        }
    }

    /// Puts the read-ahead update on screen; returns what came in and the range of what was
    /// pushed, if anything was.
    fn apply(
        &mut self,
        session: &mut PullRequestSession,
        services: &mut Services,
    ) -> Option<(ChangeSummary, Option<CompareRange>)> {
        let Pending { staged, summary, .. } = self.pending.take()?;
        let seen = session.mirror.ready().map(|mirror| mirror.head.clone());
        let pushed_head = staged.mirror.as_ref().map(|mirror| mirror.head.clone());
        session.apply_update(services, staged.detail, staged.threads, staged.mirror);
        self.generation = Some(session.generation);
        self.shown = Some(staged.revision);
        self.postponed = None;
        let since = match (seen, pushed_head) {
            (Some(seen), Some(head)) if summary.pushed => {
                Some(CompareRange::SinceSeen { seen, head })
            }
            _ => None,
        };
        Some((summary, since))
    }

    /// The user clicked Reload: the update goes on screen and a card answers what came in, with
    /// the way to what was pushed.
    fn reload(
        &mut self,
        ctx: &egui::Context,
        session: &mut PullRequestSession,
        services: &mut Services,
    ) {
        let Some((summary, since)) = self.apply(session, services) else {
            return;
        };
        let content = Content {
            look: Look::Updated { can_show_changes: since.is_some() },
            title: "Updated".to_owned(),
            detail: summary.parts().join(", "),
        };
        self.notice = Some(Notice { content, since, shown_at: ctx.input(|input| input.time) });
    }

    /// The card under the tabs (`top_center`).
    pub(crate) fn show(
        &mut self,
        ctx: &egui::Context,
        top_center: Pos2,
        width: f32,
        session: &mut PullRequestSession,
        services: &mut Services,
    ) -> Option<LiveAction> {
        let now = ctx.input(|input| input.time);
        let pending = self.pending.as_ref().filter(|pending| {
            self.is_held
                && !pending.summary.is_empty()
                && self.postponed.as_ref() != Some(&pending.staged.revision)
        });
        let wanted = match pending.map(|pending| &pending.summary) {
            Some(summary) => Some(Content {
                look: Look::Pending,
                title: summary.title().to_owned(),
                detail: summary.parts().join(", "),
            }),
            None => self
                .notice
                .as_ref()
                .filter(|notice| now - notice.shown_at < NOTICE_SECONDS)
                .map(|notice| notice.content.clone()),
        };
        let is_visible = wanted.is_some();
        if let Some(content) = wanted {
            self.card = Some(content);
        }
        let id = Id::unique(("live_card", &session.key));
        let shown = motion::toggle(ctx, id.with("shown"), is_visible, CARD_MOTION);
        let content = self.card.clone()?;
        if shown <= 0.0 {
            self.card = None;
            return None;
        }
        if let Some(notice) = &self.notice {
            let left = NOTICE_SECONDS - (now - notice.shown_at);
            if left > 0.0 {
                ctx.request_repaint_after(Duration::from_secs_f64(left));
            }
        }
        let card = banner::show(ctx, id, top_center, width.min(banner::MAX_WIDTH), &content, shown);
        if card.is_hovered
            && let Some(notice) = self.notice.as_mut()
            && content.look != Look::Pending
        {
            // Reading it keeps it there.
            notice.shown_at = now;
        }
        match card.action? {
            BannerAction::Reload => {
                self.reload(ctx, session, services);
                None
            }
            BannerAction::Later => {
                self.postponed =
                    self.pending.as_ref().map(|pending| pending.staged.revision.clone());
                None
            }
            BannerAction::Dismiss => {
                self.notice = None;
                None
            }
            BannerAction::ShowChanges => {
                let range = self.notice.take()?.since?;
                session.set_range(services, range);
                Some(LiveAction::ShowFiles)
            }
        }
    }
}

fn describe(session: &PullRequestSession, staged: &Staged) -> ChangeSummary {
    let newer = PageData { detail: &staged.detail, threads: &staged.threads };
    match session.detail.ready() {
        Some(detail) => {
            let threads = session.threads.ready().map_or(&[][..], Vec::as_slice);
            summary::describe(
                &PageData { detail, threads },
                &newer,
                &staged.commits,
                staged.is_force_push,
            )
        }
        None => ChangeSummary::default(),
    }
}
