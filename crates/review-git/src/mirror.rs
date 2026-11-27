use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use review_core::{GitRemote, ProviderError, Result};
use tokio::sync::Mutex;

use crate::changed_files::{self, ChangedFile};
use crate::command::{self, GitCommand, Objects};
use crate::commits::{self, CommitInfo};
use crate::file_diff::FileDiff;
use crate::line_counts::{self, COUNT_WAIT, CountSink, Wanted, WantedBy};
use crate::progress::{GitEvent, ProgressSink};

const LOCAL_REF_PREFIX: &str = "refs/review/";
/// What `git fetch` says when the remote has no such ref (any more).
const MISSING_REMOTE_REF: &str = "couldn't find remote ref";
/// Larger than any file a reviewer would scroll through, small enough for git's int parsing.
pub const FULL_FILE_CONTEXT: u32 = 1_000_000;
/// The file in a mirror's git dir whose lock the opener holds that creates or sets it up; git
/// ignores files it does not know there.
const INIT_LOCK: &str = "codeview-init.lock";

/// A bare, blob-less partial clone of one remote repository under the app's cache directory.
/// Cheap to clone (`Arc` it); fetches are serialized per mirror.
pub struct GitMirror {
    git_dir: PathBuf,
    authorization: Option<String>,
    fetch_lock: Mutex<()>,
    /// The files the user is near, which the line counts download first.
    wanted: Wanted,
}

impl GitMirror {
    /// Opens the mirror for `remote` under `cache_root`, creating the bare repository on first use.
    pub async fn open(cache_root: &Path, remote: &GitRemote) -> Result<Self> {
        let git_dir = mirror_dir(cache_root, &remote.clone_url);
        let authorization =
            remote.authorization_header.as_ref().map(|value| format!("Authorization: {value}"));
        let mirror =
            Self { git_dir, authorization, fetch_lock: Mutex::new(()), wanted: Wanted::default() };
        mirror.initialize(&remote.clone_url).await?;
        Ok(mirror)
    }

    /// True when [`Self::open`] for `remote` would find a mirror instead of creating one.
    pub fn exists(cache_root: &Path, remote: &GitRemote) -> bool {
        mirror_dir(cache_root, &remote.clone_url).join("HEAD").exists()
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    pub(crate) fn git(&self) -> GitCommand<'_> {
        GitCommand::new(&self.git_dir, self.authorization.as_deref())
    }

    /// Held by whoever fetches into the mirror: one fetch at a time.
    pub(crate) fn fetch_lock(&self) -> &Mutex<()> {
        &self.fetch_lock
    }

    /// Creates the bare repository when it is missing and writes the settings of a blob-less
    /// partial clone of `clone_url` that its config does not hold yet. A mirror opened before
    /// spawns no git at all; a changed clone URL is written again.
    ///
    /// One opener at a time writes: two `git init` or `git config` on one repository fail on
    /// each other's `config.lock`. The others wait on [`INIT_LOCK`], a file lock, so a second app
    /// process waits as well, and then find the repository set up.
    async fn initialize(&self, clone_url: &str) -> Result<()> {
        let settings = [
            ("core.repositoryformatversion", "1"),
            ("extensions.partialClone", "origin"),
            ("remote.origin.url", clone_url),
            ("remote.origin.promisor", "true"),
            ("remote.origin.partialclonefilter", "blob:none"),
            // A local checkout fetches from the mirror ([`crate::checkout`]): a new clone asks for
            // commits and trees only, then for the blobs of the tree it checks out, by id.
            ("uploadpack.allowFilter", "true"),
            ("uploadpack.allowAnySHA1InWant", "true"),
        ];
        if self.missing_settings(&settings).await.is_empty() {
            return Ok(());
        }
        tokio::fs::create_dir_all(&self.git_dir).await?;
        let lock_path = self.git_dir.join(INIT_LOCK);
        let _lock = tokio::task::spawn_blocking(move || {
            let file = std::fs::File::options()
                .create(true)
                .truncate(false)
                .write(true)
                .open(lock_path)?;
            file.lock()?;
            Ok::<_, std::io::Error>(file)
        })
        .await
        .map_err(|error| {
            ProviderError::Local(format!("waiting for the mirror failed: {error}"))
        })??;

        let is_initialized = self.git_dir.join("HEAD").exists();
        if !is_initialized {
            GitCommand::new(&self.git_dir, None).run(["init", "--bare", "--quiet"]).await?;
        }
        // The lock's owner before us may have written them all.
        for (name, value) in self.missing_settings(&settings).await {
            self.git().run(["config", name, value]).await?;
        }
        Ok(())
    }

