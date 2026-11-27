//! Which loaded rows a list shows and in what order: the overview's repository choice
//! ([`RowFilter`]), the starred repositories first ([`favorites_first`]) and the title bar's search
//! ([`matches_search`]).

use review_core::PullRequestSummary;
use review_store::Favorites;

/// The overview's repository choice applied to loaded rows.
pub(crate) struct RowFilter {
    repository: Option<(String, String)>,
}

impl RowFilter {
    pub(crate) fn new(repository: Option<&(String, String)>) -> Self {
        Self { repository: repository.cloned() }
    }

    pub(super) fn matches(&self, connection_id: &str, pull_request: &PullRequestSummary) -> bool {
        self.repository.as_ref().is_none_or(|(wanted_connection, wanted_repository)| {
            wanted_connection == connection_id && *wanted_repository == pull_request.repository_key
        })
    }
}

/// Moves the rows of starred repositories to the top of a card of `connection_id`, keeping the
/// order within both groups: the overview's order while its Favorites lane is hidden.
pub(super) fn favorites_first(
    rows: &mut [&PullRequestSummary],
    connection_id: &str,
    favorites: &Favorites,
) {
    rows.sort_by_key(|row| !favorites.contains(connection_id, &row.repository_key));
}

/// True when `query` (already trimmed and lowercased) is in the title, author, repository or a
/// label, or is the number (with or without `#`/`!`). The search of the title bar finds rows by this.
pub(in crate::pull_request_list) fn matches_search(
    pull_request: &PullRequestSummary,
    query: &str,
) -> bool {
    if query.is_empty() {
        return true;
    }
    let in_title = pull_request.title.to_lowercase().contains(query);
    let in_author = pull_request.author.display_name.to_lowercase().contains(query);
    let in_repository = pull_request.repository_key.to_lowercase().contains(query);
    let in_labels =
        pull_request.labels.iter().any(|label| label.name.to_lowercase().contains(query));
    let in_number = pull_request.number.to_string() == query.trim_start_matches(['!', '#']);
    in_title || in_author || in_repository || in_labels || in_number
}

#[cfg(test)]
mod tests {
    use review_store::Favorites;

    use super::favorites_first;
    use crate::pull_request_list::list_rows::tests::row;

    #[test]
    fn starred_rows_lead_in_their_own_order() {
        let mut rows: Vec<_> = (1..=5).map(row).collect();
        for (number, repository) in [(2, "team/api"), (4, "team/api"), (5, "team/web")] {
            rows[number - 1].repository_key = repository.into();
        }
        let mut favorites = Favorites::default();
        favorites.toggle("gl", "team/api");
        favorites.toggle("gh", "group/app");
        let mut order: Vec<_> = rows.iter().collect();
        favorites_first(&mut order, "gl", &favorites);
        let numbers: Vec<u64> = order.iter().map(|row| row.number).collect();
        assert_eq!(numbers, [2, 4, 1, 3, 5], "a star of another connection lifts nothing");
    }
}
