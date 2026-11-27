//! Which of the user's clones belongs to a repository: found by matching the URLs of the clones'
//! remotes against the repository's clone URL, in the folders the user names. Only the clones'
//! config files are read; no git process runs and nothing is written.

// RS-DONT-04: the input is URLs and config files from the user's disk.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

/// How deep below a search folder clones are looked for: `dev/app` and `dev/org/app`.
const MAX_DEPTH: usize = 2;
/// Folders looked into per search, at most; a search folder of a whole drive stays quick.
const MAX_VISITED: usize = 4_000;

/// A clone of the repository, and the name of its remote that points there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundClone {
    pub dir: PathBuf,
    pub remote: String,
}

/// The repository a remote URL names, comparable across its spellings: no scheme, user,
/// trailing slash or `.git`, the host in lower case. `git@github.com:acme/app.git`,
/// `ssh://git@github.com/acme/app` and `https://user@github.com/acme/app.git` are alike, and so
/// are Azure DevOps' SSH (`ssh.dev.azure.com:v3/org/project/repo`) and HTTPS URLs
/// (`dev.azure.com/org/project/_git/repo`). `None` for something that names no repository.
pub fn repository_id(url: &str) -> Option<String> {
    let url = url.trim();
    let (authority, path) = if let Some((_, rest)) = url.split_once("://") {
        rest.split_once('/')?
    } else {
        // scp-like syntax, `user@host:path`; never a Windows drive (`C:\…`) or a local path.
        let (authority, path) = url.split_once(':')?;
        if authority.contains(['/', '\\']) || authority.len() == 1 {
            return None;
        }
        (authority, path)
    };
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = host_port.split(':').next().unwrap_or_default().to_lowercase();
    let path = path.trim_matches('/').trim_end_matches(".git").trim_end_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    let azure_ssh = matches!(host.as_str(), "ssh.dev.azure.com" | "vs-ssh.visualstudio.com");
    if azure_ssh && let Some(rest) = path.strip_prefix("v3/") {
        let mut parts = rest.splitn(3, '/');
        let (organization, project, repository) = (parts.next()?, parts.next()?, parts.next()?);
        return Some(format!("dev.azure.com/{organization}/{project}/_git/{repository}"));
    }
    Some(format!("{host}/{path}"))
}

/// True when both URLs name the same repository ([`repository_id`]).
pub fn same_repository(first: &str, second: &str) -> bool {
    match (repository_id(first), repository_id(second)) {
        (Some(first), Some(second)) => first.eq_ignore_ascii_case(&second),
        _ => false,
    }
}

/// `url` without user name and password, for a remote that must never carry credentials.
pub fn without_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    if path.is_empty() { format!("{scheme}://{host}") } else { format!("{scheme}://{host}/{path}") }
}

/// The remotes of a git config file: `(name, url)` in the file's order.
pub(crate) fn remotes_in(config: &str) -> Vec<(String, String)> {
    let mut remotes = Vec::new();
    let mut section: Option<String> = None;
    for line in config.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[') {
            section = header
                .trim_end_matches(']')
                .strip_prefix("remote ")
                .map(|name| name.trim().trim_matches('"').to_owned());
            continue;
        }
        let Some(name) = &section else {
            continue;
        };
        if let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case("url")
        {
            remotes.push((name.clone(), value.trim().trim_matches('"').to_owned()));
        }
    }
    remotes
}

/// The remote of the clone in `dir` whose URL names the same repository as `url`.
pub fn matching_remote(dir: &Path, url: &str) -> Option<String> {
    let config = std::fs::read_to_string(dir.join(".git").join("config")).ok()?;
    remotes_in(&config)
        .into_iter()
        .find(|(_, remote_url)| same_repository(remote_url, url))
        .map(|(name, _)| name)
}

/// The clones in `folders` (the folders and up to [`MAX_DEPTH`] levels below) with a remote of
/// the repository `url` names. A worktree or submodule (a `.git` file) is not a clone; nothing
/// below a clone is looked into. Reads the disk: call it off the UI thread.
pub fn find_clones(folders: &[PathBuf], url: &str) -> Vec<FoundClone> {
    let mut found = Vec::new();
    let mut visited = 0;
    let mut pending: Vec<(PathBuf, usize)> =
        folders.iter().map(|folder| (folder.clone(), 0)).collect();
    while let Some((dir, depth)) = pending.pop() {
        visited += 1;
        if visited > MAX_VISITED {
            break;
        }
        if dir.join(".git").is_dir() {
            if let Some(remote) = matching_remote(&dir, url) {
                found.push(FoundClone { dir, remote });
            }
            continue;
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            let is_hidden = entry.file_name().to_string_lossy().starts_with('.');
            if is_dir && !is_hidden {
                pending.push((entry.path(), depth + 1));
            }
        }
    }
    found.sort_by(|first, second| first.dir.cmp(&second.dir));
    found.dedup_by(|first, second| first.dir == second.dir);
    found
}

#[cfg(test)]
mod tests {
    use super::{remotes_in, repository_id, same_repository, without_credentials};

    #[test]
    fn remote_urls_of_one_repository_match_in_every_spelling() {
        let github = [
            "https://github.com/acme/login-app.git",
            "https://user@GitHub.com/acme/login-app",
            "git@github.com:acme/login-app.git",
            "ssh://git@github.com/acme/login-app/",
            "ssh://git@github.com:22/acme/login-app.git",
        ];
        for url in github {
            assert_eq!(repository_id(url).as_deref(), Some("github.com/acme/login-app"), "{url}");
        }
        assert!(same_repository(
            "git@ssh.dev.azure.com:v3/contoso/Web/site",
            "https://contoso@dev.azure.com/contoso/Web/_git/site"
        ));
        assert!(!same_repository(
            "https://github.com/acme/login-app",
            "https://github.com/acme/login-app-docs"
        ));
        assert!(!same_repository("https://github.com/acme/a", "https://gitlab.com/acme/a"));
        assert_eq!(repository_id(r"C:\Development\app"), None, "a Windows path is no remote");
        assert_eq!(repository_id(""), None);
    }

    #[test]
    fn credentials_never_stay_in_a_remote_url() {
        assert_eq!(
            without_credentials("https://x-access-token:ghs_abc@github.com/acme/app.git"),
            "https://github.com/acme/app.git"
        );
        assert_eq!(
            without_credentials("https://github.com/acme/app.git"),
            "https://github.com/acme/app.git"
        );
        assert_eq!(
            without_credentials("git@github.com:acme/app.git"),
            "git@github.com:acme/app.git"
        );
    }

    #[test]
    fn remotes_are_read_from_the_config_file() {
        let config = "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@github.com:me/app.git\n\
            \tfetch = +refs/heads/*:refs/remotes/origin/*\n[remote \"upstream\"]\n\
            \turl = https://github.com/acme/app.git\n[branch \"main\"]\n\tremote = origin\n";
        assert_eq!(
            remotes_in(config),
            [
                ("origin".to_owned(), "git@github.com:me/app.git".to_owned()),
                ("upstream".to_owned(), "https://github.com/acme/app.git".to_owned())
            ]
        );
    }
}
