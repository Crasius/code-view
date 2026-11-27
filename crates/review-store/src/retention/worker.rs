//! The housekeeping thread: a sweep shortly after start and then daily, plus the requests from
//! Settings (sizes, clear cache), from removing a connection and from pull requests that finished.
//! It never blocks the window: the UI only sends requests and reads [`StorageStatus`]. Exit does
//! not wait for it.

use std::collections::HashSet;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use super::DAY;
use super::leases::{Lease, Leases};
use super::mirrors::MirrorGuard;
use super::policy::Outcome;
use super::sweep::{CacheLimits, FinishedPullRequest, Sweeper};
use super::usage::{self, StorageUsage};
use crate::{AppPaths, platform};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    pub first: Duration,
    pub every: Duration,
    /// How often a finished pull request that could not go yet (open, in use) is tried again.
    pub retry: Duration,
}

impl Schedule {
    /// Late enough to stay out of the start-up work, daily for an app left open for weeks.
    pub const APP: Self =
        Self { first: Duration::from_secs(30), every: DAY, retry: Duration::from_secs(60) };
}

/// What Settings → Storage shows.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StorageStatus {
    /// `None` until the first measurement finished.
    pub usage: Option<StorageUsage>,
    pub is_measuring: bool,
    pub is_clearing: bool,
    /// The result of the last "Clear cache" in this run.
    pub cleared: Option<Outcome>,
}

enum Request {
    Measure,
    Clear,
    ConnectionRemoved,
    PullRequestFinished(FinishedPullRequest),
}

pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// The app's handle; cheap to clone into background tasks.
#[derive(Clone)]
pub struct Retention {
    requests: mpsc::Sender<Request>,
    status: Arc<Mutex<StorageStatus>>,
    leases: Leases,
    open_pull_requests: Leases<String>,
}

impl Retention {
    /// Starts the thread; leftovers in the trash from an earlier run are deleted right away.
    /// `notify` runs after each status change (the app repaints).
    pub fn start(
        paths: AppPaths,
        guard: Arc<dyn MirrorGuard>,
        notify: Notify,
        schedule: Schedule,
    ) -> Self {
        let (requests, receiver) = mpsc::channel();
        let status = Arc::new(Mutex::new(StorageStatus::default()));
        let leases = Leases::default();
        let open_pull_requests = Leases::default();
        let worker = Worker {
            sweeper: Sweeper::new(paths.clone(), leases.clone(), guard, CacheLimits::APP),
            paths,
            status: status.clone(),
            notify,
            open_pull_requests: open_pull_requests.clone(),
            pending: Vec::new(),
            done: HashSet::new(),
        };
        // Without the thread nothing is swept this run; the app works the same.
        let _started = std::thread::Builder::new()
            .name("retention".into())
            .spawn(move || worker.run(&receiver, schedule));
        Self { requests, status, leases, open_pull_requests }
    }

    /// Held by a pull request page while it lives: its data (and the drafts on it) never goes
    /// while the page is open, even when the pull request finished meanwhile.
    pub fn hold_pull_request(&self, key: &str) -> Lease<String> {
        self.open_pull_requests.lease(key)
    }

    /// The pull request was merged or closed, or its source branch is gone: its logs, viewed
    /// marks and mirror refs go now (once its page is closed). Repeats cost nothing.
    pub fn pull_request_finished(&self, connection_id: &str, key: &str) {
        let finished =
            FinishedPullRequest { connection_id: connection_id.to_owned(), key: key.to_owned() };
        self.request(Request::PullRequestFinished(finished));
    }

    /// Paths a view holds open; sweeps and "Clear cache" leave them alone.
    pub fn leases(&self) -> &Leases {
        &self.leases
    }

    pub fn status(&self) -> StorageStatus {
        *lock(&self.status)
    }

    /// Measures the cache sizes again unless a measurement runs already.
    pub fn measure(&self) {
        let mut status = lock(&self.status);
        if !status.is_measuring {
            status.is_measuring = self.request(Request::Measure);
        }
    }

    pub fn clear_cache(&self) {
        let mut status = lock(&self.status);
        if !status.is_clearing {
            status.is_clearing = self.request(Request::Clear);
            status.cleared = None;
        }
    }

    /// Call after the connection is gone from the saved list: its logs, viewed state and the
    /// mirrors only it used are deleted.
    pub fn connection_removed(&self) {
        self.request(Request::ConnectionRemoved);
    }

    /// Hands `request` to the thread. False when the thread is gone (it never started, or the
    /// app is closing): the request is dropped and nothing waits for it.
    fn request(&self, request: Request) -> bool {
        self.requests.send(request).is_ok()
    }
}

fn lock(status: &Mutex<StorageStatus>) -> std::sync::MutexGuard<'_, StorageStatus> {
    status.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Worker {
    sweeper: Sweeper,
    paths: AppPaths,
    status: Arc<Mutex<StorageStatus>>,
    notify: Notify,
    open_pull_requests: Leases<String>,
    /// Finished pull requests still open or in use, tried again every `Schedule::retry`.
    pending: Vec<FinishedPullRequest>,
    /// Finished pull requests already cleaned up this run; lists report them on every refresh.
    done: HashSet<FinishedPullRequest>,
}

