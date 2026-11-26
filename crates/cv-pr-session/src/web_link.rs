//! Links to a changed file on its hosting platform, offered wherever the app cannot show a file
//! itself (binary content, failed loads), named after the platform.

use review_core::ProviderKind;
use review_git::{ChangeKind, ChangedFile};

use crate::session::PullRequestSession;

#[derive(Clone)]
pub struct WebLink {
    pub label: String,
    /// The platform's mark for the link's icon slot ([`cv_widgets::provider_icon::glyph`]).
    pub icon: &'static str,
    pub url: String,
}

/// The file at the compared head commit (base commit for deletions).
pub fn for_file(session: &PullRequestSession, file: &ChangedFile) -> Option<WebLink> {
    let detail = session.detail.ready()?;
    let (base, head) = session.compared_commits()?;
    let commit = if file.kind == ChangeKind::Deleted { base } else { head };
    let url = file_url(session.kind, &detail.summary.web_url, &file.path, &commit)?;
    Some(WebLink {
        label: format!("Open on {}", session.kind.label()),
        icon: cv_widgets::provider_icon::glyph(session.kind),
        url,
    })
}

pub fn for_pull_request(session: &PullRequestSession) -> Option<WebLink> {
    let detail = session.detail.ready()?;
    Some(WebLink {
        label: format!("Open pull request on {}", session.kind.label()),
        icon: cv_widgets::provider_icon::glyph(session.kind),
        url: detail.summary.web_url.clone(),
    })
}

/// The repository's (GitLab: the project's) web root, from a pull request's web address.
pub fn repository_url(kind: ProviderKind, pull_request_url: &str) -> Option<String> {
    let page = pull_request_url.split(['?', '#']).next()?.trim_end_matches('/');
    let (repository, _) = match kind {
        ProviderKind::GitHub => page.rsplit_once("/pull/")?,
        ProviderKind::GitLab => page.rsplit_once("/-/merge_requests/")?,
        ProviderKind::AzureDevOps => page.rsplit_once("/pullrequest/")?,
    };
    Some(repository.to_owned())
}

/// A repository file at `commit` as the browser shows a file itself (an image opens as the
/// image): GitHub's and GitLab's raw view, Azure DevOps' file view.
pub fn raw_file_url(
    kind: ProviderKind,
    pull_request_url: &str,
    path: &str,
    commit: &str,
) -> Option<String> {
    let repository = repository_url(kind, pull_request_url)?;
    let path = encode_path(path);
    Some(match kind {
        ProviderKind::GitHub => format!("{repository}/blob/{commit}/{path}?raw=true"),
        ProviderKind::GitLab => format!("{repository}/-/raw/{commit}/{path}"),
        ProviderKind::AzureDevOps => format!("{repository}?path=%2F{path}&version=GC{commit}"),
    })
}

/// A repository file at `commit`, scrolled to `line`, on the platform's file view.
pub fn line_url(
    kind: ProviderKind,
    pull_request_url: &str,
    path: &str,
    commit: &str,
    line: u32,
) -> Option<String> {
    let repository = repository_url(kind, pull_request_url)?;
    let path = encode_path(path);
    Some(match kind {
        ProviderKind::GitHub => format!("{repository}/blob/{commit}/{path}#L{line}"),
        ProviderKind::GitLab => format!("{repository}/-/blob/{commit}/{path}#L{line}"),
        ProviderKind::AzureDevOps => format!(
            "{repository}?path=%2F{path}&version=GC{commit}&line={line}&lineEnd={line}\
             &lineStartColumn=1&lineEndColumn=1&lineStyle=plain"
        ),
    })
}

