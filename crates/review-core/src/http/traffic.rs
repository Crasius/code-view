//! Request counters per connection, so users (and tests) can see what the app sends.
//!
//! The app runs each provider call inside [`for_connection`]; requests outside such a scope
//! (connection checks in the setup wizard) count under their origin. Only methods and path
//! templates are kept: no query strings, headers or tokens.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use reqwest::{Method, Url};

const WINDOW: Duration = Duration::from_secs(60);

tokio::task_local! {
    static CONNECTION: String;
}

/// Runs `future` with its requests counted (and rate limits reported) for `connection_id`.
pub async fn for_connection<F: Future>(connection_id: &str, future: F) -> F::Output {
    CONNECTION.scope(connection_id.to_owned(), future).await
}

pub(crate) fn current_connection() -> Option<String> {
    CONNECTION.try_with(Clone::clone).ok()
}

/// What one connection sent since the app started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectionTraffic {
    /// Connection id, or the origin for requests outside a connection.
    pub connection: String,
    /// Requests sent in the last 60 seconds.
    pub last_minute: usize,
    /// Requests sent in total (each retry and each 304 counts).
    pub sent: u64,
    /// Answers that were `304 Not Modified` (served from the local copy).
    pub not_modified: u64,
    /// Calls that joined an identical request already in flight instead of sending their own.
    pub shared: u64,
    /// Answers that were rate limits (429, or GitHub's rate-limit 403).
    pub rate_limited: u64,
    /// Sent requests per method and path template, most frequent first.
    pub endpoints: Vec<EndpointCount>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointCount {
    pub method: String,
    /// The path with ids replaced: `/api/v4/projects/:project/merge_requests/:id`.
    pub path: String,
    pub count: u64,
}

/// One connection's counters; `None` before its first request.
pub fn of(connection: &str) -> Option<ConnectionTraffic> {
    let now = Instant::now();
    counters().get_mut(connection).map(|counters| counters.view(connection, now))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Endpoint {
    method: Method,
    path: String,
}

impl Endpoint {
    pub(crate) fn of(method: &Method, url: &Url) -> Self {
        Self { method: method.clone(), path: path_template(url.path()) }
    }
}

pub(crate) fn record_sent(label: &str, endpoint: &Endpoint) {
    let now = Instant::now();
    let mut all = counters();
    let counters = all.entry(label.to_owned()).or_default();
    counters.trim(now);
    counters.recent.push_back(now);
    counters.sent += 1;
    *counters.endpoints.entry(endpoint.clone()).or_default() += 1;
}

pub(crate) fn record_not_modified(label: &str) {
    counters().entry(label.to_owned()).or_default().not_modified += 1;
}

pub(crate) fn record_shared(label: &str) {
    counters().entry(label.to_owned()).or_default().shared += 1;
}

pub(crate) fn record_limited(label: &str) {
    counters().entry(label.to_owned()).or_default().rate_limited += 1;
}

/// Replaces ids in a path so equal endpoints count together: numbers → `:id`, URL-encoded
/// project paths → `:project`, commit SHAs → `:sha`, GUIDs → `:guid`.
fn path_template(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            let is_hex = |length: usize| {
                segment.len() == length && segment.bytes().all(|byte| byte.is_ascii_hexdigit())
            };
            if !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit()) {
                ":id"
            } else if segment.contains("%2F") || segment.contains("%2f") {
                ":project"
            } else if is_hex(40) || is_hex(64) {
                ":sha"
            } else if is_guid(segment) {
                ":guid"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn is_guid(segment: &str) -> bool {
    let groups: Vec<&str> = segment.split('-').collect();
    groups.len() == 5
        && groups.iter().zip([8, 4, 4, 4, 12]).all(|(group, length)| {
            group.len() == length && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

#[derive(Default)]
struct Counters {
    recent: VecDeque<Instant>,
    sent: u64,
    not_modified: u64,
    shared: u64,
    rate_limited: u64,
    endpoints: HashMap<Endpoint, u64>,
}

impl Counters {
    fn trim(&mut self, now: Instant) {
        while self.recent.front().is_some_and(|sent| now.duration_since(*sent) > WINDOW) {
            self.recent.pop_front();
        }
    }

    fn view(&mut self, label: &str, now: Instant) -> ConnectionTraffic {
        self.trim(now);
        let mut endpoints: Vec<EndpointCount> = self
            .endpoints
            .iter()
            .map(|(endpoint, count)| EndpointCount {
                method: endpoint.method.to_string(),
                path: endpoint.path.clone(),
                count: *count,
            })
            .collect();
        endpoints.sort_by(|left, right| {
            right.count.cmp(&left.count).then_with(|| left.path.cmp(&right.path))
        });
        ConnectionTraffic {
            connection: label.to_owned(),
            last_minute: self.recent.len(),
            sent: self.sent,
            not_modified: self.not_modified,
            shared: self.shared,
            rate_limited: self.rate_limited,
            endpoints,
        }
    }
}

fn counters() -> std::sync::MutexGuard<'static, HashMap<String, Counters>> {
    static COUNTERS: OnceLock<Mutex<HashMap<String, Counters>>> = OnceLock::new();
    COUNTERS.get_or_init(Mutex::default).lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_ids_projects_shas_and_guids() {
        assert_eq!(
            path_template("/api/v4/projects/group%2Fsub%2Fapp/merge_requests/17/discussions"),
            "/api/v4/projects/:project/merge_requests/:id/discussions"
        );
        assert_eq!(
            path_template(
                "/api/v4/projects/9/repository/commits/0123456789abcdef0123456789abcdef01234567/statuses"
            ),
            "/api/v4/projects/:id/repository/commits/:sha/statuses"
        );
        assert_eq!(
            path_template(
                "/org/_apis/build/builds/12/timeline/7d1b2c3a-1111-2222-3333-444455556666"
            ),
            "/org/_apis/build/builds/:id/timeline/:guid"
        );
    }

    #[tokio::test]
    async fn counts_per_connection_scope() {
        let url = Url::parse("https://counts.test/api/v4/projects/5").unwrap();
        let endpoint = Endpoint::of(&Method::GET, &url);
        for_connection("traffic-test", async {
            let label = current_connection().expect("scoped");
            record_sent(&label, &endpoint);
            record_sent(&label, &endpoint);
            record_not_modified(&label);
        })
        .await;
        assert!(current_connection().is_none());
        let traffic = of("traffic-test").expect("counted");
        assert_eq!((traffic.sent, traffic.last_minute, traffic.not_modified), (2, 2, 1));
        assert_eq!(traffic.endpoints[0].path, "/api/v4/projects/:id");
    }
}
