//! The open log's download: the first fetch once the platform serves it (a finished job's whole
//! log comes from disk when an earlier download left it), asking again while it is not there yet, fetching it again while its job runs, and once more for its end when the
//! job finished. Repeated fetches go through the refresh schedule, so they pause while the
//! window is inactive and stop when the pane is not shown.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use cv_pr_session::session::PullRequestSession;
use cv_runtime::load::Load;
use cv_runtime::refresh::WhenInactive;
use cv_services::Services;
use review_store::retention::Lease;

use super::super::availability::{Availability, Missing};
use super::super::download::{self, DownloadRequest, Fetched, Grown, LoadedLog};

/// How often a running job's log is fetched again while it is on screen and the window is active.
const LIVE_LOG_INTERVAL: Duration = Duration::from_secs(4);

#[derive(Default)]
pub struct LogFetch {
    pub log: Load<Fetched>,
    /// The log of a running job fetched again for what it added.
    grow: Load<Grown>,
    /// Why fetching the running job's log again failed last; the log on screen stays.
    pub grow_error: Option<String>,
    /// The log's job runs and its log is fetched as it grows; the view follows its end.
    pub is_live: bool,
    /// The log was fetched while its job ran, so once the job finished it is fetched once more.
    needs_end: bool,
    /// A live log opens at its end.
    pub to_end: bool,
    /// The last answer that the log is not there yet, shown while it is asked for again.
    pub missing: Option<Missing>,
    /// The refresh key of the fetches again, while they are scheduled.
    poll_key: Option<String>,
    /// Keep retention away from the log on screen (or still downloading): its part and its whole
    /// file.
    leases: Vec<Lease>,
    received: Arc<AtomicU64>,
}

impl LogFetch {
    /// Already holding `log`, as if its download finished.
    #[cfg(test)]
    pub fn loaded_for_test(log: Arc<LoadedLog>) -> Self {
        Self { log: Load::Ready(Fetched::Log(log)), ..Self::default() }
    }

    /// Forgets the last log and its schedule, for another one.
    pub fn reset(&mut self, services: &Services) {
        self.stop_polling(services);
        *self = Self::default();
    }

    /// Bytes of the download so far.
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    /// The downloaded log, once there.
    pub fn loaded(&self) -> Option<Arc<LoadedLog>> {
        match &self.log {
            Load::Ready(Fetched::Log(log)) => Some(log.clone()),
            _ => None,
        }
    }

    /// Takes in the downloads that are done; returns what fetching a running job's log again
    /// brought.
    pub fn poll(&mut self) -> Option<Grown> {
        if self.log.poll() && matches!(self.log, Load::Ready(Fetched::Log(_))) {
            self.missing = None;
        }
        if !self.grow.poll() {
            return None;
        }
        match std::mem::take(&mut self.grow) {
            Load::Ready(grown) => {
                self.grow_error = None;
                Some(grown)
            }
            Load::Failed(message) => {
                self.grow_error = Some(message);
                None
            }
            Load::Idle | Load::Loading(_) => None,
        }
    }

    /// Starts what the log `log_id` needs now (see the module's doc).
    pub fn sync(
        &mut self,
        ctx: &egui::Context,
        log_id: &str,
        (availability, missing): (Availability, Option<Missing>),
        session: &PullRequestSession,
        services: &Services,
    ) {
        let Availability::Fetch { is_live } = availability else {
            self.is_live = false;
            self.stop_polling(services);
            // An answer from before the job finished is no answer for its log once it did.
            if matches!(self.log, Load::Ready(Fetched::Failed(_))) {
                self.log = Load::Idle;
            }
            return;
        };
        self.is_live = is_live;
        let key = format!(
            "pipeline-log/{}/{}/{}/{log_id}",
            session.key.connection_id, session.key.repository_key, session.key.number
        );
        let loaded = match &self.log {
            Load::Idle => {
                self.start_fetch(log_id, is_live, session, services);
                return;
            }
            Load::Loading(_) | Load::Failed(_) => return,
            Load::Ready(Fetched::Failed(_)) => {
                let asks_again = matches!(missing, Some(Missing::NotYet | Missing::Publishing));
                if !asks_again {
                    self.stop_polling(services);
                } else if self.is_due(ctx, &key, services) {
                    self.missing = missing;
                    self.start_fetch(log_id, is_live, session, services);
                }
                return;
            }
            Load::Ready(Fetched::Log(log)) => log.clone(),
        };
        if self.grow.is_loading() {
            return;
        }
        if is_live {
            if self.is_due(ctx, &key, services) {
                self.start_grow(log_id, loaded, session, services);
            }
            return;
        }
        self.stop_polling(services);
        if std::mem::take(&mut self.needs_end) {
            self.start_grow(log_id, loaded, session, services);
        }
    }

    pub fn stop_polling(&mut self, services: &Services) {
        if let Some(key) = self.poll_key.take() {
            services.refresh.forget(&key);
        }
    }

    fn request(
        &self,
        log_id: &str,
        is_finished: bool,
        session: &PullRequestSession,
        services: &Services,
    ) -> DownloadRequest {
        DownloadRequest {
            provider: session.provider.clone(),
            repository: session.key.repository_key.clone(),
            number: session.key.number,
            log_id: log_id.to_owned(),
            path: services.paths.pull_request_log_file(
                &session.key.connection_id,
                &session.key.storage_key(),
                log_id,
            ),
            is_finished,
            received: self.received.clone(),
            repaint: services.background.ctx(),
        }
    }

    /// Reads a finished job's whole log from disk when it is there; downloads it otherwise.
    fn start_fetch(
        &mut self,
        log_id: &str,
        is_live: bool,
        session: &PullRequestSession,
        services: &Services,
    ) {
        let request = self.request(log_id, !is_live, session, services);
        let leases = services.retention.leases();
        self.leases = vec![
            leases.lease(download::part_file(&request.path)),
            leases.lease(download::whole_file(&request.path)),
        ];
        self.needs_end = is_live;
        self.to_end = is_live;
        self.log.start(&services.background, async move { Ok(Fetched::of(request).await) });
    }

    fn start_grow(
        &mut self,
        log_id: &str,
        loaded: Arc<LoadedLog>,
        session: &PullRequestSession,
        services: &Services,
    ) {
        // Fetched for its end once the job finished: a log fetched anew then is whole.
        let request = self.request(log_id, !self.is_live, session, services);
        self.grow.start(&services.background, download::refresh(request, loaded));
    }

    /// True when the next fetch of the open log is due; the first ask starts the clock.
    fn is_due(&mut self, ctx: &egui::Context, key: &str, services: &Services) -> bool {
        if self.poll_key.as_deref() != Some(key) {
            self.stop_polling(services);
            self.poll_key = Some(key.to_owned());
        }
        services.refresh.due(ctx, key, LIVE_LOG_INTERVAL, WhenInactive::Pause)
    }
}
