//! The checkout engine against real repositories in a temp folder: an origin with a pull request,
//! the app's mirror of it, and a clone of the user's whose own remote is unreachable, so every
//! object a checkout gets must come through the mirror.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use review_core::GitRemote;

use super::{
    Blocked, CheckoutError, Event, Finished, FinishedTree, FoundClone, Keep, Made, Removal,
    Request, Step, Target, UpdateOutcome, UpdateRequest, Updated, Upstream, check_out,
    remove_finished, remove_worktree, survey, undo, update,
};
use crate::mirror::GitMirror;

const PULL_REF: &str = "refs/pull/7/head";
const CLONE_PULL_REF: &str = "refs/codeview/pull/7/head";

struct Fixture {
    root: PathBuf,
    origin: PathBuf,
    mirror: GitMirror,
    clone: PathBuf,
    base: String,
    head: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Best effort: a folder left in the temp directory harms nothing.
        let _cleanup_is_best_effort = std::fs::remove_dir_all(&self.root);
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "test driver: builds the fixture repositories with git before the code under test runs"
)]
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.org"])
        .args(["-c", "core.autocrlf=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn write(dir: &Path, path: &str, content: &str) {
    let full = dir.join(path);
    std::fs::create_dir_all(full.parent().expect("a file path has a folder")).expect("mkdir");
    std::fs::write(full, content).expect("write the file");
}

fn read(dir: &Path, path: &str) -> String {
    std::fs::read_to_string(dir.join(path)).expect("the file is there").replace("\r\n", "\n")
}

/// A commit on the pull request's branch in the origin; returns the new head.
fn push(origin: &Path, path: &str, content: &str) -> String {
    git(origin, &["checkout", "-q", "feature"]);
    write(origin, path, content);
    git(origin, &["add", "-A"]);
    git(origin, &["commit", "-q", "-m", &format!("Change {path}")]);
    let head = git(origin, &["rev-parse", "HEAD"]);
    git(origin, &["update-ref", PULL_REF, &head]);
    git(origin, &["checkout", "-q", "main"]);
    head
}

/// The pull request's branch rewritten from `main`: a force-push.
fn force_push(origin: &Path, path: &str, content: &str) -> String {
    git(origin, &["checkout", "-q", "-B", "feature", "main"]);
    write(origin, path, content);
    git(origin, &["add", "-A"]);
    git(origin, &["commit", "-q", "-m", "Rewritten"]);
    let head = git(origin, &["rev-parse", "HEAD"]);
    git(origin, &["update-ref", PULL_REF, &head]);
    git(origin, &["checkout", "-q", "main"]);
    head
}

async fn fixture(name: &str) -> Fixture {
    let root =
        std::env::temp_dir().join(format!("review-git-checkout-{name}-{}", std::process::id()));
    let _stale_folder_may_be_missing = std::fs::remove_dir_all(&root);
    let origin = root.join("origin");
    std::fs::create_dir_all(&origin).expect("origin folder");
    git(&origin, &["init", "-q", "-b", "main"]);
    git(&origin, &["config", "uploadpack.allowFilter", "true"]);
    git(&origin, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
    write(&origin, "src/app.rs", "fn main() {}\n");
    git(&origin, &["add", "-A"]);
    git(&origin, &["commit", "-q", "-m", "Start"]);
    let base = git(&origin, &["rev-parse", "HEAD"]);
    git(&origin, &["branch", "feature"]);
    let clone = root.join("dev").join("app");
    let origin_text = origin.to_string_lossy().into_owned();
    let clone_text = clone.to_string_lossy().into_owned();
    git(&root, &["clone", "-q", "--single-branch", "-b", "main", &origin_text, &clone_text]);
    // The machine's line ending settings must not make a fresh checkout look changed.
    git(&clone, &["config", "core.autocrlf", "false"]);
    // The clone's own remote is out of reach: whatever it gets comes through the mirror.
    git(&clone, &["remote", "set-url", "origin", "https://example.invalid/acme/app.git"]);
    let head = push(&origin, "src/login.rs", "pub fn login() {}\n");

    let remote = GitRemote {
        clone_url: format!("file:///{}", origin.display().to_string().replace('\\', "/")),
        authorization_header: None,
    };
    let mirror = GitMirror::open(&root.join("mirrors"), &remote).await.expect("the mirror opens");
    let fixture = Fixture { root, origin, mirror, clone, base, head };
    fixture.mirror_head().await;
    fixture
}

impl Fixture {
    /// The mirror fetches the pull request's current head, as the page does.
    async fn mirror_head(&self) -> String {
        let head = git(&self.origin, &["rev-parse", PULL_REF]);
        let refs = [PULL_REF.to_owned(), "refs/heads/main".to_owned()];
        self.mirror.ensure_commits(&refs, &[&self.base, &head], &|_| {}).await.expect("mirrored");
        head
    }

    fn request(&self, target: Target, branch: &str) -> Request {
        Request {
            clone: self.clone.clone(),
            target,
            head: self.head.clone(),
            branch: branch.to_owned(),
            upstream: None,
            pull_ref: CLONE_PULL_REF.to_owned(),
        }
    }

    fn worktree(&self) -> PathBuf {
        self.root.join("dev").join("app-wt").join("pr-7-feature")
    }

    /// The checkout's own `feature` worktree at `path`, as a finished pull request removes it.
    fn finished<'a>(&'a self, path: &'a Path) -> FinishedTree<'a> {
        FinishedTree {
            clone: &self.clone,
            path,
            branch: "feature",
            is_own_branch: true,
            head: &self.head,
        }
    }

    fn update_request(&self, branch: &str, head: &str, recorded: &str) -> UpdateRequest {
        UpdateRequest {
            clone: self.clone.clone(),
            branch: branch.to_owned(),
            head: head.to_owned(),
            recorded: Some(recorded.to_owned()),
            pull_ref: CLONE_PULL_REF.to_owned(),
        }
    }
}

fn ignore(_: Event) {}

#[tokio::test]
async fn a_new_worktree_gets_the_pull_request_from_the_mirror_with_its_upstream() {
    let fixture = fixture("worktree").await;
    let steps = Mutex::new(Vec::new());
    let events = |event: Event| {
        if let Event::Step(step) = event {
            steps.lock().unwrap().push(step);
        }
    };
    let mut request = fixture.request(Target::Worktree { path: fixture.worktree() }, "feature");
    request.upstream = Some(Upstream { remote: "origin".into(), branch: "feature".into() });
    let made = Mutex::new(Made::default());

    let done = check_out(&fixture.mirror, &request, &events, &made).await.expect("checked out");

    assert_eq!(done.path, fixture.worktree());
    assert_eq!(done.tip, fixture.head);
    assert!(done.is_new_branch);
    assert_eq!(read(&fixture.worktree(), "src/login.rs"), "pub fn login() {}\n");
    assert_eq!(git(&fixture.worktree(), &["status", "--porcelain"]), "", "a clean checkout");
    assert_eq!(git(&fixture.worktree(), &["branch", "--show-current"]), "feature");
    assert_eq!(git(&fixture.clone, &["config", "branch.feature.remote"]), "origin");
    assert_eq!(git(&fixture.clone, &["config", "branch.feature.merge"]), "refs/heads/feature");
    assert_eq!(git(&fixture.clone, &["rev-parse", CLONE_PULL_REF]), fixture.head);
    assert_eq!(git(&fixture.clone, &["branch", "--show-current"]), "main", "the clone stays");
    assert_eq!(
        *steps.lock().unwrap(),
        [Step::FindClone, Step::Fetch, Step::CreateWorktree, Step::CheckOut]
    );

    // A second worktree of the same branch is refused: git allows one.
    let second = fixture.request(Target::Worktree { path: fixture.root.join("other") }, "feature");
    let refused = check_out(&fixture.mirror, &second, &ignore, &Mutex::default()).await;
    assert!(
        matches!(&refused, Err(CheckoutError::CheckedOut(path)) if super::same_folder(path, &fixture.worktree())),
        "{refused:?}"
    );

    // What the checkout made goes again, and only that.
    let lines = undo(made.into_inner().unwrap()).await;
    assert!(!fixture.worktree().exists(), "the worktree folder is gone: {lines:?}");
    assert_eq!(git(&fixture.clone, &["branch", "--list", "feature"]), "", "its branch too");
    assert!(fixture.clone.join("src/app.rs").exists(), "the clone is untouched");
}

#[tokio::test]
async fn the_clone_switches_only_when_its_tracked_files_are_unchanged() {
    let fixture = fixture("clone").await;
    write(&fixture.clone, "src/app.rs", "fn main() { changed() }\n");
    let request = fixture.request(Target::Clone, "pr-7-feature");
    let refused = check_out(&fixture.mirror, &request, &ignore, &Mutex::default()).await;
    assert!(
        matches!(&refused, Err(CheckoutError::Dirty(files)) if files == &[" M src/app.rs"]),
        "{refused:?}"
    );
    assert_eq!(git(&fixture.clone, &["branch", "--show-current"]), "main");

    git(&fixture.clone, &["checkout", "--", "src/app.rs"]);
    write(&fixture.clone, "notes.md", "mine\n");
    let done = check_out(&fixture.mirror, &request, &ignore, &Mutex::default())
        .await
        .expect("an untracked file does not block");
    assert_eq!(done.path, fixture.clone);
    assert_eq!(git(&fixture.clone, &["branch", "--show-current"]), "pr-7-feature");
    assert_eq!(read(&fixture.clone, "notes.md"), "mine\n", "the untracked file stays");
    let config = git(&fixture.clone, &["config", "--list"]);
    assert!(!config.contains("branch.pr-7-feature."), "a fork's branch has no upstream: {config}");
}

#[tokio::test]
async fn a_new_clone_comes_from_the_mirror_and_keeps_no_credentials() {
    let fixture = fixture("new-clone").await;
    let dir = fixture.root.join("dev").join("fresh");
    let mut request = fixture.request(
        Target::NewClone { origin: "https://user:secret@example.invalid/acme/app.git".into() },
        "feature",
    );
    request.clone.clone_from(&dir);
    let done =
        check_out(&fixture.mirror, &request, &ignore, &Mutex::default()).await.expect("cloned");
    assert_eq!(done.path, dir);
    assert_eq!(read(&dir, "src/login.rs"), "pub fn login() {}\n");
    assert_eq!(
        git(&dir, &["config", "remote.origin.url"]),
        "https://example.invalid/acme/app.git",
        "origin points at the repository, without the credentials"
    );
}

#[tokio::test]
async fn updates_fast_forward_and_follow_a_force_push_but_keep_local_work() {
    let fixture = fixture("update").await;
    let path = fixture.worktree();
    let request = fixture.request(Target::Worktree { path: path.clone() }, "feature");
    check_out(&fixture.mirror, &request, &ignore, &Mutex::default()).await.expect("checked out");

    // A new commit: fast-forward.
    push(&fixture.origin, "src/login.rs", "pub fn login() { cached() }\n");
    let second = fixture.mirror_head().await;
    let outcome = update(
        &fixture.mirror,
        &fixture.update_request("feature", &second, &fixture.head),
        &ignore,
    )
    .await
    .expect("updated");
    assert_eq!(
        outcome,
        UpdateOutcome::Updated(Updated::FastForwarded { from: fixture.head.clone() })
    );
    assert_eq!(read(&path, "src/login.rs"), "pub fn login() { cached() }\n");

    // A force-push of a branch nobody committed to: the branch follows.
    let rewritten = force_push(&fixture.origin, "src/login.rs", "pub fn login() { v2() }\n");
    fixture.mirror_head().await;
    let outcome =
        update(&fixture.mirror, &fixture.update_request("feature", &rewritten, &second), &ignore)
            .await
            .expect("updated");
    assert_eq!(outcome, UpdateOutcome::Updated(Updated::Moved { from: second.clone() }));
    assert_eq!(read(&path, "src/login.rs"), "pub fn login() { v2() }\n");

    // Changed files block it, and nothing moves.
    let third = push(&fixture.origin, "src/third.rs", "third\n");
    fixture.mirror_head().await;
    write(&path, "src/login.rs", "my edit\n");
    let blocked =
        update(&fixture.mirror, &fixture.update_request("feature", &third, &rewritten), &ignore)
            .await
            .expect("asked");
    assert_eq!(blocked, UpdateOutcome::Blocked(Blocked::Changed(vec![" M src/login.rs".into()])));
    assert_eq!(read(&path, "src/login.rs"), "my edit\n", "the edit stays");

    // An untracked file the update would overwrite: git refuses, nothing is lost.
    git(&path, &["checkout", "--", "src/login.rs"]);
    write(&path, "src/third.rs", "mine\n");
    let refused =
        update(&fixture.mirror, &fixture.update_request("feature", &third, &rewritten), &ignore)
            .await
            .expect("asked");
    assert!(matches!(refused, UpdateOutcome::Blocked(Blocked::Refused(_))), "{refused:?}");
    assert_eq!(read(&path, "src/third.rs"), "mine\n");
    std::fs::remove_file(path.join("src/third.rs")).expect("remove");

    // A commit of the user's: a force-push no longer moves the branch.
    write(&path, "src/mine.rs", "wip\n");
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "wip: try shorter ttl"]);
    let forced = force_push(&fixture.origin, "src/login.rs", "pub fn login() { v3() }\n");
    fixture.mirror_head().await;
    let blocked =
        update(&fixture.mirror, &fixture.update_request("feature", &forced, &rewritten), &ignore)
            .await
            .expect("asked");
    let UpdateOutcome::Blocked(Blocked::LocalCommits(commits)) = blocked else {
        panic!("local commits block: {blocked:?}");
    };
    assert!(commits.iter().any(|line| line.ends_with("wip: try shorter ttl")), "{commits:?}");
    assert!(path.join("src/mine.rs").exists(), "the commit's file stays");
}

