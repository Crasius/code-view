use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::*;
use crate::test_repo::{Scratch, file_remote, git, numbered_lines, write};
use crate::{DiffOptions, GitMirror};

/// Files changed in the fixture besides the [`MANY`] modified ones.
const MANY: usize = 70;

/// An origin whose `feature` branch changes [`MANY`] files (several chunks) and has every other
/// kind of change: an inexact rename whose new path sorts first, an exact one, an unpaired added
/// and deleted file, a binary file, a whitespace-only change, a mode change and an empty file.
/// Returns the origin and its (base, head) commits.
fn fixture(scratch: &Scratch) -> (PathBuf, String, String) {
    let origin = scratch.origin();
    for index in 0..MANY {
        write(
            &origin,
            &format!("src/many/f{index:02}.rs"),
            numbered_lines(&format!("f{index} "), 30),
        );
    }
    write(&origin, "z/old_name.md", numbered_lines("renamed ", 40));
    write(&origin, "docs/exact.txt", numbered_lines("exact ", 10));
    write(&origin, "gone.txt", numbered_lines("gone ", 12));
    write(&origin, "logo.png", [0u8, 1, 2, 3, 0, 255]);
    write(&origin, "ws.txt", numbered_lines("ws ", 5));
    write(&origin, "run.sh", "echo run\n");
    git(&origin, &["add", "-A"]);
    git(&origin, &["commit", "-q", "-m", "base"]);
    let base = git(&origin, &["rev-parse", "HEAD"]);

    git(&origin, &["switch", "-q", "-c", "feature"]);
    for index in 0..MANY {
        let text =
            numbered_lines(&format!("f{index} "), 30).replace(" line 3\n", " line three\nmore\n");
        write(&origin, &format!("src/many/f{index:02}.rs"), text);
    }
    std::fs::remove_file(origin.join("z/old_name.md")).expect("remove");
    write(
        &origin,
        "a/new_name.md",
        numbered_lines("renamed ", 40).replace(" line 7\n", " line seven\n"),
    );
    std::fs::remove_file(origin.join("docs/exact.txt")).expect("remove");
    write(&origin, "docs/moved/exact.txt", numbered_lines("exact ", 10));
    std::fs::remove_file(origin.join("gone.txt")).expect("remove");
    write(&origin, "added.rs", numbered_lines("added ", 8));
    write(&origin, "empty.txt", "");
    write(&origin, "logo.png", [0u8, 1, 2, 4, 0, 255]);
    write(&origin, "ws.txt", numbered_lines("ws ", 5).replace('\n', "\r\n"));
    git(&origin, &["add", "-A"]);
    git(&origin, &["update-index", "--chmod=+x", "run.sh"]);
    git(&origin, &["commit", "-q", "-m", "feature"]);
    let head = git(&origin, &["rev-parse", "HEAD"]);
    (origin, base, head)
}

async fn mirror(scratch: &Scratch, name: &str, origin: &Path, commits: [&str; 2]) -> GitMirror {
    let mirror =
        GitMirror::open(&scratch.0.join(name), &file_remote(origin)).await.expect("mirror");
    let refs = ["refs/heads/main", "refs/heads/feature"].map(str::to_owned);
    mirror.ensure_commits(&refs, &commits, &|_| {}).await.expect("fetched");
    mirror
}

/// What the file list was before counts streamed: one `diff-tree -M --numstat` that downloads
/// every blob, then the same offline with `-w` for the whitespace-only marks.
async fn one_shot(mirror: &GitMirror, base: &str, head: &str) -> Vec<ChangedFile> {
    let git = GitCommand::new(mirror.git_dir(), None);
    let output =
        git.run(["diff-tree", "-r", "-z", "-M", "--raw", "--numstat", base, head]).await.unwrap();
    let mut files =
        changed_files::parse_raw_with_numstat(&String::from_utf8_lossy(&output)).unwrap();
    let ignoring_whitespace = git
        .run_offline(["diff-tree", "-r", "-z", "-M", "--numstat", "-w", base, head])
        .await
        .unwrap();
    changed_files::mark_whitespace_only(&mut files, &String::from_utf8_lossy(&ignoring_whitespace))
        .unwrap();
    files
}