fn file_url(
    kind: ProviderKind,
    pull_request_url: &str,
    path: &str,
    commit: &str,
) -> Option<String> {
    let page = pull_request_url.split(['?', '#']).next()?.trim_end_matches('/');
    match kind {
        ProviderKind::GitHub => {
            let (repository, _) = page.rsplit_once("/pull/")?;
            Some(format!("{repository}/blob/{commit}/{}", encode_path(path)))
        }
        ProviderKind::GitLab => {
            let (project, _) = page.rsplit_once("/-/merge_requests/")?;
            Some(format!("{project}/-/blob/{commit}/{}", encode_path(path)))
        }
        // Azure DevOps opens the pull request's Files view on that file.
        ProviderKind::AzureDevOps => {
            Some(format!("{page}?_a=files&path={}", urlencoding::encode(&format!("/{path}"))))
        }
    }
}

fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|segment| urlencoding::encode(segment).into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_a_line_on_each_platform() {
        let line = |kind, page| line_url(kind, page, "src/a b.rs", "abc", 12);
        assert_eq!(
            line(ProviderKind::GitHub, "https://github.com/cli/cli/pull/12").as_deref(),
            Some("https://github.com/cli/cli/blob/abc/src/a%20b.rs#L12")
        );
        assert_eq!(
            line(ProviderKind::GitLab, "https://gitlab.com/g/p/-/merge_requests/7").as_deref(),
            Some("https://gitlab.com/g/p/-/blob/abc/src/a%20b.rs#L12")
        );
        assert_eq!(
            line(ProviderKind::AzureDevOps, "https://dev.azure.com/o/p/_git/r/pullrequest/4")
                .as_deref(),
            Some(
                "https://dev.azure.com/o/p/_git/r?path=%2Fsrc/a%20b.rs&version=GCabc&line=12\
                 &lineEnd=12&lineStartColumn=1&lineEndColumn=1&lineStyle=plain"
            )
        );
    }

    #[test]
    fn links_files_on_each_platform() {
        assert_eq!(
            file_url(
                ProviderKind::GitHub,
                "https://github.com/cli/cli/pull/12",
                "pkg/cmd/a b.go",
                "abc"
            )
            .as_deref(),
            Some("https://github.com/cli/cli/blob/abc/pkg/cmd/a%20b.go")
        );
        assert_eq!(
            file_url(
                ProviderKind::GitLab,
                "https://gitlab.com/group/sub/project/-/merge_requests/7",
                "src/main.rs",
                "def"
            )
            .as_deref(),
            Some("https://gitlab.com/group/sub/project/-/blob/def/src/main.rs")
        );
        assert_eq!(
            file_url(
                ProviderKind::AzureDevOps,
                "https://dev.azure.com/org/project/_git/repo/pullrequest/42",
                "src/App.cs",
                "x"
            )
            .as_deref(),
            Some(
                "https://dev.azure.com/org/project/_git/repo/pullrequest/42?_a=files&path=%2Fsrc%2FApp.cs"
            )
        );
        assert!(
            file_url(ProviderKind::GitHub, "https://example.com/unexpected", "a", "b").is_none()
        );
    }

    #[test]
    fn links_raw_files_for_images() {
        assert_eq!(
            raw_file_url(
                ProviderKind::GitHub,
                "https://github.com/cli/cli/pull/12",
                "docs/a b.png",
                "abc"
            )
            .as_deref(),
            Some("https://github.com/cli/cli/blob/abc/docs/a%20b.png?raw=true")
        );
        assert_eq!(
            raw_file_url(
                ProviderKind::GitLab,
                "https://gitlab.com/group/project/-/merge_requests/7",
                "img/x.svg",
                "def"
            )
            .as_deref(),
            Some("https://gitlab.com/group/project/-/raw/def/img/x.svg")
        );
        assert_eq!(
            raw_file_url(
                ProviderKind::AzureDevOps,
                "https://dev.azure.com/org/project/_git/repo/pullrequest/42",
                "img/x.png",
                "c0"
            )
            .as_deref(),
            Some("https://dev.azure.com/org/project/_git/repo?path=%2Fimg/x.png&version=GCc0")
        );
        assert_eq!(
            repository_url(ProviderKind::GitLab, "https://gitlab.com/g/p/-/merge_requests/7?x")
                .as_deref(),
            Some("https://gitlab.com/g/p")
        );
    }
}
