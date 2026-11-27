//! Diffs of the files the user is likely to open next, worked out ahead from the local mirror into
//! the diff cache, so switching to one shows its diff, colored, in the first frame after the click
//! instead of a placeholder.
//!
//! What loads ahead, most wanted first:
//! - The file row the pointer rests on for [`DWELL`]. A pointer on its way across the tree
//!   crosses a row in a frame or two and starts nothing; from resting on a row to the click
//!   usually takes longer than the rest of the dwell plus a diff (git and the first colors, tens
//!   of milliseconds), so the diff is there when the click lands. A hover is asked for, so it
//!   reaches any folder.
//! - The next [`NEXT`] files after the open one in tree order, and the [`PREVIOUS`] file before
//!   it, all in the open file's own folder: the tree lists a folder's own files together, so the
//!   neighbours end where the folder ends and never reach into the next one.
//!
//! Previews, binary files, files above [`MAX_CHANGED_LINES`] and files whose lines are not counted
//! yet are skipped and take no place. Only local work: a blob the mirror does not have yet stays
//! missing until the user opens that file, so working ahead never reaches the network or the
//! provider. One diff at a time, colored behind the open file's colors, and none while the open
//! file loads. A new plan (another file opened, the pointer resting elsewhere, the order changing)
//! replaces the queue, so stepping through the tree with the keys never builds a backlog; the
//! diff in flight finishes into the cache, or becomes the open file's load when the user opens its
//! file. A diff loaded ahead that a new plan no longer asks for leaves the cache with its colors
//! unless it was opened meanwhile: opening a file in another folder releases every unopened diff
//! of the folder left behind at once, as do the summary and other compared commits for the files
//! whose versions they change (a diff is its file's versions', [`DiffKey`]). Opened diffs stay in
//! the cache's recently shown ones, so going back is instant. The cache keeps at most
//! [`cv_pr_diff::diff_cache::MAX_UNOPENED`] unopened diffs in any case.
//!
//! A thread row the pointer rests on (the Threads panel, the Overview's timeline) counts as a
//! hovered file row too: the row names its file for the frame ([`hover`]), and a file row of the
//! tree goes first.
//!
//! While the lines are counted, the open file, its neighbours and the rested file are wanted by
//! the count ([`review_git::GitMirror::want`]), uncounted ones too: their blobs download in the
//! next chunk, so they are counted, and so loaded ahead, soon after.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use review_git::{ChangedFile, DiffOptions, FileDiff, GitMirror, WantedBy};

use cv_pr_diff::diff_cache::{DiffCache, DiffKey};
use cv_pr_diff::patch_colors::Colorer;
use cv_pr_threads::hover;
use cv_runtime::background::Background;
use cv_runtime::load::Load;
use cv_syntax::pool::Priority;

/// Files after the open one that load ahead.
pub(crate) const NEXT: usize = 4;
/// Files before the open one that load ahead: the one the user just came from is usually still
/// cached, so this one is cheap.
pub(crate) const PREVIOUS: usize = 1;
/// How many files on each side of the open one the plan looks through, skipped ones included.
pub(crate) const SCAN: usize = 12;
/// How long the pointer rests on a file row before its diff loads.
pub(crate) const DWELL: Duration = Duration::from_millis(150);
/// Bigger changes take long enough to diff that working ahead could hold up the next file opened.
const MAX_CHANGED_LINES: u32 = 5_000;

/// The commits and the options every diff of a plan compares.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Compare {
    pub base: String,
    pub head: String,
    pub options: DiffOptions,
}

impl Compare {
    fn key(&self, file: &ChangedFile) -> DiffKey {
        DiffKey::of(file, self.options)
    }
}

#[derive(Default)]
pub(crate) struct Prefetch {
    /// What the current plan compares, and the diff of every file it asked for, done or not: the
    /// next plan only replaces the queue when it asks for something else.
    compare: Option<Compare>,
    planned: Vec<DiffKey>,
    queue: VecDeque<ChangedFile>,
    running: Option<(DiffKey, Load<Arc<FileDiff>>)>,
    dwell: Dwell<usize>,
    /// The thread row the pointer is on and its file, which [`Self::plan`] takes once rested on.
    thread_dwell: Dwell<String>,
    thread_rested: Option<ChangedFile>,
    /// The files the line count downloads first: the open and the planned ones, uncounted ones
    /// included.
    count_wanted: Vec<String>,
}

