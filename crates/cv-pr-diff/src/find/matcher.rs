//! Matching for the find widget: every non-overlapping occurrence of a needle in a line, as byte
//! ranges of the line, with or without case and only as whole words if asked. Plain text folds
//! case per character (`char::to_lowercase`), so a match never splits a character and a
//! multi-byte line matches in place; folds that change the length (`ß` and `SS`) do not match.
//! The widget offers match case only; whole words are for the selection's occurrences.

#![warn(clippy::string_slice, clippy::indexing_slicing)]

use std::ops::Range;

/// How the needle has to match.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct MatchOptions {
    /// Upper and lower case count as different.
    pub match_case: bool,
    /// Letters, digits or `_` on either side of a match stop it from counting.
    pub whole_word: bool,
}

/// The occurrences of `needle` in `text`, left to right, none overlapping; none for an empty
/// needle.
pub fn find_all(text: &str, needle: &str, options: MatchOptions) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    if needle.is_empty() {
        return found;
    }
    let mut from = 0;
    while let Some(start) = next_start(text, from, needle, options.match_case) {
        let Some(end) = match_end(text, start, needle, options.match_case) else {
            break;
        };
        if !options.whole_word || is_whole_word(text, start..end) {
            found.push(start..end);
            from = end;
        } else {
            from = start
                + text.get(start..).and_then(|rest| rest.chars().next()).map_or(1, char::len_utf8);
        }
    }
    found
}

/// The first byte at or after `from` where `needle` matches in `text`.
fn next_start(text: &str, from: usize, needle: &str, match_case: bool) -> Option<usize> {
    let rest = text.get(from..)?;
    if match_case {
        return rest.find(needle).map(|offset| from + offset);
    }
    rest.char_indices()
        .map(|(offset, _)| from + offset)
        .find(|start| match_end(text, *start, needle, false).is_some())
}

/// Where `needle` ends when it matches `text` at byte `start`.
fn match_end(text: &str, start: usize, needle: &str, match_case: bool) -> Option<usize> {
    let mut rest = text.get(start..)?.char_indices();
    let mut end = start;
    for wanted in needle.chars() {
        let (offset, found) = rest.next()?;
        let is_same = if match_case {
            found == wanted
        } else {
            found == wanted || found.to_lowercase().eq(wanted.to_lowercase())
        };
        if !is_same {
            return None;
        }
        end = start + offset + found.len_utf8();
    }
    Some(end)
}

/// The match `range` of `text` has no word character right before or after it.
fn is_whole_word(text: &str, range: Range<usize>) -> bool {
    let before = text.get(..range.start).and_then(|head| head.chars().next_back());
    let after = text.get(range.end..).and_then(|tail| tail.chars().next());
    !before.is_some_and(is_word) && !after.is_some_and(is_word)
}

/// A letter, a digit or `_`: what a whole word is made of.
pub fn is_word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANY: MatchOptions = MatchOptions { match_case: false, whole_word: false };
    const CASE: MatchOptions = MatchOptions { match_case: true, ..ANY };
    const WORD: MatchOptions = MatchOptions { whole_word: true, ..ANY };

    #[test]
    fn finds_every_occurrence_without_overlap() {
        assert_eq!(find_all("value = value + 1", "value", ANY), [0..5, 8..13]);
        assert_eq!(find_all("aaaa", "aa", ANY), [0..2, 2..4], "no overlapping matches");
        assert!(find_all("abc", "abcd", ANY).is_empty(), "longer than the line");
    }

    #[test]
    fn an_empty_needle_matches_nothing() {
        assert!(find_all("anything", "", ANY).is_empty(), "an empty needle in a line");
        assert!(find_all("", "", CASE).is_empty(), "an empty needle in an empty line");
    }

    #[test]
    #[expect(
        clippy::single_range_in_vec_init,
        reason = "one match: a list of one range, not a range of indices"
    )]
    fn ignores_case_unless_asked() {
        assert_eq!(find_all("Value VALUE value", "value", ANY), [0..5, 6..11, 12..17]);
        assert_eq!(find_all("Value VALUE value", "value", CASE), [12..17]);
        assert!(find_all("Value", "VALUE", CASE).is_empty(), "no match of another case");
    }

    #[test]
    #[expect(
        clippy::single_range_in_vec_init,
        reason = "one match: a list of one range, not a range of indices"
    )]
    fn whole_words_skip_matches_inside_longer_words() {
        assert_eq!(
            find_all("count counter recount count_x (count)", "count", WORD),
            [0..5, 31..36]
        );
        assert_eq!(
            find_all("countcount count", "count", WORD),
            [11..16],
            "retries after a partial hit"
        );
    }

    #[test]
    #[expect(
        clippy::single_range_in_vec_init,
        reason = "one match: a list of one range, not a range of indices"
    )]
    fn matches_multi_byte_text_by_its_byte_ranges() {
        let text = "größe Größe GRÖSSE";
        assert_eq!(find_all(text, "größe", ANY), [0..7, 8..15]);
        assert_eq!(find_all(text, "Ö", CASE), [18..20], "only the upper case one");
        assert_eq!(find_all("日本語の日本", "日本", ANY), [0..6, 12..18]);
        assert_eq!(
            find_all("über_über über", "über", WORD),
            [12..17],
            "letters beyond ASCII are word characters"
        );
    }
}
