//! A pull request that several lists show (the overview, a repository's list, the Favorites lane)
//! is one pull request: when one list's answer arrives, the rows it shows replace the older copies
//! the other lists of the connection remember, so a list shown next paints them current before its
//! own refresh. Only rows the other list already shows change; which rows a list holds stays its
//! own query's answer. A row whose new state the other list does not keep (merged out of an open
//! list) leaves it.

use cv_runtime::cache::page_cache::{self, PageCache};
use cv_services::writes::is_listed_in;
use review_core::PullRequestSummary;

/// Patches the rows of every list of `connection_id` but `from` with `rows`, the answer for `from`
/// asked for at input time `asked_at`.
pub(super) fn spread(
    cache: &mut PageCache,
    connection_id: &str,
    from: &str,
    rows: &[PullRequestSummary],
    asked_at: f64,
) {
    for list in cache.keys_with_prefix(&page_cache::lists_of(connection_id)) {
        if list == from {
            continue;
        }
        let Some(state) = page_cache::list_state(&list) else {
            continue;
        };
        cache.patch(&list, asked_at, |held: &mut Vec<PullRequestSummary>| {
            let mut is_changed = false;
            held.retain_mut(|row| {
                let fresh = rows.iter().find(|fresh| {
                    fresh.number == row.number && fresh.repository_key == row.repository_key
                });
                let Some(fresh) = fresh.filter(|fresh| !is_older(fresh, row)) else {
                    return true;
                };
                let merged = merged(fresh, row);
                if merged == *row {
                    return true;
                }
                is_changed = true;
                *row = merged;
                is_listed_in(row.state, state)
            });
            is_changed
        });
    }
}

/// The host updated `held` after it answered `fresh`: the other list asked later than it seemed.
fn is_older(fresh: &PullRequestSummary, held: &PullRequestSummary) -> bool {
    matches!((fresh.updated_at, held.updated_at), (Some(fresh), Some(held)) if fresh < held)
}

/// `fresh`, with what its listing does not report kept from `held`: the reviewers, and the checks
/// and line counts while the head is the same.
fn merged(fresh: &PullRequestSummary, held: &PullRequestSummary) -> PullRequestSummary {
    let mut row = fresh.clone();
    if row.reviewers.is_empty() {
        row.reviewers.clone_from(&held.reviewers);
    }
    let head = row.head_sha.as_ref().or(held.head_sha.as_ref());
    if head == held.head_sha.as_ref() {
        row.checks = row.checks.or(held.checks);
        row.diff_stat = row.diff_stat.or(held.diff_stat);
    }
    row.head_sha = row.head_sha.or_else(|| held.head_sha.clone());
    row.target_sha = row.target_sha.or_else(|| held.target_sha.clone());
    row
}

#[cfg(test)]
mod tests {
    use review_core::{
        DiffStat, PullRequestScope, PullRequestState, PullRequestSummary, RunStatus, StateFilter,
    };

    use super::spread;
    use crate::pull_request_list::list_rows::tests::row;
    use cv_runtime::cache::page_cache::{PageCache, list_part};

    fn key(listed: &str, state: StateFilter) -> String {
        list_part("gl|00ff", listed, PullRequestScope::Visible, state)
    }

    fn rows(cache: &mut PageCache, key: &str) -> Vec<PullRequestSummary> {
        cache.get(key).expect("the list is held")
    }

    /// The overview's answer updates the rows the repository's list shows, keeps what only that
    /// list knew, adds nothing, and drops a row merged out of an open list.
    #[test]
    fn an_answer_updates_the_rows_other_lists_show() {
        let mut cache = PageCache::default();
        let overview = key("*", StateFilter::Open);
        let repository = key("group/app", StateFilter::Open);
        let other_connection =
            list_part("gh|00ff", "*", PullRequestScope::Visible, StateFilter::Open);
        let counted = PullRequestSummary {
            diff_stat: DiffStat::new(3, 1, 2),
            checks: Some(RunStatus::Succeeded),
            ..row(1)
        };
        cache.put_answer(&repository, vec![counted, row(2), row(3)], 10.0, 11.0);
        cache.put_answer(&other_connection, vec![row(1)], 10.0, 11.0);

        let renamed = PullRequestSummary { title: "Renamed".into(), ..row(1) };
        let merged = PullRequestSummary { state: PullRequestState::Merged, ..row(2) };
        let answer = [renamed, merged, row(9)];
        spread(&mut cache, "gl", &overview, &answer, 20.0);

        let shown = rows(&mut cache, &repository);
        let numbers: Vec<u64> = shown.iter().map(|row| row.number).collect();
        assert_eq!(numbers, [1, 3], "the merged row left, the new one stays out");
        assert_eq!(shown[0].title, "Renamed");
        assert_eq!(shown[0].diff_stat, DiffStat::new(3, 1, 2), "the counts of the same head");
        assert_eq!(shown[0].checks, Some(RunStatus::Succeeded));
        assert_eq!(
            rows(&mut cache, &other_connection)[0].title,
            row(1).title,
            "another connection"
        );
        assert_eq!(cache.patches(), 1);

        let pushed = PullRequestSummary { head_sha: Some("pushed".into()), ..row(1) };
        spread(&mut cache, "gl", &overview, &[pushed], 30.0);
        let shown = rows(&mut cache, &repository);
        assert_eq!(shown[0].diff_stat, DiffStat::default(), "a new head drops the old counts");
        assert_eq!(shown[0].checks, None);
    }

    /// A list asked for after the answer keeps its rows.
    #[test]
    fn an_older_answer_changes_nothing() {
        let mut cache = PageCache::default();
        let repository = key("group/app", StateFilter::Open);
        cache.put_answer(&repository, vec![row(1)], 30.0, 31.0);
        let renamed = PullRequestSummary { title: "Renamed".into(), ..row(1) };
        spread(&mut cache, "gl", &key("*", StateFilter::Open), &[renamed], 20.0);
        assert_eq!(rows(&mut cache, &repository), [row(1)]);
        assert_eq!(cache.patches(), 0);
    }
}
