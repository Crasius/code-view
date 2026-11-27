//! Turns what the user types or pastes for a host (an organization name, a browser URL, a server
//! address) into the connection's web root and, for Azure DevOps, its organization or collection.

use review_core::ProviderKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hosting {
    Cloud,
    SelfHosted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedHost {
    pub base_url: String,
    /// Azure DevOps organization (Services) or collection (Server); `None` for GitHub and GitLab.
    pub organization: Option<String>,
}

impl ResolvedHost {
    /// `github.com`, `gitlab.example.com`, `dev.azure.com/contoso`, for headlines and buttons.
    pub(crate) fn display(&self) -> String {
        let host = self.base_url.split_once("://").map_or(self.base_url.as_str(), |(_, rest)| rest);
        match &self.organization {
            Some(organization) => format!("{host}/{organization}"),
            None => host.to_owned(),
        }
    }

    /// The bare host name, as `gh auth token --hostname` expects it.
    pub(crate) fn host_name(&self) -> &str {
        let rest = self.base_url.split_once("://").map_or(self.base_url.as_str(), |(_, rest)| rest);
        rest.split('/').next().unwrap_or(rest)
    }
}

pub(crate) fn cloud_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::GitHub => "GitHub.com",
        ProviderKind::GitLab => "GitLab.com",
        ProviderKind::AzureDevOps => "Azure DevOps Services",
    }
}

pub(crate) fn cloud_detail(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::GitHub => "github.com, including GitHub Enterprise Cloud organizations",
        ProviderKind::GitLab => "gitlab.com",
        ProviderKind::AzureDevOps => "dev.azure.com and *.visualstudio.com organizations",
    }
}

pub(crate) fn self_hosted_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::GitHub => "GitHub Enterprise Server",
        ProviderKind::GitLab => "Self-managed GitLab",
        ProviderKind::AzureDevOps => "Azure DevOps Server",
    }
}

pub(crate) fn self_hosted_detail(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::GitHub => "Your company's own GitHub server",
        ProviderKind::GitLab => "Your company's own GitLab server",
        ProviderKind::AzureDevOps => "On-premises Azure DevOps Server or Team Foundation Server",
    }
}

/// Label and example of the one field the host step asks for.
pub(crate) fn field(kind: ProviderKind, hosting: Hosting) -> (&'static str, &'static str) {
    match (kind, hosting) {
        (ProviderKind::AzureDevOps, Hosting::Cloud) => {
            ("Organization", "contoso, or paste https://dev.azure.com/contoso")
        }
        (ProviderKind::AzureDevOps, Hosting::SelfHosted) => {
            ("Collection URL", "https://tfs.example.com/tfs/DefaultCollection")
        }
        (ProviderKind::GitHub, _) => ("Server URL", "https://github.example.com"),
        (ProviderKind::GitLab, _) => ("Server URL", "https://gitlab.example.com"),
    }
}

/// The host step needs no input for GitHub.com and GitLab.com.
pub(crate) fn needs_input(kind: ProviderKind, hosting: Hosting) -> bool {
    kind == ProviderKind::AzureDevOps || hosting == Hosting::SelfHosted
}

pub(crate) fn resolve(
    kind: ProviderKind,
    hosting: Hosting,
    input: &str,
) -> Result<ResolvedHost, String> {
    let input = input.trim();
    match (kind, hosting) {
        (ProviderKind::GitHub | ProviderKind::GitLab, Hosting::Cloud) => {
            Ok(ResolvedHost { base_url: kind.default_base_url().to_owned(), organization: None })
        }
        (ProviderKind::GitHub | ProviderKind::GitLab, Hosting::SelfHosted) => {
            let url = web_root(input)?;
            let url = ["/api/v3", "/api/v4", "/api/graphql", "/api"]
                .iter()
                .find_map(|suffix| url.strip_suffix(suffix))
                .map_or(url.clone(), str::to_owned);
            Ok(ResolvedHost { base_url: url, organization: None })
        }
        (ProviderKind::AzureDevOps, Hosting::Cloud) => Ok(ResolvedHost {
            base_url: kind.default_base_url().to_owned(),
            organization: Some(services_organization(input)?),
        }),
        (ProviderKind::AzureDevOps, Hosting::SelfHosted) => {
            let url = web_root(input)?;
            let (scheme, rest) = url.split_once("://").unwrap_or(("https", &url));
            let Some((server, collection)) = rest.rsplit_once('/') else {
                return Err("Add the collection to the URL, for example https://tfs.example.com/tfs/DefaultCollection."
                    .to_owned());
            };
            Ok(ResolvedHost {
                base_url: format!("{scheme}://{server}"),
                organization: Some(decode(collection)),
            })
        }
    }
}