impl Worker {
    fn run(mut self, requests: &mpsc::Receiver<Request>, schedule: Schedule) {
        platform::enter_background_priority();
        self.sweeper.trash().purge_in_background();
        let mut next_sweep = Instant::now() + schedule.first;
        let mut next_retry = Instant::now() + schedule.retry;
        loop {
            if !self.pending.is_empty() && Instant::now() >= next_retry {
                self.retry_pending();
                next_retry = Instant::now() + schedule.retry;
            }
            let wake =
                if self.pending.is_empty() { next_sweep } else { next_sweep.min(next_retry) };
            match requests.recv_timeout(wake.saturating_duration_since(Instant::now())) {
                Ok(Request::Measure) => self.measure(),
                Ok(Request::Clear) => {
                    let outcome = self.sweeper.clear();
                    {
                        let mut status = lock(&self.status);
                        status.is_clearing = false;
                        status.cleared = Some(outcome);
                    }
                    self.measure();
                }
                Ok(Request::ConnectionRemoved) => {
                    self.sweeper.forget_removed_connections();
                    self.measure_if_shown();
                }
                Ok(Request::PullRequestFinished(finished)) => {
                    if !self.done.contains(&finished) && !self.pending.contains(&finished) {
                        self.pending.push(finished);
                        self.retry_pending();
                        next_retry = Instant::now() + schedule.retry;
                    }
                }
                Err(RecvTimeoutError::Timeout) if Instant::now() >= next_sweep => {
                    self.sweeper.sweep(SystemTime::now());
                    next_sweep = Instant::now() + schedule.every;
                    self.measure_if_shown();
                }
                Err(RecvTimeoutError::Timeout) => {}
                // The app dropped its handle (closing, or a test ended).
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// Cleans up every pending finished pull request whose page is closed; the rest wait.
    fn retry_pending(&mut self) {
        let mut is_cleaned = false;
        for finished in std::mem::take(&mut self.pending) {
            let is_open = self.open_pull_requests.is_leased(finished.key.as_str());
            if !is_open && self.sweeper.forget_pull_request(&finished) {
                self.done.insert(finished);
                is_cleaned = true;
            } else {
                self.pending.push(finished);
            }
        }
        if is_cleaned {
            self.measure_if_shown();
        }
    }

    fn measure(&self) {
        lock(&self.status).is_measuring = true;
        let usage = usage::measure(&self.paths);
        {
            let mut status = lock(&self.status);
            status.usage = Some(usage);
            status.is_measuring = false;
        }
        (self.notify)();
    }

    /// Sizes already on screen follow a sweep; nobody measures for a page never opened.
    fn measure_if_shown(&self) {
        if lock(&self.status).usage.is_some() {
            self.measure();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retention::mirrors::NothingInUse;
    use crate::test_support::{TestDir, wait_for, write_file};

    #[test]
    fn sweeps_on_schedule_and_resumes_the_trash() {
        let dir = TestDir::new("worker-schedule");
        let paths = dir.paths();
        std::fs::create_dir_all(paths.trash_dir().join("1-0-left.git")).unwrap();
        let old_log = write_file(&paths.logs_dir().join("c").join("old.log"), b"log");
        let month_ago = SystemTime::now() - DAY * 30;
        std::fs::File::options()
            .write(true)
            .open(&old_log)
            .unwrap()
            .set_modified(month_ago)
            .unwrap();

        let schedule = Schedule {
            first: Duration::from_millis(50),
            every: Duration::from_secs(3600),
            retry: Duration::from_secs(3600),
        };
        let retention =
            Retention::start(paths.clone(), Arc::new(NothingInUse), Arc::new(|| {}), schedule);
        assert!(wait_for(|| !old_log.exists()), "the scheduled sweep removes the week-old log");
        assert!(wait_for(|| std::fs::read_dir(paths.trash_dir())
            .is_ok_and(|mut entries| entries.next().is_none())));
        drop(retention);
    }

    #[test]
    fn clear_cache_reports_and_measures() {
        let dir = TestDir::new("worker-clear");
        let paths = dir.paths();
        write_file(&paths.logs_dir().join("c").join("a.log"), &[0u8; 64]);
        let schedule = Schedule {
            first: Duration::from_secs(3600),
            every: Duration::from_secs(3600),
            retry: Duration::from_secs(3600),
        };
        let retention = Retention::start(paths, Arc::new(NothingInUse), Arc::new(|| {}), schedule);

        retention.clear_cache();
        assert!(wait_for(|| retention.status().usage.is_some()));
        let status = retention.status();
        assert_eq!(status.cleared.map(|cleared| cleared.freed), Some(64));
        assert_eq!(status.usage.map(|usage| usage.logs), Some(0));
        assert!(!status.is_clearing);
    }

    /// Regression: a request the thread never gets left Settings showing "Measuring…" and
    /// "Clearing…" for the rest of the run.
    #[test]
    fn without_the_thread_nothing_shows_as_running() {
        let (requests, receiver) = mpsc::channel();
        drop(receiver);
        let retention = Retention {
            requests,
            status: Arc::default(),
            leases: Leases::default(),
            open_pull_requests: Leases::default(),
        };

        retention.measure();
        retention.clear_cache();
        assert_eq!(retention.status(), StorageStatus::default());
    }
}
