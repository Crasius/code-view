//! The Files tab's diffs: the open file's diff (and its previous version while a newer one loads
//! after a live update), the cache and the diffs loaded ahead of the user, their syntax colors
//! by diff line and the file versions that color the whole file. The folder summary loads through
//! the same cache and colors ([`FileDiffs::summary_source`]), so either finds the other's diffs.
//! The threads a file's diff shows come from here too ([`threads_on`]).

use std::sync::Arc;

use review_core::ReviewThread;
use review_git::{ChangedFile, DiffOptions, FileDiff};

use super::super::prefetch::{self, Compare, Prefetch};
use super::choices::diff_options;
use super::notices;
use super::source::MirrorSource;
use cv_code::contents::{ContentsLoader, FileContents};
use cv_code::diff_rows::Layout;
use cv_pr_diff::diff_cache::{DiffCache, DiffKey};
use cv_pr_diff::diff_skeleton;
use cv_pr_diff::patch_colors::PatchColors;
use cv_pr_diff::warm_up;
use cv_pr_rendered::preview::Preview;
use cv_pr_session::session::PullRequestSession;
use cv_pr_session::web_link::WebLink;
use cv_pr_tree::FileTree;
use cv_runtime::background::Background;
use cv_runtime::load::Load;
use cv_services::Services;
use cv_syntax::highlight::LineSpans;
use cv_syntax::pool::Priority;
use cv_widgets::skeleton;

#[derive(Default)]
pub(crate) struct FileDiffs {
    diff: Load<Arc<FileDiff>>,
    diff_key: Option<DiffKey>,
    /// When `diff` started loading, on egui's clock: its placeholder waits a moment.
    diff_started: f64,
    cache: DiffCache,
    prefetch: Prefetch,
    /// Syntax colors by diff line for the open diff and the ones worked out ahead, there before
    /// the file versions are.
    patch_colors: PatchColors,
    /// The changed files whose grammars were warmed up, by their list.
    warmed_files: Option<Arc<Vec<ChangedFile>>>,
    contents: ContentsLoader,
    /// Blob of the file the user asked to color although it is above the automatic size limit.
    forced_colors: Option<String>,
    /// The open file's previous diff, shown while its newer version loads (a live update).
    stale_diff: Option<(String, Arc<FileDiff>)>,
    /// The page was out of sight ([`Self::hibernate`]): the open file's diff loads again, into
    /// the same view.
    is_hibernating: bool,
}

impl FileDiffs {
    /// The page went out of sight: the diffs, their colors and the file versions go, each loads
    /// again from the mirror when it shows. The open file stays named, so its diff loads into the
    /// view the user left (see [`Self::ensure`]), and so does a file the user colored anyway.
    pub(crate) fn hibernate(&mut self) {
        self.patch_colors.release_all();
        *self = Self {
            diff_key: self.diff_key.take(),
            forced_colors: self.forced_colors.take(),
            is_hibernating: true,
            ..Self::default()
        };
    }

    /// Takes in the colors worked out since the last frame, on every tab.
    pub(crate) fn poll_colors(&mut self) {
        self.patch_colors.poll();
    }

    /// Takes in what finished loading since the last frame.
    pub(crate) fn poll(&mut self) {
        if self.diff.poll()
            && let (Some(key), Load::Ready(diff)) = (&self.diff_key, &self.diff)
        {
            self.cache.insert(key.clone(), diff.clone());
        }
        // After the diff: a diff's load waits for its first colors, so both show in one frame.
        self.patch_colors.poll();
        self.contents.poll();
    }

    /// Whether a diff of `path` is cached (shown before or loaded ahead).
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn has_cached_diff(&self, path: &str) -> bool {
        self.cache.has_path(path)
    }

