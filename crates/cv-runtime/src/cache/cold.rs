//! The app's handle on the disk tier behind its memory caches ([`ColdCache`], one budget for all
//! kinds). Writes queue for one background thread, so the UI never waits for the disk; reads
//! happen on the background threads that need them (a coloring job), or once at start for the
//! page answers ([`super::page_cache`]). Which cache keeps what there is the policy table in
//! [`super::memory`].

use std::sync::Arc;
use std::sync::mpsc::{self, Sender};

use review_store::{AppPaths, COLD_CACHE_BUDGET_BYTES, ColdCache};

type Write = Box<dyn FnOnce(&ColdCache) + Send>;

#[derive(Clone)]
pub struct ColdTier {
    cache: Arc<ColdCache>,
    writes: Sender<Write>,
}

impl ColdTier {
    /// The tier in `paths`' cache folder; its budget is enforced first, in the background.
    pub fn new(paths: &AppPaths) -> Self {
        let cache = Arc::new(ColdCache::new(paths, COLD_CACHE_BUDGET_BYTES));
        let (writes, queued) = mpsc::channel::<Write>();
        let writer = cache.clone();
        // Without the thread every write is dropped: the cold tier stays empty, nothing breaks.
        let _cold_tier_stays_empty =
            std::thread::Builder::new().name("cold-cache".into()).spawn(move || {
                writer.enforce_budget();
                while let Ok(write) = queued.recv() {
                    write(&writer);
                }
            });
        Self { cache, writes }
    }

    /// Stores what `encode` returns for `key` of `kind` at `version`, on the writer thread; `None`
    /// stores nothing. A failed write leaves a miss.
    pub fn write(
        &self,
        kind: &'static str,
        version: u32,
        key: String,
        encode: impl FnOnce() -> Option<Vec<u8>> + Send + 'static,
    ) {
        let write = move |cache: &ColdCache| {
            if let Some(payload) = encode() {
                // A full disk or a locked file costs the next start its head start, nothing else.
                let _miss_next_time = cache.write(kind, version, &key, &payload);
            }
        };
        // Fails only when the writer thread never started: the cold tier stays empty.
        let _cold_tier_stays_empty = self.writes.send(Box::new(write));
    }

    /// Waits until the writer thread has handled every write queued before; returns at once when
    /// it never started. Writes run in order on one thread, so a marker write behind them is
    /// handled last.
    #[cfg(any(test, feature = "test-support"))]
    pub fn flush(&self) {
        let (done, handled) = mpsc::channel::<()>();
        let marker = move |_: &ColdCache| {
            let _waiter_gone = done.send(());
        };
        if self.writes.send(Box::new(marker)).is_ok() {
            // An error means the writer thread ended without handling the marker: nothing waits.
            let _writer_gone = handled.recv();
        }
    }

    /// The payload stored for `key`. Reads the disk: background threads only, or once at start.
    pub fn read(&self, kind: &str, version: u32, key: &str) -> Option<Vec<u8>> {
        self.cache.read(kind, version, key)
    }

    /// The payloads of `kind` at `version` with their keys, the most recently used first, up to
    /// `max_bytes`. Reads the disk: once at start.
    pub(crate) fn read_all(
        &self,
        kind: &str,
        version: u32,
        max_bytes: u64,
    ) -> Vec<(String, Vec<u8>)> {
        self.cache.read_all(kind, version, max_bytes)
    }
}
