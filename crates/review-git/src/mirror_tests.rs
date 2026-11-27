//! End-to-end: a local "remote" repository served over file://, mirrored blob-less, then diffed.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use review_core::{FileCategory, GitRemote, ProviderError};

use crate::test_repo::{Scratch, file_remote, git, numbered_lines, write};
use crate::{
    ChangeKind, ChangedFile, DiffOptions, EdgeChange, FULL_FILE_CONTEXT, FileDiff, GitEvent,
    GitMirror, LineKind, WhitespaceSummary, local_ref_name,
};

/// `main` with a source file and a doc; `feature` changes the source, adds a test and renames the
/// doc.
struct FeatureBranch {
    scratch: Scratch,
    origin: PathBuf,
    head: String,
    target: String,
    /// The source file at the head.
    changed: String,
}

fn feature_branch(name: &str) -> FeatureBranch {
    let scratch = Scratch::new(name);
    let origin = scratch.origin();
    let original = numbered_lines("", 50);
    write(&origin, "src/lib.rs", &original);
    write(
        &origin,
        "docs/old-name.md",
        "# Title\n\nSame text that stays identical across the rename.\n",
    );
    git(&origin, &["add", "-A"]);
    git(&origin, &["commit", "-q", "-m", "base"]);

    git(&origin, &["switch", "-q", "-c", "feature"]);
    let changed = original.replace("line 10\n", "line ten\nline ten and a half\n");
    write(&origin, "src/lib.rs", &changed);
    write(&origin, "tests/lib_test.rs", "#[test]\nfn works() {}\n");
    git(&origin, &["mv", "docs/old-name.md", "docs/new-name.md"]);
    git(&origin, &["add", "-A"]);
    git(&origin, &["commit", "-q", "-m", "feature work"]);
    let head = git(&origin, &["rev-parse", "HEAD"]);
    let target = git(&origin, &["rev-parse", "main"]);
    FeatureBranch { scratch, origin, head, target, changed }
}

#[tokio::test]
async fn mirrors_and_diffs_a_feature_branch() {
    let fixture = feature_branch("roundtrip");
    let (head, target) = (fixture.head.as_str(), fixture.target.as_str());
    let cache = fixture.scratch.0.join("cache");
    let remote = file_remote(&fixture.origin);
    let mirror = GitMirror::open(&cache, &remote).await.unwrap();
    fetches_with_progress_and_reports_gone_refs(&mirror, head, target).await;

    let merge_base = mirror.merge_base(target, head).await.unwrap();
    assert_eq!(merge_base, target);
    assert!(GitMirror::exists(&cache, &remote));
    let source = lists_changed_files_without_and_with_blobs(&mirror, &merge_base, head).await;
    diffs_and_reads_blobs(&mirror, &merge_base, head, &source, &fixture.changed).await;
    finds_files_in_trees(&mirror, &fixture, &source).await;

    let commits = mirror.commits(&merge_base, head).await.unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].subject, "feature work");
}

async fn fetches_with_progress_and_reports_gone_refs(mirror: &GitMirror, head: &str, target: &str) {
    let refs = ["refs/heads/feature", "refs/heads/main", "refs/heads/missing"].map(str::to_owned);
    let updates = Mutex::new(Vec::new());
    let sink = |event: GitEvent| updates.lock().expect("not poisoned").push(event);
    let gone = mirror.ensure_commits(&refs, &[head, target], &sink).await.expect("fetched");
    let events = updates.lock().expect("not poisoned").clone();
    assert!(
        matches!(events.first(), Some(GitEvent::Command(command)) if command.starts_with("git fetch --progress")),
        "the fetch command comes first: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(event, GitEvent::Progress(_))),
        "git fetch --progress reports at least one phase: {events:?}"
    );
    assert_eq!(gone, ["refs/heads/missing"], "a ref the remote does not have is reported as gone");
    assert_eq!(
        git(mirror.git_dir(), &["rev-parse", &local_ref_name("refs/heads/feature")]),
        head,
        "the fetch wrote the ref where callers look"
    );
}

