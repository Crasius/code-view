//! The line count follows the user, end to end: the real app against the mocked GitHub host
//! ([`super::github_mock`]) with a blob-less mirror. On a large pull request whose chunked
//! download of the changed blobs is held, a file opened deep in tree order gets its blobs from a
//! small chunk of the count with its neighbours, not from a fetch of its own. Their blobs come at
//! most twice: once in that chunk, once more only when the chunk outlasts `COUNT_WAIT`. The diffs
//! never wait for the counts: while every chunk is held, the folder summary shows its first card's
//! code and a file opened from the tree its diff. The counts arrive chunk by chunk: one held chunk
//! let through adds its files while the other still waits. A thread row the pointer rests on has
//! its file's diff loaded ahead, so the click on it shows the diff at once.
//! `cargo test -p review-app --bin CodeView count_priority`

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{AccessKitNode, NodeT, Queryable};
use review_store::FilesStart;

use super::ReviewApp;
use super::github_mock::{
    ExtraFile, FILE, GitHubHost, MockThread, REPOSITORY, THREAD_TEXT, open_pull_request,
};
use super::walkthrough::{
    app, files_tab, has_cached_diff, open_in_tree, settle_background, shows_file_view,
    shows_folder_view, wait_for, wait_until, wait_until_unsettled,
};
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

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
            ids.push(cv_testkit::git(&hosted, &["rev-parse", &format!("{version}:{path}")]));
        }
    }
    ids
}

/// The app's mirror of the repository.
fn mirror_dir(scratch: &ScratchDir) -> PathBuf {
    let paths = scratch.app_paths();
    let mirrors = std::fs::read_dir(paths.mirrors_dir()).expect("the mirrors folder");
    mirrors.flatten().map(|entry| entry.path()).find(|path| path.is_dir()).expect("a mirror")
}

/// Every fetch into the partial clone leaves one promisor pack.
fn promisor_packs(mirror: &Path) -> Vec<PathBuf> {
    let packs = std::fs::read_dir(mirror.join("objects/pack")).expect("the pack folder");
    packs
        .flatten()
        .map(|entry| entry.path())
        .filter(|pack| pack.extension().is_some_and(|extension| extension == "promisor"))
        .collect()
}

/// The blobs of each fetch, in fetch order is not known, once per pack they came in.
fn fetched_blobs(mirror: &Path) -> Vec<Vec<String>> {
    let mut fetches = Vec::new();
    for pack in promisor_packs(mirror) {
        let index = pack.with_extension("idx").display().to_string();
        let listed = cv_testkit::git(mirror, &["verify-pack", "-v", &index]);
        let blobs = listed.lines().filter(|line| line.contains(" blob "));
        fetches.push(blobs.filter_map(|line| line.get(..40)).map(str::to_owned).collect());
    }
    fetches
}

/// Types `text` into the file tree's filter, so its rows fit on screen.
fn filter_tree(harness: &mut Harness<'_, ReviewApp>, text: &str) {
    let is_filter = |node: &AccessKitNode<'_>| {
        node.placeholder().is_some_and(|hint| hint.ends_with("Filter files"))
    };
    wait_until(harness, "the file tree's filter", |harness| harness.query_by(is_filter).is_some());
    harness.get_by(is_filter).focus();
    harness.step();
    harness.get_by(is_filter).type_text(text);
    harness.step();
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
    wait_until_unsettled(&mut harness, "the first chunks to go out", |_| {
        held.waiting() >= PARALLEL_CHUNKS
    });
    let mirror = mirror_dir(&scratch);
    let opened = DEEP[1];
    filter_tree(&mut harness, "zz/");
    wait_until(&mut harness, "the deep folder's rows", |harness| {
        harness.query_by_role_and_label(Role::TreeItem, opened).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, opened).rect());
    wait_until(&mut harness, "the deep file's diff", |harness| shows_file_view(harness, opened));
    // The neighbours (the previous and the next file of its folder) came with it, as wanted
    // files of the count: a fetch of the opened file's own would bring its two blobs alone.
    let neighbours = blob_ids(&scratch, &[DEEP[0], DEEP[2]]);
    let fetched = loop {
        let packs = promisor_packs(&mirror).len();
        let fetched: HashSet<String> = fetched_blobs(&mirror).into_iter().flatten().collect();
        if neighbours.iter().all(|id| fetched.contains(id)) {
            break fetched;
        }
        wait_until_unsettled(&mut harness, "the next fetch into the mirror", |_| {
            promisor_packs(&mirror).len() > packs
        });
    };
    assert!(fetched.is_superset(&blob_ids(&scratch, &[opened]).into_iter().collect()));
    assert!(harness.query_by_label(COUNTING).is_some(), "the held chunks still count");

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
    let (mut frames, mut skeleton) = (0, false);
    wait_for(
        harness,
        &format!("{path}'s diff"),
        |harness| {
            frames += 1;
            skeleton |= harness.query_by_label("Loading the diff").is_some();
            shows_file_view(harness, path).then_some(())
        },
        |_| {},
    );
    (frames, skeleton)
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
        shows_file_view(harness, "a/first.rs")
    });
    wait_until(&mut harness, "the thread rows", |harness| {
        thread_row(harness, THREAD_TEXT).is_some() && thread_row(harness, FAR_THREAD).is_some()
    });
    settle_background(&mut harness);

    // Not rested on: src/lib.rs's diff loads after the click. How many frames that takes is the
    // machine's speed (a local git diff can land in the click's next frame), so the cache tells.
    assert!(!has_cached_diff(&harness, FILE), "{FILE} was not loaded before its row was clicked");
    click_row_until_shown(&mut harness, THREAD_TEXT, FILE);
    // Rested on: web/far.rs's diff is in the cache when the click lands.
    assert!(!has_cached_diff(&harness, FAR), "{FAR} was not loaded before the pointer rested");
    let row = thread_row(&harness, FAR_THREAD).expect("the far thread's row").rect();
    harness.hover_at(row.center());
    for _ in 0..10 {
        harness.step();
        cv_testkit::let_threads_run(Duration::from_millis(20));
    }
    settle_background(&mut harness);
    assert!(has_cached_diff(&harness, FAR), "the resting pointer loaded {FAR} ahead");
    let (warm_frames, warm_skeleton) = click_row_until_shown(&mut harness, FAR_THREAD, FAR);
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

    // The diffs never wait for the counts: a folder summary card draws its code with the file
    // view's diff view, under the same label, and a file opened from the tree shows its diff.
    wait_until(&mut harness, "the first card's code", |harness| {
        shows_folder_view(harness) && harness.query_by_label(&format!("Diff of {FILE}")).is_some()
    });
    assert!(harness.query_by_label(COUNTING).is_some(), "the first card showed while counting");
    let opened = "src/part_03.rs";
    open_in_tree(&mut harness, opened);
    wait_until(&mut harness, "the opened file's diff", |harness| shows_file_view(harness, opened));
    assert!(harness.query_by_label(COUNTING).is_some(), "the opened file showed while counting");

    held.let_through(1);
    // Either chunk: a summary card's diff or the opened file may have brought a few blobs of
    // their own.
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
