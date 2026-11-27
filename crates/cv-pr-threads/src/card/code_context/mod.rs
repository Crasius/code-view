//! The code a thread is about, at the top of its card away from the diff (the overview): the
//! commented lines highlighted with a few lines around them, in New only, as a unified Diff or
//! side by side (Split), and more lines on demand.
//!
//! The code comes from the pull request's mirror in the background, never on the UI thread: a card
//! that scrolls into view asks for its file and the next [`CodeContexts::begin_frame`] starts the
//! load. Files are kept per commits and path, so the threads of one file share one load. A current
//! thread reads the pull request's whole change (merge base to head); an outdated one the commits
//! it was written on, fetched by id like "Show original version" does.
//!
//! While the lines are counted, the threads' files are wanted by the count
//! ([`review_git::GitMirror::want`]), so they come in its first chunk. A current thread's excerpt
//! loads its file as soon as the mirror is ready, and its diff waits in the mirror until the count
//! brought the file's blobs, however long the chunk takes
//! ([`review_git::GitMirror::file_diff_after_count`]): no blob downloads twice. Unlike a diff in
//! the Files tab it is not bounded by [`review_git::COUNT_WAIT`]: the reader is on the overview,
//! not waiting for this code, and a 3,000-file pull request's first chunk takes about that long.
//! So the wait has no frame-clock deadline.

mod code_file;
mod rows;
mod view;
#[cfg(test)]
mod walkthrough;
mod window;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cv_pr_session::session::PullRequestSession;
use cv_runtime::load::Load;
use cv_services::Services;
use review_core::{ProviderError, ReviewThread, ThreadAnchor};
use review_git::{ChangeKind, ChangedFile, DiffOptions, FileDiff, GitMirror, WantedBy};
use review_store::AppSettings;

use self::code_file::CodeFile;
pub(crate) use self::view::show;
use self::window::{Extra, Known, Mode, Shape};

/// The commits and file one excerpt reads.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FileKey {
    /// `None` where the platform keeps no base for an outdated thread (GitHub): the merge base of
    /// the target and `head` stands in.
    base: Option<String>,
    head: String,
    path: String,
    old_path: Option<String>,
    /// The commits may be gone from the mirror's refs (a force push): fetch them by id first.
    is_origin: bool,
}

/// What a card's excerpt keeps between frames.
#[derive(Default)]
struct CardState {
    /// The mode the user picked on this card; `None` follows the last one picked anywhere.
    mode: Option<Mode>,
    extra: Extra,
    /// When the card first waited for its code, for the skeleton's grace period.
    waiting_since: Option<f64>,
    /// What the code took last time it was shown, for the skeleton when it loads again (a push,
    /// a refresh).
    last: Option<LastShape>,
}

/// The shape of a card's code with the mode and the lines asked for when it was shown.
#[derive(Clone, Copy)]
struct LastShape {
    preferred: Mode,
    extra: Extra,
    shape: Shape,
}

/// The mirror's commits for current threads.
struct Mirror {
    merge_base: String,
    head: String,
    /// The pull request's target commit, for the merge base of an outdated thread without a base.
    target: Option<String>,
}

/// Every excerpt of one pull request: the files loaded for them and each card's mode and size.
pub(crate) struct CodeContexts {
    font_size: f32,
    now: f64,
    mirror: Option<Mirror>,
    /// Why the mirror could not be prepared; the excerpts say so and offer Retry.
    mirror_error: Option<String>,
    retry_mirror: bool,
    files: HashMap<FileKey, Load<Arc<CodeFile>>>,
    /// Files cards on screen asked for, started by [`Self::begin_frame`] once they can be.
    wanted: HashSet<FileKey>,
    cards: HashMap<String, CardState>,
    /// The changed files shown now, for the file of a hovered thread row.
    changed: Option<Arc<Vec<ChangedFile>>>,
    /// The threads' files the line count downloads first, of the threads' version.
    thread_files: (u64, Vec<String>),
}

impl Default for CodeContexts {
    fn default() -> Self {
        Self {
            font_size: AppSettings::DEFAULT_CODE_FONT_SIZE,
            now: 0.0,
            mirror: None,
            mirror_error: None,
            retry_mirror: false,
            files: HashMap::new(),
            wanted: HashSet::new(),
            cards: HashMap::new(),
            changed: None,
            thread_files: (0, Vec::new()),
        }
    }
}

/// The app-wide mode the cards follow until one is switched: the last one picked, for the session.
fn last_mode_id() -> egui::Id {
    egui::Id::unique("thread_code_mode")
}

