//! GET answers shared and revalidated: identical requests in flight at the same time send once,
//! and an answer that carried an `ETag` is asked for again with `If-None-Match`, so an unchanged
//! resource costs a `304` without a body.
//!
//! GitHub documents conditional requests (304s do not count against the primary rate limit when
//! the request is authorized): <https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api>.
//! GitLab and Azure DevOps do not document them; the validator is only sent where the host
//! returned an `ETag` itself, so hosts without support simply answer 200 as before.
//!
//! Keys hash the URL with the auth and media-type headers using a per-process random hasher;
//! no header value is stored.
//!
//! A write ends the sharing: every request that is neither a GET nor a GraphQL query bumps a
//! process-wide write epoch when it finishes, answered or failed ([`Write`]), and a GET joins a
//! request in flight only when that one started in the current epoch. Otherwise the request in
//! flight may have been answered before the write landed, and the refresh after a merge would show
//! the pull request still open; the GET then sends its own request, which later identical GETs
//! join.
//!
//! The write epoch, the requests in flight and the kept answers live in a [`Cache`]. The app has
//! one, [`Cache::shared`]: every GET of `http::fetch` and every write go through it, which is what
//! makes the epoch process-wide. A test that expects a GET to join one in flight asks through a
//! `Cache` of its own: `cargo test` runs the tests of a binary on threads of one process, so a
//! write that any other test finishes between two calls ends their sharing in the shared cache,
//! as it would in the app. A test that drives the app or a provider cannot hand it a cache: an
//! exact request count there that rests on a join still loses its premise to a neighbour's write,
//! and [`write_epoch`] tells it whether one finished.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hash, Hasher, RandomState};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use reqwest::header::{ETAG, HeaderValue, IF_NONE_MATCH};
use reqwest::{Client, Request, Response, StatusCode};
use tokio::sync::watch;

use super::{Fetched, budget, traffic};
use crate::error::{ProviderError, Result};

const MAX_CACHED_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 4 * 1024 * 1024;
/// Headers that change who asks or what comes back; GitLab's token header included.
const KEY_HEADERS: [&str; 4] = ["authorization", "private-token", "accept", "x-github-api-version"];

type Shared = Option<Result<Fetched>>;

/// The sharing and revalidation state; see the module doc. Cloning hands out the same state: the
/// task that sends a request keeps it until the request ends.
#[derive(Clone, Default)]
pub(crate) struct Cache(Arc<State>);

#[derive(Default)]
struct State {
    /// Bumped by every finished write.
    write_epoch: AtomicU64,
    /// Tells the requests in flight under one key apart, so an older one ending leaves a newer one be.
    next_flight: AtomicU64,
    in_flight: Mutex<HashMap<u64, Flight>>,
    store: Mutex<Store>,
}

/// A write (a request neither a GET nor a GraphQL query), while it runs: dropping it (answered,
/// failed or cancelled) bumps the write epoch of its cache, so GETs sent from then on never join
/// one that may predate the write.
pub(crate) struct Write(Cache);

impl Drop for Write {
    fn drop(&mut self) {
        self.0.0.write_epoch.fetch_add(1, Ordering::SeqCst);
    }
}

/// One request in flight: the epoch it started in and the answer its callers wait for.
struct Flight {
    id: u64,
    epoch: u64,
    receiver: watch::Receiver<Shared>,
}