    /// The open file's versions are loaded and colored.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn coloring_is_done(&self) -> bool {
        self.contents
            .current()
            .is_some_and(|contents| contents.coloring == cv_code::contents::Coloring::Done)
    }

    /// The languages of the pull request's changed files whose compiled grammars the process keeps.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn held_languages(&self) -> Vec<String> {
        let used: Vec<String> = self
            .warmed_files
            .iter()
            .flat_map(|files| files.iter())
            .filter_map(|file| cv_syntax::grammar::for_path(&file.path, ""))
            .map(|grammar| grammar.syntax.name.clone())
            .collect();
        let mut kept = cv_syntax::grammar::kept_languages();
        kept.retain(|language| used.contains(language));
        kept
    }

    /// The open file's diff, once it loaded.
    pub(crate) fn ready(&self) -> Option<&Arc<FileDiff>> {
        self.diff.ready()
    }

    /// The grammars of `files` were compiled already.
    pub(crate) fn is_warmed(&self, files: &Arc<Vec<ChangedFile>>) -> bool {
        self.warmed_files.as_ref().is_some_and(|warmed| Arc::ptr_eq(warmed, files))
    }

    /// Compiles the grammars of all languages in `files` in the background, `first`'s first.
    pub(crate) fn warm_up(&mut self, files: &Arc<Vec<ChangedFile>>, first: Option<String>) {
        self.warmed_files = Some(files.clone());
        self.patch_colors.warm_up(files, first);
    }

    /// Starts loading `file`'s diff unless it is shown or loading already: from the cache at once,
    /// else from the mirror. The diff is the file's versions' ([`DiffKey`]), so an update that
    /// leaves them as they are keeps it. Returns true when another file's diff started, so its
    /// view starts anew; a newer diff of the same file keeps the older one on screen meanwhile.
    pub(crate) fn ensure(
        &mut self,
        ctx: &egui::Context,
        session: &PullRequestSession,
        services: &Services,
        file: &ChangedFile,
    ) -> bool {
        self.patch_colors.use_cold(&services.cold);
        let key = DiffKey::of(file, diff_options(&services.settings));
        if self.diff_key.as_ref() == Some(&key) && !self.is_hibernating {
            return false;
        }
        let Some((base, head)) = session.compared_commits() else {
            return false;
        };
        self.is_hibernating = false;
        let is_same_file = self.diff_key.as_ref().is_some_and(|previous| previous.path == key.path);
        if !is_same_file {
            self.stale_diff = None;
        } else if let Load::Ready(shown) = &self.diff {
            self.stale_diff = Some((key.path.clone(), shown.clone()));
        }
        self.diff_key = Some(key.clone());
        // A diff loaded ahead that finished since the last frame is there too.
        self.prefetch.collect(&mut self.cache);
        if let Some(cached) = self.cache.get(&key) {
            self.patch_colors.ensure(ctx, &key, &cached);
            self.diff = Load::Ready(cached);
            return !is_same_file;
        }
        self.diff_started = skeleton::now(ctx);
        if let Some(loading_ahead) = self.prefetch.take_running(&key) {
            self.diff = loading_ahead;
            return !is_same_file;
        }
        let Some(mirror) = session.mirror.ready() else {
            return !is_same_file;
        };
        let colorer = self.patch_colors.colorer(ctx);
        // The grammar compiles while the mirror works out the diff.
        warm_up::path_now(file.path.clone());
        let mirror_git = mirror.git.clone();
        let file = file.clone();
        let options = key.options;
        self.diff.start(&services.background, async move {
            let diff = Arc::new(mirror_git.file_diff(&base, &head, &file, options).await?);
            colorer.color(key, diff.clone(), Priority::Urgent).await;
            Ok(diff)
        });
        !is_same_file
    }

    /// The next [`Self::ensure`] loads the open file's diff again.
    pub(crate) fn reload(&mut self) {
        self.diff_key = None;
    }

    /// The diff to show for `path`: the loaded one, or its previous version while a newer one
    /// loads. Shows the failure or the skeleton in its place otherwise.
    pub(crate) fn diff_to_show(
        &mut self,
        ui: &mut egui::Ui,
        path: &str,
        layout: Layout,
        font_size: f32,
        web_link: Option<&WebLink>,
    ) -> Option<Arc<FileDiff>> {
        let stale = self
            .stale_diff
            .as_ref()
            .filter(|(stale_path, _)| stale_path == path)
            .map(|(_, diff)| diff.clone());
        match (&self.diff, stale) {
            (Load::Ready(diff), _) => Some(diff.clone()),
            // The newer version of this file is on its way; the older one stays meanwhile.
            (Load::Loading(_), Some(stale)) => Some(stale),
            (Load::Failed(message), _) => {
                if notices::diff_failure(ui, message, web_link) {
                    self.reload();
                }
                None
            }
            (Load::Idle | Load::Loading(_), _) => {
                let shown = skeleton::reveal(ui.ctx(), self.diff_started);
                diff_skeleton::show(ui, layout, font_size, shown);
                None
            }
        }
    }

    /// Loads `file`'s versions from the mirror for coloring the whole file, unless they are
    /// there; above the size limit only when the user asked for this file's colors.
    pub(crate) fn ensure_contents(
        &mut self,
        file: &ChangedFile,
        session: &PullRequestSession,
        services: &Services,
    ) {
        if let Some(mirror) = session.mirror.ready() {
            let force = self.forced_colors.as_deref() == Some(file.new_oid.as_str());
            self.contents.ensure(file, &mirror.git, services, force);
        }
    }

    /// "Color anyway" on a file above the automatic size limit.
    pub(crate) fn force_colors(&mut self, file: &ChangedFile) {
        self.forced_colors = Some(file.new_oid.clone());
    }

    /// The file versions failed to load: the next [`Self::ensure_contents`] tries again.
    pub(crate) fn retry_contents(&mut self) {
        self.contents.retry();
    }

    /// The colors for `diff` on screen: the file versions, and the colors by diff line of the
    /// diff loaded for the key (not of a stale one shown meanwhile).
    pub(crate) fn coloring(
        &mut self,
        diff: &Arc<FileDiff>,
    ) -> (Option<&FileContents>, Option<&LineSpans>) {
        let shows_loaded = matches!(&self.diff, Load::Ready(loaded) if Arc::ptr_eq(loaded, diff));
        let line_spans = match &self.diff_key {
            Some(key) if shows_loaded => self.patch_colors.spans(key),
            _ => None,
        };
        (self.contents.current(), line_spans)
    }

    /// Loads the diffs the user is likely to open next ahead, from the local mirror only: the
    /// file whose tree row the pointer rests on, and the files around the open file `open` (an
    /// index into `files`) in the tree's order. Files that show as a preview are left out.
    pub(crate) fn load_ahead(
        &mut self,
        session: &PullRequestSession,
        services: &Services,
        files: &[ChangedFile],
        open: Option<usize>,
        tree: &FileTree,
        preview: &Preview,
    ) {
        let ctx = services.background.ctx();
        let rested = self.prefetch.rested_on(&ctx, tree.hovered_file());
        let replanned = if let Some((base, head)) = session.compared_commits() {
            let compare = Compare { base, head, options: diff_options(&services.settings) };
            // A file outside the filtered tree has no neighbours to work ahead on.
            let position =
                open.and_then(|index| tree.file_order().position(|position| position == index));
            // Only the files within reach, so planning every frame stays cheap.
            let start = position.map_or(0, |position| position.saturating_sub(prefetch::SCAN));
            let window: Vec<&ChangedFile> = match position {
                Some(_) => tree
                    .file_order()
                    .skip(start)
                    .take(prefetch::SCAN * 2 + 1)
                    .filter_map(|index| files.get(index))
                    .collect(),
                None => Vec::new(),
            };
            let rested = rested.and_then(|index| files.get(index));
            let open = position.map(|position| position - start);
            self.prefetch.plan(&compare, &window, open, rested, |file| !preview.is_active(file))
        } else {
            self.prefetch.clear();
            true
        };
        let git = session.mirror.ready().map(|mirror| &mirror.git);
        let colorer = self.patch_colors.colorer(&ctx);
        let finished = self.prefetch.step(
            &mut self.cache,
            git,
            &services.background,
            &colorer,
            self.diff.is_loading(),
        );
        // What the user moved away from goes, with its colors.
        if replanned || finished {
            let prefetch = &self.prefetch;
            for key in self.cache.release_unopened(|key| prefetch.is_planned(key)) {
                self.patch_colors.release(&key);
            }
        }
    }

    /// Where the folder summary loads its diffs: this cache and these colors, then the mirror.
    pub(crate) fn summary_source<'a>(
        &'a mut self,
        ctx: &egui::Context,
        session: &PullRequestSession,
        background: &'a Background,
        options: DiffOptions,
    ) -> MirrorSource<'a> {
        MirrorSource {
            cache: &mut self.cache,
            patch_colors: &mut self.patch_colors,
            ctx: ctx.clone(),
            git: session.mirror.ready().map(|mirror| mirror.git.clone()),
            background,
            commits: session.compared_commits(),
            options,
            platform: session.kind,
        }
    }
}

/// The threads on `path`'s lines.
pub(crate) fn threads_on<'a>(session: &'a PullRequestSession, path: &str) -> Vec<&'a ReviewThread> {
    session
        .threads
        .ready()
        .into_iter()
        .flatten()
        .filter(|thread| thread.anchor.as_ref().is_some_and(|anchor| anchor.path == path))
        .collect()
}
