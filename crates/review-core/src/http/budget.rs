//! The per-host request budget: a cap on requests waiting for an answer, and a pause after the
//! host signalled a rate limit.
//!
//! Signals, per the platforms' documentation:
//! - GitLab: `429` with `Retry-After` (seconds) and `RateLimit-Remaining` / `RateLimit-Reset`
//!   (Unix seconds) on API answers. <https://docs.gitlab.com/administration/settings/user_and_ip_rate_limits/>
//! - GitHub: primary limit `403`/`429` with `x-ratelimit-remaining: 0` until `x-ratelimit-reset`
//!   (Unix seconds); secondary limits `403`/`429` with `retry-after`, else at least one minute.
//!   <https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api>
//! - Azure DevOps: blocked requests get `429` with `Retry-After`; delayed ones a `200` that still
//!   carries `Retry-After` / `X-RateLimit-Delay`. <https://learn.microsoft.com/en-us/azure/devops/integrate/concepts/rate-limits>
//!
//! A limited answer pauses the host's [`Api`] it came from: every request of that kind waits until
//! the reset (or until [`resume`] is called from "Retry now"), then the limited request is sent
//! once more. Requests inside [`without_waiting`] fail at once instead.
//!
//! GraphQL is metered apart from REST: GitHub keeps a separate `graphql` quota, and gitlab.com
//! answered anonymous GraphQL with 429 (`RateLimit-Name: unauthenticated_traffic_per_ip`, a
//! `Retry-After` until the next full hour) while its REST answers still had room under
//! `throttle_unauthenticated_api`. A GraphQL limit therefore never holds back REST requests.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Client, Request, Response, StatusCode, Url};
use tokio::sync::{Notify, Semaphore};

use super::traffic::{self, Endpoint};
use crate::error::{ProviderError, Result};

tokio::task_local! {
    static WITHOUT_WAITING: ();
}

/// Runs `future` with its requests failing at once with [`ProviderError::RateLimited`] where they
/// would wait out a rate-limit pause: for optional extras that must not hold back the data they
/// add to. A limited answer still pauses the host for everyone else.
pub async fn without_waiting<F: Future>(future: F) -> F::Output {
    WITHOUT_WAITING.scope((), future).await
}

/// False inside [`without_waiting`].
pub(crate) fn may_wait() -> bool {
    WITHOUT_WAITING.try_with(|()| ()).is_err()
}

/// The kind of API a request goes to; each host keeps one budget per kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Api {
    Rest,
    /// `/api/graphql` (GitLab, GitHub Enterprise Server) or `/graphql` (api.github.com).
    GraphQl,
}

impl Api {
    pub(crate) fn of(url: &Url) -> Self {
        if url.path().trim_end_matches('/').ends_with("/graphql") {
            Self::GraphQl
        } else {
            Self::Rest
        }
    }
}

/// Requests per host that may wait for an answer at the same time; the rest queue.
pub(crate) const MAX_IN_FLIGHT: usize = 6;
/// Characters of an error answer's body kept in [`ProviderError::Status`].
const MAX_ERROR_BODY_CHARS: usize = 600;
/// GitHub: "wait for at least one minute before retrying" when a limited answer names no time.
const DEFAULT_LIMITED_WAIT: Duration = Duration::from_secs(60);
/// GitHub's primary window is one hour; no host asks the app for longer.
const MAX_WAIT: Duration = Duration::from_hours(1);
/// `RateLimit-Reset` values below this are seconds from now, not a Unix timestamp.
const EPOCH_THRESHOLD: u64 = 1_000_000_000;

/// A host that currently pauses this app's requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitNotice {
    /// `scheme://host[:port]`.
    pub origin: String,
    /// The requests that wait; the host's other kind goes on.
    pub api: Api,
    /// Connections whose requests ran into the limit (connection ids from [`super::for_connection`]).
    pub connection_ids: Vec<String>,
    pub resumes_in: Duration,
}

/// Hosts paused right now, soonest resume first.
pub fn rate_limits() -> Vec<RateLimitNotice> {
    let now = Instant::now();
    let gates: Vec<Arc<Gate>> = registry().values().cloned().collect();
    let mut notices: Vec<RateLimitNotice> = gates
        .iter()
        .filter_map(|gate| {
            let state = gate.lock();
            let until = state.paused_until.filter(|until| *until > now)?;
            Some(RateLimitNotice {
                origin: gate.origin.clone(),
                api: gate.api,
                connection_ids: state.connections.iter().cloned().collect(),
                resumes_in: until - now,
            })
        })
        .collect();
    notices.sort_by_key(|notice| notice.resumes_in);
    notices
}

