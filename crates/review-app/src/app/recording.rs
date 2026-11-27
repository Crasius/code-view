//! What a walkthrough's mock host was asked, by phase: the request budget and the list-status
//! walkthroughs count requests per endpoint (method and path template) in the phases they enter.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use serde_json::Value;

/// Requests by phase and endpoint, each with one detail its test reads back (a path, a body).
#[derive(Default)]
pub(super) struct Recorder {
    phase: Mutex<String>,
    /// (phase, endpoint, detail)
    hits: Mutex<Vec<(String, String, String)>>,
}

impl Recorder {
    /// Counts the requests from now on under `phase`.
    pub(super) fn enter(&self, phase: &str) {
        *self.phase.lock().unwrap_or_else(PoisonError::into_inner) = phase.to_owned();
    }

    pub(super) fn record(&self, endpoint: String, detail: String) {
        let phase = self.phase.lock().unwrap_or_else(PoisonError::into_inner).clone();
        self.hits.lock().unwrap_or_else(PoisonError::into_inner).push((phase, endpoint, detail));
    }

    /// The details of the requests `keep` picks by phase and endpoint, in order.
    pub(super) fn details(&self, keep: impl Fn(&str, &str) -> bool) -> Vec<String> {
        self.hits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(phase, endpoint, _)| keep(phase, endpoint))
            .map(|(_, _, detail)| detail.clone())
            .collect()
    }

    pub(super) fn count(&self, phase: &str) -> usize {
        self.hits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(hit, _, _)| hit == phase)
            .count()
    }

    /// How often each endpoint was asked in `phase`.
    pub(super) fn endpoints(&self, phase: &str) -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::new();
        for (hit, endpoint, _) in self.hits.lock().unwrap_or_else(PoisonError::into_inner).iter() {
            if hit == phase {
                *counts.entry(endpoint.clone()).or_default() += 1;
            }
        }
        counts
    }
}

/// `path` with its ids, URL-encoded project paths and commit hashes as placeholders, so equal
/// endpoints count together.
pub(super) fn template(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit()) {
                ":id"
            } else if segment.contains("%2F") || segment.contains("%2f") {
                ":project"
            } else if segment.len() == 40 && segment.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                ":sha"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The merge request iids a GitLab GraphQL request's body names.
pub(super) fn iids(body: &[u8]) -> Vec<u64> {
    let body: Value = serde_json::from_slice(body).unwrap_or_default();
    body["variables"]["iids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|iid| iid.parse().ok())
        .collect()
}