impl Cache {
    /// The app's one cache, behind `http::fetch` and every write.
    pub(crate) fn shared() -> &'static Self {
        static SHARED: OnceLock<Cache> = OnceLock::new();
        SHARED.get_or_init(Cache::default)
    }

    /// The guard of a write that runs now; see [`Write`].
    pub(crate) fn write(&self) -> Write {
        Write(self.clone())
    }

    /// The first caller starts the request; callers asking the same while it is in flight, with
    /// no write finished since it started, wait for the same answer. The request runs on its own
    /// task, so it is answered even when the caller that started it goes away first (its screen
    /// dropped or restarted the load) while others still wait.
    pub(crate) async fn fetch(&self, client: Client, request: Request) -> Result<Fetched> {
        // A caller that waits out rate limits never shares the answer of one that does not.
        let may_wait = budget::may_wait();
        let key = key_of(&request, may_wait);
        let label = traffic::current_connection().unwrap_or_else(|| super::origin(request.url()));
        let (mut receiver, is_shared) = {
            let mut in_flight = self.in_flight();
            let epoch = self.0.write_epoch.load(Ordering::SeqCst);
            if let Some(flight) = in_flight.get(&key).filter(|flight| flight.epoch == epoch) {
                (flight.receiver.clone(), true)
            } else {
                let (sender, receiver) = watch::channel(None);
                let id = self.0.next_flight.fetch_add(1, Ordering::Relaxed);
                in_flight.insert(key, Flight { id, epoch, receiver: receiver.clone() });
                let scope = Scope { connection: traffic::current_connection(), may_wait };
                let leader = Leader { cache: self.clone(), key, id };
                tokio::spawn(send(client, request, leader, sender, scope));
                (receiver, false)
            }
        };
        match receiver.wait_for(Option::is_some).await {
            Ok(answer) => {
                if is_shared {
                    traffic::record_shared(&label);
                }
                answer.clone().expect("waited for a value")
            }
            // Only a panic inside `send` drops the sender without an answer.
            Err(_) => {
                Err(ProviderError::Network("the request stopped before it was answered".to_owned()))
            }
        }
    }

    fn in_flight(&self) -> MutexGuard<'_, HashMap<u64, Flight>> {
        self.0.in_flight.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        self.0.store.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The caller's task-local scopes, carried onto the task that sends.
struct Scope {
    connection: Option<String>,
    may_wait: bool,
}

/// Sends the request in the caller's scopes and hands the answer to every waiting caller.
async fn send(
    client: Client,
    request: Request,
    leader: Leader,
    sender: watch::Sender<Shared>,
    scope: Scope,
) {
    let cache = &leader.cache;
    let key = leader.key;
    // Boxed: the request future holds the whole budget and revalidation state (about 17 KB),
    // which would otherwise sit inline in every task this spawns and move with it.
    let request = Box::pin(async {
        if scope.may_wait {
            revalidate(cache, &client, request, key).await
        } else {
            budget::without_waiting(revalidate(cache, &client, request, key)).await
        }
    });
    let result = match scope.connection {
        Some(connection) => traffic::for_connection(&connection, request).await,
        None => request.await,
    };
    sender.send_replace(Some(result));
}

/// Removes the in-flight entry when the request ends, answered or not, unless a request started
/// after a write replaced it meanwhile.
struct Leader {
    cache: Cache,
    key: u64,
    id: u64,
}

impl Drop for Leader {
    fn drop(&mut self) {
        let mut in_flight = self.cache.in_flight();
        if in_flight.get(&self.key).is_some_and(|flight| flight.id == self.id) {
            in_flight.remove(&self.key);
        }
    }
}

async fn revalidate(
    cache: &Cache,
    client: &Client,
    mut request: Request,
    key: u64,
) -> Result<Fetched> {
    let label = traffic::current_connection().unwrap_or_else(|| super::origin(request.url()));
    let validator = cache.store().etag(key);
    let unconditional = validator.as_ref().and_then(|_| request.try_clone());
    if let Some(etag) = validator {
        request.headers_mut().insert(IF_NONE_MATCH, etag);
    }
    let response = budget::execute(client, request).await?;
    if response.status() != StatusCode::NOT_MODIFIED {
        return keep(cache, key, response).await;
    }
    traffic::record_not_modified(&label);
    if let Some(cached) = cache.store().hit(key) {
        return Ok(cached);
    }
    // Evicted meanwhile: ask once more without the validator.
    let request = unconditional.ok_or_else(|| {
        ProviderError::Decode("304 Not Modified without a cached copy".to_owned())
    })?;
    keep(cache, key, budget::execute(client, request).await?).await
}

async fn keep(cache: &Cache, key: u64, response: Response) -> Result<Fetched> {
    let etag = response.headers().get(ETAG).cloned();
    let fetched = Fetched::read(response).await?;
    match etag {
        Some(etag) => cache.store().put(key, etag, fetched.clone()),
        None => cache.store().remove(key),
    }
    Ok(fetched)
}

/// Bytes of the answers kept for revalidation, at most [`MAX_CACHED_BYTES`].
pub fn cached_bytes() -> usize {
    Cache::shared().store().bytes
}

/// How many writes the app's cache has seen finish: a GET joins one in flight only while this
/// number stands where it stood when that one started. Only reads it, so a test that drives the
/// app can tell whether a write anywhere in the process ended the sharing its count rests on.
pub fn write_epoch() -> u64 {
    Cache::shared().0.write_epoch.load(Ordering::SeqCst)
}