#[tokio::test]
async fn a_branch_checked_out_nowhere_just_moves() {
    let fixture = fixture("ref-only").await;
    let request = fixture.request(Target::Clone, "feature");
    check_out(&fixture.mirror, &request, &ignore, &Mutex::default()).await.expect("checked out");
    git(&fixture.clone, &["switch", "-q", "main"]);
    let next = push(&fixture.origin, "src/next.rs", "next\n");
    fixture.mirror_head().await;
    let outcome =
        update(&fixture.mirror, &fixture.update_request("feature", &next, &fixture.head), &ignore)
            .await
            .expect("updated");
    assert!(matches!(outcome, UpdateOutcome::Updated(Updated::FastForwarded { .. })));
    assert_eq!(git(&fixture.clone, &["rev-parse", "feature"]), next);
    assert_eq!(git(&fixture.clone, &["branch", "--show-current"]), "main");
}

#[tokio::test]
async fn removing_a_worktree_asks_twice_when_it_has_changes() {
    let fixture = fixture("remove").await;
    let path = fixture.worktree();
    let request = fixture.request(Target::Worktree { path: path.clone() }, "feature");
    check_out(&fixture.mirror, &request, &ignore, &Mutex::default()).await.expect("checked out");
    write(&path, "scratch.txt", "notes\n");
    let own = Some(("feature", fixture.head.as_str()));

    let first = remove_worktree(&fixture.clone, &path, false, own).await.expect("asked");
    assert_eq!(first, Removal::Changed(vec!["?? scratch.txt".into()]));
    assert!(path.exists(), "nothing removed without the second yes");

    let second = remove_worktree(&fixture.clone, &path, true, own).await.expect("removed");
    assert_eq!(second, Removal::Removed);
    assert!(!path.exists());
    assert_eq!(git(&fixture.clone, &["branch", "--list", "feature"]), "", "its own branch went");
}