    /// The `settings` the mirror's config does not hold; all of them when there is no config yet
    /// or it cannot be read, as after `git init`.
    async fn missing_settings<'s>(
        &self,
        settings: &[(&'s str, &'s str)],
    ) -> Vec<(&'s str, &'s str)> {
        let config =
            tokio::fs::read_to_string(self.git_dir.join("config")).await.unwrap_or_default();
        let is_initialized = self.git_dir.join("HEAD").exists();
        settings
            .iter()
            .filter(|(name, value)| !is_initialized || config_value(&config, name) != Some(*value))
            .copied()
            .collect()
    }

    /// Makes `commits` available locally, fetching `refs` (trees and commits only) when any is missing.
    /// Refs that do not exist on the remote are skipped; missing commits after that are fetched by id.
    /// Returns the refs the remote reported as gone (e.g. a deleted source branch); empty when
    /// nothing had to be fetched or everything was there.
    pub async fn ensure_commits(
        &self,
        refs: &[String],
        commits: &[&str],
        progress: &ProgressSink<'_>,
    ) -> Result<Vec<String>> {
        let _guard = self.fetch_lock.lock().await;
        if self.has_commits(commits).await? {
            progress(GitEvent::Message(
                "All commits are in the mirror already, nothing to fetch".to_owned(),
            ));
            return Ok(Vec::new());
        }

        let refspecs: Vec<String> = refs.iter().map(|name| local_refspec(name)).collect();
        let mut gone = Vec::new();
        let fetched_all = self.fetch(&refspecs, progress).await.is_ok();
        if !fetched_all {
            for (remote_ref, refspec) in refs.iter().zip(&refspecs) {
                // A single missing ref (e.g. a merge ref of a conflicting PR) must not block the rest.
                if let Err(error) = self.fetch(std::slice::from_ref(refspec), progress).await
                    && error.to_string().contains(MISSING_REMOTE_REF)
                {
                    gone.push(remote_ref.clone());
                }
            }
        }
        if self.has_commits(commits).await? {
            return Ok(gone);
        }

        let by_id: Vec<String> = commits.iter().map(|sha| (*sha).to_owned()).collect();
        self.fetch(&by_id, progress).await?;
        if self.has_commits(commits).await? {
            return Ok(gone);
        }
        Err(ProviderError::Local(format!(
            "commits {} are not reachable from the remote refs {}",
            commits.join(", "),
            refs.join(", ")
        )))
    }

    /// Makes `commits` available locally, fetching the missing ones by id (trees and commits only)
    /// and nothing when all are there. For commits no ref of the pull request reaches any more,
    /// such as the head an older comment was written on before a force push.
    pub async fn ensure_commits_by_id(
        &self,
        commits: &[&str],
        progress: &ProgressSink<'_>,
    ) -> Result<()> {
        let _guard = self.fetch_lock.lock().await;
        if self.has_commits(commits).await? {
            return Ok(());
        }
        let by_id: Vec<String> = commits.iter().map(|sha| (*sha).to_owned()).collect();
        self.fetch(&by_id, progress).await?;
        if self.has_commits(commits).await? {
            return Ok(());
        }
        Err(ProviderError::Local(format!("the remote no longer has commit {}", commits.join(", "))))
    }

    /// One commit's metadata, from the mirror.
    pub async fn commit(&self, sha: &str) -> Result<CommitInfo> {
        let output = self
            .git()
            .run_offline(["log", "-1", "--no-walk", "--no-color", commits::LOG_FORMAT, sha])
            .await?;
        commits::parse(&output)
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::Local(format!("commit {sha} is not in the mirror")))
    }

    async fn fetch(&self, refspecs: &[String], progress: &ProgressSink<'_>) -> Result<()> {
        let mut args: Vec<&str> = vec![
            "fetch",
            "--progress",
            "--no-tags",
            "--no-write-fetch-head",
            "--filter=blob:none",
            "origin",
        ];
        args.extend(refspecs.iter().map(String::as_str));
        self.git().run_with_progress(args, progress).await?;
        Ok(())
    }

    /// True when every one of `commits` is a commit in the mirror; one git process asks for all.
    async fn has_commits(&self, commits: &[&str]) -> Result<bool> {
        if commits.is_empty() {
            return Ok(true);
        }
        let mut input = String::new();
        for sha in commits {
            input.push_str(sha);
            input.push_str("^{commit}\n");
        }
        // One line per object: `<oid> commit <size>`, or `<name> missing` (exit code 0 either way).
        let output = self
            .git()
            .run_offline_with_input(["cat-file", "--batch-check"], input.as_bytes())
            .await?;
        let output = String::from_utf8_lossy(&output);
        let found = output.lines().filter(|line| line.split(' ').nth(1) == Some("commit")).count();
        Ok(found == commits.len())
    }

    pub async fn merge_base(&self, first: &str, second: &str) -> Result<String> {
        let output = self.git().run(["merge-base", first, second]).await?;
        let merge_base = String::from_utf8_lossy(&output).trim().to_owned();
        Ok(merge_base)
    }

    /// Phase 1 of the file list: paths and blob ids from trees only, no blob downloads. Instant even for huge PRs.
    pub async fn changed_files_fast(&self, base: &str, head: &str) -> Result<Vec<ChangedFile>> {
        let output =
            self.git().run(["diff-tree", "-r", "-z", "--no-renames", "--raw", base, head]).await?;
        changed_files::parse_raw(&String::from_utf8_lossy(&output))
    }

    /// Phase 2: renames, line counts and whitespace-only marks. Files whose blobs are local count
    /// at once, the files [`Self::want`] names next, the rest download in chunks, and `publish`
    /// gets the list after each chunk while blobs download (`crate::line_counts`). The result
    /// equals one `git diff-tree -M --numstat`.
    pub async fn changed_files_detailed(
        &self,
        base: &str,
        head: &str,
        publish: &CountSink<'_>,
    ) -> Result<Vec<ChangedFile>> {
        line_counts::count(&self.git(), (base, head), &self.wanted, publish).await
    }

    /// Replaces the files (paths) `by` wants the line counts on their way to download first; a
    /// path no count waits for costs nothing.
    pub fn want(&self, by: WantedBy, paths: &[String]) {
        self.wanted.feed(by, paths);
    }

    /// [`Self::changed_files_detailed`] of the merge base of `target` and `head` against `head`,
    /// from objects the mirror already has; `None` when a commit or a changed blob is not local
    /// yet, so asking never downloads anything.
    pub async fn local_changed_files(
        &self,
        target: &str,
        head: &str,
    ) -> Result<Option<Vec<ChangedFile>>> {
        if !self.has_commits(&[target, head]).await? {
            return Ok(None);
        }
        let merge_base =
            String::from_utf8_lossy(&self.git().run_offline(["merge-base", target, head]).await?)
                .trim()
                .to_owned();
        // Offline, a blob that is not local fails the command instead of being fetched.
        let Ok(output) = self
            .git()
            .run_offline(["diff-tree", "-r", "-z", "-M", "--raw", "--numstat", &merge_base, head])
            .await
        else {
            return Ok(None);
        };
        changed_files::parse_raw_with_numstat(&String::from_utf8_lossy(&output)).map(Some)
    }

    /// Unified diff of one file between two commits, as `options` say. Downloads the file's blobs
    /// when the mirror does not have them yet; while a line count of the same commits has not
    /// reached the file, the count downloads them first and the diff waits for it, so no blob
    /// downloads twice, for [`crate::COUNT_WAIT`] at most: a slow chunk never holds a diff up.
    pub async fn file_diff(
        &self,
        base: &str,
        head: &str,
        file: &ChangedFile,
        options: DiffOptions,
    ) -> Result<FileDiff> {
        self.diff_after_count(base, head, file, options, Some(COUNT_WAIT)).await
    }

    /// [`Self::file_diff`] for work beside what the user waits on, such as a thread's excerpt:
    /// waits for the line count to bring the file's blobs however long its chunk takes, so they
    /// never download twice. A count that ends or fails frees it.
    pub async fn file_diff_after_count(
        &self,
        base: &str,
        head: &str,
        file: &ChangedFile,
        options: DiffOptions,
    ) -> Result<FileDiff> {
        self.diff_after_count(base, head, file, options, None).await
    }

    async fn diff_after_count(
        &self,
        base: &str,
        head: &str,
        file: &ChangedFile,
        options: DiffOptions,
        limit: Option<Duration>,
    ) -> Result<FileDiff> {
        let paths: Vec<&str> =
            std::iter::once(file.path.as_str()).chain(file.old_path.as_deref()).collect();
        self.wanted.wait_counted(base, head, &paths, limit).await;
        self.diff(base, head, file, options, Objects::FetchMissing).await
    }

    /// [`Self::file_diff`] from objects the mirror already has: fails rather than download a blob,
    /// so work done ahead of the user never touches the network.
    pub async fn local_file_diff(
        &self,
        base: &str,
        head: &str,
        file: &ChangedFile,
        options: DiffOptions,
    ) -> Result<FileDiff> {
        self.diff(base, head, file, options, Objects::LocalOnly).await
    }

    async fn diff(
        &self,
        base: &str,
        head: &str,
        file: &ChangedFile,
        options: DiffOptions,
        objects: Objects,
    ) -> Result<FileDiff> {
        let context_arg = format!("-U{}", options.context);
        // A path is a name, not a pattern: `a[b].txt` must not match `ab.txt` as well.
        let mut args: Vec<&str> = vec![
            "--literal-pathspecs",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "-M",
            context_arg.as_str(),
        ];
        if options.ignore_whitespace {
            // Line endings, the final newline and all other whitespace, as GitHub's "Hide whitespace".
            args.push("--ignore-all-space");
        }
        args.extend([base, head, "--"]);
        if let Some(old_path) = file.old_path.as_deref() {
            args.push(old_path);
        }
        args.push(file.path.as_str());
        let output = command::stdout_of(self.git().command(args, objects)).await?;
        Ok(FileDiff::parse_unified(&output))
    }

    /// Raw blob bytes by object id (from [`ChangedFile::old_oid`] / [`ChangedFile::new_oid`]).
    pub async fn blob(&self, oid: &str) -> Result<Vec<u8>> {
        self.git().run(["cat-file", "blob", oid]).await
    }

    /// The blob of `path` (repository-relative, `/`-separated) in `commit`, from the commit's trees
    /// alone: no blob is downloaded. `None` when the commit has no file at that path. No size: a
    /// blob not downloaded yet has none, and `ls-tree -l` stops at it in git 2.39 (Debian 12)
    /// with "could not fetch … from promisor remote".
    pub async fn file_entry(&self, commit: &str, path: &str) -> Result<Option<FileEntry>> {
        let is_relative = !path.is_empty()
            && !path.starts_with('/')
            && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..");
        if !is_relative {
            return Err(ProviderError::Local(format!(
                "'{path}' is not a path inside the repository"
            )));
        }
        let output = self.git().run_offline(["ls-tree", "-z", commit, "--", path]).await?;
        // `<mode> SP <type> SP <oid> TAB <path> NUL`, one entry per match.
        let entry = output.split(|byte| *byte == 0).find_map(|line| {
            let line = std::str::from_utf8(line).ok()?;
            let (meta, found) = line.split_once('\t')?;
            let mut fields = meta.split_whitespace();
            let (_mode, kind, oid) = (fields.next()?, fields.next()?, fields.next()?);
            (found == path && kind == "blob").then(|| FileEntry { oid: oid.to_owned() })
        });
        Ok(entry)
    }

    /// Commits reachable from `head` but not from `base`, newest first.
    pub async fn commits(&self, base: &str, head: &str) -> Result<Vec<CommitInfo>> {
        let range = format!("{base}..{head}");
        let output =
            self.git().run(["log", "--no-color", commits::LOG_FORMAT, range.as_str()]).await?;
        Ok(commits::parse(&output))
    }

    /// A detached working tree of `commit` at `dest`, for the pull request assistant to read; its
    /// missing blobs download in one batch, through the mirror's authorization. Returns `dest`.
    ///
    /// An existing tree at `dest` moves to `commit` in place: a running agent holds `dest` as its
    /// working directory, so the folder is never removed. A registration whose folder is gone is
    /// pruned before a new tree is added. A tree whose own git dir is gone (the mirror was deleted
    /// and made anew) is emptied, folder kept, and added again.
    pub async fn checkout_detached(&self, commit: &str, dest: &Path) -> Result<PathBuf> {
        let gitfile = dest.join(".git");
        let is_registered = gitfile.is_file()
            && tree_git_dir(&gitfile).await.is_some_and(|git_dir| git_dir.join("HEAD").is_file());
        if gitfile.is_file() && !is_registered {
            // `git worktree add` takes an empty folder only.
            empty_folder(dest).await?;
        }
        if is_registered {
            // The tree's own git dir (the `.git` file points into the mirror's `worktrees/`), so
            // the checkout moves this tree's HEAD and not the bare mirror's.
            GitCommand::new(&gitfile, self.authorization.as_deref())
                .run([
                    OsStr::new("--work-tree"),
                    dest.as_os_str(),
                    OsStr::new("checkout"),
                    OsStr::new("--quiet"),
                    OsStr::new("--detach"),
                    OsStr::new("--force"),
                    OsStr::new(commit),
                ])
                .await?;
            return Ok(dest.to_path_buf());
        }
        self.git().run_offline(["worktree", "prune"]).await?;
        self.git()
            .run([
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("--quiet"),
                OsStr::new("--detach"),
                dest.as_os_str(),
                OsStr::new(commit),
            ])
            .await?;
        Ok(dest.to_path_buf())
    }
}