fn key_of(request: &Request, may_wait: bool) -> u64 {
    static SEED: OnceLock<RandomState> = OnceLock::new();
    let mut hasher = SEED.get_or_init(RandomState::new).build_hasher();
    request.url().as_str().hash(&mut hasher);
    may_wait.hash(&mut hasher);
    for name in KEY_HEADERS {
        name.hash(&mut hasher);
        for value in request.headers().get_all(name) {
            value.as_bytes().hash(&mut hasher);
        }
    }
    hasher.finish()
}

#[derive(Default)]
struct Store {
    entries: HashMap<u64, Entry>,
    bytes: usize,
    clock: u64,
}

struct Entry {
    etag: HeaderValue,
    fetched: Fetched,
    used: u64,
}

impl Store {
    fn etag(&self, key: u64) -> Option<HeaderValue> {
        self.entries.get(&key).map(|entry| entry.etag.clone())
    }

    fn hit(&mut self, key: u64) -> Option<Fetched> {
        self.clock += 1;
        let entry = self.entries.get_mut(&key)?;
        entry.used = self.clock;
        Some(entry.fetched.clone())
    }

    fn put(&mut self, key: u64, etag: HeaderValue, fetched: Fetched) {
        self.remove(key);
        let size = fetched.bytes.len();
        if size > MAX_ENTRY_BYTES {
            return;
        }
        while self.bytes + size > MAX_CACHED_BYTES {
            let Some(oldest) =
                self.entries.iter().min_by_key(|(_, entry)| entry.used).map(|(key, _)| *key)
            else {
                break;
            };
            self.remove(oldest);
        }
        self.clock += 1;
        self.bytes += size;
        self.entries.insert(key, Entry { etag, fetched, used: self.clock });
    }

    fn remove(&mut self, key: u64) {
        if let Some(entry) = self.entries.remove(&key) {
            self.bytes -= entry.fetched.bytes.len();
        }
    }
}