#[tokio::test]
async fn streamed_counts_end_with_the_one_shot_list() {
    let scratch = Scratch::new("counts-stream");
    let (origin, base, head) = fixture(&scratch);
    let expected =
        one_shot(&mirror(&scratch, "once", &origin, [&base, &head]).await, &base, &head).await;
    assert!(expected.iter().any(|file| file.path == "a/new_name.md" && file.old_path.is_some()));
    assert!(expected.iter().any(|file| file.whitespace_only), "ws.txt changed only line endings");

    let streamed = mirror(&scratch, "streamed", &origin, [&base, &head]).await;
    // The user opened the file counted last in tree order: its blobs are local now.
    let opened = streamed
        .changed_files_fast(&base, &head)
        .await
        .unwrap()
        .into_iter()
        .find(|file| file.path == "ws.txt")
        .expect("listed");
    streamed.file_diff(&base, &head, &opened, DiffOptions::context(3)).await.unwrap();
    let published = Mutex::new(Vec::new());
    let publish = |files: &[ChangedFile]| published.lock().unwrap().push(files.to_vec());
    let files = streamed.changed_files_detailed(&base, &head, &publish).await.unwrap();
    assert_eq!(files, expected, "the streamed list ends where one diff-tree -M --numstat does");

    let published = published.into_inner().unwrap();
    assert!(
        published.len() >= 4,
        "local files, then chunks of 16, 32 and more: {}",
        published.len()
    );
    let is_counted = |file: &ChangedFile| file.additions.is_some() || file.is_binary;
    let first = published.first().expect("published");
    let counted: Vec<&str> =
        first.iter().filter(|file| is_counted(file)).map(|file| file.path.as_str()).collect();
    assert!(counted.contains(&"ws.txt"), "the opened file counts before any download: {counted:?}");
    assert!(counted.len() < 4, "only the files with local blobs count first: {counted:?}");
    for snapshot in &published {
        assert_eq!(snapshot.len(), expected.len() + 2, "without renames until the end");
        for file in snapshot.iter().filter(|file| is_counted(file)) {
            let known = expected.iter().find(|known| known.path == file.path);
            if let Some(known) = known.filter(|known| known.old_path.is_none()) {
                assert_eq!(file, known, "a published count is the final one");
            }
        }
    }
    let last = published.last().expect("published");
    assert!(last.iter().all(is_counted), "the last chunk counted every file");
}

#[tokio::test]
async fn counts_local_blobs_at_once_without_publishing() {
    let scratch = Scratch::new("counts-local");
    let (origin, base, head) = fixture(&scratch);
    let mirror = mirror(&scratch, "mirror", &origin, [&base, &head]).await;
    let expected = one_shot(&mirror, &base, &head).await;
    let published = Mutex::new(0);
    let publish = |_: &[ChangedFile]| *published.lock().unwrap() += 1;
    let files = mirror.changed_files_detailed(&base, &head, &publish).await.unwrap();
    assert_eq!(files, expected);
    assert_eq!(
        published.into_inner().unwrap(),
        0,
        "every blob was local: the result comes at once"
    );
}

fn file(path: &str) -> ChangedFile {
    ChangedFile::sample(path, ChangeKind::Modified)
}

#[test]
fn lists_folders_before_files_at_every_level() {
    let files: Vec<ChangedFile> =
        ["b.rs", "a/z.rs", "a.rs", "a/b/c.rs", "a/b.rs"].into_iter().map(file).collect();
    let order: Vec<&str> =
        tree_order(&files).into_iter().map(|index| files[index].path.as_str()).collect();
    assert_eq!(order, ["a/b/c.rs", "a/b.rs", "a/z.rs", "a.rs", "b.rs"]);
}