/// The tree-only list first, then the detailed one that downloads the blobs; returns the changed
/// source file.
async fn lists_changed_files_without_and_with_blobs(
    mirror: &GitMirror,
    merge_base: &str,
    head: &str,
) -> ChangedFile {
    let fast = mirror.changed_files_fast(merge_base, head).await.expect("listed");
    assert_eq!(fast.len(), 4, "rename shows as delete + add without rename detection: {fast:?}");
    let blobless = fast.iter().find(|file| file.path == "src/lib.rs").expect("listed");
    assert!(
        mirror.local_file_diff(merge_base, head, blobless, DiffOptions::context(3)).await.is_err(),
        "a local diff fails on blobs the mirror does not have rather than fetching them"
    );
    assert_eq!(
        mirror.local_changed_files(merge_base, head).await.expect("asked"),
        None,
        "counting locally never fetches the blobs"
    );

    let detailed = mirror.changed_files_detailed(merge_base, head, &|_| {}).await.expect("listed");
    assert_eq!(
        mirror.local_changed_files(merge_base, head).await.expect("asked").as_ref(),
        Some(&detailed),
        "once the blobs are local, the local count is the detailed file list"
    );
    assert_eq!(detailed.len(), 3, "{detailed:?}");
    let renamed = detailed.iter().find(|file| file.kind == ChangeKind::Renamed).expect("renamed");
    assert_eq!(renamed.old_path.as_deref(), Some("docs/old-name.md"));
    let test_file = detailed.iter().find(|file| file.path == "tests/lib_test.rs").expect("added");
    assert_eq!(test_file.category, FileCategory::Tests);
    let source = detailed.iter().find(|file| file.path == "src/lib.rs").expect("changed");
    assert_eq!((source.additions, source.deletions), (Some(2), Some(1)));
    source.clone()
}

async fn diffs_and_reads_blobs(
    mirror: &GitMirror,
    merge_base: &str,
    head: &str,
    source: &ChangedFile,
    changed: &str,
) {
    let three_lines = DiffOptions::context(3);
    let diff = mirror.file_diff(merge_base, head, source, three_lines).await.expect("diffed");
    assert_eq!(diff.count(LineKind::Added), 2);
    assert_eq!(diff.count(LineKind::Deleted), 1);
    let whole_file = DiffOptions::context(FULL_FILE_CONTEXT);
    let full = mirror.file_diff(merge_base, head, source, whole_file).await.expect("diffed");
    assert_eq!(full.count(LineKind::Context), 49);
    let local = mirror.local_file_diff(merge_base, head, source, DiffOptions::context(3)).await;
    assert_eq!(local.ok(), Some(diff), "once the blobs are local, the local diff is the same");

    let blob = mirror.blob(&source.new_oid).await.expect("read");
    assert_eq!(String::from_utf8(blob).expect("text"), changed);
}

/// A rendered README's images: found by path in a commit's tree, without a download.
async fn finds_files_in_trees(mirror: &GitMirror, fixture: &FeatureBranch, source: &ChangedFile) {
    let (head, target) = (fixture.head.as_str(), fixture.target.as_str());
    let entry = mirror.file_entry(head, "src/lib.rs").await.expect("read").expect("in the tree");
    assert_eq!(entry.oid, source.new_oid);
    assert_eq!(
        mirror.file_entry(head, "docs/old-name.md").await.expect("read"),
        None,
        "renamed away at the head"
    );
    assert!(mirror.file_entry(target, "docs/old-name.md").await.expect("read").is_some());
    let folder = mirror.file_entry(head, "src").await.expect("read");
    assert_eq!(folder, None, "a folder is not a file");
    assert!(
        mirror.file_entry(head, "../outside.png").await.is_err(),
        "paths stay inside the repository"
    );
}