/// Ends a host's pauses now ("Retry now"); waiting requests go out immediately.
pub fn resume(origin: &str) {
    let gates: Vec<Arc<Gate>> =
        registry().values().filter(|gate| gate.origin == origin).cloned().collect();
    for gate in gates {
        gate.resume();
    }
}

/// Sends one request within the host's budget for its [`Api`]. Non-success answers become errors;
/// a limited answer pauses that budget and the request is sent once more after the pause (inside
/// [`without_waiting`] it fails instead, as does a request that finds the budget paused).
pub(crate) async fn execute(client: &Client, request: Request) -> Result<Response> {
    execute_with(client, request, |status| {
        status.is_success() || status == StatusCode::NOT_MODIFIED
    })
    .await
}

/// [`execute`] with `is_answer` deciding which statuses come back as answers rather than errors
/// (redirects, for requests that follow them by hand).
pub(crate) async fn execute_with(
    client: &Client,
    request: Request,
    is_answer: fn(StatusCode) -> bool,
) -> Result<Response> {
    let origin = super::origin(request.url());
    let host = request.url().host_str().unwrap_or_default().to_owned();
    let gate = gate(&origin, Api::of(request.url()));
    let connection = traffic::current_connection();
    let label = connection.clone().unwrap_or_else(|| origin.clone());
    let endpoint = Endpoint::of(request.method(), request.url());
    let may_wait = may_wait();
    let mut spare = if may_wait { request.try_clone() } else { None };
    let mut current = request;
    loop {
        if !may_wait && let Some(remaining) = gate.remaining(Instant::now()) {
            return Err(rate_limited(&host, remaining));
        }
        gate.wait_open().await;
        let sent = {
            let _permit =
                gate.permits.acquire().await.expect("the budget semaphore is never closed");
            traffic::record_sent(&label, &endpoint);
            client.execute(current).await
        };
        let response = sent?;
        let status = response.status();
        let signal = RateSignal::read(response.headers(), SystemTime::now());
        if is_answer(status) {
            if let Some(wait) = signal.exhausted_wait() {
                gate.pause(wait, connection.as_deref());
            }
            return Ok(response);
        }
        let path = response.url().path().to_owned();
        let body: String =
            response.text().await.unwrap_or_default().chars().take(MAX_ERROR_BODY_CHARS).collect();
        if !signal.is_limited(status.as_u16(), &body) {
            return Err(ProviderError::Status { status: status.as_u16(), path, body });
        }
        let wait = signal.limited_wait();
        traffic::record_limited(&label);
        gate.pause(wait, connection.as_deref());
        match spare.take() {
            Some(again) => current = again,
            None => return Err(rate_limited(&host, wait)),
        }
    }
}

fn rate_limited(host: &str, wait: Duration) -> ProviderError {
    ProviderError::RateLimited { host: host.to_owned(), retry_after_secs: wait.as_secs().max(1) }
}

/// One gate per origin and [`Api`].
type Gates = HashMap<(String, Api), Arc<Gate>>;

fn registry() -> std::sync::MutexGuard<'static, Gates> {
    static GATES: OnceLock<Mutex<Gates>> = OnceLock::new();
    GATES.get_or_init(Mutex::default).lock().unwrap_or_else(PoisonError::into_inner)
}

fn gate(origin: &str, api: Api) -> Arc<Gate> {
    registry()
        .entry((origin.to_owned(), api))
        .or_insert_with(|| Arc::new(Gate::new(origin, api)))
        .clone()
}

struct Gate {
    origin: String,
    api: Api,
    permits: Semaphore,
    state: Mutex<GateState>,
    resumed: Notify,
}

#[derive(Default)]
struct GateState {
    paused_until: Option<Instant>,
    connections: BTreeSet<String>,
}

