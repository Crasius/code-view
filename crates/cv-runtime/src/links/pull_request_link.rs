//! Turns a pull request's web URL (as copied from the browser) into a route on a saved connection,
//! and back: the web URL of a pull request, for "Open on GitHub" when the API gave none. Issue and
//! work item URLs resolve the same way, for the overview's linked items.

// RS-DONT-04: parses pasted URLs, so indexing, string slicing and narrowing casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing, clippy::cast_possible_truncation)]

use review_core::{Connection, ItemKind, ItemRef, ProviderKind};

use crate::route::PullRequestKey;

/// Finds the connection whose host serves `url` and the pull request it names.
/// Connections with a token win over anonymous ones on the same host.
pub fn resolve(url: &str, connections: &[Connection]) -> Result<PullRequestKey, String> {
    let url = url.trim();
    let address = url.split(['?', '#']).next().unwrap_or_default().trim_end_matches('/');
    let mut candidates: Vec<&Connection> = connections.iter().collect();
    candidates.sort_by_key(|connection| connection.token().is_none());
    for connection in candidates {
        let Some(path) = web_path(address, connection) else {
            continue;
        };
        let Some((repository_key, number)) = parse_path(connection.kind, &path) else {
            continue;
        };
        return Ok(PullRequestKey { connection_id: connection.id.clone(), repository_key, number });
    }
    Err(format!("No saved connection can open {url}. Add a connection for its host here."))
}

/// A pull request on a platform's cloud host (github.com, gitlab.com, dev.azure.com and
/// `{org}.visualstudio.com`) that no saved connection serves: the platform and, for Azure DevOps,
/// the organization, so the connection setup can start with them. Other hosts give `None`: their
/// platform is unknown without a connection.
pub fn unconnected_cloud_link(
    url: &str,
    connections: &[Connection],
) -> Option<(ProviderKind, Option<String>)> {
    if resolve(url, connections).is_ok() {
        return None;
    }
    ProviderKind::ALL.into_iter().find_map(|kind| {
        let cloud = Connection {
            id: String::new(),
            name: String::new(),
            kind,
            base_url: kind.default_base_url().to_owned(),
            organizations: Vec::new(),
            token: None,
        };
        let key = resolve(url, std::slice::from_ref(&cloud)).ok()?;
        let organization = (kind == ProviderKind::AzureDevOps)
            .then(|| key.repository_key.split('/').next().unwrap_or_default().to_owned());
        Some((kind, organization))
    })
}

/// The connection and item a pull request, issue or work item URL names, like [`resolve`].
pub fn resolve_item(url: &str, connections: &[Connection]) -> Option<(String, ItemRef)> {
    let address = url.trim().split(['?', '#']).next().unwrap_or_default().trim_end_matches('/');
    let mut candidates: Vec<&Connection> = connections.iter().collect();
    candidates.sort_by_key(|connection| connection.token().is_none());
    candidates.into_iter().find_map(|connection| {
        let path = web_path(address, connection)?;
        let item = match parse_path(connection.kind, &path) {
            Some((repository_key, number)) => {
                ItemRef { kind: ItemKind::PullRequest, repository_key, number }
            }
            None => parse_item_path(connection.kind, &path)?,
        };
        Some((connection.id.clone(), item))
    })
}

/// The item's page on its platform, the inverse of [`resolve_item`].
pub fn item_web_url(connection: &Connection, item: &ItemRef) -> String {
    let base = connection.base_url();
    let encoded = |key: &str| {
        key.split('/')
            .map(|segment| urlencoding::encode(segment).into_owned())
            .collect::<Vec<_>>()
            .join("/")
    };
    match (item.kind, connection.kind) {
        (ItemKind::PullRequest, _) => web_url(connection, &item.repository_key, item.number),
        (ItemKind::WorkItem, _) => {
            format!("{base}/{}/_workitems/edit/{}", encoded(&item.repository_key), item.number)
        }
        (_, ProviderKind::GitLab) => {
            format!("{base}/{}/-/issues/{}", encoded(&item.repository_key), item.number)
        }
        // GitHub sends `/issues/{n}` of a pull request on to `/pull/{n}`.
        _ => format!("{base}/{}/issues/{}", encoded(&item.repository_key), item.number),
    }
}