/// A comment written on a head that a force push replaced: no ref reaches that head any more, so
/// the mirror fetches it by id on demand, and the file reads as it was then. A commit the remote
/// does not have fails with a message.
#[tokio::test]
async fn fetches_a_force_pushed_head_by_id_and_reads_the_file_as_it_was() {
    let scratch = Scratch::new("origin-version");
    let origin = scratch.origin();
    let original = numbered_lines("", 20);
    write(&origin, "src/lib.rs", &original);
    git(&origin, &["add", "-A"]);
    git(&origin, &["commit", "-q", "-m", "base"]);
    let base = git(&origin, &["rev-parse", "HEAD"]);

    git(&origin, &["switch", "-q", "-c", "feature"]);
    let commented = original.replace("line 7\n", "line seven\n");
    write(&origin, "src/lib.rs", &commented);
    git(&origin, &["commit", "-q", "-am", "first try"]);
    let old_head = git(&origin, &["rev-parse", "HEAD"]);
    // The force push: the branch starts over from main with another change.
    git(&origin, &["reset", "-q", "--hard", "main"]);
    write(&origin, "src/lib.rs", original.replace("line 9\n", "line nine\n"));
    git(&origin, &["commit", "-q", "-am", "second try"]);
    let new_head = git(&origin, &["rev-parse", "HEAD"]);

    let mirror = GitMirror::open(&scratch.0.join("cache"), &file_remote(&origin)).await.unwrap();
    let refs = vec!["refs/heads/feature".to_owned(), "refs/heads/main".to_owned()];
    mirror.ensure_commits(&refs, &[&new_head, &base], &|_| {}).await.unwrap();
    assert!(mirror.commit(&old_head).await.is_err(), "no fetched ref reaches the replaced head");

    let fetches = Mutex::new(0);
    let sink = |event: GitEvent| {
        if matches!(event, GitEvent::Command(_)) {
            *fetches.lock().unwrap() += 1;
        }
    };
    mirror.ensure_commits_by_id(&[&base, &old_head], &sink).await.unwrap();
    assert_eq!(*fetches.lock().unwrap(), 1, "one fetch by id");
    mirror.ensure_commits_by_id(&[&base, &old_head], &sink).await.unwrap();
    assert_eq!(*fetches.lock().unwrap(), 1, "nothing to fetch once the commit is there");
    let info = mirror.commit(&old_head).await.unwrap();
    assert_eq!((info.sha.as_str(), info.subject.as_str()), (old_head.as_str(), "first try"));

    let files = mirror.changed_files_fast(&base, &old_head).await.unwrap();
    let file = files.iter().find(|file| file.path == "src/lib.rs").expect("the commented file");
    let blob = mirror.blob(&file.new_oid).await.unwrap();
    assert_eq!(String::from_utf8(blob).unwrap(), commented, "the file as the comment saw it");
    let diff = mirror.file_diff(&base, &old_head, file, DiffOptions::context(3)).await.unwrap();
    let added: Vec<(u32, &str)> = diff
        .lines()
        .iter()
        .filter(|line| line.kind == LineKind::Added)
        .map(|line| (line.new_line, diff.text(line)))
        .collect();
    assert_eq!(added, [(7, "line seven")]);

    let missing = "0123456789abcdef0123456789abcdef01234567";
    let error = mirror.ensure_commits_by_id(&[missing], &|_| {}).await.unwrap_err();
    assert!(
        matches!(&error, ProviderError::Local(message) if message.starts_with("git failed")),
        "{error}"
    );
}

