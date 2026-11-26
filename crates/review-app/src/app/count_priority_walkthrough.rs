//! The line count follows the user, end to end: the real app against the mocked GitHub host
//! ([`super::github_mock`]) with a blob-less mirror. On a large pull request whose chunked
//! download of the changed blobs is held, a file opened deep in tree order gets its blobs from a
//! small chunk of the count with its neighbours, not from a fetch of its own. Their blobs come at
//! most twice: once in that chunk, once more only when the chunk outlasts `COUNT_WAIT`. The counts arrive chunk by chunk: one held chunk let through adds its files
//! while the other still waits. A thread row the pointer rests on has its file's diff loaded
//! ahead, so the click on it shows the diff at once.
//! `cargo test -p review-app --bin CodeView count_priority`

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_store::{AppPaths, FilesStart};

use super::ReviewApp;
use super::action_e2e::{app, has_cached_diff, open_pull_request, settle_background, wait_until};
use super::clicks::Clicks;
use super::github_mock::{ExtraFile, FILE, GitHubHost, MockThread, REPOSITORY, THREAD_TEXT};
use cv_testkit::{self as test_support, ScratchDir};

/// Changed files in `src/m00/` to `src/m19/`, counted in tree order before `zz/`: more than the
/// first four chunks take (16 + 32 + 64 + 128), so `zz/` waits for a chunk while those are held.
const MANY: usize = 600;
/// The last folder in tree order, far behind the chunks the count starts with.
const DEEP: [&str; 3] = ["zz/a.rs", "zz/b.rs", "zz/c.rs"];
/// The spinner in the file tree's progress line while the lines are counted.
const COUNTING: &str = "Counting lines";
/// Chunks the count downloads at once (`review_git`'s `PARALLEL`); [`MANY`] files fill more.
const PARALLEL_CHUNKS: usize = 4;

/// The paths of the [`MANY`] files; the mock takes paths that live as long as the program.
static PATHS: LazyLock<Vec<String>> = LazyLock::new(|| {
    (0..MANY).map(|index| format!("src/m{:02}/f{index:04}.rs", index / 30)).collect()
});

/// A file whose two versions share their blobs with no other file, so a blob fetched twice is
/// one file's blob fetched twice.
fn changed(path: &'static str, index: usize) -> ExtraFile {
    ExtraFile {
        path,
        base: format!("pub fn part_{index}() -> u32 {{\n    1\n}}\n").into_bytes(),
        head: format!("pub fn part_{index}() -> u32 {{\n    2\n}}\n").into_bytes(),
    }
}

fn many_files() -> Vec<ExtraFile> {
    let many = PATHS.iter().enumerate().map(|(index, path)| changed(path.as_str(), index));
    many.chain(DEEP.iter().enumerate().map(|(index, path)| changed(path, MANY + index))).collect()
}

/// The blob ids of `paths` in both versions, from the hosted repository.
fn blob_ids(scratch: &ScratchDir, paths: &[&str]) -> Vec<String> {
    let hosted = scratch.path().join(format!("hosted/{REPOSITORY}.git"));
    let mut ids = Vec::new();
    for path in paths {
        for version in ["main", "feature"] {
            ids.push(test_support::git(&hosted, &["rev-parse", &format!("{version}:{path}")]));
        }
    }
    ids
}

/// The app's mirror of the repository.
fn mirror_dir(scratch: &ScratchDir) -> PathBuf {
    let paths = AppPaths::new(scratch.path().join("config"), scratch.path().join("cache"));
    let mirrors = std::fs::read_dir(paths.mirrors_dir()).expect("the mirrors folder");
    mirrors.flatten().map(|entry| entry.path()).find(|path| path.is_dir()).expect("a mirror")
}

/// Every fetch into the partial clone leaves one promisor pack: the blobs of each, in fetch
/// order is not known, once per pack they came in.
fn fetched_blobs(mirror: &Path) -> Vec<Vec<String>> {
    let packs = std::fs::read_dir(mirror.join("objects/pack")).expect("the pack folder");
    let mut fetches = Vec::new();
    for pack in packs.flatten().map(|entry| entry.path()) {
        if pack.extension().is_some_and(|extension| extension == "promisor") {
            let index = pack.with_extension("idx").display().to_string();
            let listed = test_support::git(mirror, &["verify-pack", "-v", &index]);
            let blobs = listed.lines().filter(|line| line.contains(" blob "));
            fetches.push(blobs.filter_map(|line| line.get(..40)).map(str::to_owned).collect());
        }
    }
    fetches
}

/// Types `text` into the file tree's filter, so its rows fit on screen.
fn filter_tree(harness: &mut Harness<'_, ReviewApp>, text: &str) {
    const HINT: &str = "Filter files";
    wait_until(harness, "the file tree's filter", |harness| {
        harness
            .query_by(|node| node.placeholder().is_some_and(|hint| hint.ends_with(HINT)))
            .is_some()
    });
    harness.get_by(|node| node.placeholder().is_some_and(|hint| hint.ends_with(HINT))).focus();
    harness.step();
    harness
        .get_by(|node| node.placeholder().is_some_and(|hint| hint.ends_with(HINT)))
        .type_text(text);
    harness.step();
}

