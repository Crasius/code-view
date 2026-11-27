//! Reads a live update ahead, off screen: the newer detail and threads, and when the commits
//! moved, the new objects fetched into the mirror plus the merge base, the commits that came in
//! and the files whose diff changes. Applying it afterwards swaps everything in at once, without
//! a "preparing" screen.

use std::collections::HashSet;

use review_core::{PullRequestDetail, PullRequestRevision, ReviewThread};
use review_git::{CommitInfo, GitMirror};

use cv_pr_session::session::{Mirror, PullRequestSession};
use cv_runtime::load::Load;
use cv_services::Services;

pub(crate) struct Staged {
    /// The probe answer that asked for this update.
    pub revision: PullRequestRevision,
    pub detail: PullRequestDetail,
    pub threads: Vec<ReviewThread>,
    /// The mirror prepared for the new commits; `None` when they did not move or the page's own
    /// mirror is not ready yet (the page prepares it itself then).
    pub mirror: Option<Mirror>,
    /// Commits in the new head that the shown one lacks, newest first.
    pub commits: Vec<CommitInfo>,
    pub is_force_push: bool,
    /// Paths whose diff of all changes differs from the shown one; `None` when the commits moved
    /// and the shown mirror was not ready to compare with.
    pub changed_files: Option<HashSet<String>>,
}

pub(crate) fn start(
    staging: &mut Load<Staged>,
    session: &PullRequestSession,
    services: &Services,
    revision: PullRequestRevision,
) {
    let provider = session.provider.clone();
    let repository = session.key.repository_key.clone();
    let number = session.key.number;
    let shown = session.mirror.ready().cloned();
    let mirrored = session.mirrored().cloned();
    staging.start(&services.background, async move {
        let (detail, threads) = tokio::try_join!(
            provider.pull_request(&repository, number),
            provider.threads(&repository, number)
        )?;
        let wanted = (detail.target_sha.clone(), detail.head_sha.clone());
        let mut staged = Staged {
            revision,
            detail,
            threads,
            mirror: None,
            commits: Vec::new(),
            is_force_push: false,
            changed_files: Some(HashSet::new()),
        };
        if mirrored.as_ref() == Some(&wanted) {
            return Ok(staged);
        }
        let Some(shown) = shown else {
            staged.changed_files = None;
            return Ok(staged);
        };
        let git = shown.git.clone();
        let quiet = |_: review_git::GitEvent| {};
        git.ensure_commits(&staged.detail.git_fetch_refs, &[&wanted.0, &wanted.1], &quiet).await?;
        let merge_base = git.merge_base(&wanted.0, &wanted.1).await?;
        if shown.head != wanted.1 {
            staged.commits = git.commits(&shown.head, &wanted.1).await?;
            // Without a common ancestor the history was rewritten as surely as when the old head is
            // not the ancestor.
            staged.is_force_push = git
                .merge_base(&shown.head, &wanted.1)
                .await
                .map_or(true, |base| base != shown.head);
        }
        // A file's diff of all changes moves when the file changed at either end: in the head or
        // in the merge base.
        let mut changed = changed_paths(&git, &shown.head, &wanted.1).await?;
        changed.extend(changed_paths(&git, &shown.merge_base, &merge_base).await?);
        staged.changed_files = Some(changed);
        staged.mirror = Some(Mirror { git, merge_base, head: wanted.1 });
        Ok(staged)
    });
}

/// Paths that differ between two commits, both paths of a rename; none when they are the same.
async fn changed_paths(
    git: &GitMirror,
    from: &str,
    to: &str,
) -> review_core::Result<HashSet<String>> {
    if from == to {
        return Ok(HashSet::new());
    }
    let files = git.changed_files_fast(from, to).await?;
    Ok(files.into_iter().flat_map(|file| std::iter::once(file.path).chain(file.old_path)).collect())
}