/// A mirror opened again finds its settings in its config and spawns no `git config`; a clone URL
/// that changed but names the same folder is written again, and the mirror fetches from it.
#[tokio::test]
async fn reopening_a_mirror_writes_its_config_only_when_the_clone_url_moved() {
    let fixture = feature_branch("reopen");
    let cache = fixture.scratch.0.join("cache");
    let remote = file_remote(&fixture.origin);
    let mirror = GitMirror::open(&cache, &remote).await.unwrap();
    let config = mirror.git_dir().join("config");
    // git writes `config.lock` and renames it over `config`: any write gives the file a new time.
    let long_ago = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    let modified = || std::fs::metadata(&config).unwrap().modified().unwrap();
    std::fs::File::options().write(true).open(&config).unwrap().set_modified(long_ago).unwrap();
    assert_eq!(modified(), long_ago);

    GitMirror::open(&cache, &remote).await.unwrap();
    assert_eq!(modified(), long_ago, "opening an existing mirror wrote its config");

    let moved =
        GitRemote { clone_url: format!("{}/", remote.clone_url), authorization_header: None };
    let reopened = GitMirror::open(&cache, &moved).await.unwrap();
    assert_eq!(reopened.git_dir(), mirror.git_dir(), "a trailing slash names the same mirror");
    assert_ne!(modified(), long_ago);
    let setting = |name: &str| git(mirror.git_dir(), &["config", "--get", name]);
    assert_eq!(setting("remote.origin.url"), moved.clone_url);
    assert_eq!(
        [setting("remote.origin.promisor"), setting("remote.origin.partialclonefilter")],
        ["true", "blob:none"]
    );
    let (head, target) = (fixture.head.as_str(), fixture.target.as_str());
    let refs = vec!["refs/heads/feature".to_owned(), "refs/heads/main".to_owned()];
    reopened.ensure_commits(&refs, &[head, target], &|_| {}).await.unwrap();
    assert!(reopened.local_changed_files(target, head).await.unwrap().is_none(), "no blobs yet");
    let missing = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(
        reopened.local_changed_files(missing, head).await.unwrap(),
        None,
        "one commit of two missing is missing"
    );
}

/// One file per kind of whitespace-only change, a real change and attributes that must not
/// apply: `(path, base, head)`.
const WHITESPACE_CASES: [(&str, &[u8], &[u8]); 8] = [
    ("eol.txt", b"one\ntwo\nthree\n", b"one\r\ntwo\r\nthree\r\n"),
    ("mixed.txt", b"one\r\ntwo\nthree\r\nfour\n", b"one\ntwo\nthree\nfour\n"),
    ("eof.txt", b"one\ntwo", b"one\ntwo\n"),
    ("trailing.txt", b"one\ntwo\n", b"one  \ntwo\t\n"),
    ("tabs.txt", b"fn a() {\n\tb();\n}\n", b"fn a() {\n    b();\n}\n"),
    ("bom.txt", b"\xEF\xBB\xBFone\ntwo\n", b"one\ntwo\n"),
    ("real.txt", b"one\ntwo\n", b"one\r\nTWO\n"),
    (".gitattributes", b"* -text\n*.txt -diff\n", b"* -text\n*.txt -diff\n"),
];

/// The diffs of one pull request's files, as the tests ask for them.
struct Diffs<'a> {
    mirror: &'a GitMirror,
    files: &'a [ChangedFile],
    target: &'a str,
    head: &'a str,
}

impl Diffs<'_> {
    async fn of(&self, path: &str, ignore_whitespace: bool) -> FileDiff {
        let file = self.files.iter().find(|file| file.path == path).expect("listed");
        let options = DiffOptions { context: 3, ignore_whitespace };
        self.mirror.file_diff(self.target, self.head, file, options).await.expect("diffed")
    }

    /// What changed in a file whose only change is whitespace.
    async fn summary(&self, path: &str) -> WhitespaceSummary {
        self.of(path, false).await.whitespace().copied().expect("only whitespace changed")
    }
}