impl CodeContexts {
    /// Takes the mirror and the code font from this frame, applies finished loads and starts the
    /// ones cards asked for.
    pub(crate) fn begin_frame(
        &mut self,
        ctx: &egui::Context,
        session: &mut PullRequestSession,
        services: &Services,
    ) {
        self.now = ctx.input(|input| input.time);
        self.font_size = services.settings.code_font_size;
        let target = session.detail.ready().map(|detail| detail.target_sha.clone());
        self.mirror = session.mirror.ready().map(|mirror| Mirror {
            merge_base: mirror.merge_base.clone(),
            head: mirror.head.clone(),
            target,
        });
        self.changed = session.files().cloned();
        self.want_thread_files(session);
        self.mirror_error = session.mirror.error().map(str::to_owned);
        if std::mem::take(&mut self.retry_mirror) && self.mirror_error.is_some() {
            session.refresh(services);
        }
        let Some(mirror) = &self.mirror else {
            return;
        };
        // A push moved the head: the files of the commits before it are not shown any more.
        self.files.retain(|key, _| key.is_origin || key.head == mirror.head);
        for file in self.files.values_mut() {
            file.poll();
        }
        let Some(git) = session.mirror.ready().map(|ready| &ready.git) else {
            return;
        };
        for key in self.wanted.drain() {
            let file = self.files.entry(key.clone()).or_default();
            if file.is_idle() {
                let (git, target, ctx) = (git.clone(), mirror.target.clone(), ctx.clone());
                file.start(
                    &services.background,
                    async move { load(&git, target, &key, &ctx).await },
                );
            }
        }
    }

    /// Feeds the files the threads' excerpts read from the pull request's change (not an outdated
    /// thread's own commits) to the line count, to download first.
    fn want_thread_files(&mut self, session: &PullRequestSession) {
        let Some(git) = session.mirror.ready().map(|mirror| &mirror.git) else {
            return;
        };
        let version = session.threads.version();
        if self.thread_files.0 != version {
            let threads = session.threads.ready().map_or(&[][..], Vec::as_slice);
            let current =
                threads.iter().filter(|thread| !(thread.is_outdated && thread.origin.is_some()));
            let anchors = current.filter_map(|thread| thread.anchor.as_ref());
            let mut paths: Vec<String> = Vec::new();
            for path in
                anchors.flat_map(|anchor| std::iter::once(&anchor.path).chain(&anchor.old_path))
            {
                if !paths.contains(path) {
                    paths.push(path.clone());
                }
            }
            self.thread_files = (version, paths);
        }
        git.want(WantedBy::Threads, &self.thread_files.1);
    }

    /// The changed file at `path`, when the pull request's change has it.
    pub(crate) fn changed_file(&self, path: &str) -> Option<&ChangedFile> {
        self.changed.as_ref()?.iter().find(|file| file.path == path)
    }

