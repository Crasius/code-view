//! The Files tab's file tree pane, at the window edge the user picked: the changed files as a tree
//! with category filters, search and viewed marks (`side_panel`), its rows (`rows`), the flattened
//! tree model (`model`) and the file-type icons (`file_icon`). [`FileTree`] keeps the pane's state
//! between frames and rebuilds its rows only when the files, the filter, the viewed marks, the open
//! file or a folder's state changed. "Hide viewed" keeps the open file listed (dimmed, marked
//! viewed) until another one opens, so the keys and buttons that step from it keep working. The
//! review progress line is in `progress`, Ctrl+P's go to file over the filter in `goto`.
#![forbid(unsafe_code)]
// RS-LINT-02: egui lays out in f32 points while rows and file indices are usize, u32 or u64.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "egui pixel math converts between f32 points and usize indices"
)]

mod category_filter;
pub mod file_icon;
pub mod goto;
mod model;
pub mod progress;
pub mod rows;
pub mod side_panel;
#[cfg(test)]
mod side_panel_shots;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use egui::Ui;
use review_core::{ChangeBreakdown, FileCategory};
use review_git::ChangedFile;
use review_store::TreeSide;

use self::goto::GoTo;
use self::model::TreeRow;
pub use self::progress::VoteOffer;
use self::rows::{CountColumns, ROW_HEIGHT, RowAction};
use cv_base::theme::icons;
use cv_widgets::change_breakdown;

pub enum TreeEvent {
    Select(usize),
    ToggleViewed(usize),
    /// Mark every changed file below the folder at this path viewed, or unmark them all when they
    /// all are.
    ToggleFolderViewed(String),
    /// Show the summary of the folder at this path.
    SelectFolder(String),
    /// Move the tree to the other window edge.
    FlipSide,
    /// A link of the review progress line.
    Review(ReviewStep),
}

impl TreeEvent {
    /// The event picked on `listed`, for `files`: a file index finds its file again by path, as
    /// a newer list may have replaced the one it was picked on (line counts streaming in, a live
    /// update). `None` when the file left the list.
    pub fn carried_to(self, listed: &[ChangedFile], files: &[ChangedFile]) -> Option<Self> {
        let find = |index: usize| {
            let path = &listed.get(index)?.path;
            if files.get(index).is_some_and(|file| file.path == *path) {
                return Some(index);
            }
            files.iter().position(|file| file.path == *path)
        };
        match self {
            Self::Select(index) => find(index).map(Self::Select),
            Self::ToggleViewed(index) => find(index).map(Self::ToggleViewed),
            event => Some(event),
        }
    }
}

/// Where the review goes next: what the progress line's links and the toasts about the review
/// ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewStep {
    /// The next file not viewed yet.
    NextUnviewed,
    /// Approve the pull request.
    Approve,
    /// The pull request's Overview, where the other votes are.
    Overview,
    /// Clear the tree's filters, which hide the files left to review.
    ClearFilters,
    /// Mark the open file viewed: it is the last one left.
    MarkViewed,
}

/// The icon and the tooltip of the control that moves the tree away from `side`: an arrow towards
/// the edge it goes to.
pub fn flip_control(side: TreeSide) -> (&'static str, &'static str) {
    match side {
        TreeSide::Left => (icons::ARROW_LINE_RIGHT, "Move the file tree to the right"),
        TreeSide::Right => (icons::ARROW_LINE_LEFT, "Move the file tree to the left"),
    }
}

/// How many of a folder's changed files are viewed, as its row's tri-state mark shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FolderMark {
    #[default]
    None,
    Some,
    All,
}

impl FolderMark {
    fn of(viewed: u32, total: u32) -> Self {
        match viewed {
            0 => Self::None,
            viewed if viewed >= total => Self::All,
            _ => Self::Some,
        }
    }
}

/// `path` lies in `folder` or a folder below it.
pub fn is_below(path: &str, folder: &str) -> bool {
    path.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/'))
}