/// The pull request's page on its platform, the inverse of [`resolve`].
pub fn web_url(connection: &Connection, repository_key: &str, number: u64) -> String {
    let base = connection.base_url();
    let encoded = |segments: &[&str]| {
        segments
            .iter()
            .map(|segment| urlencoding::encode(segment).into_owned())
            .collect::<Vec<_>>()
            .join("/")
    };
    let segments: Vec<&str> = repository_key.split('/').collect();
    match (connection.kind, segments.as_slice()) {
        (ProviderKind::AzureDevOps, [organization, project, repository]) => {
            format!(
                "{base}/{}/_git/{}/pullrequest/{number}",
                encoded(&[organization, project]),
                encoded(&[repository])
            )
        }
        (ProviderKind::GitLab, _) => {
            format!("{base}/{}/-/merge_requests/{number}", encoded(&segments))
        }
        _ => format!("{base}/{}/pull/{number}", encoded(&segments)),
    }
}

/// The path below the connection's web root, with legacy `{org}.visualstudio.com` hosts
/// rewritten to the `dev.azure.com/{org}` form.
fn web_path(address: &str, connection: &Connection) -> Option<String> {
    let base = connection.base_url();
    if let Some(path) =
        strip_prefix_ignore_case(address, base).and_then(|rest| rest.strip_prefix('/'))
    {
        return Some(path.to_owned());
    }
    let is_services = connection.kind == ProviderKind::AzureDevOps
        && base.eq_ignore_ascii_case(ProviderKind::AzureDevOps.default_base_url());
    if !is_services {
        return None;
    }
    let rest = address.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    let organization = host.to_ascii_lowercase().strip_suffix(".visualstudio.com")?.to_owned();
    // Older links name the collection, which dev.azure.com folds into the organization.
    let path = strip_prefix_ignore_case(path, "DefaultCollection/").unwrap_or(path);
    Some(format!("{organization}/{path}"))
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let (head, rest) = text.split_at_checked(prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then_some(rest)
}

/// `(repository key, number)` from the platform's pull request path.
fn parse_path(kind: ProviderKind, path: &str) -> Option<(String, u64)> {
    let segments: Vec<String> = path.split('/').map(decode).collect::<Option<_>>()?;
    let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
    match (kind, segments.as_slice()) {
        // {owner}/{repo}/pull/{n}[/files]
        (ProviderKind::GitHub, [owner, repository, "pull", number, ..]) => {
            Some((format!("{owner}/{repository}"), number.parse().ok()?))
        }
        // {group/.../project}/-/merge_requests/{n}[/diffs]
        (ProviderKind::GitLab, _) => {
            let marker = segments.iter().position(|segment| *segment == "-")?;
            let Some(["merge_requests", number, ..]) = segments.get(marker + 1..) else {
                return None;
            };
            if marker == 0 {
                return None;
            }
            Some((segments.get(..marker)?.join("/"), number.parse().ok()?))
        }
        // {organization or collection}/{project}/_git/{repo}/pullrequest/{n}
        (
            ProviderKind::AzureDevOps,
            [organization, project, "_git", repository, "pullrequest", number, ..],
        ) => Some((format!("{organization}/{project}/{repository}"), number.parse().ok()?)),
        // {organization}/_git/{repo}/pullrequest/{n}: the repository named like its project
        (
            ProviderKind::AzureDevOps,
            [organization, "_git", repository, "pullrequest", number, ..],
        ) => Some((format!("{organization}/{repository}/{repository}"), number.parse().ok()?)),
        _ => None,
    }
}

/// Issues (GitHub, GitLab) and work items (Azure DevOps) from their web paths.
fn parse_item_path(kind: ProviderKind, path: &str) -> Option<ItemRef> {
    let segments: Vec<String> = path.split('/').map(decode).collect::<Option<_>>()?;
    let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
    let item = |kind: ItemKind, repository_key: String, number: &str| {
        Some(ItemRef { kind, repository_key, number: number.parse().ok()? })
    };
    match (kind, segments.as_slice()) {
        // {owner}/{repo}/issues/{n}
        (ProviderKind::GitHub, [owner, repository, "issues", number, ..]) => {
            item(ItemKind::Issue, format!("{owner}/{repository}"), number)
        }
        // {group/.../project}/-/issues/{n} or /-/work_items/{n}
        (ProviderKind::GitLab, _) => {
            let marker =
                segments.iter().position(|segment| *segment == "-").filter(|marker| *marker > 0)?;
            let Some(["issues" | "work_items", number, ..]) = segments.get(marker + 1..) else {
                return None;
            };
            item(ItemKind::Issue, segments.get(..marker)?.join("/"), number)
        }
        // {organization}/{project}/_workitems/edit/{n}
        (ProviderKind::AzureDevOps, [organization, project, "_workitems", "edit", number, ..]) => {
            item(ItemKind::WorkItem, format!("{organization}/{project}"), number)
        }
        _ => None,
    }
}

fn decode(segment: &str) -> Option<String> {
    urlencoding::decode(segment).ok().map(std::borrow::Cow::into_owned)
}

#[cfg(test)]
mod tests {
    use cv_testkit::samples::connection;
    use review_core::{Connection, ItemKind, ProviderKind};

    use super::{item_web_url, resolve, resolve_item, unconnected_cloud_link, web_url};

    #[test]
    fn resolves_links_of_every_platform() {
        let connections = [
            connection("gh", ProviderKind::GitHub, "https://github.com"),
            connection("gl", ProviderKind::GitLab, "https://gitlab.example.com/"),
            connection("ado", ProviderKind::AzureDevOps, "https://dev.azure.com"),
            connection("server", ProviderKind::AzureDevOps, "https://tfs.example.com/tfs"),
        ];
        let cases = [
            ("https://github.com/cli/cli/pull/14727/files", "gh", "cli/cli", 14727),
            (
                "https://gitlab.example.com/group/sub/app/-/merge_requests/12/diffs?view=inline",
                "gl",
                "group/sub/app",
                12,
            ),
            (
                "https://dev.azure.com/Fabrikam-Platform/Fabrikam%20Services/_git/fabrikam-services/pullrequest/741?_a=files",
                "ado",
                "Fabrikam-Platform/Fabrikam Services/fabrikam-services",
                741,
            ),
            (
                "https://contoso.visualstudio.com/Web/_git/site/pullrequest/5",
                "ado",
                "contoso/Web/site",
                5,
            ),
            (
                "https://tfs.example.com/tfs/DefaultCollection/Web/_git/site/pullrequest/9",
                "server",
                "DefaultCollection/Web/site",
                9,
            ),
        ];
        for (url, connection_id, repository_key, number) in cases {
            let key = resolve(url, &connections).unwrap_or_else(|error| panic!("{url}: {error}"));
            assert_eq!(
                (key.connection_id.as_str(), key.repository_key.as_str(), key.number),
                (connection_id, repository_key, number),
                "{url}"
            );
        }
    }

    /// Links Azure DevOps still serves: the collection of the `visualstudio.com` era, and the short
    /// form for a repository named like its project.
    #[test]
    fn resolves_legacy_and_short_azure_devops_links() {
        let connections = [
            connection("ado", ProviderKind::AzureDevOps, "https://dev.azure.com"),
            connection("server", ProviderKind::AzureDevOps, "https://tfs.example.com/tfs"),
        ];
        let cases = [
            (
                "https://contoso.visualstudio.com/DefaultCollection/Web/_git/site/pullrequest/5",
                "ado",
                "contoso/Web/site",
            ),
            ("https://dev.azure.com/contoso/_git/Web/pullrequest/5", "ado", "contoso/Web/Web"),
            ("https://contoso.visualstudio.com/_git/Web/pullrequest/5", "ado", "contoso/Web/Web"),
            ("https://tfs.example.com/tfs/Coll/_git/Web/pullrequest/5", "server", "Coll/Web/Web"),
        ];
        for (url, connection_id, repository_key) in cases {
            let key = resolve(url, &connections).unwrap_or_else(|error| panic!("{url}: {error}"));
            assert_eq!(
                (key.connection_id.as_str(), key.repository_key.as_str(), key.number),
                (connection_id, repository_key, 5),
                "{url}"
            );
        }
    }

    #[test]
    fn builds_web_urls_that_resolve_back() {
        let connections = [
            connection("gh", ProviderKind::GitHub, "https://github.com"),
            connection("gl", ProviderKind::GitLab, "https://gitlab.example.com/"),
            connection("ado", ProviderKind::AzureDevOps, "https://dev.azure.com"),
        ];
        let cases = [
            ("gh", "cli/cli", "https://github.com/cli/cli/pull/7"),
            ("gl", "group/sub/app", "https://gitlab.example.com/group/sub/app/-/merge_requests/7"),
            (
                "ado",
                "Org/My Project/repo",
                "https://dev.azure.com/Org/My%20Project/_git/repo/pullrequest/7",
            ),
        ];
        for (connection_id, repository_key, expected) in cases {
            let connection = connections
                .iter()
                .find(|connection| connection.id == connection_id)
                .expect("connection");
            let url = web_url(connection, repository_key, 7);
            assert_eq!(url, expected);
            let key = resolve(&url, &connections).expect("resolves back");
            assert_eq!((key.repository_key.as_str(), key.number), (repository_key, 7));
        }
    }

    #[test]
    fn resolves_issue_and_work_item_links_on_several_hosts() {
        let connections = [
            connection("gh", ProviderKind::GitHub, "https://github.com"),
            connection("ghe", ProviderKind::GitHub, "https://git.example.com"),
            connection("gl", ProviderKind::GitLab, "https://gitlab.com"),
            connection("gl-self", ProviderKind::GitLab, "https://gitlab.example.com"),
            connection("ado", ProviderKind::AzureDevOps, "https://dev.azure.com"),
            connection("ado-server", ProviderKind::AzureDevOps, "https://tfs.example.com/tfs"),
        ];
        let cases = [
            ("https://github.com/cli/cli/issues/5", "gh", ItemKind::Issue, "cli/cli", 5),
            (
                "https://tfs.example.com/tfs/Coll/Proj/_git/repo/pullrequest/43",
                "ado-server",
                ItemKind::PullRequest,
                "Coll/Proj/repo",
                43,
            ),
            (
                "https://git.example.com/team/app/pull/8/files",
                "ghe",
                ItemKind::PullRequest,
                "team/app",
                8,
            ),
            ("https://gitlab.com/group/app/-/issues/4", "gl", ItemKind::Issue, "group/app", 4),
            (
                "https://gitlab.example.com/a/b/c/-/work_items/6",
                "gl-self",
                ItemKind::Issue,
                "a/b/c",
                6,
            ),
            (
                "https://gitlab.example.com/a/b/-/merge_requests/2",
                "gl-self",
                ItemKind::PullRequest,
                "a/b",
                2,
            ),
            (
                "https://dev.azure.com/org/My%20Project/_workitems/edit/77",
                "ado",
                ItemKind::WorkItem,
                "org/My Project",
                77,
            ),
            (
                "https://org.visualstudio.com/Web/_workitems/edit/3",
                "ado",
                ItemKind::WorkItem,
                "org/Web",
                3,
            ),
        ];
        for (url, connection_id, kind, repository_key, number) in cases {
            let (found, item) = resolve_item(url, &connections).unwrap_or_else(|| panic!("{url}"));
            assert_eq!(
                (found.as_str(), item.kind, item.repository_key.as_str(), item.number),
                (connection_id, kind, repository_key, number),
                "{url}"
            );
            let connection =
                connections.iter().find(|connection| connection.id == found).expect("connection");
            let back = item_web_url(connection, &item);
            assert_eq!(
                resolve_item(&back, &connections).map(|(_, again)| again),
                Some(item),
                "{back}"
            );
        }
        assert!(resolve_item("https://github.com/cli/cli/discussions/1", &connections).is_none());
        assert!(resolve_item("https://example.org/a/b/issues/1", &connections).is_none());
    }

    #[test]
    fn prefers_a_connection_with_token() {
        let connections = [
            connection("anonymous", ProviderKind::GitHub, "https://github.com"),
            Connection {
                token: Some("t".into()),
                ..connection("work", ProviderKind::GitHub, "https://github.com")
            },
        ];
        let key = resolve("https://github.com/a/b/pull/1", &connections).expect("resolves");
        assert_eq!(key.connection_id, "work");
    }

    #[test]
    fn names_the_cloud_platform_of_links_without_a_connection() {
        let connections = [
            connection("ghe", ProviderKind::GitHub, "https://git.example.com"),
            connection("gl-self", ProviderKind::GitLab, "https://gitlab.example.com"),
        ];
        let cases = [
            ("https://github.com/cli/cli/pull/3/files", ProviderKind::GitHub, None),
            ("https://gitlab.com/group/app/-/merge_requests/4", ProviderKind::GitLab, None),
            (
                "https://dev.azure.com/Contoso/Web/_git/site/pullrequest/5",
                ProviderKind::AzureDevOps,
                Some("Contoso"),
            ),
            (
                "https://contoso.visualstudio.com/Web/_git/site/pullrequest/6",
                ProviderKind::AzureDevOps,
                Some("contoso"),
            ),
        ];
        for (url, kind, organization) in cases {
            assert_eq!(
                unconnected_cloud_link(url, &connections),
                Some((kind, organization.map(str::to_owned))),
                "{url}"
            );
        }
        // Served by a connection, on an unknown host, or no pull request: nothing to set up.
        assert_eq!(
            unconnected_cloud_link("https://git.example.com/team/app/pull/8", &connections),
            None
        );
        assert_eq!(
            unconnected_cloud_link(
                "https://gitlab.example.com/a/b/-/merge_requests/2",
                &connections
            ),
            None
        );
        assert_eq!(unconnected_cloud_link("https://example.org/a/b/pull/1", &connections), None);
        assert_eq!(
            unconnected_cloud_link("https://github.com/cli/cli/issues/1", &connections),
            None
        );
    }

    #[test]
    fn rejects_unknown_hosts_and_paths() {
        let connections = [connection("gh", ProviderKind::GitHub, "https://github.com")];
        assert!(resolve("https://example.com/a/b/pull/1", &connections).is_err());
        assert!(resolve("https://github.com/a/b/issues/1", &connections).is_err());
    }
}