/// The git dir a working tree's `.git` file (`gitdir: <path>`) points to.
async fn tree_git_dir(gitfile: &Path) -> Option<PathBuf> {
    let link = tokio::fs::read_to_string(gitfile).await.ok()?;
    let git_dir = Path::new(link.trim().strip_prefix("gitdir:")?.trim());
    let tree = gitfile.parent()?;
    Some(if git_dir.is_absolute() { git_dir.to_path_buf() } else { tree.join(git_dir) })
}

/// Removes everything inside `folder` and keeps the folder itself.
async fn empty_folder(folder: &Path) -> Result<()> {
    let mut entries = tokio::fs::read_dir(folder).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_dir() {
            tokio::fs::remove_dir_all(entry.path()).await?;
        } else {
            tokio::fs::remove_file(entry.path()).await?;
        }
    }
    Ok(())
}

/// One file of a commit's tree ([`GitMirror::file_entry`]); [`GitMirror::blob`] reads its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub oid: String,
}

/// How [`GitMirror::file_diff`] compares a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiffOptions {
    /// Unchanged lines around each change; [`FULL_FILE_CONTEXT`] for the whole file.
    pub context: u32,
    /// Lines that differ only in whitespace (line endings included) count as unchanged.
    pub ignore_whitespace: bool,
}

