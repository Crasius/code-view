//! Word-level highlights for a deleted/added line pair, computed only for rows on screen.

use std::ops::Range;

use similar::{Algorithm, ChangeTag, TextDiff};

/// Lines longer than this are highlighted as a whole; word diffs on minified code cost too much.
const MAX_LINE_BYTES: usize = 2_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Highlights {
    pub old: Vec<Range<usize>>,
    pub new: Vec<Range<usize>>,
}

pub fn compute(old: &str, new: &str) -> Highlights {
    let is_too_long = old.len() > MAX_LINE_BYTES || new.len() > MAX_LINE_BYTES;
    if is_too_long || old == new {
        return Highlights::default();
    }
    // The shortest edit script: `Algorithm::Myers` splits some edits Git-style into other words.
    let diff = TextDiff::configure().algorithm(Algorithm::RawMyers).diff_unicode_words(old, new);
    let mut highlights = Highlights::default();
    let mut old_offset = 0;
    let mut new_offset = 0;
    let mut shared = 0;
    for change in diff.iter_all_changes() {
        let length = change.value().len();
        match change.tag() {
            ChangeTag::Equal => {
                shared += solid_len(change.value());
                old_offset += length;
                new_offset += length;
            }
            ChangeTag::Delete => {
                push_merged(&mut highlights.old, old_offset..old_offset + length);
                old_offset += length;
            }
            ChangeTag::Insert => {
                push_merged(&mut highlights.new, new_offset..new_offset + length);
                new_offset += length;
            }
        }
    }
    // Lines that share less than half their text (ignoring indentation and spacing) were replaced,
    // not edited: marking the few words they happen to share would only be noise.
    let is_replaced = shared * 2 < solid_len(old).max(solid_len(new));
    if is_replaced {
        return Highlights::default();
    }
    highlights
}

fn solid_len(text: &str) -> usize {
    text.chars().filter(|character| !character.is_whitespace()).map(char::len_utf8).sum()
}

fn push_merged(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
    if let Some(last) = ranges.last_mut()
        && last.end == range.start
    {
        last.end = range.end;
        return;
    }
    ranges.push(range);
}

#[cfg(test)]
mod tests {
    use super::compute;

    #[test]
    #[expect(
        clippy::single_range_in_vec_init,
        reason = "one changed word: a list of one range, not a range of indices"
    )]
    fn highlights_changed_words_only() {
        let highlights = compute("let total = price * 2;", "let total = price * 3;");
        assert_eq!(highlights.old, [20..21]);
        assert_eq!(highlights.new, [20..21]);
    }

    #[test]
    fn skips_lines_that_changed_completely() {
        let highlights = compute("alpha beta", "gamma delta");
        assert!(highlights.old.is_empty() && highlights.new.is_empty());
    }

    #[test]
    fn skips_unrelated_lines_that_only_share_spacing_and_punctuation() {
        let highlights =
            compute("    let nextSymbolId = 1;", "    ArrayBindingPattern, arrayFrom, Block;");
        assert!(highlights.old.is_empty() && highlights.new.is_empty());
    }

    #[test]
    #[expect(
        clippy::single_range_in_vec_init,
        reason = "one changed run: a list of one range, not a range of indices"
    )]
    fn marks_a_changed_indentation() {
        let highlights = compute("    const anon = 1;", "const anon = 1;");
        assert_eq!(highlights.old, [0..4]);
        assert!(highlights.new.is_empty(), "the new line lost the indentation, nothing to mark");
    }
}