#[test]
fn chunks_grow_from_the_first_size_and_take_local_files_first() {
    let files: Vec<ChangedFile> = (0..400).map(|index| file(&format!("f{index:03}.rs"))).collect();
    let mut plan = Plan::new((0..files.len()).collect());
    let local =
        plan.take_local(&files, |file| file.path.ends_with("7.rs") || file.path == "f399.rs");
    assert_eq!(local.concat().len(), 41, "f..7 files and the last one, in one chunk");
    assert_eq!(local.concat().last(), Some(&399));
    let sizes: Vec<usize> =
        std::iter::from_fn(|| plan.take_download(&files)).map(|chunk| chunk.len()).collect();
    assert_eq!(sizes, [16, 32, 64, 128, 119]);
    assert_eq!(download_chunk_sizes(359), sizes, "the published sizes are the plan's");
}

#[test]
fn a_file_of_a_running_download_counts_once_its_blobs_arrived() {
    let files: Vec<ChangedFile> = (0..20).map(|index| file(&format!("f{index:03}.rs"))).collect();
    let mut plan = Plan::new((0..files.len()).collect());
    let chunk = plan.take_download(&files).expect("a chunk");
    assert_eq!(chunk.len(), FIRST_CHUNK);
    let opened = plan.take_local(&files, |file| file.path == "f003.rs");
    assert_eq!(opened, [vec![3]], "the opened file counts without waiting for its chunk");
    plan.downloaded(&chunk);
    assert_eq!(plan.uncounted(), (16..20).collect::<Vec<_>>());
    let last = plan.take_download(&files).expect("the rest");
    let landed = plan.take_local(&files, |_| true);
    assert!(landed.is_empty(), "a download whose blobs all landed is counting them already");
    plan.downloaded(&last);
    assert!(plan.is_counted());
}

#[test]
fn a_chunk_keeps_its_paths_below_the_command_line_budget() {
    let long = "x".repeat(4000);
    let files: Vec<ChangedFile> =
        (0..10).map(|index| file(&format!("{long}/{index}.rs"))).collect();
    let mut plan = Plan::new((0..files.len()).collect());
    let first = plan.take_download(&files).expect("a chunk");
    assert_eq!(first.len(), 4, "four paths of 4,000 bytes fit in {MAX_PATH_BYTES} bytes");
    let local = plan.take_local(&files, |_| true);
    assert_eq!(local.iter().map(Vec::len).collect::<Vec<_>>(), [4, 2]);
}

#[test]
fn wanted_files_make_the_next_chunk_and_the_first_one_fills_up() {
    let files: Vec<ChangedFile> = (0..400).map(|index| file(&format!("f{index:03}.rs"))).collect();
    let mut plan = Plan::new((0..files.len()).collect());
    assert_eq!(plan.take_wanted(&files, &[]), None, "nothing wanted, nothing taken");
    let first = plan.take_wanted(&files, &[399, 398]).expect("a wanted chunk");
    assert_eq!(first.len(), FIRST_CHUNK, "the first chunk keeps its size");
    assert_eq!(first[..3], [399, 398, 0], "the wanted files first, then the front");
    let sizes: Vec<usize> =
        std::iter::from_fn(|| plan.take_download(&files)).map(|chunk| chunk.len()).collect();
    assert_eq!(
        sizes.len() + 1,
        download_chunk_sizes(files.len()).len(),
        "wanted files at the start fetch as often as none: {sizes:?}"
    );

    let mut plan = Plan::new((0..files.len()).collect());
    let front = plan.take_download(&files).expect("the first chunk");
    let wanted: Vec<usize> = std::iter::once(3).chain(200..230).collect();
    let chunk = plan.take_wanted(&files, &wanted).expect("a wanted chunk");
    assert_eq!(chunk, (200..216).collect::<Vec<_>>(), "a file in a running chunk is left to it");
    assert_eq!(plan.take_download(&files).map(|chunk| chunk.len()), Some(32), "sizes go on");
    let rest = plan.take_wanted(&files, &wanted).expect("the other wanted files");
    assert_eq!(rest, (216..230).collect::<Vec<_>>(), "only wanted files after the first chunk");
    assert!(plan.take_wanted(&files, &wanted).is_none(), "every wanted file is on its way");
    for chunk in [front, chunk, rest] {
        plan.downloaded(&chunk);
    }
    assert!(!plan.uncounted().contains(&215));
}