impl Gate {
    fn new(origin: &str, api: Api) -> Self {
        Self {
            origin: origin.to_owned(),
            api,
            permits: Semaphore::new(MAX_IN_FLIGHT),
            state: Mutex::default(),
            resumed: Notify::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Time left in the pause; clears an expired pause.
    fn remaining(&self, now: Instant) -> Option<Duration> {
        let mut state = self.lock();
        match state.paused_until {
            Some(until) if until > now => Some(until - now),
            Some(_) => {
                *state = GateState::default();
                None
            }
            None => None,
        }
    }

    async fn wait_open(&self) {
        loop {
            let resumed = self.resumed.notified();
            tokio::pin!(resumed);
            resumed.as_mut().enable();
            let Some(remaining) = self.remaining(Instant::now()) else {
                return;
            };
            tokio::select! {
                () = tokio::time::sleep(remaining) => {}
                () = &mut resumed => {}
            }
        }
    }

    fn pause(&self, wait: Duration, connection: Option<&str>) {
        let until = Instant::now() + wait.min(MAX_WAIT);
        let mut state = self.lock();
        state.paused_until = Some(state.paused_until.map_or(until, |current| current.max(until)));
        if let Some(connection) = connection {
            state.connections.insert(connection.to_owned());
        }
    }

    fn resume(&self) {
        *self.lock() = GateState::default();
        self.resumed.notify_waiters();
    }
}

/// The rate-limit headers of one answer.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RateSignal {
    retry_after: Option<Duration>,
    remaining: Option<u64>,
    reset_in: Option<Duration>,
    /// GitHub's `x-ratelimit-resource` (`core`, `graphql`, `search`, ...).
    resource: Option<String>,
}

impl RateSignal {
    pub(crate) fn read(headers: &HeaderMap, now: SystemTime) -> Self {
        let text =
            |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).map(str::trim);
        let number = |names: &[&str]| names.iter().find_map(|name| text(name)?.parse::<u64>().ok());
        let now_secs = now.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
        let retry_after = text(RETRY_AFTER.as_str())
            .and_then(|value| parse_retry_after(value, now_secs))
            .or_else(|| {
                text("x-ratelimit-delay")
                    .and_then(|value| value.parse::<f64>().ok())
                    .map(Duration::from_secs_f64)
            });
        let reset_in = number(&["ratelimit-reset", "x-ratelimit-reset"]).map(|reset| {
            if reset < EPOCH_THRESHOLD {
                Duration::from_secs(reset)
            } else {
                Duration::from_secs(reset.saturating_sub(now_secs))
            }
        });
        Self {
            retry_after,
            remaining: number(&["ratelimit-remaining", "x-ratelimit-remaining"]),
            reset_in,
            resource: text("x-ratelimit-resource").map(str::to_owned),
        }
    }

    /// 429 always; 403 only when GitHub marks it as a rate limit (headers or message).
    pub(crate) fn is_limited(&self, status: u16, body: &str) -> bool {
        match status {
            429 => true,
            403 => {
                self.retry_after.is_some()
                    || self.remaining == Some(0)
                    || body.to_ascii_lowercase().contains("rate limit")
            }
            _ => false,
        }
    }

    /// How long a limited host wants the app to wait.
    pub(crate) fn limited_wait(&self) -> Duration {
        let until_reset = self.reset_in.filter(|_| self.remaining == Some(0));
        self.retry_after
            .or(until_reset)
            .unwrap_or(DEFAULT_LIMITED_WAIT)
            .clamp(Duration::from_secs(1), MAX_WAIT)
    }

    /// After a successful answer: pause when the host asks to slow down (Azure DevOps delays) or
    /// the quota for the whole host is used up. GitHub's separate `search` and `graphql` quotas do
    /// not pause REST calls.
    pub(crate) fn exhausted_wait(&self) -> Option<Duration> {
        if let Some(wait) = self.retry_after {
            return Some(wait.clamp(Duration::from_secs(1), MAX_WAIT));
        }
        let is_host_wide = self.resource.as_deref().is_none_or(|resource| resource == "core");
        if self.remaining == Some(0) && is_host_wide {
            return self.reset_in.map(|reset| reset.clamp(Duration::from_secs(1), MAX_WAIT));
        }
        None
    }
}

/// `Retry-After` is either seconds or an HTTP date (`Tue, 05 Jan 2021 11:00:00 GMT`).
fn parse_retry_after(value: &str, now_secs: u64) -> Option<Duration> {
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let format = time::macros::format_description!(
        "[weekday repr:short], [day] [month repr:short] [year] [hour]:[minute]:[second] GMT"
    );
    let at = time::PrimitiveDateTime::parse(value, &format).ok()?.assume_utc().unix_timestamp();
    let at = u64::try_from(at).ok()?;
    Some(Duration::from_secs(at.saturating_sub(now_secs)))
}

#[cfg(test)]
mod tests {
    use reqwest::header::{HeaderName, HeaderValue};

    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn gitlab_429_waits_for_retry_after() {
        let signal = RateSignal::read(
            &headers(&[
                ("Retry-After", "42"),
                ("RateLimit-Remaining", "0"),
                ("RateLimit-Reset", "1609844400"),
            ]),
            at(1_609_844_390),
        );
        assert!(signal.is_limited(429, ""));
        assert_eq!(signal.limited_wait(), Duration::from_secs(42));
    }

