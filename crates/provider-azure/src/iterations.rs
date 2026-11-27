//! A pull request's iterations (one per push) and the file changes of one iteration. Threads,
//! statuses and diff stats all read them, so the requests and the change paging live here once.

use review_core::Result;

use crate::client::{Client, RepoKey};
use crate::dto::ValueList;
use crate::dto::iteration::{CHANGES_PAGE_SIZE, ChangeEntry, Iteration, IterationChanges};

/// The iterations with their source and common commits; `includeCommits=false` leaves out each
/// iteration's commit list, which no caller reads.
pub(crate) async fn list(client: &Client, key: RepoKey<'_>, number: u64) -> Result<Vec<Iteration>> {
    let params = [("includeCommits", "false".to_owned())];
    let iterations: ValueList<Iteration> =
        client.get(&client.pull_request_api(key, number, "iterations"), &params).await?;
    Ok(iterations.value)
}

pub(crate) fn latest(iterations: &[Iteration]) -> Option<u32> {
    iterations.iter().map(|iteration| iteration.id).max()
}

/// Pages `iterations/{iteration}/changes` with `$top`/`$skip`, handing each page to `on_page`
/// until it returns a value or the last page passed (`None`).
pub(crate) async fn changes_until<T>(
    client: &Client,
    key: RepoKey<'_>,
    number: u64,
    iteration: u32,
    mut on_page: impl FnMut(&[ChangeEntry]) -> Option<T>,
) -> Result<Option<T>> {
    let url = client.pull_request_api(key, number, &format!("iterations/{iteration}/changes"));
    let mut skip = 0;
    loop {
        let params = [("$top", CHANGES_PAGE_SIZE.to_string()), ("$skip", skip.to_string())];
        let changes: IterationChanges = client.get(&url, &params).await?;
        if let Some(found) = on_page(&changes.change_entries) {
            return Ok(Some(found));
        }
        match changes.next_skip {
            Some(next) if next > skip && !changes.change_entries.is_empty() => skip = next,
            _ => return Ok(None),
        }
    }
}
