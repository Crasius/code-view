//! Shorthand references in plain text, as each platform links them:
//!
//! - GitHub: `#12` (issue or pull request of this repository), `owner/repo#12`.
//! - GitLab: `#12` (issue), `!12` (merge request), `group/project#12`, `group/sub/project!12`.
//! - Azure DevOps: `#12` (work item), `!12` (pull request).
//!
//! A reference starts a word and ends one: `abc#12`, `#12abc`, `&#39;` and `v1.2#3` are text.
//! Also finds ticket keys (`ABC-123`) for the overview's linked tickets.

// RS-DONT-04: parses references from pull request text, so indexing, string slicing and narrowing
// casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing, clippy::cast_possible_truncation)]

use std::ops::Range;

use review_core::ProviderKind;

const MAX_DIGITS: usize = 10;

/// Byte ranges of every reference in `text`, in order.
pub fn find(text: &str, platform: ProviderKind) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while let Some(&marker) = bytes.get(index) {
        let is_marker = marker == b'#' || (marker == b'!' && platform != ProviderKind::GitHub);
        if !is_marker {
            index += 1;
            continue;
        }
        let Some(end) = number_end(bytes, index + 1) else {
            index += 1;
            continue;
        };
        if let Some(start) = reference_start(text, index, platform) {
            found.push(start..end);
            index = end;
        } else {
            index += 1;
        }
    }
    found
}

/// End of the number after a marker, when the number is a whole word.
fn number_end(bytes: &[u8], from: usize) -> Option<usize> {
    let rest = bytes.get(from..)?;
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    if digits == 0 || digits > MAX_DIGITS || rest.first() == Some(&b'0') {
        return None;
    }
    let end = from + digits;
    match bytes.get(end) {
        Some(next) if next.is_ascii_alphanumeric() || *next == b'_' => None,
        // `#12.5` is a version, `#12.` ends a sentence.
        Some(b'.' | b',') if bytes.get(end + 1).is_some_and(u8::is_ascii_digit) => None,
        // The end of the text or any other character ends the number.
        _ => Some(end),
    }
}

/// Start of the reference whose marker sits at `marker`: the marker itself, or the start of a
/// repository path directly before it where the platform allows one.
fn reference_start(text: &str, marker: usize, platform: ProviderKind) -> Option<usize> {
    let before = text.get(..marker)?;
    let path_len = before
        .chars()
        .rev()
        .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-' | '/'))
        .map(char::len_utf8)
        .sum::<usize>();
    let start = marker - path_len;
    if !starts_word(text.get(..start)?) {
        return None;
    }
    if path_len == 0 {
        return Some(marker);
    }
    let path = text.get(start..marker)?;
    let segments: Vec<&str> = path.split('/').collect();
    let is_valid_segment = |segment: &&str| {
        !segment.is_empty() && !segment.starts_with('.') && !segment.ends_with('.')
    };
    let fits = match platform {
        ProviderKind::GitHub => segments.len() == 2,
        ProviderKind::GitLab => segments.len() >= 2,
        ProviderKind::AzureDevOps => false,
    };
    (fits && segments.iter().all(is_valid_segment)).then_some(start)
}

/// True when a reference may begin right after `before`: at the start or after a space or an
/// opening punctuation mark, not inside a word, URL or HTML entity.
fn starts_word(before: &str) -> bool {
    match before.chars().next_back() {
        None => true,
        Some(ch) => {
            !(ch.is_alphanumeric()
                || matches!(ch, '_' | '&' | '/' | '#' | '!' | '@' | '.' | '-' | ':' | '='))
        }
    }
}

/// Ticket keys like `ABC-123`: two or more capitals or digits starting with a capital, a dash and
/// a number, as a whole word. `UTF-8` and `SHA-256` match too, so callers only link keys whose
/// tracker they know.
pub fn ticket_keys(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut keys = Vec::new();
    let mut index = 0;
    while let Some(&first) = bytes.get(index) {
        let previous = index.checked_sub(1).and_then(|previous| bytes.get(previous));
        let is_word_start = previous.is_none_or(|&previous| {
            !(previous.is_ascii_alphanumeric() || matches!(previous, b'_' | b'-'))
        });
        if !is_word_start || !first.is_ascii_uppercase() {
            index += 1;
            continue;
        }
        let project = bytes
            .get(index..)
            .unwrap_or_default()
            .iter()
            .take_while(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            .count();
        let dash = index + project;
        let digits = bytes
            .get(dash + 1..)
            .map_or(0, |rest| rest.iter().take_while(|byte| byte.is_ascii_digit()).count());
        let end = dash + 1 + digits;
        let is_whole =
            bytes.get(end).is_none_or(|next| !(next.is_ascii_alphanumeric() || *next == b'_'));
        if project >= 2
            && bytes.get(dash) == Some(&b'-')
            && digits > 0
            && bytes.get(dash + 1) != Some(&b'0')
            && is_whole
            && let Some(key) = text.get(index..end)
        {
            keys.push(key);
            index = end;
        } else {
            index += project.max(1);
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use review_core::ProviderKind::{AzureDevOps, GitHub, GitLab};

    use super::{find, ticket_keys};

    fn refs(text: &str, platform: review_core::ProviderKind) -> Vec<&str> {
        find(text, platform)
            .into_iter()
            .map(|range| text.get(range).expect("find returns char boundaries"))
            .collect()
    }

    #[test]
    fn github_numbers_and_repository_references() {
        assert_eq!(refs("Fixes #59 and cli/cli#3, see (#7).", GitHub), ["#59", "cli/cli#3", "#7"]);
        assert_eq!(refs("!12 is not a GitHub reference", GitHub), Vec::<&str>::new());
        assert_eq!(refs("a/b/c#4 has too many segments", GitHub), Vec::<&str>::new());
        assert_eq!(refs("my-org/my.repo#12", GitHub), ["my-org/my.repo#12"]);
    }

    #[test]
    fn gitlab_issues_merge_requests_and_nested_groups() {
        assert_eq!(
            refs("Closes #4, needs !12 and group/sub/app!9 and group/app#2", GitLab),
            ["#4", "!12", "group/sub/app!9", "group/app#2"]
        );
    }

    #[test]
    fn azure_work_items_and_pull_requests() {
        assert_eq!(refs("AB#1 no, #123 yes, !45 yes, repo!3 no", AzureDevOps), ["#123", "!45"]);
    }

    #[test]
    fn words_versions_entities_and_urls_stay_text() {
        for text in [
            "abc#12",
            "#12abc",
            "&#39;",
            "v1.2#3",
            "#0",
            "#12.5",
            "x!3",
            "Hello!12",
            "/path#12",
            "a_b#1",
            "#",
            "#12345678901",
        ] {
            assert_eq!(refs(text, GitLab), Vec::<&str>::new(), "{text}");
        }
        assert_eq!(refs("#12. Next", GitHub), ["#12"]);
        assert_eq!(refs("über #5", GitHub), ["#5"]);
    }

    #[test]
    fn ticket_keys_are_whole_words() {
        assert_eq!(
            ticket_keys("feature/ABC-123-login and PROJ2-7, not abc-1, A-1 or XABC-0"),
            ["ABC-123", "PROJ2-7"]
        );
        assert_eq!(ticket_keys("ABC-12x"), Vec::<&str>::new());
    }
}
