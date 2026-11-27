//! The folder summary in the Files tab while no file is open: the folder it shows, its cards in
//! the tree's order (taken again only when the tree or the folder changed) and what it is given
//! each frame. What its events do is the tab's.

use std::collections::HashMap;
use std::sync::Arc;

use review_core::{ProviderKind, ReviewThread};
use review_git::ChangedFile;

use super::super::Env;
use super::file_diffs::FileDiffs;
use super::source;
use cv_pr_summary::{Compare, FolderSummary, SummaryEvent, SummaryInputs};
use cv_pr_tree::{FileFilter, FileTree};

#[derive(Default)]
pub struct SummaryPane {
    summary: FolderSummary,
    /// The changes below this folder show (`""` for all of them).
    folder: String,
    /// The cards in tree order, for the tree version and folder they were taken for.
    order: Vec<usize>,
    order_key: Option<(u64, String)>,
    order_version: u64,
}

impl SummaryPane {
    pub fn folder(&self) -> &str {
        &self.folder
    }

    /// Shows the summary of `folder` (`""` for every changed file) once no file is open.
    pub fn set_folder(&mut self, folder: String) {
        self.folder = folder;
    }

    /// A file is open: the summary's queued coloring makes way.
    pub fn hide(&mut self) {
        self.summary.hide();
    }

    /// The summary of the folder, its diffs loaded through the file view's cache and colors.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        env: &mut Env<'_>,
        files: &Arc<Vec<ChangedFile>>,
        thread_counts: &HashMap<String, usize>,
        tree: &FileTree,
        diffs: &mut FileDiffs,
    ) -> Option<SummaryEvent> {
        self.refresh_order(tree, files);
        let session = &*env.session;
        let settings = &env.services.settings;
        let options = source::summary_diff_options(settings);
        let commits = session.compared_commits();
        let (base, head) =
            commits.as_ref().map_or(("", ""), |(base, head)| (base.as_str(), head.as_str()));
        let threads: Vec<&ReviewThread> =
            session.threads.ready().map(|threads| threads.iter().collect()).unwrap_or_default();
        let inputs = SummaryInputs {
            files,
            order: &self.order,
            order_version: self.order_version,
            folder: &self.folder,
            compare: Compare { base, head, options },
            font_size: settings.code_font_size,
            thread_counts,
            threads: &threads,
            can_comment: session.has_token,
            can_comment_outside_hunks: session.kind != ProviderKind::GitHub,
            can_load: commits.is_some(),
            is_filtered: tree.filter != FileFilter::default(),
            layout: settings.diff_layout,
            show_threads: settings.show_comments_in_diff,
            preview: settings.file_filters.summary_preview,
            platform: session.kind,
            rendered_width: settings.rendered_width,
        };
        let mut source = diffs.summary_source(ui.ctx(), session, &env.services.background, options);
        self.summary.show(
            ui,
            &inputs,
            &mut source,
            env.threads_ui,
            &mut env.services.settings.panes.diff_split,
        )
    }

    /// The visible files below the folder in the tree's order, taken again only when the tree or
    /// the folder changed.
    fn refresh_order(&mut self, tree: &FileTree, files: &[ChangedFile]) {
        let key = (tree.version(), self.folder.clone());
        if self.order_key.as_ref() == Some(&key) {
            return;
        }
        let folder = &self.folder;
        self.order = tree
            .full_order()
            .iter()
            .map(|&index| index as usize)
            .filter(|&index| cv_pr_tree::is_below(&files[index].path, folder))
            .collect();
        self.order_key = Some(key);
        self.order_version += 1;
    }
}
