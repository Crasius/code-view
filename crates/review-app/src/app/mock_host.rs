//! The start of a walkthrough's own mock host: a wiremock server on a runtime of its own, behind
//! one responder of the scenario's. The scenarios' answers differ (route tables, JSON, phases),
//! so they stay in their files; what every host needs comes from here. One step above all:
//! wiremock pools its servers, so a new host can get an earlier test's origin, and with it a
//! rate-limit pause that origin left in the process ([`cv_testkit::fresh_origin`]). A start that
//! forgot it waited out that pause for some twenty minutes (GitLab job 25128).
//!
//! [`super::github_mock::GitHubHost`] keeps its own start on its shared runtime.

use wiremock::{Mock, MockServer, Request, Respond};

/// A wiremock server on a runtime of its own that answers every request through the responder
/// [`MockHost::start`] built, unless a mock [`MockHost::mount`]ed later matches; its origin starts
/// without an earlier test's rate-limit pause.
pub(super) struct MockHost {
    pub(super) server: MockServer,
    /// Runs the server; declared last so it drops after the server.
    runtime: tokio::runtime::Runtime,
}

impl MockHost {
    /// Starts the server and answers through the responder `responder` builds from the server's
    /// origin (`http://127.0.0.1:{port}`): the scenarios' JSON carries links on the host, and a
    /// request's own URL names no port.
    pub(super) fn start<R: Respond + 'static>(responder: impl FnOnce(&str) -> R) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("mock runtime");
        let server = runtime.block_on(MockServer::start());
        cv_testkit::fresh_origin(&server.uri());
        // The lowest priority: a mock mounted later answers before it.
        let fallback = Mock::given(wiremock::matchers::any())
            .respond_with(responder(&server.uri()))
            .with_priority(u8::MAX);
        runtime.block_on(fallback.mount(&server));
        Self { server, runtime }
    }

    pub(super) fn uri(&self) -> String {
        self.server.uri()
    }

    /// Every request the server received so far.
    pub(super) fn requests(&self) -> Vec<Request> {
        self.runtime.block_on(self.server.received_requests()).unwrap_or_default()
    }

    /// Answers the requests `mock` matches through it from now on.
    pub(super) fn mount(&self, mock: Mock) {
        self.runtime.block_on(mock.mount(&self.server));
    }
}