/// Files whose only change is whitespace, a line ending, the final newline or the BOM: each is
/// listed as whitespace-only, its diff has real changed lines with a summary of what changed, and
/// with whitespace ignored it has no changes (but for the BOM, which is not whitespace). The
/// repository's own attributes (`eol`, `-diff`) never change how blobs compare.
#[tokio::test]
async fn whitespace_only_changes_are_changes() {
    let scratch = Scratch::new("whitespace");
    let origin = scratch.origin();
    for (path, base, _) in WHITESPACE_CASES {
        write(&origin, path, base);
    }
    git(&origin, &["add", "-A"]);
    git(&origin, &["commit", "-q", "-m", "base"]);
    git(&origin, &["switch", "-q", "-c", "feature"]);
    for (path, _, head) in WHITESPACE_CASES {
        write(&origin, path, head);
    }
    git(&origin, &["add", "-A"]);
    git(&origin, &["commit", "-q", "-m", "whitespace"]);
    let head = git(&origin, &["rev-parse", "HEAD"]);
    let target = git(&origin, &["rev-parse", "main"]);

    let mirror = GitMirror::open(&scratch.0.join("cache"), &file_remote(&origin)).await.unwrap();
    let refs = vec!["refs/heads/feature".to_owned(), "refs/heads/main".to_owned()];
    mirror.ensure_commits(&refs, &[&head, &target], &|_| {}).await.unwrap();

    let files = mirror.changed_files_detailed(&target, &head, &|_| {}).await.unwrap();
    let marked: Vec<(&str, bool)> =
        files.iter().map(|file| (file.path.as_str(), file.whitespace_only)).collect();
    assert_eq!(
        marked,
        [
            ("bom.txt", false),
            ("eof.txt", true),
            ("eol.txt", true),
            ("mixed.txt", true),
            ("real.txt", false),
            ("tabs.txt", true),
            ("trailing.txt", true)
        ],
        "the BOM is not whitespace to git, so only its diff tells"
    );
    let diffs = Diffs { mirror: &mirror, files: &files, target: &target, head: &head };
    summarizes_each_kind(&diffs).await;
    ignoring_whitespace_leaves_only_real_changes(&diffs).await;
}

async fn summarizes_each_kind(diffs: &Diffs<'_>) {
    let eol = diffs.of("eol.txt", false).await;
    assert!(
        !eol.is_binary,
        "the repository's -diff attribute does not apply to the mirror's diffs"
    );
    assert_eq!(
        (eol.count(LineKind::Deleted), eol.count(LineKind::Added)),
        (3, 3),
        "a new line ending is a changed line"
    );
    let summary = eol.whitespace().copied().expect("only line endings changed");
    assert_eq!((summary.to_crlf, summary.lines, summary.whole_file), (3, 3, true));

    let mixed = diffs.summary("mixed.txt").await;
    assert_eq!((mixed.to_lf, mixed.to_crlf, mixed.whole_file), (2, 0, false));
    assert_eq!(diffs.summary("eof.txt").await.final_newline, Some(EdgeChange::Added));
    assert_eq!(diffs.summary("trailing.txt").await.trailing, 2);
    assert_eq!(diffs.summary("tabs.txt").await.tabs_to_spaces, 1);
    assert_eq!(diffs.summary("bom.txt").await.bom, Some(EdgeChange::Removed));
    assert!(
        diffs.of("real.txt", false).await.whitespace().is_none(),
        "a real change among whitespace ones"
    );
}

async fn ignoring_whitespace_leaves_only_real_changes(diffs: &Diffs<'_>) {
    for path in ["eol.txt", "mixed.txt", "eof.txt", "trailing.txt", "tabs.txt"] {
        assert!(diffs.of(path, true).await.is_empty(), "{path} has no changes ignoring whitespace");
    }
    assert!(
        !diffs.of("bom.txt", true).await.is_empty(),
        "the BOM still shows with whitespace ignored"
    );
    let real = diffs.of("real.txt", true).await;
    assert_eq!(
        (real.count(LineKind::Deleted), real.count(LineKind::Added)),
        (1, 1),
        "only the real change is left"
    );
}