fn files_tab(harness: &mut Harness<'_, ReviewApp>) {
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::Tab
                    && node.label().is_some_and(|label| label.starts_with("Files"))
            })
            .rect(),
    );
}

#[test]
fn count_priority_a_file_opened_deep_in_tree_order_counts_with_its_neighbours_first() {
    let scratch = ScratchDir::new("count-priority");
    let mut host = GitHubHost::with_pull_request_files(scratch.path(), &many_files());
    let held = host.hold_batch_downloads();
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    // Dropped before the app, so a failing test lets the held downloads go before it shuts down.
    let held = held;
    open_pull_request(&mut harness);
    files_tab(&mut harness);
    wait_until(&mut harness, "the count on its way", |harness| {
        harness.query_by_label(COUNTING).is_some()
    });
    // The count lists the files, then starts its first chunks at once, all held. Opened before
    // that, the deep file would join the first chunk, filled up to its size in tree order.
    let started = Instant::now();
    while held.waiting() < PARALLEL_CHUNKS {
        harness.step();
        test_support::let_threads_run(Duration::from_millis(20));
        assert!(started.elapsed() < Duration::from_secs(60), "the first chunks never went out");
    }
    let mirror = mirror_dir(&scratch);
    let opened = DEEP[1];
    filter_tree(&mut harness, "zz/");
    wait_until(&mut harness, "the deep folder's rows", |harness| {
        harness.query_by_role_and_label(Role::TreeItem, opened).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, opened).rect());
    wait_until(&mut harness, "the deep file's diff", |harness| {
        harness.query_by_label(&format!("Diff of {opened}")).is_some()
    });
    // The neighbours (the previous and the next file of its folder) came with it, as wanted
    // files of the count: a fetch of the opened file's own would bring its two blobs alone.
    let neighbours = blob_ids(&scratch, &[DEEP[0], DEEP[2]]);
    let mut fetched: HashSet<String> = HashSet::new();
    let started = Instant::now();
    while !neighbours.iter().all(|id| fetched.contains(id)) {
        harness.step();
        test_support::let_threads_run(Duration::from_millis(50));
        fetched = fetched_blobs(&mirror).into_iter().flatten().collect();
        assert!(started.elapsed() < Duration::from_secs(60), "the neighbours never came");
    }
    assert!(fetched.is_superset(&blob_ids(&scratch, &[opened]).into_iter().collect()));
    assert!(harness.query_by_label(COUNTING).is_some(), "the held chunks still count");
    let early = fetched_blobs(&mirror).len();

    held.open();
    wait_until(&mut harness, "every file counted", |harness| {
        harness.query_by_label(COUNTING).is_none()
    });
    // The deep folder's blobs came in the wanted chunk (above), and at most once more: a diff
    // waits for its chunk only `COUNT_WAIT`, then fetches its file itself, which a loaded machine
    // reaches. Elsewhere the summary's cards do the same beside their held chunk.
    let fetches = fetched_blobs(&mirror);
    for id in blob_ids(&scratch, &DEEP) {
        let times = fetches.iter().filter(|blobs| blobs.contains(&id)).count();
        assert!(times <= 2, "a blob of the deep folder came in {times} fetches");
    }
    let changed = MANY + DEEP.len() + 1;
    let chunks = review_git::download_chunk_sizes(changed).len();
    println!(
        "count_priority: {early} fetches until the deep file showed (commits and trees included), {} in all; tree-order chunks alone: {chunks}",
        fetches.len()
    );
}

fn thread_row<'h>(
    harness: &'h Harness<'_, ReviewApp>,
    text: &str,
) -> Option<egui_kittest::Node<'h>> {
    harness.query_all_by_role(Role::Button).find(|node| {
        node.accesskit_node().label().is_some_and(|label| label.ends_with(&format!(": {text}")))
    })
}

/// Clicks the threads panel's row of `text` and steps until `path`'s diff shows: the frames from
/// the click to the first frame that shows it, and whether the diff's skeleton showed meanwhile.
fn click_row_until_shown(
    harness: &mut Harness<'_, ReviewApp>,
    text: &str,
    path: &str,
) -> (usize, bool) {
    harness.click_at(thread_row(harness, text).expect("the thread's row").rect());
    let started = Instant::now();
    let (mut frames, mut skeleton) = (0, false);
    loop {
        harness.step();
        frames += 1;
        skeleton |= harness.query_by_label("Loading the diff").is_some();
        if harness.query_by_label(&format!("Diff of {path}")).is_some() {
            return (frames, skeleton);
        }
        assert!(started.elapsed() < Duration::from_secs(60), "{path} never showed");
        test_support::let_threads_run(Duration::from_millis(1));
    }
}

const FAR: &str = "web/far.rs";
const FAR_THREAD: &str = "Is this far file right?";