    #[test]
    fn exhausted_quota_pauses_until_reset() {
        let signal = RateSignal::read(
            &headers(&[("RateLimit-Remaining", "0"), ("RateLimit-Reset", "1000000030")]),
            at(1_000_000_000),
        );
        assert_eq!(signal.exhausted_wait(), Some(Duration::from_secs(30)));
        let plenty = RateSignal::read(
            &headers(&[("RateLimit-Remaining", "1999"), ("RateLimit-Reset", "1000000030")]),
            at(1_000_000_000),
        );
        assert_eq!(plenty.exhausted_wait(), None);
    }

    #[test]
    fn github_secondary_limit_without_headers_waits_a_minute() {
        let signal = RateSignal::read(&headers(&[]), at(0));
        assert!(
            signal.is_limited(403, r#"{"message":"You have exceeded a secondary rate limit."}"#)
        );
        assert!(
            !signal.is_limited(
                403,
                r#"{"message":"Resource not accessible by personal access token"}"#
            )
        );
        assert_eq!(signal.limited_wait(), DEFAULT_LIMITED_WAIT);
    }

    #[test]
    fn github_primary_limit_waits_for_reset_and_ignores_other_quotas() {
        let primary = RateSignal::read(
            &headers(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "1000000600")]),
            at(1_000_000_000),
        );
        assert!(primary.is_limited(403, ""));
        assert_eq!(primary.limited_wait(), Duration::from_secs(600));
        let search = RateSignal::read(
            &headers(&[
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", "1000000020"),
                ("x-ratelimit-resource", "search"),
            ]),
            at(1_000_000_000),
        );
        assert_eq!(search.exhausted_wait(), None);
    }

    #[test]
    fn azure_delay_on_success_slows_down() {
        let signal = RateSignal::read(
            &headers(&[("Retry-After", "3"), ("X-RateLimit-Remaining", "0")]),
            at(0),
        );
        assert_eq!(signal.exhausted_wait(), Some(Duration::from_secs(3)));
        let delay = RateSignal::read(&headers(&[("X-RateLimit-Delay", "1.5")]), at(0));
        assert_eq!(delay.exhausted_wait(), Some(Duration::from_millis(1500)));
    }

    #[test]
    fn parses_http_date_retry_after() {
        let now = 1_609_844_400 - 10;
        assert_eq!(
            parse_retry_after("Tue, 05 Jan 2021 11:00:00 GMT", now),
            Some(Duration::from_secs(10))
        );
        assert_eq!(parse_retry_after("soon", now), None);
    }

    #[tokio::test]
    async fn paused_gate_waits_and_resume_wakes() {
        let gate = Arc::new(Gate::new("https://pause.test", Api::Rest));
        gate.pause(Duration::from_secs(30), Some("c1"));
        assert!(gate.remaining(Instant::now()).is_some());
        let waiter = tokio::spawn({
            let gate = gate.clone();
            async move { gate.wait_open().await }
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        gate.resume();
        waiter.await.expect("woken");
        assert!(gate.remaining(Instant::now()).is_none());
    }

    #[test]
    fn graphql_has_its_own_budget() {
        let url = |text: &str| Url::parse(text).expect("url");
        assert_eq!(Api::of(&url("https://gitlab.com/api/graphql")), Api::GraphQl);
        assert_eq!(Api::of(&url("https://api.github.com/graphql")), Api::GraphQl);
        assert_eq!(Api::of(&url("https://ghe.example.com/api/graphql/")), Api::GraphQl);
        assert_eq!(
            Api::of(&url("https://gitlab.com/api/v4/projects/1/pipelines/2/jobs")),
            Api::Rest
        );
        assert_eq!(Api::of(&url("https://gitlab.com/api/v4/projects/graphql-tools")), Api::Rest);
    }

    /// An optional request does not wait out a pause: it fails before anything is sent.
    #[tokio::test]
    async fn without_waiting_fails_at_a_paused_budget_at_once() {
        let origin = "http://without-waiting.test";
        gate(origin, Api::GraphQl).pause(Duration::from_mins(40), None);
        let request = Client::new().post(format!("{origin}/api/graphql")).build().expect("request");
        let error = without_waiting(execute(&Client::new(), request)).await.expect_err("paused");
        assert!(
            matches!(error, ProviderError::RateLimited { retry_after_secs: 2390..=2400, .. }),
            "{error:?}"
        );
        let notice = rate_limits()
            .into_iter()
            .find(|notice| notice.origin == origin)
            .expect("still paused for the others");
        assert_eq!(notice.api, Api::GraphQl);
        resume(origin);
        assert!(
            rate_limits().iter().all(|notice| notice.origin != origin),
            "Retry now ends every pause of the host"
        );
    }
}
