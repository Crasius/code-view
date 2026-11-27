//! Brings a pull request's commits from the mirror into a clone of the user's. The mirror is a
//! blob-less partial clone, and git does not let a fetch from it download the blobs it lacks
//! ("lazy fetching disabled"), so the mirror first downloads, in one batch through its own
//! authorization, exactly the blobs the clone does not have yet; then the clone fetches from the
//! mirror's folder like from any remote. The clone never talks to the host and never sees a
//! credential.

// RS-DONT-04: parses git's output.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use std::collections::HashSet;
use std::ffi::OsString;

use review_core::Result;

use super::repo::LocalRepo;
use crate::line_counts;
use crate::mirror::GitMirror;
use crate::progress::{GitEvent, ProgressSink};

/// Object ids per download from the host: well below the command line's limit on Windows.
const IDS_PER_FETCH: usize = 400;

/// Fetches `head` from the mirror into `repo` as the ref `into`, after the mirror downloaded what
/// the clone lacks of it.
pub async fn supply(
    mirror: &GitMirror,
    repo: &LocalRepo,
    head: &str,
    into: &str,
    sink: &ProgressSink<'_>,
) -> Result<()> {
    let haves = haves(repo).await?;
    let mut wanted = format!("{head}\n");
    for have in &haves {
        wanted.push('^');
        wanted.push_str(have);
        wanted.push('\n');
    }
    let listed = ["rev-list", "--objects", "--missing=print", "--ignore-missing", "--stdin"];
    let missing = {
        let _guard = mirror.fetch_lock().lock().await;
        let output = mirror.git().run_offline_with_input(listed, wanted.as_bytes()).await?;
        let missing: Vec<String> = String::from_utf8_lossy(&output)
            .lines()
            .filter_map(|line| line.strip_prefix('?'))
            .map(str::to_owned)
            .collect();
        download(mirror, &missing, sink).await?;
        missing
    };
    if missing.is_empty() {
        sink(GitEvent::Message("The mirror has every file the clone needs".to_owned()));
    }
    let source = mirror.git_dir().as_os_str().to_owned();
    fetch_into(repo, source, head, into, &[], sink).await
}

/// Fetches `head` from the mirror into `repo`, a new partial clone whose remote `origin` is the
/// mirror, as the ref `into`: commits and trees only. The blobs of `head`'s tree are downloaded
/// into the mirror first, so the clone's checkout finds them there.
pub async fn supply_partial(
    mirror: &GitMirror,
    repo: &LocalRepo,
    head: &str,
    into: &str,
    sink: &ProgressSink<'_>,
) -> Result<()> {
    {
        let _guard = mirror.fetch_lock().lock().await;
        let tree = mirror.git().run_offline(["ls-tree", "-r", "-z", head]).await?;
        let missing: Vec<String> = line_counts::missing_blobs(&mirror.git(), tree_blobs(&tree))
            .await?
            .into_iter()
            .collect();
        download(mirror, &missing, sink).await?;
    }
    fetch_into(repo, "origin".into(), head, into, &["--filter=blob:none"], sink).await
}

/// The commits the clone's refs point to: what it has, so the mirror leaves those out.
async fn haves(repo: &LocalRepo) -> Result<Vec<String>> {
    let output = repo.git().run(["for-each-ref", "--format=%(objectname)"]).await?;
    let mut seen = HashSet::new();
    Ok(String::from_utf8_lossy(&output)
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty() && seen.insert(id.to_owned()))
        .map(str::to_owned)
        .collect())
}

/// The blob ids of `git ls-tree -r -z`: records of `<mode> <type> <oid>\t<path>\0`. Only the part
/// before the tab is decoded, so a path that is not UTF-8 costs nothing.
fn tree_blobs(ls_tree_z: &[u8]) -> Vec<&str> {
    ls_tree_z
        .split(|byte| *byte == 0)
        .filter_map(|record| {
            let meta = record.split(|byte| *byte == b'\t').next()?;
            let mut fields = std::str::from_utf8(meta).ok()?.split_whitespace();
            let (_mode, kind, oid) = (fields.next()?, fields.next()?, fields.next()?);
            (kind == "blob").then_some(oid)
        })
        .collect()
}

/// Downloads `ids` into the mirror from its host, in batches, by id: what git's own lazy fetch
/// would ask for, in as few requests as the command line allows.
async fn download(mirror: &GitMirror, ids: &[String], sink: &ProgressSink<'_>) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    sink(GitEvent::Message(format!("The mirror downloads {} files the clone needs", ids.len())));
    for batch in ids.chunks(IDS_PER_FETCH) {
        let mut args: Vec<&str> = vec![
            "-c",
            "fetch.negotiationAlgorithm=noop",
            "fetch",
            "--progress",
            "--no-tags",
            "--no-write-fetch-head",
            "--filter=blob:none",
            "origin",
        ];
        args.extend(batch.iter().map(String::as_str));
        mirror.git().run_with_progress(args, sink).await?;
    }
    Ok(())
}

/// `git fetch <source> +<head>:<into>` in the clone; `source` is the mirror's folder or a remote
/// that points there.
async fn fetch_into(
    repo: &LocalRepo,
    source: OsString,
    head: &str,
    into: &str,
    extra: &[&str],
    sink: &ProgressSink<'_>,
) -> Result<()> {
    let refspec = format!("+{head}:{into}");
    let mut args: Vec<OsString> =
        ["fetch", "--progress", "--no-tags", "--no-write-fetch-head"].map(OsString::from).into();
    args.extend(extra.iter().map(OsString::from));
    args.push(source);
    args.push(refspec.into());
    repo.git().run_with_progress(args, sink).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::tree_blobs;

    /// Regression: the listing was decoded as one UTF-8 text, so a single Latin-1 file name made
    /// it empty and a new clone downloaded no blob at all.
    #[test]
    fn tree_blobs_keep_every_blob_whatever_its_path() {
        let listing: &[u8] = b"100644 blob 1111111111111111111111111111111111111111\tREADME.md\0\
            100644 blob 2222222222222222222222222222222222222222\tcaf\xe9.txt\0\
            040000 tree 3333333333333333333333333333333333333333\tsrc\0\
            160000 commit 4444444444444444444444444444444444444444\tvendor/lib\0\
            100755 blob 5555555555555555555555555555555555555555\tsrc/run.sh\0";
        assert_eq!(
            tree_blobs(listing),
            [
                "1111111111111111111111111111111111111111",
                "2222222222222222222222222222222222222222",
                "5555555555555555555555555555555555555555"
            ],
            "trees and submodules are no blobs"
        );
        assert_eq!(tree_blobs(b""), Vec::<&str>::new());
    }
}
