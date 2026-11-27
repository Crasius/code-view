//! Where the images of one pull request's Markdown point, and the [`ImageSource`] its page
//! publishes for them.
//!
//! * Absolute `http(s)` addresses load through the connection's provider: it adds the token only
//!   for the connection's own hosts (GitLab uploads, Azure DevOps attachments), signs GitHub
//!   attachments of the pull request's repository, and asks every other host anonymously.
//!   `//host/…` is https. The cache key is the address as written, never a signed one.
//! * In a rendered repository file, relative paths (`./img/a.png`, `../docs/x.webp`) are relative
//!   to the file and `/assets/y.svg` to the repository root, at the file's commit; they load from
//!   the local mirror. A path that leaves the repository is not loaded.
//! * In descriptions and comments, GitLab's `/uploads/…` belongs to the project; other paths
//!   starting with `/` belong to the platform's host, and relative paths are repository files at the
//!   pull request's head.
//! * `file:`, `data:` and other schemes are never loaded.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use cv_markdown::{self as markdown, FileBase, ImageRequest, ImageSource};
use cv_media::image_state::ImageState;
use cv_media::images::{Fetch, ImageStore, Owner, Target};
use cv_media::picture::Picture;
use cv_runtime::intents;
use cv_runtime::links::pull_request_link;
use cv_runtime::route::PullRequestKey;
use cv_services::Services;
use egui::Context;
use review_core::{Provider, ProviderKind};
use review_git::GitMirror;

use crate::session::PullRequestSession;
use crate::web_link;

/// Where one image source points, before anything is loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Resolved {
    Web(String),
    /// `path` in the repository at `commit`.
    Repository {
        commit: String,
        path: String,
    },
    Blocked(String),
}

/// What a page knows to resolve image sources.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Origin {
    kind: ProviderKind,
    /// The pull request's web page, for the repository's address.
    pull_request_url: String,
    /// The commit relative paths in descriptions and comments point into; `None` until the mirror is ready.
    head: Option<String>,
}

/// Resolves `src` as written in a document: a repository file at `base`, or a description or
/// comment of the pull request in `context`.
fn resolve(src: &str, base: Option<&FileBase>, context: &Origin) -> Option<Resolved> {
    let src = src.trim();
    if src.is_empty() {
        return Some(Resolved::Blocked("The image has no address".to_owned()));
    }
    let lower = src.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        return Some(Resolved::Web(src.to_owned()));
    }
    if let Some(rest) = src.strip_prefix("//") {
        return Some(Resolved::Web(format!("https://{rest}")));
    }
    if let Some(scheme) = scheme_of(src) {
        let reason = match scheme.as_str() {
            "file" => "Local files are not loaded".to_owned(),
            "data" => "Embedded data images are not shown".to_owned(),
            _ => format!("{scheme}: addresses are not loaded"),
        };
        return Some(Resolved::Blocked(reason));
    }
    if let Some(base) = base {
        return Some(match repository_file(&base.path, src) {
            Some(path) => Resolved::Repository { commit: base.commit.clone(), path },
            None => Resolved::Blocked("The path leaves the repository".to_owned()),
        });
    }
    let repository = web_link::repository_url(context.kind, &context.pull_request_url);
    if context.kind == ProviderKind::GitLab
        && src.starts_with("/uploads/")
        && let Some(project) = &repository
    {
        return Some(Resolved::Web(format!("{project}{src}")));
    }
    if src.starts_with('/') {
        let Ok(url) = reqwest::Url::parse(&context.pull_request_url) else {
            return Some(Resolved::Blocked("The pull request has no web address".to_owned()));
        };
        let origin = url.origin().ascii_serialization();
        return Some(Resolved::Web(format!("{origin}{src}")));
    }
    let head = context.head.as_ref()?;
    Some(match repository_path("", src) {
        Some(path) => Resolved::Repository { commit: head.clone(), path },
        None => Resolved::Blocked("The path leaves the repository".to_owned()),
    })
}

/// The repository path an image source of the rendered file `file` points at; `None` for web
/// addresses and sources that are not loaded.
pub fn repository_file(file: &str, src: &str) -> Option<String> {
    let src = src.trim();
    if src.is_empty() || src.starts_with("//") || scheme_of(src).is_some() {
        return None;
    }
    repository_path(file, src)
}