/// (viewed, total) changed files below every folder, by folder path, over all changed files:
/// a folder's mark counts the files the filters hide too, as its toggle marks them.
fn folder_counts<'a>(
    files: &'a [ChangedFile],
    is_viewed: &dyn Fn(&ChangedFile) -> bool,
) -> HashMap<&'a str, (u32, u32)> {
    let mut counts: HashMap<&str, (u32, u32)> = HashMap::new();
    for file in files {
        let viewed = u32::from(is_viewed(file));
        for (end, _) in file.path.match_indices('/') {
            let Some(folder) = file.path.get(..end) else {
                continue;
            };
            let entry = counts.entry(folder).or_default();
            entry.0 += viewed;
            entry.1 += 1;
        }
    }
    counts
}

#[derive(Clone, PartialEq, Eq, Default)]
pub struct FileFilter {
    /// Empty means every category.
    pub categories: BTreeSet<FileCategory>,
    pub search: String,
    pub hide_viewed: bool,
}

impl FileFilter {
    /// The filter as it applies to `files`: a category none of them has counts as not picked, as
    /// the category dropdown shows it. The categories are saved for every pull request, so one
    /// picked on another pull request never hides every file here.
    fn for_files(&self, files: &[ChangedFile]) -> Self {
        let categories = self
            .categories
            .iter()
            .copied()
            .filter(|category| files.iter().any(|file| file.category == *category))
            .collect();
        Self { categories, ..self.clone() }
    }

    pub fn accepts(&self, file: &ChangedFile, is_viewed: bool) -> bool {
        let category_matches =
            self.categories.is_empty() || self.categories.contains(&file.category);
        let needle = self.search.trim();
        let search_matches = needle.is_empty() || contains_ignore_case(&file.path, needle);
        let viewed_matches = !(self.hide_viewed && is_viewed);
        category_matches && search_matches && viewed_matches
    }
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
}

/// What the tree shows besides its own state, gathered by the Files tab every frame.
pub struct TreeInputs<'a> {
    pub files: &'a Arc<Vec<ChangedFile>>,
    /// The open file, by index into `files`.
    pub selected: Option<usize>,
    /// The folder whose summary shows while no file is open.
    pub selected_folder: Option<&'a str>,
    pub is_viewed: &'a dyn Fn(&ChangedFile) -> bool,
    /// Open comment threads by path.
    pub thread_counts: &'a HashMap<String, usize>,
    /// What the progress line offers for the vote once every file is viewed.
    pub vote: VoteOffer,
}

#[derive(Default)]
pub struct FileTree {
    pub filter: FileFilter,
    collapsed: HashSet<String>,
    rows: Vec<TreeRow>,
    built_for: Option<(usize, u64)>,
    collapse_version: u64,
    built_filter: FileFilter,
    /// The open file's path, as the tree last showed it: "Hide viewed" keeps it listed.
    open: Option<String>,
    /// The open file the rows were built with, while "Hide viewed" was on.
    built_open: Option<String>,
    scroll_to_file: Option<usize>,
    /// Widest line counts of the change list, so every row shares one count column.
    widest_counts: Option<(u32, u32)>,
    /// Every changed file's lines by kind of file, behind the pane's line totals.
    breakdown: ChangeBreakdown,
    /// Every file the filter lets through in tree order, also those in collapsed folders.
    full_order: Vec<u32>,
    /// Bumped whenever the rows are rebuilt, so views built from them know when to follow.
    version: u64,
    /// The window edge the pane sits at this frame, which its flip control and its tooltips
    /// (towards the diff) follow; the Files tab places it from the setting before it shows.
    side: TreeSide,
    /// The file whose row the pointer was on when the tree last showed.
    hovered_file: Option<usize>,
    /// Ctrl+P's go to file over the filter.
    goto: GoTo,
}