/// The assistant's checkout: a detached tree of the head whose blobs come from the remote, moved
/// in place to a newer head (a running agent keeps its working directory), and added again after
/// its folder was deleted while the mirror still listed it.
#[tokio::test]
async fn checks_out_a_detached_tree_and_moves_it_in_place() {
    let scratch = Scratch::new("checkout");
    let origin = scratch.origin();
    write(&origin, "src/lib.rs", "first\n");
    git(&origin, &["add", "-A"]);
    git(&origin, &["commit", "-q", "-m", "first"]);
    let first = git(&origin, &["rev-parse", "HEAD"]);
    write(&origin, "src/lib.rs", "second\n");
    git(&origin, &["commit", "-q", "-am", "second"]);
    let second = git(&origin, &["rev-parse", "HEAD"]);

    let mirror = GitMirror::open(&scratch.0.join("cache"), &file_remote(&origin)).await.unwrap();
    let refs = vec!["refs/heads/main".to_owned()];
    mirror.ensure_commits(&refs, &[&first, &second], &|_| {}).await.unwrap();
    let dest = scratch.0.join("assistant").join("pr-1");
    let read = |dest: &Path| std::fs::read_to_string(dest.join("src/lib.rs")).unwrap();

    assert_eq!(mirror.checkout_detached(&first, &dest).await.unwrap(), dest);
    assert_eq!(read(&dest), "first\n", "the blob came from the remote");
    assert_eq!(git(&dest, &["rev-parse", "HEAD"]), first);

    let marker = dest.join("agent-was-here.txt");
    write(&dest, "agent-was-here.txt", "kept");
    mirror.checkout_detached(&second, &dest).await.unwrap();
    assert_eq!(read(&dest), "second\n");
    assert_eq!(git(&dest, &["rev-parse", "HEAD"]), second);
    assert!(marker.exists(), "the folder moved in place instead of being replaced");
    assert_eq!(git(mirror.git_dir(), &["rev-parse", "--is-bare-repository"]), "true");

    std::fs::remove_dir_all(&dest).unwrap();
    mirror.checkout_detached(&first, &dest).await.unwrap();
    assert_eq!(read(&dest), "first\n", "a stale registration is pruned and the tree added again");
}

/// Retention or "Clear cache" deleted the mirror while the assistant's checkout stayed: the
/// checkout's `.git` file points into a folder that is gone, and the mirror made anew adds the
/// tree again in the same folder.
#[tokio::test]
async fn a_checkout_whose_mirror_was_made_anew_is_added_again() {
    let scratch = Scratch::new("checkout-new-mirror");
    let origin = scratch.origin();
    write(&origin, "src/lib.rs", "first\n");
    git(&origin, &["add", "-A"]);
    git(&origin, &["commit", "-q", "-m", "first"]);
    let first = git(&origin, &["rev-parse", "HEAD"]);
    let cache = scratch.0.join("cache");
    let refs = vec!["refs/heads/main".to_owned()];
    let dest = scratch.0.join("assistant").join("pr-1");

    let mirror = GitMirror::open(&cache, &file_remote(&origin)).await.unwrap();
    mirror.ensure_commits(&refs, &[&first], &|_| {}).await.unwrap();
    mirror.checkout_detached(&first, &dest).await.unwrap();
    let git_dir = mirror.git_dir().to_path_buf();
    drop(mirror);
    std::fs::remove_dir_all(&git_dir).unwrap();

    let mirror = GitMirror::open(&cache, &file_remote(&origin)).await.unwrap();
    mirror.ensure_commits(&refs, &[&first], &|_| {}).await.unwrap();
    mirror.checkout_detached(&first, &dest).await.expect("checked out again");
    assert_eq!(std::fs::read_to_string(dest.join("src/lib.rs")).unwrap(), "first\n");
    assert_eq!(git(&dest, &["rev-parse", "HEAD"]), first);
}