#[tokio::test]
async fn a_finished_pull_requests_clean_worktree_goes_with_its_merged_branch() {
    let fixture = fixture("finished").await;
    let path = fixture.worktree();
    let request = fixture.request(Target::Worktree { path: path.clone() }, "feature");
    check_out(&fixture.mirror, &request, &ignore, &Mutex::default()).await.expect("checked out");
    // The clone's main has the merged pull request, so git sees the branch merged.
    git(&fixture.clone, &["merge", "-q", "--ff-only", &fixture.head]);

    let finished = remove_finished(fixture.finished(&path)).await.expect("removed");
    assert_eq!(finished, Finished::Removed { branch_kept: None });
    assert!(!path.exists());
    assert_eq!(git(&fixture.clone, &["branch", "--list", "feature"]), "", "its own branch went");
}

#[tokio::test]
async fn a_finished_pull_requests_worktree_stays_while_anything_of_the_users_is_in_it() {
    let fixture = fixture("finished-kept").await;
    let path = fixture.worktree();
    let request = fixture.request(Target::Worktree { path: path.clone() }, "feature");
    check_out(&fixture.mirror, &request, &ignore, &Mutex::default()).await.expect("checked out");

    write(
        &path,
        "src/app.rs",
        "fn main() { edited() }
",
    );
    let tracked = remove_finished(fixture.finished(&path)).await.expect("looked");
    assert_eq!(tracked, Finished::Kept(Keep::Changed(vec![" M src/app.rs".into()])));
    git(&path, &["checkout", "-q", "--", "src/app.rs"]);
    write(
        &path,
        "scratch.txt",
        "notes
",
    );
    let untracked = remove_finished(fixture.finished(&path)).await.expect("looked");
    assert_eq!(untracked, Finished::Kept(Keep::Changed(vec!["?? scratch.txt".into()])));
    git(&path, &["add", "scratch.txt"]);
    git(&path, &["commit", "-q", "-m", "Take notes"]);
    let committed = remove_finished(fixture.finished(&path)).await.expect("looked");
    assert!(
        matches!(&committed, Finished::Kept(Keep::LocalCommits(commits)) if commits.len() == 1),
        "{committed:?}"
    );
    let unknown = FinishedTree { head: "", ..fixture.finished(&path) };
    assert_eq!(remove_finished(unknown).await.expect("looked"), Finished::Kept(Keep::UnknownHead));
    assert!(path.join("scratch.txt").exists(), "nothing was removed");

    // Without the local work it goes; git keeps the branch it does not see merged (-d, not -D).
    git(&path, &["reset", "-q", "--keep", &fixture.head]);
    let removed = remove_finished(fixture.finished(&path)).await.expect("removed");
    assert!(matches!(removed, Finished::Removed { branch_kept: Some(_) }), "{removed:?}");
    assert!(!path.exists());
    assert_eq!(git(&fixture.clone, &["rev-parse", "feature"]), fixture.head, "the branch stays");
}