#[tokio::test(start_paused = true)]
async fn a_diff_waits_for_the_count_that_has_not_reached_its_file() {
    let wanted = Wanted::default();
    let files = [file("a.rs"), file("b.rs")];
    let run = wanted.start("base", "head");
    run.listed(&files);
    wanted.wait_counted("other", "head", &["a.rs"], Some(COUNT_WAIT)).await;
    wanted.wait_counted("base", "head", &["not-changed.rs"], Some(COUNT_WAIT)).await;

    let waiting = wanted.wait_counted("base", "head", &["b.rs"], Some(COUNT_WAIT));
    tokio::pin!(waiting);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut waiting).await.is_err(),
        "b.rs is not counted yet"
    );
    assert_eq!(wanted.paths(), ["b.rs"], "the file a diff waits for is wanted first");
    run.counted(files[..1].iter());
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut waiting).await.is_err());
    run.counted(files[1..].iter());
    tokio::time::timeout(Duration::from_secs(5), &mut waiting).await.expect("counted: done");
    assert!(wanted.paths().is_empty(), "nobody waits any more");

    let run = wanted.start("base", "head");
    run.listed(&files);
    let waiting = wanted.wait_counted("base", "head", &["a.rs"], Some(COUNT_WAIT));
    tokio::pin!(waiting);
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut waiting).await.is_err());
    drop(run);
    tokio::time::timeout(Duration::from_secs(5), waiting).await.expect("a count gone frees it");
}

/// Regression (counting walkthrough): a chunk that never lands (a slow host) held a diff of its
/// file up for good; the diff fetches the blobs itself after [`COUNT_WAIT`].
#[tokio::test(start_paused = true)]
async fn a_diff_waits_for_a_slow_count_no_longer_than_one_fetch_of_its_own() {
    let wanted = Wanted::default();
    let files = [file("a.rs")];
    let run = wanted.start("base", "head");
    run.listed(&files);
    let started = tokio::time::Instant::now();
    tokio::time::timeout(
        COUNT_WAIT * 5,
        wanted.wait_counted("base", "head", &["a.rs"], Some(COUNT_WAIT)),
    )
    .await
    .expect("the wait ends without the count");
    assert!(started.elapsed() >= COUNT_WAIT, "it waited for the count first");
    assert!(wanted.paths().is_empty(), "the file is not wanted for that diff any more");
}

/// Regression (request budget): a thread excerpt asked for its file while a large pull request's
/// count still listed its files; its wait ran out before the count took the file up, so the
/// excerpt fetched the blobs and the count fetched them again. The wait starts once listed.
#[tokio::test(start_paused = true)]
async fn a_diff_asked_while_the_count_lists_its_files_waits_from_the_listing_on() {
    let wanted = Wanted::default();
    let files = [file("a.rs"), file("b.rs")];
    let run = wanted.start("base", "head");
    let waiting = wanted.wait_counted("base", "head", &["a.rs"], Some(COUNT_WAIT));
    tokio::pin!(waiting);
    assert!(
        tokio::time::timeout(COUNT_WAIT + Duration::from_millis(200), &mut waiting).await.is_err(),
        "still listing: the wait has not started"
    );
    run.listed(&files);
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut waiting).await.is_err());
    assert_eq!(wanted.paths(), ["a.rs"], "listed: the file is wanted first");
    run.counted(files[..1].iter());
    tokio::time::timeout(Duration::from_secs(5), &mut waiting).await.expect("counted: done");

    let unlisted = wanted.start("base", "other");
    let waiting = wanted.wait_counted("base", "other", &["a.rs"], Some(COUNT_WAIT));
    tokio::pin!(waiting);
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut waiting).await.is_err());
    drop(unlisted);
    tokio::time::timeout(Duration::from_secs(5), waiting).await.expect("a failed listing frees it");
}