/// A file name with glob characters names only itself: `a[b].txt` is no pattern that also
/// matches `ab.txt`, whose lines would land in the other file's diff.
#[tokio::test]
async fn a_diff_of_a_path_with_glob_characters_shows_only_that_file() {
    let scratch = Scratch::new("glob-path");
    let origin = scratch.origin();
    write(&origin, "a[b].txt", "one\n");
    write(&origin, "ab.txt", "uno\n");
    git(&origin, &["add", "-A"]);
    git(&origin, &["commit", "-q", "-m", "base"]);
    let base = git(&origin, &["rev-parse", "HEAD"]);
    write(&origin, "a[b].txt", "two\n");
    write(&origin, "ab.txt", "dos\n");
    git(&origin, &["commit", "-q", "-am", "change"]);
    let head = git(&origin, &["rev-parse", "HEAD"]);

    let mirror = GitMirror::open(&scratch.0.join("cache"), &file_remote(&origin)).await.unwrap();
    let refs = vec!["refs/heads/main".to_owned()];
    mirror.ensure_commits(&refs, &[&base, &head], &|_| {}).await.unwrap();
    let files = mirror.changed_files_fast(&base, &head).await.unwrap();
    let file = files.iter().find(|file| file.path == "a[b].txt").expect("listed");
    let diff = mirror.file_diff(&base, &head, file, DiffOptions::context(3)).await.unwrap();
    let changed: Vec<&str> = diff
        .lines()
        .iter()
        .filter(|line| line.kind != LineKind::Hunk)
        .map(|line| diff.text(line))
        .collect();
    assert_eq!(changed, ["one", "two"]);
}

/// Two repositories whose clone URLs differ only where the folder name has an underscore (a `/`
/// in one, a `_` in the other) get a mirror each: one shared folder would fetch both into one
/// repository and lazily fetch one's blobs from the other's remote.
#[tokio::test]
async fn clone_urls_that_sanitize_alike_get_their_own_mirrors() {
    let scratch = Scratch::new("mirror-names");
    let make_origin = |relative: &str, content: &str| {
        let origin = scratch.0.join(relative);
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["config", "uploadpack.allowFilter", "true"]);
        write(&origin, "README.md", content);
        git(&origin, &["add", "-A"]);
        git(&origin, &["commit", "-q", "-m", "only"]);
        let head = git(&origin, &["rev-parse", "HEAD"]);
        (origin, head)
    };
    let (nested, nested_head) = make_origin("acme/web_app", "nested\n");
    let (flat, flat_head) = make_origin("acme_web/app", "flat\n");
    let cache = scratch.0.join("cache");
    let refs = vec!["refs/heads/main".to_owned()];

    let first = GitMirror::open(&cache, &file_remote(&nested)).await.unwrap();
    first.ensure_commits(&refs, &[&nested_head], &|_| {}).await.unwrap();
    let second = GitMirror::open(&cache, &file_remote(&flat)).await.unwrap();
    assert_ne!(first.git_dir(), second.git_dir(), "one folder for two repositories");
    second.ensure_commits(&refs, &[&flat_head], &|_| {}).await.unwrap();
    assert!(GitMirror::exists(&cache, &file_remote(&flat)));

    // The blobs download now, each from its own remote.
    let entry = first.file_entry(&nested_head, "README.md").await.unwrap().expect("in the tree");
    assert_eq!(String::from_utf8(first.blob(&entry.oid).await.unwrap()).unwrap(), "nested\n");
    let reopened = GitMirror::open(&cache, &file_remote(&nested)).await.unwrap();
    assert_eq!(reopened.git_dir(), first.git_dir(), "the first repository keeps its folder");
    let entry = second.file_entry(&flat_head, "README.md").await.unwrap().expect("in the tree");
    assert_eq!(String::from_utf8(second.blob(&entry.oid).await.unwrap()).unwrap(), "flat\n");
}