impl Prefetch {
    /// The file (by index) whose row the pointer rested on for [`DWELL`], called every frame with
    /// the row it is on. Wakes the UI when the dwell will be over, as a resting pointer sends no
    /// events.
    /// A thread row the pointer is on ([`hover`]) rests the same way; its file goes to the next
    /// [`Self::plan`].
    pub(crate) fn rested_on(
        &mut self,
        ctx: &egui::Context,
        hovered: Option<usize>,
    ) -> Option<usize> {
        let now = ctx.input(|input| input.time);
        let thread_file = hover::file(ctx);
        let thread_path = thread_file.as_ref().map(|file| file.path.clone());
        self.thread_rested = rest(ctx, self.thread_dwell.update(thread_path, now)).and(thread_file);
        rest(ctx, self.dwell.update(hovered, now))
    }

    /// Plans the diffs to load ahead, called every frame: `order` is the files in the order the
    /// tree shows them (or a window of it), `open` the open file's place in it, `rested` the file
    /// the pointer rested on. Another open file, the pointer resting elsewhere, line counts
    /// arriving or a filter changing the order makes a new plan; otherwise the queue goes on
    /// where it was. Returns whether the plan changed.
    pub(crate) fn plan(
        &mut self,
        compare: &Compare,
        order: &[&ChangedFile],
        open: Option<usize>,
        rested: Option<&ChangedFile>,
        is_wanted: impl Fn(&ChangedFile) -> bool,
    ) -> bool {
        let rested = rested.or(self.thread_rested.as_ref());
        let wanted = wanted(order, open, rested, is_wanted, can_load_ahead);
        // The count does not know sizes before it counted: the near files whatever their size.
        let near = self::wanted(order, open, rested, |_| true, |_| true);
        let open_file = open.and_then(|position| order.get(position)).copied();
        self.count_wanted.clear();
        for file in open_file.into_iter().chain(near).chain(wanted.iter().copied()) {
            if !self.count_wanted.contains(&file.path) {
                self.count_wanted.push(file.path.clone());
            }
        }
        // Compared in place: planning runs every frame.
        let is_same = self.compare.as_ref() == Some(compare)
            && wanted.len() == self.planned.len()
            && wanted.iter().zip(&self.planned).all(|(file, key)| key.is_of(file, compare.options));
        if is_same {
            return false;
        }
        self.compare = Some(compare.clone());
        self.planned = wanted.iter().map(|file| compare.key(file)).collect();
        self.queue = wanted.into_iter().cloned().collect();
        true
    }

    /// The current plan asks for `key`; a diff loaded ahead that it does not ask for any more is
    /// released.
    pub(crate) fn is_planned(&self, key: &DiffKey) -> bool {
        self.planned.contains(key)
    }

    /// Drops the plan, for a pull request without compared commits yet.
    pub(crate) fn clear(&mut self) {
        self.planned.clear();
        self.queue.clear();
        self.count_wanted.clear();
    }

    /// Moves a diff that finished loading ahead into `cache`. Returns whether one finished.
    pub(crate) fn collect(&mut self, cache: &mut DiffCache) -> bool {
        let Some((key, load)) = &mut self.running else {
            return false;
        };
        if !load.poll() {
            return false;
        }
        // A blob that is not local yet fails the diff; the file loads normally once opened.
        if let Load::Ready(diff) = std::mem::take(load) {
            cache.insert_ahead(key.clone(), diff);
        }
        self.running = None;
        true
    }

    /// The load of `key` in flight, for the open file: the user opened the file loading ahead,
    /// and it goes on instead of starting again.
    pub(crate) fn take_running(&mut self, key: &DiffKey) -> Option<Load<Arc<FileDiff>>> {
        let is_running = self.running.as_ref().is_some_and(|(running, _)| running == key);
        if !is_running {
            return None;
        }
        self.running.take().map(|(_, load)| load)
    }

