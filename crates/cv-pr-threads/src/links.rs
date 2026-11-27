//! Links to one thread on its platform, for "Copy link" and outdated threads.

use review_core::{Provider, ReviewThread};

/// The thread's place on the pull request's web page: the page without its query or fragment,
/// then what `provider` appends for `thread`. `None` without a page or where the platform has no
/// link to the thread.
pub fn thread_url(
    provider: &dyn Provider,
    pull_request_url: &str,
    thread: &ReviewThread,
) -> Option<String> {
    let page = pull_request_url.split(['?', '#']).next()?.trim_end_matches('/');
    if page.is_empty() {
        return None;
    }
    let suffix = provider.thread_page_suffix(thread)?;
    Some(format!("{page}{suffix}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use cv_testkit::provider::{Script, Scripted};
    use review_core::{Connection, ProviderKind, ThreadStatus, UserIdentity};

    use super::*;
    use crate::samples;

    /// A GitHub provider that never sends: only its thread links and comment actions are asked.
    pub(crate) fn github() -> Arc<dyn Provider> {
        let connection = Connection {
            id: "test".into(),
            name: "test".into(),
            kind: ProviderKind::GitHub,
            base_url: "http://127.0.0.1:9".into(),
            organizations: Vec::new(),
            token: None,
        };
        Arc::new(provider_github::GitHubProvider::new(connection, reqwest::Client::new()))
    }

    /// A platform without links to threads: the trait's default.
    struct Unlinked;

    impl Script for Unlinked {}

    fn general_comment() -> ReviewThread {
        let comment = samples::comment_at("5", &UserIdentity::default(), 0, "");
        samples::thread("issue-comment:5", ThreadStatus::Active, None, vec![comment])
    }

    #[test]
    fn joins_the_bare_page_and_the_suffix() {
        let cases = [
            (
                "https://code.example/acme/shop/pull/12",
                Some("https://code.example/acme/shop/pull/12#issuecomment-5"),
            ),
            (
                "https://code.example/acme/shop/pull/12/",
                Some("https://code.example/acme/shop/pull/12#issuecomment-5"),
            ),
            (
                "https://code.example/acme/shop/pull/12?tab=files",
                Some("https://code.example/acme/shop/pull/12#issuecomment-5"),
            ),
            (
                "https://code.example/acme/shop/pull/12#discussion_r1",
                Some("https://code.example/acme/shop/pull/12#issuecomment-5"),
            ),
            ("", None),
            ("?tab=files", None),
        ];
        let provider = github();
        for (page, expected) in cases {
            assert_eq!(
                thread_url(&*provider, page, &general_comment()).as_deref(),
                expected,
                "{page}"
            );
        }
    }

    #[test]
    fn has_no_link_where_the_platform_has_none() {
        let page = "https://code.example/acme/shop/pull/12";
        assert_eq!(thread_url(&Scripted(Unlinked), page, &general_comment()), None);
    }
}