/// Tests that expect a GET to join one in flight ask through a [`Cache`] of their own (module
/// doc); tests that expect a GET not to join may use the shared one, since another test's write
/// only ends more sharing.
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use reqwest::header::HeaderValue;
    use reqwest::{Client, Url};
    use serde_json::{Value, json};
    use tokio::task::JoinHandle;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{Cache, Fetched, MAX_CACHED_BYTES, MAX_ENTRY_BYTES, Store};
    use crate::error::Result;
    use crate::http;

    fn fetched(bytes: usize) -> Fetched {
        Fetched {
            bytes: Bytes::from(vec![0; bytes]),
            next_link: None,
            url: Url::parse("https://cache.test/x").expect("url"),
        }
    }

    /// The keys in `1..=last` the store can revalidate (it holds their etag).
    fn kept(store: &Store, last: u64) -> Vec<u64> {
        (1..=last).filter(|key| store.etag(*key).is_some()).collect()
    }

    /// A GET of `url` through `cache`, on a task of its own.
    fn spawn_get(cache: &Cache, client: &Client, url: &str) -> JoinHandle<Result<Fetched>> {
        let cache = cache.clone();
        let client = client.clone();
        let request = client.get(url).build().expect("request");
        tokio::spawn(async move { cache.fetch(client, request).await })
    }

    /// A host whose `/pipelines` answers after 300 ms and expects to be asked once.
    async fn slow_pipelines_once() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/pipelines"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([{ "id": 1 }]))
                    .set_delay(Duration::from_millis(300)),
            )
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    /// The cache stays under its byte budget by dropping the answer used longest ago; an answer
    /// too large for one entry is never kept, so its next request goes out unconditionally.
    #[test]
    fn evicts_the_least_recently_used_answer_and_skips_oversized_ones() {
        let capacity = (MAX_CACHED_BYTES / MAX_ENTRY_BYTES) as u64;
        let etag = HeaderValue::from_static("\"v1\"");
        let mut store = Store::default();
        for key in 1..=capacity {
            store.put(key, etag.clone(), fetched(MAX_ENTRY_BYTES));
        }
        assert!(store.hit(1).is_some(), "1 is now the most recently used");
        store.put(capacity + 1, etag.clone(), fetched(MAX_ENTRY_BYTES));
        let mut expected: Vec<u64> = [1].into_iter().chain(3..=capacity + 1).collect();
        assert_eq!(kept(&store, capacity + 1), expected, "only 2, used longest ago, goes");

        store.put(3, etag.clone(), fetched(MAX_ENTRY_BYTES + 1));
        expected.retain(|key| *key != 3);
        assert_eq!(kept(&store, capacity + 1), expected, "the oversized answer replaces 3 unkept");
        // Dropping 3 freed one full entry of budget, so the next one fits without an eviction.
        store.put(capacity + 2, etag, fetched(MAX_ENTRY_BYTES));
        expected.push(capacity + 2);
        assert_eq!(kept(&store, capacity + 2), expected);
    }

    /// A refresh after a merge asks the host itself: it never joins the probe that went out before
    /// the merge and comes back with the pull request still open. Through the shared cache, so the
    /// write is the app's own `http::unit`.
    #[tokio::test]
    async fn a_get_after_a_write_never_joins_one_sent_before_it() {
        let server = MockServer::start().await;
        let delayed = ResponseTemplate::new(200)
            .set_body_string("open")
            .set_delay(Duration::from_millis(500));
        Mock::given(method("GET"))
            .and(path("/pulls/1"))
            .respond_with(delayed)
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/pulls/1"))
            .respond_with(ResponseTemplate::new(200).set_body_string("merged"))
            .with_priority(2)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/pulls/1/merge"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let client = Client::new();
        let url = format!("{}/pulls/1", server.uri());

        let probe = tokio::spawn(http::fetch(client.get(&url)));
        let started = std::time::Instant::now();
        while server.received_requests().await.unwrap_or_default().is_empty() {
            assert!(started.elapsed() < Duration::from_secs(10), "the probe reaches the host");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        http::unit(client.put(format!("{}/pulls/1/merge", server.uri()))).await.unwrap();
        let refreshed = http::fetch(client.get(&url)).await.unwrap();

        assert_eq!(&refreshed.bytes[..], b"merged", "the refresh sent its own request");
        let probed = probe.await.unwrap().unwrap();
        assert_eq!(&probed.bytes[..], b"open", "the probe keeps its own, older answer");
    }

    /// When the caller that started a shared request goes away before the answer (a screen
    /// dropped or restarted its load), the callers waiting for it still get that answer, without a
    /// second request. On the test's single thread one yield lets a spawned fetch run up to its
    /// first wait, so the starter is in flight before the waiter joins and the waiter has joined
    /// before the starter is aborted.
    #[tokio::test]
    async fn a_waiting_caller_gets_the_answer_when_the_starting_caller_goes_away() {
        let server = slow_pipelines_once().await;
        let cache = Cache::default();
        let client = Client::new();
        let url = format!("{}/pipelines", server.uri());
        let starter = spawn_get(&cache, &client, &url);
        tokio::task::yield_now().await;
        let waiter = spawn_get(&cache, &client, &url);
        tokio::task::yield_now().await;
        starter.abort();
        let fetched = waiter
            .await
            .expect("the waiting task ran")
            .expect("an answer, not a stopped shared request");
        assert_eq!(fetched.json::<Value>().expect("json"), json!([{ "id": 1 }]));
    }

    /// Regression: a write that another test of the binary finished between the starter's and the
    /// waiter's fetch moved the one process-wide epoch, so the waiter sent a request of its own and
    /// the host counted two (`a_waiting_caller_gets_the_answer...`, once in a full workspace run).
    /// Here such a write lands in that window every time; a cache of the test's own still sends
    /// once. The assertion is the mock's `expect(1)`, checked when the server drops.
    #[tokio::test]
    async fn a_write_in_the_shared_cache_leaves_a_test_cache_sharing() {
        let server = slow_pipelines_once().await;
        let cache = Cache::default();
        let client = Client::new();
        let url = format!("{}/pipelines", server.uri());
        let starter = spawn_get(&cache, &client, &url);
        tokio::task::yield_now().await;
        drop(Cache::shared().write());
        let waiter = spawn_get(&cache, &client, &url);
        let (started, waited) = (starter.await, waiter.await);
        started.expect("ran").expect("answer");
        waited.expect("ran").expect("answer");
    }

    /// A finished write moves the number a walkthrough reads to tell whether its sharing held;
    /// writes of other tests only move it further.
    #[test]
    fn a_finished_write_moves_the_write_epoch() {
        let before = super::write_epoch();
        drop(Cache::shared().write());
        assert!(super::write_epoch() > before);
    }
}