    /// Collects a finished diff and starts the next queued one, unless one is in flight, the
    /// open file is still loading (`is_busy`) or there is no mirror yet. Each diff is colored by
    /// diff line ahead too, so the file opens colored. Returns whether a diff finished.
    pub(crate) fn step(
        &mut self,
        cache: &mut DiffCache,
        git: Option<&Arc<GitMirror>>,
        background: &Background,
        colorer: &Colorer,
        is_busy: bool,
    ) -> bool {
        let finished = self.collect(cache);
        if let Some(git) = git {
            git.want(WantedBy::Files, &self.count_wanted);
        }
        if self.running.is_some() || is_busy {
            return finished;
        }
        let (Some(git), Some(compare)) = (git, &self.compare) else {
            return finished;
        };
        while let Some(file) = self.queue.pop_front() {
            let key = compare.key(&file);
            if cache.contains(&key) {
                continue;
            }
            let git = git.clone();
            let (base, head, options) = (compare.base.clone(), compare.head.clone(), key.options);
            let (colorer, colored) = (colorer.clone(), key.clone());
            let mut load = Load::Idle;
            load.start(background, async move {
                let diff = Arc::new(git.local_file_diff(&base, &head, &file, options).await?);
                colorer.color(colored, diff.clone(), Priority::Queued).await;
                Ok(diff)
            });
            self.running = Some((key, load));
            return finished;
        }
        finished
    }
}

/// The files to load ahead, most wanted first and each once: `rested`, the [`NEXT`] files after
/// `order[open]`, then the [`PREVIOUS`] ones before it, both in its folder. Files `is_wanted` or
/// `can_load` (by size: [`can_load_ahead`]) turns down take no place.
fn wanted<'a>(
    order: &[&'a ChangedFile],
    open: Option<usize>,
    rested: Option<&'a ChangedFile>,
    is_wanted: impl Fn(&ChangedFile) -> bool,
    can_load: impl Fn(&ChangedFile) -> bool,
) -> Vec<&'a ChangedFile> {
    let open_file = open.and_then(|position| order.get(position).copied());
    let is_open = |file: &ChangedFile| open_file.is_some_and(|open| open.path == file.path);
    let can_load = |file: &&ChangedFile| !is_open(file) && is_wanted(file) && can_load(file);
    let mut wanted: Vec<&ChangedFile> = rested.filter(can_load).into_iter().collect();
    if let (Some(position), Some(open_file)) = (open, open_file) {
        let in_folder = |file: &&ChangedFile| folder(&file.path) == folder(&open_file.path);
        let after = order.iter().skip(position + 1).copied().take_while(in_folder);
        let after = after.filter(can_load).take(NEXT);
        let before = order.get(..position).unwrap_or_default().iter().rev().copied();
        let before = before.take_while(in_folder).filter(can_load).take(PREVIOUS);
        for file in after.chain(before) {
            if !wanted.iter().any(|planned| planned.path == file.path) {
                wanted.push(file);
            }
        }
    }
    wanted
}

/// The folder `path` is in, `""` at the top.
fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// Small enough to diff ahead; unknown sizes (lines not counted yet) are left out.
fn can_load_ahead(file: &ChangedFile) -> bool {
    !file.is_binary
        && match (file.additions, file.deletions) {
            (Some(additions), Some(deletions)) => {
                additions.saturating_add(deletions) <= MAX_CHANGED_LINES
            }
            _ => false,
        }
}

/// What `rest` rested on; a dwell still running wakes the UI when it will be over, as a resting
/// pointer sends no events.
fn rest<T>(ctx: &egui::Context, rest: Rest<T>) -> Option<T> {
    match rest {
        Rest::Over(rested) => Some(rested),
        Rest::Waiting(remaining) => {
            ctx.request_repaint_after(remaining);
            None
        }
        Rest::None => None,
    }
}

/// The row the pointer is on (a file index, a thread's path), and since when on egui's clock.
struct Dwell<T> {
    on: Option<(T, f64)>,
}

impl<T> Default for Dwell<T> {
    fn default() -> Self {
        Self { on: None }
    }
}

enum Rest<T> {
    None,
    Waiting(Duration),
    Over(T),
}