impl DiffOptions {
    pub fn context(context: u32) -> Self {
        Self { context, ignore_whitespace: false }
    }
}

fn local_refspec(remote_ref: &str) -> String {
    format!("+{remote_ref}:{}", local_ref_name(remote_ref))
}

/// Where a fetched remote ref lives in the mirror: `refs/pull/12/head` → `refs/review/pull/12/head`.
pub fn local_ref_name(remote_ref: &str) -> String {
    let local_name = remote_ref.trim_start_matches("refs/");
    format!("{LOCAL_REF_PREFIX}{local_name}")
}

/// The value of `name` (`section.key` or `section.subsection.key`) in a git config file the way
/// git writes it. `None` when it is missing, or quoted or escaped, which this reader does not
/// follow: the caller then writes the value again, which is always safe.
fn config_value<'a>(config: &'a str, name: &str) -> Option<&'a str> {
    let (section, key) = name.rsplit_once('.')?;
    let header = match section.split_once('.') {
        Some((section, subsection)) => format!("[{section} \"{subsection}\"]"),
        None => format!("[{section}]"),
    };
    let mut is_in_section = false;
    let mut value = None;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            is_in_section = line == header;
        } else if is_in_section
            && let Some((found, rest)) = line.split_once('=')
            && found.trim().eq_ignore_ascii_case(key)
        {
            // As in git, the last of several values wins.
            value = Some(rest.trim());
        }
    }
    value.filter(|value| !value.contains(['"', '\\']))
}