/// Regression (request budget): the excerpt's diff waited [`COUNT_WAIT`] for its chunk, which a
/// 3,000-file pull request's first chunk takes about as long, and fetched the file a second time.
/// An excerpt waits for the chunk however long it takes; the count's end frees it.
#[tokio::test(start_paused = true)]
async fn an_excerpt_waits_for_its_chunk_past_the_diffs_limit() {
    let wanted = Wanted::default();
    let files = [file("a.rs"), file("b.rs")];
    let run = wanted.start("base", "head");
    run.listed(&files);
    let excerpt = wanted.wait_counted("base", "head", &["a.rs"], None);
    tokio::pin!(excerpt);
    assert!(
        tokio::time::timeout(COUNT_WAIT + Duration::from_millis(200), &mut excerpt).await.is_err(),
        "the chunk has not come: the excerpt still waits"
    );
    run.counted(files[..1].iter());
    tokio::time::timeout(Duration::from_secs(5), &mut excerpt).await.expect("counted: done");

    let excerpt = wanted.wait_counted("base", "head", &["b.rs"], None);
    tokio::pin!(excerpt);
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut excerpt).await.is_err());
    drop(run);
    tokio::time::timeout(Duration::from_secs(5), excerpt).await.expect("the count's end frees it");
}

#[test]
fn feeds_are_replaced_whole_and_wanted_in_their_order() {
    let wanted = Wanted::default();
    let mut changes = wanted.subscribe();
    wanted.feed(WantedBy::Threads, &["t.rs".to_owned()]);
    wanted.feed(WantedBy::Files, &["open.rs".to_owned(), "next.rs".to_owned()]);
    assert_eq!(wanted.paths(), ["open.rs", "next.rs", "t.rs"]);
    assert!(changes.has_changed().unwrap());
    changes.mark_unchanged();
    wanted.feed(WantedBy::Files, &["open.rs".to_owned(), "next.rs".to_owned()]);
    assert!(!changes.has_changed().unwrap(), "the same files again wake no count");
    wanted.feed(WantedBy::Files, &[]);
    assert_eq!(wanted.paths(), ["t.rs"]);
}

/// The blobs of every pack the mirror fetched, once per pack they came in.
fn fetched_blobs(mirror: &GitMirror) -> Vec<String> {
    let packs = std::fs::read_dir(mirror.git_dir().join("objects/pack")).expect("the pack folder");
    let mut blobs = Vec::new();
    for pack in packs.flatten().map(|entry| entry.path()) {
        if pack.extension().is_some_and(|extension| extension == "idx") {
            let listed = git(mirror.git_dir(), &["verify-pack", "-v", &pack.display().to_string()]);
            let ids = listed
                .lines()
                .filter(|line| line.contains(" blob "))
                .map(|line| line[..40].to_owned());
            blobs.extend(ids);
        }
    }
    blobs
}

#[tokio::test]
async fn a_diff_during_the_count_downloads_no_blob_twice() {
    let scratch = Scratch::new("counts-once");
    let (origin, base, head) = fixture(&scratch);
    let mirror = mirror(&scratch, "mirror", &origin, [&base, &head]).await;
    let files = mirror.changed_files_fast(&base, &head).await.unwrap();
    let last = files.iter().find(|file| file.path == "ws.txt").expect("listed").clone();
    let first_chunk = tokio::sync::Notify::new();
    let publish = |_: &[ChangedFile]| first_chunk.notify_one();
    let diff = async {
        // Once the count is on its way, the user opens the file it counts last in tree order.
        first_chunk.notified().await;
        mirror.file_diff(&base, &head, &last, DiffOptions::context(3)).await
    };
    let (counted, diff) = tokio::join!(mirror.changed_files_detailed(&base, &head, &publish), diff);
    counted.unwrap();
    assert!(!diff.unwrap().lines().is_empty(), "the diff came from the blobs the count brought");
    let blobs = fetched_blobs(&mirror);
    let distinct: HashSet<&String> = blobs.iter().collect();
    assert_eq!(blobs.len(), distinct.len(), "a blob came in two fetches");
}