impl<T: Clone + PartialEq> Dwell<T> {
    fn update(&mut self, hovered: Option<T>, now: f64) -> Rest<T> {
        let Some(index) = hovered else {
            self.on = None;
            return Rest::None;
        };
        let since = match &self.on {
            Some((on, since)) if *on == index => *since,
            _ => {
                self.on = Some((index.clone(), now));
                now
            }
        };
        let waited = Duration::from_secs_f64((now - since).max(0.0));
        match DWELL.checked_sub(waited) {
            Some(remaining) if !remaining.is_zero() => Rest::Waiting(remaining),
            _ => Rest::Over(index),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use review_core::FileCategory;
    use review_git::{ChangeKind, ChangedFile, DiffOptions, FileDiff};

    use super::{Compare, DWELL, Dwell, NEXT, PREVIOUS, Prefetch, Rest};
    use cv_pr_diff::diff_cache::{DiffCache, DiffKey};
    use cv_runtime::load::Load;

    fn file(path: &str, changed_lines: Option<u32>) -> ChangedFile {
        ChangedFile {
            additions: changed_lines,
            deletions: changed_lines.map(|_| 0),
            category: FileCategory::Code,
            ..ChangedFile::sample(path, ChangeKind::Modified)
        }
    }

    fn compare(context: u32) -> Compare {
        Compare { base: "base".into(), head: "head".into(), options: DiffOptions::context(context) }
    }

    /// The key of `path`'s diff as [`file`] changes it, with 3 lines of context.
    fn key(path: &str) -> DiffKey {
        compare(3).key(&file(path, Some(10)))
    }

    fn queued(prefetch: &Prefetch) -> Vec<&str> {
        prefetch.queue.iter().map(|file| file.path.as_str()).collect()
    }

    /// The tree's order of a pull request: `src/cart/` with a huge file, a preview and a file
    /// without line counts among its files, then `src/lib.rs` and `web/`.
    fn tree() -> Vec<ChangedFile> {
        vec![
            file("src/cart/a.rs", Some(10)),
            file("src/cart/b.rs", Some(10)),
            file("src/cart/c_huge.rs", Some(60_000)),
            file("src/cart/d.md", Some(10)),
            file("src/cart/e.rs", None),
            file("src/cart/f.rs", Some(10)),
            file("src/cart/g.rs", Some(10)),
            file("src/lib.rs", Some(10)),
            file("web/app.ts", Some(10)),
            file("web/util.ts", Some(10)),
        ]
    }

    fn is_not_markdown(file: &ChangedFile) -> bool {
        !std::path::Path::new(&file.path)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    }

    #[test]
    fn loads_the_next_files_and_the_previous_one_of_the_open_files_folder_only() {
        let files = tree();
        let order: Vec<&ChangedFile> = files.iter().collect();
        let mut prefetch = Prefetch::default();
        prefetch.plan(&compare(3), &order, Some(1), None, is_not_markdown);
        assert_eq!(
            queued(&prefetch),
            ["src/cart/f.rs", "src/cart/g.rs", "src/cart/a.rs"],
            "the huge file, the preview and the uncounted file take no place, and the neighbours \
             end with the folder: src/lib.rs and web/ are not loaded"
        );

        prefetch.plan(&compare(3), &order, Some(8), None, is_not_markdown);
        assert_eq!(
            queued(&prefetch),
            ["web/util.ts"],
            "the previous file in tree order is in another folder"
        );
        prefetch.plan(&compare(3), &order, Some(7), None, is_not_markdown);
        assert!(queued(&prefetch).is_empty(), "src/lib.rs is alone in its own files of src/");

        let many: Vec<ChangedFile> =
            (0..9).map(|index| file(&format!("src/{index}.rs"), Some(10))).collect();
        let order_of_many: Vec<&ChangedFile> = many.iter().collect();
        prefetch.plan(&compare(3), &order_of_many, Some(3), None, |_| true);
        assert_eq!(queued(&prefetch), ["src/4.rs", "src/5.rs", "src/6.rs", "src/7.rs", "src/2.rs"]);
        assert_eq!(queued(&prefetch).len(), NEXT + PREVIOUS);

        prefetch.plan(&compare(10), &order, Some(8), None, is_not_markdown);
        assert!(
            prefetch.compare.as_ref().is_some_and(|compare| compare.options.context == 10),
            "another context plans again"
        );
    }

    #[test]
    fn the_file_the_pointer_rested_on_goes_first_and_once() {
        let files = tree();
        let order: Vec<&ChangedFile> = files.iter().collect();
        let mut prefetch = Prefetch::default();
        prefetch.plan(&compare(3), &order, Some(0), Some(&files[9]), is_not_markdown);
        assert_eq!(
            queued(&prefetch),
            ["web/util.ts", "src/cart/b.rs", "src/cart/f.rs", "src/cart/g.rs"],
            "a hover reaches into another folder"
        );
        prefetch.plan(&compare(3), &order, Some(0), Some(&files[1]), is_not_markdown);
        assert_eq!(
            queued(&prefetch),
            ["src/cart/b.rs", "src/cart/f.rs", "src/cart/g.rs"],
            "a neighbour rested on loads first, once"
        );
        for (rested, why) in [(0, "the open file"), (2, "a huge file")] {
            prefetch.plan(&compare(3), &order, Some(0), Some(&files[rested]), is_not_markdown);
            assert_eq!(queued(&prefetch).len(), 3, "{why} does not load ahead");
        }
        prefetch.plan(&compare(3), &order, None, Some(&files[9]), is_not_markdown);
        assert_eq!(queued(&prefetch), ["web/util.ts"], "with no file open, only the rested one");
    }

    #[test]
    fn stepping_through_the_tree_replaces_the_queue_instead_of_growing_it() {
        let files: Vec<ChangedFile> =
            (0..40).map(|index| file(&format!("src/{index:02}.rs"), Some(10))).collect();
        let order: Vec<&ChangedFile> = files.iter().collect();
        let mut prefetch = Prefetch::default();
        for open in 0..files.len() {
            prefetch.plan(&compare(3), &order, Some(open), None, |_| true);
            assert!(queued(&prefetch).len() <= NEXT + PREVIOUS);
        }
        prefetch.queue.pop_front();
        let left = queued(&prefetch).len();
        prefetch.plan(&compare(3), &order, Some(files.len() - 1), None, |_| true);
        assert_eq!(queued(&prefetch).len(), left, "the same plan next frame goes on where it was");
    }

    /// Regression: what was loaded ahead for one place in the tree is released once the user moves
    /// on, unless they opened it.
    #[test]
    fn moving_away_releases_the_unopened_diffs_of_the_last_plan() {
        let files: Vec<ChangedFile> =
            (0..20).map(|index| file(&format!("src/{index:02}.rs"), Some(10))).collect();
        let order: Vec<&ChangedFile> = files.iter().collect();
        let diff = Arc::new(FileDiff::parse_unified(b"@@ -1 +1 @@\n-a\n+b\n"));
        let mut prefetch = Prefetch::default();
        let mut cache = DiffCache::default();
        assert!(prefetch.plan(&compare(3), &order, Some(0), None, |_| true), "a new plan");
        for file in &prefetch.queue {
            cache.insert_ahead(compare(3).key(file), diff.clone());
        }
        assert!(cache.get(&key("src/01.rs")).is_some(), "the user opens the next file");
        assert!(!prefetch.plan(&compare(3), &order, Some(0), None, |_| true), "nothing changed");

        assert!(prefetch.plan(&compare(3), &order, Some(15), None, |_| true));
        let mut released = cache.release_unopened(|key| prefetch.is_planned(key));
        released.sort_by(|left, right| left.path.cmp(&right.path));
        let released: Vec<&str> = released.iter().map(|key| key.path.as_str()).collect();
        assert_eq!(released, ["src/02.rs", "src/03.rs", "src/04.rs"], "far from the open file now");
        assert!(cache.contains(&key("src/01.rs")), "the opened diff stays");

        cache.insert_ahead(key("src/16.rs"), diff);
        prefetch.plan(&compare(10), &order, Some(15), None, |_| true);
        assert_eq!(
            cache.release_unopened(|key| prefetch.is_planned(key)).len(),
            1,
            "another context releases what the old one loaded"
        );
    }

    /// Regression: opening a file in another folder releases every unopened diff loaded ahead in
    /// the folder left behind at once; the opened ones stay.
    #[test]
    fn opening_a_file_in_another_folder_releases_the_old_folders_unopened_diffs() {
        let paths = ["src/cart/a.rs", "src/cart/b.rs", "src/cart/c.rs", "src/cart/d.rs"];
        let mut files: Vec<ChangedFile> = paths.iter().map(|path| file(path, Some(10))).collect();
        files.extend(["web/app.ts", "web/util.ts"].map(|path| file(path, Some(10))));
        let order: Vec<&ChangedFile> = files.iter().collect();
        let diff = Arc::new(FileDiff::parse_unified(b"@@ -1 +1 @@\n-a\n+b\n"));
        let mut prefetch = Prefetch::default();
        let mut cache = DiffCache::default();
        cache.insert(key("src/cart/a.rs"), diff.clone());
        prefetch.plan(&compare(3), &order, Some(0), None, |_| true);
        assert_eq!(queued(&prefetch), ["src/cart/b.rs", "src/cart/c.rs", "src/cart/d.rs"]);
        for file in &prefetch.queue {
            cache.insert_ahead(compare(3).key(file), diff.clone());
        }
        assert!(cache.get(&key("src/cart/b.rs")).is_some(), "b.rs was opened");

        assert!(prefetch.plan(&compare(3), &order, Some(4), None, |_| true), "web/app.ts opens");
        let mut released = cache.release_unopened(|key| prefetch.is_planned(key));
        released.sort_by(|left, right| left.path.cmp(&right.path));
        let released: Vec<&str> = released.iter().map(|key| key.path.as_str()).collect();
        assert_eq!(released, ["src/cart/c.rs", "src/cart/d.rs"], "the whole old folder's");
        for opened in ["src/cart/a.rs", "src/cart/b.rs"] {
            assert!(cache.contains(&key(opened)), "{opened} stays for going back");
        }
    }

    #[test]
    fn opening_the_file_loading_ahead_takes_over_its_load() {
        let diff = Arc::new(FileDiff::parse_unified(b"@@ -1 +1 @@\n-a\n+b\n"));
        let mut prefetch =
            Prefetch { running: Some((key("b.rs"), Load::Ready(diff))), ..Prefetch::default() };
        assert!(prefetch.take_running(&key("c.rs")).is_none(), "another file's load stays");
        assert!(prefetch.take_running(&key("b.rs")).is_some(), "the opened file's load moves");
        assert!(prefetch.running.is_none());
    }

    #[test]
    fn a_row_loads_once_the_pointer_rested_on_it_for_the_dwell() {
        let dwell_seconds = DWELL.as_secs_f64();
        let mut dwell = Dwell::default();
        assert!(
            matches!(dwell.update(Some(3), 10.0), Rest::Waiting(remaining) if remaining == DWELL)
        );
        assert!(matches!(dwell.update(Some(4), 10.1), Rest::Waiting(_)), "passing on starts over");
        assert!(matches!(dwell.update(Some(4), 10.1 + dwell_seconds / 2.0), Rest::Waiting(_)));
        assert!(matches!(dwell.update(Some(4), 10.1 + dwell_seconds + 0.001), Rest::Over(4)));
        assert!(matches!(dwell.update(Some(4), 11.0), Rest::Over(4)), "and stays over");
        assert!(matches!(dwell.update(None, 11.1), Rest::None));
        assert!(
            matches!(dwell.update(Some(4), 11.2), Rest::Waiting(_)),
            "leaving the row starts over"
        );
    }

    /// The count downloads the open file and its neighbours first, also while their lines are
    /// not counted and they are not loaded ahead yet.
    #[test]
    fn the_open_file_and_its_neighbours_are_wanted_by_the_count_before_they_are_counted() {
        let files = tree();
        let order: Vec<&ChangedFile> = files.iter().collect();
        let mut prefetch = Prefetch::default();
        prefetch.plan(&compare(3), &order, Some(3), Some(&files[9]), is_not_markdown);
        assert_eq!(
            prefetch.count_wanted,
            [
                "src/cart/d.md",
                "web/util.ts",
                "src/cart/e.rs",
                "src/cart/f.rs",
                "src/cart/g.rs",
                "src/cart/c_huge.rs",
                "src/cart/b.rs"
            ],
            "the open file, the rested one, the next and the previous, uncounted or huge, then \
             what loads ahead"
        );
        let ahead = ["web/util.ts", "src/cart/f.rs", "src/cart/g.rs", "src/cart/b.rs"];
        assert_eq!(queued(&prefetch), ahead);
        prefetch.clear();
        assert_eq!(prefetch.count_wanted, Vec::<String>::new());
    }

    /// A thread row the pointer rested on loads its file ahead, and a row of the tree rested on
    /// goes before it.
    #[test]
    fn a_thread_row_rested_on_loads_its_file_ahead() {
        let files = tree();
        let order: Vec<&ChangedFile> = files.iter().collect();
        let mut prefetch =
            Prefetch { thread_rested: Some(files[8].clone()), ..Prefetch::default() };
        prefetch.plan(&compare(3), &order, Some(0), None, is_not_markdown);
        assert_eq!(queued(&prefetch)[0], "web/app.ts", "the thread's file goes first");
        assert!(prefetch.count_wanted.contains(&"web/app.ts".to_owned()));
        prefetch.plan(&compare(3), &order, Some(0), Some(&files[9]), is_not_markdown);
        assert_eq!(queued(&prefetch)[0], "web/util.ts", "the tree's rested row goes first");
        assert!(!queued(&prefetch).contains(&"web/app.ts"));
    }
}
