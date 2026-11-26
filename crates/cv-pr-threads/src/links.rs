//! Links to one thread on its platform, for "Copy link" and outdated threads.

use review_core::{ProviderKind, ReviewThread};

/// The thread's place on the pull request's web page: GitHub's comment anchors, GitLab's note
/// anchor, Azure DevOps' `discussionId` parameter. `None` without a page or a first comment.
pub fn thread_url(
    platform: ProviderKind,
    pull_request_url: &str,
    thread: &ReviewThread,
) -> Option<String> {
    let page = pull_request_url.split(['?', '#']).next()?.trim_end_matches('/');
    if page.is_empty() {
        return None;
    }
    let first = thread.comments.first()?;
    let url = match platform {
        ProviderKind::GitHub => {
            // Ids as the GitHub provider encodes them: general comments and review bodies are
            // prefixed, review threads link to their first comment.
            if thread.id.starts_with("issue-comment:") {
                format!("{page}#issuecomment-{}", first.id)
            } else if let Some(review) = thread.id.strip_prefix("review:") {
                format!("{page}#pullrequestreview-{review}")
            } else {
                format!("{page}#discussion_r{}", first.id)
            }
        }
        ProviderKind::GitLab => format!("{page}#note_{}", first.id),
        ProviderKind::AzureDevOps => format!("{page}?discussionId={}", thread.id),
    };
    Some(url)
}

#[cfg(test)]
mod tests {
    use review_core::{ThreadComment, ThreadStatus, UserIdentity};

    use super::*;

    fn thread(id: &str, comment_id: &str) -> ReviewThread {
        ReviewThread {
            id: id.into(),
            status: ThreadStatus::Active,
            anchor: None,
            is_outdated: false,
            origin: None,
            is_system: false,
            can_reply: true,
            can_resolve: true,
            comments: vec![ThreadComment {
                id: comment_id.into(),
                author: UserIdentity::default(),
                body: String::new(),
                created_at: time::OffsetDateTime::UNIX_EPOCH,
                updated_at: None,
            }],
        }
    }

    #[test]
    fn links_threads_on_each_platform() {
        let github = "https://github.com/cli/cli/pull/12";
        assert_eq!(
            thread_url(ProviderKind::GitHub, github, &thread("PRRT_kw", "991")).as_deref(),
            Some("https://github.com/cli/cli/pull/12#discussion_r991")
        );
        assert_eq!(
            thread_url(ProviderKind::GitHub, github, &thread("issue-comment:5", "5")).as_deref(),
            Some("https://github.com/cli/cli/pull/12#issuecomment-5")
        );
        assert_eq!(
            thread_url(ProviderKind::GitHub, github, &thread("review:7", "7")).as_deref(),
            Some("https://github.com/cli/cli/pull/12#pullrequestreview-7")
        );
        assert_eq!(
            thread_url(
                ProviderKind::GitLab,
                "https://gitlab.com/group/app/-/merge_requests/7",
                &thread("abc", "301")
            )
            .as_deref(),
            Some("https://gitlab.com/group/app/-/merge_requests/7#note_301")
        );
        assert_eq!(
            thread_url(
                ProviderKind::AzureDevOps,
                "https://dev.azure.com/org/p/_git/r/pullrequest/42?_a=overview",
                &thread("17", "1")
            )
            .as_deref(),
            Some("https://dev.azure.com/org/p/_git/r/pullrequest/42?discussionId=17")
        );
        assert_eq!(thread_url(ProviderKind::GitLab, "", &thread("abc", "1")), None);
    }
}