/// The scheme of an absolute address (`file`, `data`, `javascript`), lowercased. A Windows path
/// (`C:\…`) counts as one, so it is refused like `file:`.
fn scheme_of(src: &str) -> Option<String> {
    let (scheme, _) = src.split_once(':')?;
    let mut characters = scheme.chars();
    let is_scheme = characters.next().is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        });
    (is_scheme && !scheme.contains('/')).then(|| scheme.to_ascii_lowercase())
}

/// The repository path `src` points at from the file `file` (both `/`-separated): relative to the
/// file's folder, or to the root with a leading `/`. Query and fragment are dropped and escapes
/// decoded. `None` when it leaves the repository or names no file.
fn repository_path(file: &str, src: &str) -> Option<String> {
    let src = src.split(['?', '#']).next().unwrap_or_default();
    let src = urlencoding::decode(src).ok()?;
    let mut parts: Vec<&str> = if src.starts_with('/') {
        Vec::new()
    } else {
        let mut folder: Vec<&str> = file.split('/').filter(|part| !part.is_empty()).collect();
        folder.pop();
        folder
    };
    for part in src.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    if parts.is_empty() || src.ends_with('/') {
        return None;
    }
    Some(parts.join("/"))
}

/// What one page publishes: rebuilt when the pull request's commits change.
#[derive(Default)]
pub struct PageImages {
    source: Option<(Origin, Arc<PullRequestImages>)>,
}

impl PageImages {
    /// Makes the images of every Markdown text this frame load for this pull request.
    pub fn publish(&mut self, ctx: &Context, services: &Services, session: &PullRequestSession) {
        let Some(connection) = services.connection(&session.key.connection_id) else {
            return;
        };
        let context = Origin {
            kind: session.kind,
            pull_request_url: pull_request_link::web_url(
                connection,
                &session.key.repository_key,
                session.key.number,
            ),
            head: session.compared_commits().map(|(_, head)| head),
        };
        let is_stale = self.source.as_ref().is_none_or(|(built, _)| *built != context);
        if is_stale {
            let images = PullRequestImages {
                store: services.images.clone(),
                owner: Owner {
                    connection_id: session.key.connection_id.clone(),
                    provider: session.provider.clone(),
                    repository: session.key.repository_key.clone(),
                },
                git: session.mirror.ready().map(|mirror| mirror.git.clone()),
                scope: scope_of(&session.key),
                disk: services.paths.connection_images_dir(&session.key.connection_id),
                context: context.clone(),
                targets: Mutex::default(),
            };
            self.source = Some((context, Arc::new(images)));
        }
        if let Some((_, images)) = &self.source {
            markdown::publish_images(ctx, Arc::<PullRequestImages>::downgrade(images));
        }
    }
}

/// Connection and repository of `key`, part of every image key of its pull request.
fn scope_of(key: &PullRequestKey) -> u64 {
    egui::epaint::util::hash((&key.connection_id, &key.repository_key))
}