/// The folder of `clone_url`'s mirror under `cache_root`: [`mirror_dir_name`], unless that folder
/// holds the mirror of another repository whose URL sanitizes alike (`a/b_c` and `a_b/c`); then
/// the name ends in a hash of the repository. Mirrors made before keep their folder.
fn mirror_dir(cache_root: &Path, clone_url: &str) -> PathBuf {
    let name = mirror_dir_name(clone_url);
    let shared = cache_root.join(&name);
    let config = std::fs::read_to_string(shared.join("config")).unwrap_or_default();
    let repository = repository_of(clone_url);
    let is_taken = config_value(&config, "remote.origin.url")
        .is_some_and(|url| repository_of(url) != repository);
    if !is_taken {
        return shared;
    }
    // FNV-1a 64: stable across builds, so the folder is found again after a restart.
    let hash = repository.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    let stem = name.strip_suffix(".git").unwrap_or(&name);
    cache_root.join(format!("{stem}-{hash:016x}.git"))
}

/// The part of a clone URL a mirror's folder is named after: without scheme, user, trailing slash
/// and `.git`. Byte-stable on purpose, since it names existing folders; comparing a user's remotes
/// is `crate::checkout::remotes::repository_id`.
fn repository_of(clone_url: &str) -> &str {
    let without_scheme = clone_url.split_once("://").map_or(clone_url, |(_, rest)| rest);
    let without_user = without_scheme.split_once('@').map_or(without_scheme, |(_, rest)| rest);
    without_user.trim_end_matches('/').trim_end_matches(".git")
}

/// `https://dev.azure.com/org/project/_git/repo` → `dev.azure.com_org_project__git_repo.git`.
fn mirror_dir_name(clone_url: &str) -> String {
    let sanitized: String = repository_of(clone_url)
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' => character,
            _ => '_',
        })
        .collect();
    format!("{sanitized}.git")
}

#[cfg(test)]
#[path = "mirror_tests.rs"]
mod tests;