impl FileTree {
    /// Rows and visible file order for keyboard navigation (rebuilt only when inputs changed).
    pub fn refresh(
        &mut self,
        files: &Arc<Vec<ChangedFile>>,
        is_viewed: &dyn Fn(&ChangedFile) -> bool,
        viewed_version: u64,
    ) {
        let key =
            (Arc::as_ptr(files) as usize, viewed_version.wrapping_add(self.collapse_version << 32));
        // Only "Hide viewed" lists the open file differently from the others.
        let open = self.open.as_deref().filter(|_| self.filter.hide_viewed);
        let is_current = self.built_for == Some(key)
            && self.built_filter == self.filter
            && self.built_open.as_deref() == open;
        if is_current {
            return;
        }
        let filter = self.filter.for_files(files);
        let visible = files
            .iter()
            .enumerate()
            .filter(|(_, file)| {
                let is_open = open == Some(file.path.as_str());
                filter.accepts(file, is_viewed(file) && !is_open)
            })
            .map(|(index, file)| (index, file.path.as_str()));
        let (mut rows, full_order) = model::build_with_order(visible, &self.collapsed);
        let counts = folder_counts(files, is_viewed);
        for row in &mut rows {
            if let TreeRow::Folder { path, mark, .. } = row
                && let Some(&(viewed, total)) = counts.get(path.as_str())
            {
                *mark = FolderMark::of(viewed, total);
            }
        }
        self.rows = rows;
        self.full_order = full_order;
        self.version += 1;
        // The files' own figures change only with the files, not with the filter or the marks.
        let is_new_files = self.built_for.map(|(files, _)| files) != Some(key.0);
        if is_new_files {
            self.widest_counts = widest_counts(files);
            self.breakdown = change_breakdown::tally(files.iter());
        }
        self.built_for = Some(key);
        self.built_filter = self.filter.clone();
        self.built_open = open.map(str::to_owned);
    }

    /// The file open now (`None` while a folder's summary shows): the next refresh keeps it
    /// listed under "Hide viewed" and lets go of the one open before.
    pub fn keep_open(&mut self, path: Option<&str>) {
        if self.open.as_deref() != path {
            self.open = path.map(str::to_owned);
        }
    }

    /// Visible files in tree order.
    pub fn file_order(&self) -> impl Iterator<Item = usize> + '_ {
        self.rows.iter().filter_map(|row| match row {
            TreeRow::File { index, .. } => Some(*index as usize),
            TreeRow::Folder { .. } => None,
        })
    }

    /// The files the filter lets through in tree order, collapsed folders included.
    pub fn full_order(&self) -> &[u32] {
        &self.full_order
    }

    /// Changes whenever [`Self::full_order`] or the rows may have changed.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn breakdown(&self) -> &ChangeBreakdown {
        &self.breakdown
    }

    /// Puts the pane at `side` of the window for this frame.
    pub fn place(&mut self, side: TreeSide) {
        self.side = side;
    }

    pub fn side(&self) -> TreeSide {
        self.side
    }

    /// Opens the collapsed folders `path` lies in, so its row shows.
    pub fn reveal(&mut self, path: &str) {
        let collapsed = self.collapsed.len();
        self.collapsed.retain(|folder| !is_below(path, folder));
        if self.collapsed.len() != collapsed {
            self.collapse_version += 1;
        }
    }

    /// A go to file runs: the tree shows, also where it is folded away.
    pub fn is_going_to(&self) -> bool {
        self.goto.is_active()
    }

    pub fn scroll_to(&mut self, file_index: usize) {
        self.scroll_to_file = Some(file_index);
    }

    /// The file (by index) whose row the pointer was on when the tree last showed.
    pub fn hovered_file(&self) -> Option<usize> {
        self.hovered_file
    }

    /// The tree is not shown this frame, so no row is under the pointer.
    pub fn forget_hover(&mut self) {
        self.hovered_file = None;
    }

    pub fn show(&mut self, ui: &mut Ui, inputs: &TreeInputs<'_>) -> Option<TreeEvent> {
        let mut event = None;
        let mut toggled_folder = None;
        let scroll_target = self.scroll_to_file.take().and_then(|file_index| {
            self.rows.iter().position(
                |row| matches!(row, TreeRow::File { index, .. } if *index as usize == file_index),
            )
        });
        let mut scroll = egui::ScrollArea::vertical().id_salt("file_tree").auto_shrink(false);
        if let Some(row_index) = scroll_target {
            let height = ui.available_height();
            scroll = scroll
                .vertical_scroll_offset((row_index as f32 * ROW_HEIGHT - height / 2.0).max(0.0));
        }
        let columns = CountColumns::measure(ui, self.widest_counts);
        goto::follow_search(self);
        let highlighted = goto::highlighted(self);
        let mut hovered_file = None;
        ui.scope(|ui| {
            // `show_rows` adds the item spacing between rows; rows touch instead.
            ui.spacing_mut().item_spacing.y = 0.0;
            scroll.show_rows(ui, ROW_HEIGHT, self.rows.len(), |ui, range| {
                for row in &self.rows[range] {
                    let at = egui::Rect::from_min_size(
                        ui.cursor().min,
                        egui::vec2(ui.available_width(), ROW_HEIGHT),
                    );
                    let shown = rows::show(ui, row, inputs, &columns, self.side);
                    if let TreeRow::File { index, .. } = row
                        && highlighted == Some(*index as usize)
                    {
                        goto::paint_highlight(ui, at);
                    }
                    if let (true, TreeRow::File { index, .. }) = (shown.is_hovered, row) {
                        hovered_file = Some(*index as usize);
                    }
                    match shown.action {
                        Some(RowAction::Tree(tree_event)) => event = Some(tree_event),
                        Some(RowAction::ToggleFolder(path)) => toggled_folder = Some(path),
                        None => {}
                    }
                }
            });
        });
        self.hovered_file = hovered_file;
        if let Some(path) = toggled_folder {
            if !self.collapsed.remove(&path) {
                self.collapsed.insert(path);
            }
            self.collapse_version += 1;
        }
        event
    }
}