    /// The file and the lines `thread`'s excerpt reads: the commits it was written on when it is
    /// outdated and the platform says which, else the pull request's change. `None` while the
    /// mirror is not there yet, and for threads without lines.
    fn source<'a>(&self, thread: &'a ReviewThread) -> Option<(FileKey, &'a ThreadAnchor)> {
        let anchor = thread.anchor.as_ref()?;
        if thread.is_outdated
            && let Some(origin) = &thread.origin
        {
            let key = FileKey {
                base: origin.base_sha.clone(),
                head: origin.head_sha.clone(),
                path: origin.anchor.path.clone(),
                old_path: origin.anchor.old_path.clone(),
                is_origin: true,
            };
            return Some((key, &origin.anchor));
        }
        let mirror = self.mirror.as_ref()?;
        let key = FileKey {
            base: Some(mirror.merge_base.clone()),
            head: mirror.head.clone(),
            path: anchor.path.clone(),
            old_path: anchor.old_path.clone(),
            is_origin: false,
        };
        Some((key, anchor))
    }

    fn card(&mut self, thread_id: &str) -> &mut CardState {
        self.cards.entry(thread_id.to_owned()).or_default()
    }

    /// What the list of changed files says about the file at `path` before it is read: an added
    /// file has its added lines, a deleted one its removed lines, all changed.
    fn known(&self, path: &str) -> Known {
        let Some(file) = self.changed_file(path) else {
            return Known::default();
        };
        let len = match file.kind {
            ChangeKind::Added => file.additions,
            ChangeKind::Deleted => file.deletions,
            _ => return Known::default(),
        };
        Known { len: len.map(|len| len as usize), is_all_changed: true }
    }

    /// The shape the code of `thread_id` had when it was last shown with `preferred` and `extra`.
    fn last_shape(&self, thread_id: &str, preferred: Mode, extra: Extra) -> Option<Shape> {
        let last = self.cards.get(thread_id)?.last?;
        (last.preferred == preferred && last.extra == extra).then_some(last.shape)
    }

    /// The mode the card of `thread_id` asks for: its own pick, else the last one picked anywhere.
    fn preferred(&self, ctx: &egui::Context, thread_id: &str) -> Mode {
        let picked = self.cards.get(thread_id).and_then(|card| card.mode);
        picked.unwrap_or_else(|| {
            ctx.data(|data| data.get_temp(last_mode_id())).unwrap_or(Mode::NewOnly)
        })
    }

    /// The mode the card of `thread_id` shows: [`Self::preferred`], but Diff where the card's own
    /// pick is not set and New only would leave the commented lines out.
    fn mode(&self, ctx: &egui::Context, thread_id: &str, is_on_removed_lines: bool) -> Mode {
        let preferred = self.preferred(ctx, thread_id);
        let is_picked = self.cards.get(thread_id).is_some_and(|card| card.mode.is_some());
        if !is_picked && is_on_removed_lines && preferred == Mode::NewOnly {
            Mode::Diff
        } else {
            preferred
        }
    }

    fn pick_mode(&mut self, ctx: &egui::Context, thread_id: &str, mode: Mode) {
        self.card(thread_id).mode = Some(mode);
        ctx.data_mut(|data| data.insert_temp(last_mode_id(), mode));
    }

    /// Loads the file of `key` again (Retry), or prepares the mirror again when that failed.
    fn retry(&mut self, key: Option<&FileKey>) {
        match key {
            Some(key) => {
                self.files.remove(key);
                self.wanted.insert(key.clone());
            }
            None => self.retry_mirror = true,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    /// The mirror at (`merge_base`, `head`), and `file` loaded for `thread`.
    pub(crate) fn insert_for_tests(
        &mut self,
        thread: &ReviewThread,
        merge_base: &str,
        head: &str,
        diff: &str,
        text: &str,
    ) {
        self.mirror =
            Some(Mirror { merge_base: merge_base.to_owned(), head: head.to_owned(), target: None });
        let (key, _) = self.source(thread).expect("a thread with lines");
        let file = CodeFile::build(&FileDiff::parse_unified(diff.as_bytes()), text.into());
        self.files.insert(key, Load::Ready(Arc::new(file)));
    }

    #[cfg(test)]
    /// The pull request's changed files, as [`Self::begin_frame`] takes them from the session.
    fn set_changed_for_tests(&mut self, changed: Vec<ChangedFile>) {
        self.changed = Some(Arc::new(changed));
    }

    #[cfg(test)]
    /// Drops every loaded file, as a push does with the files of the commits before it.
    fn forget_files_for_tests(&mut self) {
        self.files.clear();
    }
}

/// Brings an outdated thread's commits into the mirror (by id, only when missing), diffs its file
/// between the commits without context and reads the head version: the rows of [`CodeFile`].
async fn load(
    git: &GitMirror,
    target: Option<String>,
    key: &FileKey,
    ctx: &egui::Context,
) -> Result<Arc<CodeFile>, ProviderError> {
    let head = key.head.as_str();
    if key.is_origin {
        let commits: Vec<&str> = std::iter::once(head).chain(key.base.as_deref()).collect();
        git.ensure_commits_by_id(&commits, &|_| {}).await.map_err(|error| {
            ProviderError::Local(format!(
                "The version the comment was written on could not be fetched ({}).",
                error.to_string().trim_end_matches('.')
            ))
        })?;
    }
    let base = match (&key.base, target) {
        (Some(base), _) => base.clone(),
        (None, Some(target)) => git.merge_base(&target, head).await?,
        (None, None) => {
            return Err(ProviderError::Local("The pull request's target is not known yet.".into()));
        }
    };
    let changed =
        git.changed_files_fast(&base, head).await?.into_iter().find(|file| {
            file.path == key.path || key.old_path.as_deref() == Some(file.path.as_str())
        });
    let (diff, text) = if let Some(file) = changed {
        let diff = git.file_diff_after_count(&base, head, &file, DiffOptions::context(0)).await?;
        let text = if file.has_new_blob() { git.blob(&file.new_oid).await? } else { Vec::new() };
        (diff, text)
    } else {
        // Unchanged between the commits: the file as it is.
        let entry = git
            .file_entry(head, &key.path)
            .await?
            .ok_or_else(|| ProviderError::Local(format!("{} is not in this version.", key.path)))?;
        (FileDiff::default(), git.blob(&entry.oid).await?)
    };
    if diff.is_binary {
        return Err(ProviderError::Local(format!("{} is a binary file.", key.path)));
    }
    let file = Arc::new(CodeFile::build(&diff, text));
    file.color(&key.path, ctx);
    Ok(file)
}