#[tokio::test]
async fn the_survey_tells_a_fork_from_a_branch_of_the_repository() {
    let fixture = fixture("survey").await;
    let found = FoundClone { dir: fixture.clone.clone(), remote: "origin".into() };
    let fork = survey(&found, "feature", 7).await.expect("surveyed");
    assert!(fork.is_fork, "the clone knows no origin/feature");
    assert_eq!(fork.branch, "pr-7-feature");
    assert_eq!(fork.state.branch.as_deref(), Some("main"));
    assert_eq!(fork.checked_out, None);
    assert_eq!(fork.upstream("feature"), None);

    git(&fixture.clone, &["update-ref", "refs/remotes/origin/feature", &fixture.base]);
    let own = survey(&found, "refs/heads/feature", 7).await.expect("surveyed");
    assert!(!own.is_fork);
    assert_eq!(own.branch, "feature");
    assert_eq!(
        own.upstream("refs/heads/feature"),
        Some(Upstream { remote: "origin".into(), branch: "feature".into() })
    );
    let on_main = survey(&found, "main", 7).await.expect("surveyed");
    assert_eq!(
        on_main.checked_out.as_deref().map(|path| super::same_folder(path, &fixture.clone)),
        Some(true)
    );

    write(
        &fixture.clone,
        "src/app.rs",
        "fn main() { changed() }
",
    );
    let dirty = survey(&found, "feature", 7).await.expect("surveyed");
    assert_eq!(dirty.state.tracked_changes(), [" M src/app.rs"]);
}