fn widest_counts(files: &[ChangedFile]) -> Option<(u32, u32)> {
    files.iter().filter_map(|file| file.additions.zip(file.deletions)).reduce(
        |(widest_added, widest_deleted), (added, deleted)| {
            (widest_added.max(added), widest_deleted.max(deleted))
        },
    )
}

/// Drives the Files side panel with a fake change list.
#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    use egui::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use review_core::FileCategory;
    use review_git::{ChangeKind, ChangedFile};
    use review_store::ThemePreference;

    use super::{FileTree, TreeEvent, TreeInputs, side_panel};
    use cv_base::theme;

    struct Scene {
        tree: FileTree,
        files: Arc<Vec<ChangedFile>>,
        viewed: HashSet<String>,
        /// Bumped with every change of `viewed`, as `ViewedMarks::version`.
        viewed_version: u64,
        thread_counts: HashMap<String, usize>,
        selected: Option<usize>,
        theme: ThemePreference,
        /// The side panel's width; the whole window when `None`.
        panel_width: Option<f32>,
        last_event: Option<String>,
    }

    fn file(
        path: &str,
        kind: ChangeKind,
        counts: Option<(u32, u32)>,
        category: FileCategory,
    ) -> ChangedFile {
        ChangedFile {
            path: path.to_owned(),
            old_path: matches!(kind, ChangeKind::Renamed | ChangeKind::Copied)
                .then(|| format!("legacy/{path}")),
            kind,
            old_oid: "1".repeat(40),
            new_oid: "2".repeat(40),
            additions: counts.map(|(added, _)| added),
            deletions: counts.map(|(_, deleted)| deleted),
            is_binary: counts.is_none(),
            category,
            whitespace_only: false,
        }
    }

    fn scene(theme: ThemePreference) -> Scene {
        use ChangeKind::*;
        use FileCategory::*;
        let files = vec![
            file(".claude/rules/css-tailwind.md", Modified, Some((1, 1)), Docs),
            file("docs/engineering/billing-invoicing.md", Added, Some((1270, 0)), Docs),
            file("docs/engineering/billing-payments-core.md", Modified, Some((4, 3)), Docs),
            file("docs/engineering/billing-stripe-provider.md", Modified, Some((3, 1)), Docs),
            file("docs/engineering/build-sources.md", Modified, Some((4, 4)), Docs),
            file("docs/engineering/plan-lifecycle-rework.md", Modified, Some((25, 18)), Docs),
            file(
                "docs/engineering/withdrawal-of-a-plan-change-after-the-billing-period-has-started.md",
                Modified,
                Some((54, 11)),
                Docs,
            ),
            file("docs/operations/runbook.md", Deleted, Some((0, 212)), Docs),
            file("src/app/features/billing/invoice_service.rs", Renamed, Some((12, 9)), Code),
            file("src/app/features/billing/invoice_service_tests.rs", Added, Some((340, 0)), Tests),
            file("src/app/features/billing/mod.rs", Modified, Some((2, 1)), Code),
            file("src/app/shell/layout.rs", Copied, Some((88, 0)), Code),
            file("src/assets/logo.png", Modified, None, Assets),
            file("scripts/deploy.sh", TypeChanged, Some((0, 0)), Config),
            file("Cargo.lock", Modified, Some((12_345, 9_876)), Generated),
            file("README.md", Modified, Some((7, 2)), Docs),
        ];
        let viewed = [
            "docs/engineering/billing-stripe-provider.md",
            "docs/engineering/build-sources.md",
            "src/app/shell/layout.rs",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let thread_counts =
            [("docs/engineering/build-sources.md", 2), ("src/app/features/billing/mod.rs", 12)]
                .into_iter()
                .map(|(path, count)| (path.to_owned(), count))
                .collect();
        Scene {
            tree: FileTree::default(),
            files: Arc::new(files),
            viewed,
            viewed_version: 0,
            thread_counts,
            selected: Some(6),
            theme,
            panel_width: None,
            last_event: None,
        }
    }

    /// Common git-tracked files and large-file exotics, with every kind of change.
    fn icon_scene(theme: ThemePreference) -> Scene {
        use ChangeKind::*;
        use FileCategory::*;
        let counts = Some((12, 4));
        let files = vec![
            file(".github/workflows/ci.yml", Modified, counts, Config),
            file("assets/fonts/Inter.woff2", Added, None, Assets),
            file("assets/hero.psd", Modified, None, Assets),
            file("assets/intro.mp4", Added, None, Assets),
            file("assets/logo.png", Modified, None, Assets),
            file("assets/theme.svg", Modified, counts, Assets),
            file("assets/voice.wav", Deleted, None, Assets),
            file("game/Arena.umap", Modified, None, Assets),
            file("game/level.blend", Modified, None, Assets),
            file("game/robot.glb", Added, None, Assets),
            file("infra/main.bicep", Modified, counts, Config),
            file("infra/main.tf", Renamed, counts, Config),
            file("ml/events.parquet", Added, None, Assets),
            file("ml/llm.safetensors", Modified, None, Assets),
            file("ml/store.sqlite", Modified, None, Assets),
            file("src/api/orders.proto", Modified, counts, Code),
            file("src/api/schema.graphql", Added, Some((88, 0)), Code),
            file("src/App/Index.razor", Modified, counts, Code),
            file("src/App/Program.cs", Modified, counts, Code),
            file("src/App/App.csproj", Modified, Some((2, 1)), Config),
            file("src/main.rs", Modified, Some((40, 12)), Code),
            file("src/server/main.go", Modified, counts, Code),
            file("src/server/Main.java", Deleted, Some((0, 120)), Code),
            file("src/tools/tool.py", Copied, Some((64, 0)), Code),
            file("src/tools/query.sql", Modified, counts, Code),
            file("src/web/App.tsx", Modified, counts, Code),
            file("src/web/index.js", Modified, counts, Code),
            file("src/web/site.scss", Modified, counts, Code),
            file("src/web/settings.json", Modified, counts, Config),
            file("src/web/App.swift", Added, Some((210, 0)), Code),
            file("scripts/build.ps1", Modified, counts, Config),
            file("scripts/deploy.sh", TypeChanged, Some((0, 0)), Config),
            file("vendor/native.dll", Modified, None, Assets),
            file("vendor/App.1.2.0.nupkg", Added, None, Assets),
            file("vendor/setup.iso", Modified, None, Assets),
            file(".editorconfig", Modified, Some((1, 1)), Config),
            file(".env.example", Added, Some((6, 0)), Config),
            file(".gitignore", Modified, Some((2, 0)), Config),
            file("Cargo.lock", Modified, Some((1_204, 310)), Generated),
            file("Cargo.toml", Modified, Some((3, 1)), Config),
            file("compose.yaml", Modified, counts, Config),
            file("Dockerfile", Modified, counts, Config),
            file("LICENSE", Modified, Some((1, 1)), Docs),
            file("Makefile", Modified, counts, Config),
            file("package.json", Modified, Some((4, 2)), Config),
            file("pnpm-lock.yaml", Modified, Some((520, 211)), Generated),
            file("README.md", Modified, Some((7, 2)), Docs),
            file("release-notes.unknown", Added, Some((9, 0)), Docs),
        ];
        let viewed =
            ["src/web/index.js", "assets/logo.png"].into_iter().map(str::to_owned).collect();
        let selected = files.iter().position(|file| file.path == "src/main.rs");
        Scene {
            tree: FileTree::default(),
            files: Arc::new(files),
            viewed,
            viewed_version: 0,
            thread_counts: HashMap::new(),
            selected,
            theme,
            panel_width: Some(340.0),
            last_event: None,
        }
    }

    fn harness(theme: ThemePreference) -> Harness<'static, Scene> {
        build(scene(theme), [760.0, 980.0])
    }

    fn build(scene: Scene, size: [f32; 2]) -> Harness<'static, Scene> {
        let builder = Harness::builder().with_size(size).with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::motion::set_reduced(ctx, false);
        });
        builder.build_ui_state(
            |ui, scene: &mut Scene| {
                theme::activate(ui.ctx(), scene.theme);
                let palette = theme::palette(ui.ctx());
                ui.painter().rect_filled(ui.max_rect(), 0, palette.ground);
                let viewed = scene.viewed.clone();
                let is_viewed = move |file: &ChangedFile| viewed.contains(&file.path);
                // As the Files tab does: the open file first, then the refresh.
                let open = scene.selected.map(|index| scene.files[index].path.as_str());
                scene.tree.keep_open(open);
                scene.tree.refresh(&scene.files, &is_viewed, scene.viewed_version);
                let width = scene.panel_width.unwrap_or(ui.available_width());
                let inputs = TreeInputs {
                    files: &scene.files,
                    selected: scene.selected,
                    selected_folder: None,
                    is_viewed: &is_viewed,
                    thread_counts: &scene.thread_counts,
                    vote: super::VoteOffer::None,
                };
                let event = side_panel::show(ui, width, &mut scene.tree, &inputs, false);
                match event {
                    Some(TreeEvent::Select(index)) => {
                        scene.last_event = Some(format!("select {}", scene.files[index].path));
                    }
                    Some(TreeEvent::ToggleViewed(index)) => {
                        scene.last_event = Some(format!("viewed {}", scene.files[index].path));
                    }
                    Some(TreeEvent::SelectFolder(folder)) => {
                        scene.last_event = Some(format!("folder {folder}"));
                    }
                    Some(TreeEvent::ToggleFolderViewed(folder)) => {
                        // As the Files tab does: all viewed unless all already are.
                        let below: Vec<String> = scene
                            .files
                            .iter()
                            .filter(|file| super::is_below(&file.path, &folder))
                            .map(|file| file.path.clone())
                            .collect();
                        if below.iter().all(|path| scene.viewed.contains(path)) {
                            for path in &below {
                                scene.viewed.remove(path);
                            }
                        } else {
                            scene.viewed.extend(below);
                        }
                        scene.viewed_version += 1;
                        scene.last_event = Some(format!("viewed folder {folder}"));
                    }
                    Some(TreeEvent::FlipSide) => {
                        scene.tree.place(scene.tree.side().flipped());
                        scene.last_event = Some("flip side".to_owned());
                    }
                    Some(TreeEvent::Review(step)) => {
                        scene.last_event = Some(format!("review {step:?}"));
                    }
                    None => {}
                }
            },
            scene,
        )
    }

    #[test]
    fn file_tree_keeps_its_interactions() {
        let mut harness = harness(ThemePreference::Light);
        harness.run_steps(4);
        assert!(harness.query_by_label("docs/engineering/build-sources.md").is_some());
        assert!(harness.query_by_label("Viewed README.md").is_some());

        harness.get_by_label("Viewed README.md").click();
        harness.run_steps(2);
        assert_eq!(harness.state().last_event.as_deref(), Some("viewed README.md"));

        harness.get_by_label("Cargo.lock").click();
        harness.run_steps(2);
        assert_eq!(harness.state().last_event.as_deref(), Some("select Cargo.lock"));

        assert!(harness.query_by_label("scripts/deploy.sh").is_some());
        harness.get_by_label("scripts").click();
        harness.run_steps(2);
        assert_eq!(
            harness.state().last_event.as_deref(),
            Some("folder scripts"),
            "a folder's row opens its summary"
        );
        assert!(harness.query_by_label("scripts/deploy.sh").is_some(), "and keeps the folder open");
        harness.get_by_label("Collapse scripts").click();
        harness.run_steps(4);
        assert!(harness.query_by_label("scripts/deploy.sh").is_none());
        assert!(harness.query_by_label("scripts").is_some());
        assert!(harness.query_all_by_role(Role::TreeItem).count() > 10);
    }

    /// The tri-state counts every file below a folder, deeper ones too, and only those.
    /// A folder's viewed mark sits in the files' viewed column, left of the folder glyph, with the
    /// same hit area as a file's checkbox at the same depth.
    #[test]
    fn folder_and_file_marks_share_one_column() {
        let mut harness = harness(ThemePreference::Light);
        harness.run_steps(4);
        let folder = harness.get_by_label("Viewed folder docs").rect();
        let file = harness.get_by_label("Viewed README.md").rect();
        assert_eq!((folder.left(), folder.width()), (file.left(), file.width()), "{folder:?}");
        let row = harness.get_by_role_and_label(Role::TreeItem, "docs").rect();
        assert!(folder.right() < row.left() + 48.0, "left of the folder glyph: {folder:?}");
    }

    /// The header's side switch names the edge the pane goes to and reports the flip.
    #[test]
    fn the_side_switch_flips_the_pane() {
        let mut harness = harness(ThemePreference::Light);
        harness.run_steps(4);
        harness.get_by_label("Move the file tree to the right").click();
        harness.run_steps(2);
        assert_eq!(harness.state().last_event.as_deref(), Some("flip side"));
        assert!(harness.query_by_label("Move the file tree to the left").is_some());
        assert!(harness.query_by_label("Move the file tree to the right").is_none());
    }

    #[test]
    fn folder_marks_count_every_file_below_the_folder() {
        use super::{FolderMark, folder_counts, is_below};
        let files = scene(ThemePreference::Light).files;
        let viewed = ["src/app/shell/layout.rs", "src/assets/logo.png"];
        let counts = folder_counts(&files, &|file| viewed.contains(&file.path.as_str()));
        let mark = |folder: &str| {
            let (viewed, total) = counts[folder];
            FolderMark::of(viewed, total)
        };
        assert_eq!(counts["src"], (2, 5));
        assert_eq!(mark("src"), FolderMark::Some);
        assert_eq!(mark("src/assets"), FolderMark::All);
        assert_eq!(mark("src/app/features/billing"), FolderMark::None);
        assert!(!counts.contains_key("README.md"), "files at the top belong to no folder");
        assert!(is_below("src/app/mod.rs", "src") && !is_below("src-old/a.rs", "src"));
        assert!(!is_below("src", "src"), "a folder is not below itself");
    }

    /// A folder's mark shows how many of its files are viewed and marks them all at once: some
    /// viewed shows the dash, a click marks every file below it (deeper folders too) and checks
    /// the folder, a second click unmarks them all.
    #[test]
    fn a_folder_mark_marks_every_file_below_it() {
        use egui::accesskit::Toggled;
        let mut harness = harness(ThemePreference::Light);
        harness.run_steps(4);
        let toggled = |harness: &Harness<'static, Scene>, label: &str| {
            harness.get_by_label(label).accesskit_node().toggled()
        };
        assert_eq!(toggled(&harness, "Viewed folder docs"), Some(Toggled::Mixed), "2 of 7 viewed");
        assert_eq!(toggled(&harness, "Viewed folder src/app"), Some(Toggled::Mixed));
        assert_eq!(toggled(&harness, "Viewed folder scripts"), Some(Toggled::False));

        harness.get_by_label("Viewed folder docs").click();
        harness.run_steps(3);
        assert_eq!(harness.state().last_event.as_deref(), Some("viewed folder docs"));
        assert_eq!(toggled(&harness, "Viewed folder docs"), Some(Toggled::True));
        assert_eq!(toggled(&harness, "Viewed folder docs/engineering"), Some(Toggled::True));
        let file_viewed = |harness: &Harness<'static, Scene>, path: &str| {
            toggled(harness, &format!("Viewed {path}")) == Some(Toggled::True)
        };
        assert!(file_viewed(&harness, "docs/operations/runbook.md"), "the files below are viewed");
        assert!(file_viewed(&harness, "docs/engineering/billing-invoicing.md"));
        assert!(!file_viewed(&harness, "README.md"), "files outside stay as they were");

        harness.get_by_label("Viewed folder docs").click();
        harness.run_steps(3);
        assert_eq!(toggled(&harness, "Viewed folder docs"), Some(Toggled::False));
        assert!(!file_viewed(&harness, "docs/engineering/build-sources.md"), "all unmarked");

        // Hide viewed hides a folder once all its files are viewed.
        harness.get_by_label("Viewed folder scripts").click();
        harness.run_steps(2);
        harness.state_mut().tree.filter.hide_viewed = true;
        harness.run_steps(3);
        assert!(harness.query_by_label("scripts").is_none(), "the viewed folder hides");
        assert!(harness.query_by_label("docs").is_some(), "the others stay");
    }

    /// "Hide viewed" keeps the open file listed and in the order once it is marked viewed, so the
    /// steps from it work; it leaves when another file opens.
    #[test]
    fn hide_viewed_keeps_the_open_file_until_another_opens() {
        let mut harness = harness(ThemePreference::Light);
        harness.state_mut().tree.filter.hide_viewed = true;
        harness.state_mut().selected = Some(15);
        harness.run_steps(3);
        let in_order = |harness: &Harness<'static, Scene>, index: usize| {
            harness.state().tree.file_order().any(|listed| listed == index)
        };
        assert!(in_order(&harness, 15) && in_order(&harness, 14));

        let scene = harness.state_mut();
        scene.viewed.insert("README.md".to_owned());
        scene.viewed_version += 1;
        harness.run_steps(3);
        assert!(in_order(&harness, 15), "the open file stays in the order");
        assert!(harness.query_by_label("Viewed README.md").is_some(), "and in the tree");

        harness.state_mut().selected = Some(14);
        harness.run_steps(3);
        assert!(!in_order(&harness, 15), "it leaves once another file is open");
        assert!(harness.query_by_label("Viewed README.md").is_none());
        // src/app/shell/layout.rs, viewed from the start.
        assert!(!in_order(&harness, 11), "a viewed file that is not open stays hidden");
    }

    /// The categories are saved for every pull request: one picked on another pull request that
    /// none of these files has hides nothing here, as the dropdown shows "All files" then; the
    /// picked categories these files have still filter.
    #[test]
    fn a_category_no_file_has_hides_nothing() {
        use std::collections::BTreeSet;
        let files = scene(ThemePreference::Light).files;
        let mut tree = FileTree::default();
        tree.filter.categories = BTreeSet::from([FileCategory::Tests]);
        tree.refresh(&Arc::new(vec![files[0].clone(), files[8].clone()]), &|_| false, 0);
        assert_eq!(tree.file_order().count(), 2, "no file is a test: every file shows");

        tree.filter.categories = BTreeSet::from([FileCategory::Tests, FileCategory::Docs]);
        tree.refresh(&Arc::new(vec![files[0].clone(), files[8].clone()]), &|_| false, 0);
        assert_eq!(tree.file_order().collect::<Vec<_>>(), [0], "only the docs file");
        assert_eq!(tree.full_order(), [0]);
    }

    /// The change kind stays readable without the old letter badges: in words for assistive
    /// technology, and the rename source in the row's description.
    #[test]
    fn change_kinds_are_named_in_the_description() {
        let mut harness = build(icon_scene(ThemePreference::Light), [760.0, 1880.0]);
        harness.run_steps(4);
        let description = |label: &str| harness.get_by_label(label).accesskit_node().description();
        assert_eq!(description("assets/voice.wav").as_deref(), Some("Deleted, binary"));
        assert_eq!(
            description("infra/main.tf").as_deref(),
            Some("Renamed from legacy/infra/main.tf, 12 added, 4 deleted")
        );
        assert_eq!(
            description("src/web/App.swift").as_deref(),
            Some("Added, 210 added, 0 deleted")
        );
        assert_eq!(
            description("src/web/index.js").as_deref(),
            Some("Modified, 12 added, 4 deleted, viewed")
        );
    }
}