/// `contoso`, `https://dev.azure.com/contoso/Project/...` or `https://contoso.visualstudio.com/...` → `contoso`.
fn services_organization(input: &str) -> Result<String, String> {
    if input.is_empty() {
        return Err("Enter your organization name.".to_owned());
    }
    let without_scheme = input.split_once("://").map_or(input, |(_, rest)| rest);
    let mut parts = without_scheme.split('/');
    let first = parts.next().unwrap_or_default().to_ascii_lowercase();
    let organization = if first == "dev.azure.com" {
        parts.next().unwrap_or_default().to_owned()
    } else if let Some(name) = first.strip_suffix(".visualstudio.com") {
        name.to_owned()
    } else if without_scheme.contains('/') || without_scheme.contains('.') {
        return Err("Use the organization name or a dev.azure.com address.".to_owned());
    } else {
        without_scheme.to_owned()
    };
    let organization = decode(organization.trim());
    if organization.is_empty() {
        return Err("The address has no organization after dev.azure.com/.".to_owned());
    }
    if organization.contains(char::is_whitespace) {
        return Err("Organization names have no spaces.".to_owned());
    }
    Ok(organization)
}

/// Adds `https://` when missing (a scheme in any case reads as lowercase) and drops trailing
/// slashes, query and fragment.
fn web_root(input: &str) -> Result<String, String> {
    if input.is_empty() {
        return Err("Enter the server address.".to_owned());
    }
    let address = input.split(['?', '#']).next().unwrap_or_default().trim_end_matches('/');
    let with_scheme = ["https://", "http://"].into_iter().find_map(|scheme| {
        let start = address.get(..scheme.len())?;
        let rest = address.get(scheme.len()..)?;
        start.eq_ignore_ascii_case(scheme).then(|| format!("{scheme}{rest}"))
    });
    let url = with_scheme.unwrap_or_else(|| format!("https://{address}"));
    let host = url
        .split_once("://")
        .map(|(_, rest)| rest.split('/').next().unwrap_or_default())
        .unwrap_or_default();
    if host.is_empty() || host.contains(char::is_whitespace) {
        return Err("That doesn't look like a server address.".to_owned());
    }
    Ok(url)
}

fn decode(segment: &str) -> String {
    urlencoding::decode(segment).map_or_else(|_| segment.to_owned(), std::borrow::Cow::into_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(kind: ProviderKind, hosting: Hosting, input: &str) -> (String, Option<String>) {
        let host = resolve(kind, hosting, input).unwrap_or_else(|error| panic!("{input}: {error}"));
        (host.base_url, host.organization)
    }

    #[test]
    fn accepts_azure_organization_names_and_urls() {
        for input in [
            "contoso",
            "https://dev.azure.com/contoso",
            "dev.azure.com/contoso/Web/_git/site/pullrequest/5",
            "https://contoso.visualstudio.com/Web",
        ] {
            assert_eq!(
                resolved(ProviderKind::AzureDevOps, Hosting::Cloud, input),
                ("https://dev.azure.com".to_owned(), Some("contoso".to_owned())),
                "{input}"
            );
        }
        assert!(resolve(ProviderKind::AzureDevOps, Hosting::Cloud, "").is_err());
        assert!(
            resolve(ProviderKind::AzureDevOps, Hosting::Cloud, "https://dev.azure.com/").is_err()
        );
    }

    #[test]
    fn splits_azure_server_collection_urls() {
        assert_eq!(
            resolved(
                ProviderKind::AzureDevOps,
                Hosting::SelfHosted,
                "tfs.example.com/tfs/DefaultCollection/"
            ),
            ("https://tfs.example.com/tfs".to_owned(), Some("DefaultCollection".to_owned()))
        );
        assert!(
            resolve(ProviderKind::AzureDevOps, Hosting::SelfHosted, "https://tfs.example.com")
                .is_err()
        );
    }

    #[test]
    fn normalizes_self_hosted_servers() {
        assert_eq!(
            resolved(ProviderKind::GitHub, Hosting::SelfHosted, "github.example.com/api/v3/"),
            ("https://github.example.com".to_owned(), None)
        );
        assert_eq!(
            resolved(ProviderKind::GitLab, Hosting::SelfHosted, "http://gitlab.local:8080/"),
            ("http://gitlab.local:8080".to_owned(), None)
        );
        assert_eq!(
            resolved(ProviderKind::GitLab, Hosting::Cloud, "ignored").0,
            "https://gitlab.com"
        );
        assert!(resolve(ProviderKind::GitHub, Hosting::SelfHosted, " ").is_err());
    }

    #[test]
    fn reads_a_scheme_in_capitals_as_the_scheme() {
        assert_eq!(
            resolved(ProviderKind::GitLab, Hosting::SelfHosted, "HTTPS://GitLab.Example.com/"),
            ("https://GitLab.Example.com".to_owned(), None)
        );
        assert_eq!(
            resolved(ProviderKind::GitHub, Hosting::SelfHosted, "Http://git.local:8080/api/v3"),
            ("http://git.local:8080".to_owned(), None)
        );
        assert_eq!(
            resolved(
                ProviderKind::AzureDevOps,
                Hosting::SelfHosted,
                "HTTPS://tfs.example.com/tfs/DefaultCollection"
            ),
            ("https://tfs.example.com/tfs".to_owned(), Some("DefaultCollection".to_owned()))
        );
    }

    #[test]
    fn displays_host_and_organization() {
        let host = resolve(ProviderKind::AzureDevOps, Hosting::Cloud, "contoso").unwrap();
        assert_eq!(host.display(), "dev.azure.com/contoso");
        let server =
            resolve(ProviderKind::GitHub, Hosting::SelfHosted, "https://git.corp.example/")
                .unwrap();
        assert_eq!(server.host_name(), "git.corp.example");
    }
}