#[test]
fn count_priority_a_resting_pointer_on_a_thread_row_loads_its_file_ahead() {
    let scratch = ScratchDir::new("count-priority-hover");
    let lines = |name: &str, changed: bool| -> Vec<u8> {
        let text = (0..400).fold(String::new(), |mut text, line| {
            let value = if changed && line % 7 == 0 { line + 1 } else { line };
            writeln!(text, "pub fn {name}_{line}() -> u32 {{ {value} }}").unwrap();
            text
        });
        text.into_bytes()
    };
    let first =
        ExtraFile { path: "a/first.rs", base: lines("first", false), head: lines("first", true) };
    let far = ExtraFile { path: FAR, base: lines("far", false), head: lines("far", true) };
    let host = GitHubHost::with_pull_request_files(scratch.path(), &[first, far]);
    host.add_threads(&[MockThread { id: "PRRT_far", path: FAR, lines: 14..=14, text: FAR_THREAD }]);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    harness.state_mut().services.settings.files_start = FilesStart::FirstFile;
    harness.state_mut().services.settings.threads_panel.is_open = true;
    open_pull_request(&mut harness);
    files_tab(&mut harness);
    wait_until(&mut harness, "the first file's diff", |harness| {
        harness.query_by_label("Diff of a/first.rs").is_some()
    });
    wait_until(&mut harness, "the thread rows", |harness| {
        thread_row(harness, THREAD_TEXT).is_some() && thread_row(harness, FAR_THREAD).is_some()
    });
    settle_background(&mut harness);

    // Not rested on: src/lib.rs's diff loads after the click. How many frames that takes is the
    // machine's speed (a local git diff can land in the click's next frame), so the cache tells.
    assert!(!has_cached_diff(&harness, FILE), "{FILE} was not loaded before its row was clicked");
    let (cold_frames, cold_skeleton) = click_row_until_shown(&mut harness, THREAD_TEXT, FILE);
    // Rested on: web/far.rs's diff is in the cache when the click lands.
    assert!(!has_cached_diff(&harness, FAR), "{FAR} was not loaded before the pointer rested");
    let row = thread_row(&harness, FAR_THREAD).expect("the far thread's row").rect();
    harness.hover_at(row.center());
    for _ in 0..10 {
        harness.step();
        test_support::let_threads_run(Duration::from_millis(20));
    }
    settle_background(&mut harness);
    assert!(has_cached_diff(&harness, FAR), "the resting pointer loaded {FAR} ahead");
    let (warm_frames, warm_skeleton) = click_row_until_shown(&mut harness, FAR_THREAD, FAR);
    println!(
        "count_priority hover: cold {cold_frames} frames (skeleton {cold_skeleton}), rested {warm_frames} frames (skeleton {warm_skeleton})"
    );
    assert!(!warm_skeleton, "the rested thread's diff showed without loading");
    assert_eq!(warm_frames, FIRST_FRAME_AFTER_THE_CLICK, "the rested thread's diff shows at once");
}

/// The frame of the click hands the pick on, so the first frame after it is the earliest that can
/// show the picked file.
const FIRST_FRAME_AFTER_THE_CLICK: usize = 2;

/// Changed files besides the mock's own file: two download chunks of 16 and 25 files (the first
/// one is [`super::github_mock::FILE`] and `part_00` to `part_14`).
const FEW: usize = 40;

static FEW_PATHS: LazyLock<Vec<String>> =
    LazyLock::new(|| (0..FEW).map(|index| format!("src/part_{index:02}.rs")).collect());

fn counted_label(counted: usize) -> String {
    format!("{counted} of {} files counted", FEW + 1)
}

#[test]
fn count_priority_counts_arrive_chunk_by_chunk() {
    let scratch = ScratchDir::new("count-priority-chunks");
    let files: Vec<ExtraFile> =
        FEW_PATHS.iter().enumerate().map(|(index, path)| changed(path.as_str(), index)).collect();
    let mut host = GitHubHost::with_pull_request_files(scratch.path(), &files);
    let held = host.hold_batch_downloads();
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    // Dropped before the app, so a failing test lets the held downloads go before it shuts down.
    let held = held;
    open_pull_request(&mut harness);
    files_tab(&mut harness);
    wait_until(&mut harness, "the count on its way", |harness| {
        harness.query_by_label(&counted_label(0)).is_some()
    });

    held.let_through(1);
    // Either chunk: a summary card's diff may have brought a few blobs before the count began.
    let with_one_chunk = |harness: &Harness<'_, ReviewApp>| {
        (16..=FEW).any(|counted| harness.query_by_label(&counted_label(counted)).is_some())
    };
    wait_until(&mut harness, "one chunk's counts", with_one_chunk);
    assert!(harness.query_by_label(COUNTING).is_some(), "the other chunk is still held");

    held.open();
    wait_until(&mut harness, "the totals", |harness| {
        harness.query_by_label(COUNTING).is_none()
            && harness.query_by_label("+42 added, −42 deleted").is_some()
    });
}
