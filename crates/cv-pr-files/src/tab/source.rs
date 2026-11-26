//! Where the folder summary's diffs come from: the file view's cache first, then the mirror, with
//! the same options and colors as the file view, so either finds the other's diffs. The versions
//! of its rendered Markdown cards load from the mirror as the file view's preview loads them.

use std::sync::Arc;

use review_core::ProviderKind;
use review_git::{ChangedFile, DiffOptions, FileDiff, GitMirror};
use review_store::AppSettings;

use cv_code::compaction;
use cv_pr_diff::diff_cache::{DiffCache, DiffKey};
use cv_pr_diff::patch_colors::PatchColors;
use cv_pr_rendered::preview::{self, Loaded};
use cv_pr_summary::DiffSource;
use cv_runtime::background::Background;
use cv_runtime::load::Load;
use cv_syntax::highlight::LineSpans;
use cv_syntax::pool::Priority;

/// The summary loads its diffs as the file view does when the context is small, so either finds
/// the other's in the cache; with a large context it loads the few lines around changes it shows.
pub fn summary_diff_options(settings: &AppSettings) -> DiffOptions {
    let options = super::choices::diff_options(settings);
    let context = if options.context <= 10 { options.context } else { compaction::SUMMARY_CONTEXT };
    DiffOptions { context, ..options }
}

/// `path` lies below `folder` (every path lies below the root, `""`).
pub fn is_below(path: &str, folder: &str) -> bool {
    folder.is_empty() || path.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/'))
}

/// The summary's diffs: the file view's cache first, then the mirror, as the file view loads them,
/// with their colors by diff line.
pub struct MirrorSource<'a> {
    pub cache: &'a mut DiffCache,
    pub patch_colors: &'a mut PatchColors,
    pub ctx: egui::Context,
    pub git: Option<Arc<GitMirror>>,
    pub background: &'a Background,
    pub commits: Option<(String, String)>,
    pub options: DiffOptions,
    pub platform: ProviderKind,
}

impl MirrorSource<'_> {
    fn key(&self, file: &ChangedFile) -> DiffKey {
        DiffKey::of(file, self.options)
    }
}

impl DiffSource for MirrorSource<'_> {
    fn cached(&mut self, file: &ChangedFile) -> Option<Arc<FileDiff>> {
        self.cache.get(&self.key(file))
    }

    fn start(&mut self, file: &ChangedFile) -> Option<Load<Arc<FileDiff>>> {
        let mirror_git = self.git.clone()?;
        let (base, head) = self.commits.clone()?;
        let key = self.key(file);
        let (file, options) = (file.clone(), self.options);
        let colorer = self.patch_colors.colorer(&self.ctx);
        let mut load = Load::Idle;
        load.start(self.background, async move {
            let diff = Arc::new(mirror_git.file_diff(&base, &head, &file, options).await?);
            colorer.color(key, diff.clone(), Priority::Queued).await;
            Ok(diff)
        });
        Some(load)
    }

    fn spans(&mut self, file: &ChangedFile) -> Option<Arc<LineSpans>> {
        self.patch_colors.poll();
        self.patch_colors.finished(&self.key(file))
    }

    fn store(&mut self, file: &ChangedFile, diff: Arc<FileDiff>) {
        self.cache.insert(self.key(file), diff);
    }

    fn start_rendered(&mut self, file: &ChangedFile) -> Option<Load<Arc<Loaded>>> {
        let git = self.git.clone()?;
        let commits = Some(self.commits.clone()?);
        Some(preview::load_markdown(&self.ctx, git, commits, self.platform, self.background, file))
    }
}

#[cfg(test)]
mod tests {
    use super::is_below;

    #[test]
    fn a_folder_holds_only_the_paths_below_it() {
        assert!(is_below("src/app/cart.rs", ""), "the root holds every path");
        assert!(is_below("src/app/cart.rs", "src/app"));
        assert!(is_below("src/app/cart.rs", "src"));
        assert!(!is_below("src/application.rs", "src/app"), "a name that only starts the same");
        assert!(!is_below("src/app", "src/app"), "the folder itself is no file below it");
    }
}