/// The target of the web image `url` in the texts of the pull request `scope` names, its body kept
/// in `disk` (a folder); the same key a page's widget asks with.
fn web_target(scope: u64, disk: &Path, owner: &Owner, url: String, video: bool) -> Target {
    let key = egui::epaint::util::hash(("web", scope, &url, video));
    let place = reqwest::Url::parse(&url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default();
    let disk = Some(disk.join(format!("{key:016x}")));
    Target {
        key,
        fetch: Fetch::Remote { url, owner: Some(owner.clone()), disk },
        place,
        video: None,
    }
}

/// The web images `description` of the pull request `key` shows, as its page will ask for them,
/// for reading them ahead ([`ImageStore::fetch_ahead`]). Repository files and videos are left out:
/// those need the mirror, these are large.
pub fn description_targets(
    services: &Services,
    key: &PullRequestKey,
    kind: ProviderKind,
    provider: Arc<dyn Provider>,
    description: &str,
) -> Vec<Target> {
    let Some(connection) = services.connection(&key.connection_id) else {
        return Vec::new();
    };
    let context = Origin {
        kind,
        pull_request_url: pull_request_link::web_url(connection, &key.repository_key, key.number),
        head: None,
    };
    let owner = Owner {
        connection_id: key.connection_id.clone(),
        provider,
        repository: key.repository_key.clone(),
    };
    let disk = services.paths.connection_images_dir(&key.connection_id);
    markdown::still_image_sources(description, Some(kind))
        .iter()
        .filter_map(|src| match resolve(src, None, &context)? {
            Resolved::Web(url) => Some(web_target(scope_of(key), &disk, &owner, url, false)),
            Resolved::Repository { .. } | Resolved::Blocked(_) => None,
        })
        .collect()
}

/// The [`ImageSource`] of one pull request page.
struct PullRequestImages {
    store: Arc<ImageStore>,
    owner: Owner,
    git: Option<Arc<GitMirror>>,
    /// Connection and repository, part of every key: the same address may answer differently
    /// with another connection's token.
    scope: u64,
    disk: PathBuf,
    context: Origin,
    /// Targets by source and base, so a frame does not resolve the same image again.
    targets: Mutex<HashMap<u64, Arc<Target>>>,
}

impl PullRequestImages {
    fn target(&self, image: &ImageRequest<'_>) -> Option<Arc<Target>> {
        let lookup = egui::epaint::util::hash((image.src, image.base, image.video));
        let mut targets = self.targets.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(target) = targets.get(&lookup) {
            return Some(target.clone());
        }
        let resolved = resolve(image.src, image.base, &self.context)?;
        let target = Arc::new(self.build(resolved, image.video)?);
        targets.insert(lookup, target.clone());
        Some(target)
    }

    /// The target of `resolved`; a `video` keeps its body in a file of its own for ffmpeg.
    fn build(&self, resolved: Resolved, video: bool) -> Option<Target> {
        let video_file = |key: u64| video.then(|| self.disk.join(format!("{key:016x}.video")));
        Some(match resolved {
            Resolved::Web(url) => {
                let target = web_target(self.scope, &self.disk, &self.owner, url, video);
                Target { video: video_file(target.key), ..target }
            }
            Resolved::Repository { commit, path } => {
                let key = egui::epaint::util::hash(("git", self.scope, &commit, &path, video));
                let place = format!("{path} at {}", commit.get(..8).unwrap_or(&commit));
                let git = self.git.clone()?;
                let video = video_file(key);
                Target { key, fetch: Fetch::Git { git, commit, path }, place, video }
            }
            Resolved::Blocked(reason) => {
                let key = egui::epaint::util::hash(("blocked", &reason));
                Target { key, fetch: Fetch::Blocked(reason), place: String::new(), video: None }
            }
        })
    }
}

impl ImageSource for PullRequestImages {
    fn state(&self, ctx: &Context, image: &ImageRequest<'_>, want: bool) -> ImageState {
        match self.target(image) {
            Some(target) => self.store.state(ctx, &target, want),
            // The mirror is not ready yet: the image waits in its placeholder.
            None => ImageState::Waiting { size: None },
        }
    }

    fn retry(&self, image: &ImageRequest<'_>) {
        if let Some(target) = self.target(image) {
            self.store.retry(target.key);
        }
    }

    fn web_url(&self, image: &ImageRequest<'_>) -> Option<String> {
        match resolve(image.src, image.base, &self.context)? {
            Resolved::Web(url) => Some(url),
            Resolved::Repository { commit, path } => web_link::raw_file_url(
                self.context.kind,
                &self.context.pull_request_url,
                &path,
                &commit,
            ),
            Resolved::Blocked(_) => None,
        }
    }

    fn picture(&self, image: &ImageRequest<'_>) -> Option<Picture> {
        self.store.picture(self.target(image)?.key)
    }

    fn fix_access(&self, ctx: &Context) {
        intents::request_fix(ctx, &self.owner.connection_id);
    }
}

#[cfg(test)]
mod tests {
    use review_core::ProviderKind;

    use super::{Origin, Resolved, repository_path, resolve};
    use cv_markdown::FileBase;

    fn context(kind: ProviderKind, pull_request_url: &str) -> Origin {
        Origin {
            kind,
            pull_request_url: pull_request_url.to_owned(),
            head: Some("head".to_owned()),
        }
    }

    fn web(url: &str) -> Resolved {
        Resolved::Web(url.to_owned())
    }

    #[test]
    fn relative_paths_resolve_against_the_file_and_stay_in_the_repository() {
        assert_eq!(
            repository_path("docs/guide/README.md", "./img/a.png").as_deref(),
            Some("docs/guide/img/a.png")
        );
        assert_eq!(
            repository_path("docs/guide/README.md", "../shots/x.webp").as_deref(),
            Some("docs/shots/x.webp")
        );
        assert_eq!(
            repository_path("docs/guide/README.md", "/assets/y.svg").as_deref(),
            Some("assets/y.svg")
        );
        assert_eq!(
            repository_path("README.md", "img/My%20shot.png?raw=true#top").as_deref(),
            Some("img/My shot.png")
        );
        assert_eq!(repository_path("README.md", "../outside.png"), None, "leaves the repository");
        assert_eq!(repository_path("docs/README.md", "../../etc/passwd"), None);
        assert_eq!(repository_path("README.md", "img/"), None, "a folder is no image");
    }

    #[test]
    fn rendered_files_load_repository_images_at_their_commit() {
        let base = FileBase { commit: "new".into(), path: "docs/README.md".into() };
        let context = context(ProviderKind::GitHub, "https://github.com/o/r/pull/1");
        assert_eq!(
            resolve("./img/a.png", Some(&base), &context),
            Some(Resolved::Repository { commit: "new".into(), path: "docs/img/a.png".into() })
        );
        assert_eq!(
            resolve("https://x.test/a.png", Some(&base), &context),
            Some(web("https://x.test/a.png"))
        );
        assert_eq!(
            resolve("//cdn.test/a.png", Some(&base), &context),
            Some(web("https://cdn.test/a.png"))
        );
        assert!(matches!(
            resolve("../../a.png", Some(&base), &context),
            Some(Resolved::Blocked(_))
        ));
    }

    #[test]
    fn local_files_and_other_schemes_are_never_loaded() {
        let context = context(ProviderKind::GitHub, "https://github.com/o/r/pull/1");
        for src in [
            "file:///etc/passwd",
            "C:\\Users\\me\\a.png",
            "data:image/png;base64,AAAA",
            "javascript:alert(1)",
            "",
        ] {
            assert!(matches!(resolve(src, None, &context), Some(Resolved::Blocked(_))), "{src}");
        }
    }

    #[test]
    fn comment_images_per_provider() {
        let github = context(ProviderKind::GitHub, "https://github.com/o/r/pull/1");
        assert_eq!(
            resolve("https://github.com/user-attachments/assets/1b2c", None, &github),
            Some(web("https://github.com/user-attachments/assets/1b2c"))
        );
        assert_eq!(
            resolve("docs/a.png", None, &github),
            Some(Resolved::Repository { commit: "head".into(), path: "docs/a.png".into() })
        );

        let gitlab = context(
            ProviderKind::GitLab,
            "https://gitlab.example.com/group/sub/project/-/merge_requests/7",
        );
        assert_eq!(
            resolve("/uploads/0123456789abcdef0123456789abcdef/shot.png", None, &gitlab),
            Some(web(
                "https://gitlab.example.com/group/sub/project/uploads/0123456789abcdef0123456789abcdef/shot.png"
            ))
        );
        assert_eq!(
            resolve("/-/project/4/uploads/ab/x.png", None, &gitlab),
            Some(web("https://gitlab.example.com/-/project/4/uploads/ab/x.png"))
        );

        let azure = context(
            ProviderKind::AzureDevOps,
            "https://dev.azure.com/org/project/_git/repo/pullrequest/42",
        );
        let attachment = "https://dev.azure.com/org/project/_apis/git/repositories/r/pullRequests/42/attachments/a.png";
        assert_eq!(resolve(attachment, None, &azure), Some(web(attachment)));
        assert_eq!(
            resolve("/org/_apis/wit/attachments/9?fileName=a.png", None, &azure),
            Some(web("https://dev.azure.com/org/_apis/wit/attachments/9?fileName=a.png"))
        );

        let unparsable = context(ProviderKind::GitHub, "");
        assert!(
            matches!(resolve("/x.png", None, &unparsable), Some(Resolved::Blocked(_))),
            "an absolute path without the pull request's web address is blocked, not waiting"
        );
    }

    #[test]
    fn relative_comment_images_wait_for_the_mirror() {
        let context =
            Origin { head: None, ..context(ProviderKind::GitHub, "https://github.com/o/r/pull/1") };
        assert_eq!(resolve("docs/a.png", None, &context), None);
    }
}
