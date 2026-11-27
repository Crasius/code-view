//! Lines up the pieces of two versions of a Markdown file the way a side-by-side code diff lines
//! up lines: equal pieces pair by content (patience diff over their signatures), and within a run
//! of differing pieces an edited piece pairs with the old version it resembles most (same kind,
//! enough shared words), so it shows as modified, next to its old version, with the changed words
//! marked. What pairs with nothing was added or removed.

use std::ops::Range;
use std::time::Duration;

use similar::{Algorithm, ChangeTag, DiffOp, TextDiff, capture_diff_slices};

use cv_markdown::Units;

/// Pieces sharing less of their words than this were replaced, not edited.
const MIN_SIMILARITY: f32 = 0.35;
/// Runs of differing pieces larger than this (old × new) are not paired piece by piece.
const MAX_PAIRING: usize = 4_000;
/// Longer pieces are compared and word-marked by their start only, or not at all.
const MAX_COMPARED_BYTES: usize = 4_000;
const MAX_MARKED_BYTES: usize = 60_000;
const WORD_DIFF_TIMEOUT: Duration = Duration::from_millis(40);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    Equal,
    Added,
    Removed,
    /// An edited piece: its old and new version side by side, the changed words marked.
    Modified,
}

impl Change {
    pub(crate) fn is_change(self) -> bool {
        self != Change::Equal
    }
}

/// One line of the aligned pieces: the old piece, the new piece, or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pair {
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub change: Change,
    /// For a modified pair: the changed words of each side, byte ranges of the piece's text.
    pub old_words: Vec<Range<usize>>,
    pub new_words: Vec<Range<usize>>,
}

impl Pair {
    pub(crate) fn one_sided(old: Option<usize>, new: Option<usize>, change: Change) -> Self {
        Self { old, new, change, old_words: Vec::new(), new_words: Vec::new() }
    }
}

pub(crate) fn align(old: &Units, new: &Units) -> Vec<Pair> {
    let old_signatures: Vec<u64> = old.units.iter().map(|unit| unit.signature).collect();
    let new_signatures: Vec<u64> = new.units.iter().map(|unit| unit.signature).collect();
    let mut pairs = Vec::with_capacity(old.len().max(new.len()));
    // Consecutive deletions and insertions form one run, paired as a whole.
    let mut run: Option<(Range<usize>, Range<usize>)> = None;
    for op in capture_diff_slices(Algorithm::Patience, &old_signatures, &new_signatures) {
        let (olds, news) = match op {
            DiffOp::Equal { old_index, new_index, len } => {
                if let Some((olds, news)) = run.take() {
                    pair_run(old, new, olds, news, &mut pairs);
                }
                pairs.extend((0..len).map(|offset| {
                    Pair::one_sided(
                        Some(old_index + offset),
                        Some(new_index + offset),
                        Change::Equal,
                    )
                }));
                continue;
            }
            DiffOp::Delete { old_index, old_len, new_index } => {
                (old_index..old_index + old_len, new_index..new_index)
            }
            DiffOp::Insert { old_index, new_index, new_len } => {
                (old_index..old_index, new_index..new_index + new_len)
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                (old_index..old_index + old_len, new_index..new_index + new_len)
            }
        };
        run = Some(match run.take() {
            Some((run_old, run_new)) => (
                run_old.start.min(olds.start)..run_old.end.max(olds.end),
                run_new.start.min(news.start)..run_new.end.max(news.end),
            ),
            None => (olds, news),
        });
    }
    if let Some((olds, news)) = run {
        pair_run(old, new, olds, news, &mut pairs);
    }
    pairs
}

/// Pairs the differing pieces `olds` and `news` (one run between equal pieces): the pairing with
/// the most similarity in order wins; removed pieces come before added ones between two pairs.
fn pair_run(
    old: &Units,
    new: &Units,
    olds: Range<usize>,
    news: Range<usize>,
    pairs: &mut Vec<Pair>,
) {
    let (rows, columns) = (olds.len(), news.len());
    let matched = if rows == 0 || columns == 0 || rows * columns > MAX_PAIRING {
        Vec::new()
    } else {
        best_pairing(old, new, olds.clone(), news.clone())
    };
    let (mut next_old, mut next_new) = (olds.start, news.start);
    for (old_index, new_index) in matched.into_iter().chain([(olds.end, news.end)]) {
        pairs.extend(
            (next_old..old_index).map(|index| Pair::one_sided(Some(index), None, Change::Removed)),
        );
        pairs.extend(
            (next_new..new_index).map(|index| Pair::one_sided(None, Some(index), Change::Added)),
        );
        if old_index < olds.end && new_index < news.end {
            let (old_words, new_words) =
                changed_words(&old.units[old_index].text, &new.units[new_index].text);
            pairs.push(Pair {
                old: Some(old_index),
                new: Some(new_index),
                change: Change::Modified,
                old_words,
                new_words,
            });
        }
        (next_old, next_new) = (old_index + 1, new_index + 1);
    }
}

/// The in-order pairs of `olds` × `news` with the most total similarity (a weighted longest
/// common subsequence); only pieces of the same kind that are similar enough pair.
fn best_pairing(
    old: &Units,
    new: &Units,
    olds: Range<usize>,
    news: Range<usize>,
) -> Vec<(usize, usize)> {
    let (rows, columns) = (olds.len(), news.len());
    let similarity: Vec<f32> = olds
        .clone()
        .flat_map(|old_index| news.clone().map(move |new_index| (old_index, new_index)))
        .map(|(old_index, new_index)| {
            let (before, after) = (&old.units[old_index], &new.units[new_index]);
            if before.kind != after.kind {
                return 0.0;
            }
            // Texts can share enough words only when their lengths are alike: skip the word diff
            // of the others (half the bound, as bytes only approximate words).
            let (short, long) = (
                before.text.len().min(after.text.len()) as f32,
                before.text.len().max(after.text.len()) as f32,
            );
            if short * (2.0 - MIN_SIMILARITY) < long * MIN_SIMILARITY * 0.5 {
                return 0.0;
            }
            let score = similarity(&before.text, &after.text);
            if score >= MIN_SIMILARITY { score } else { 0.0 }
        })
        .collect();
    let width = columns + 1;
    let mut best = vec![0.0_f32; (rows + 1) * width];
    for row in 1..=rows {
        for column in 1..=columns {
            let score = similarity[(row - 1) * columns + column - 1];
            let paired =
                if score > 0.0 { best[(row - 1) * width + column - 1] + score } else { 0.0 };
            best[row * width + column] =
                paired.max(best[(row - 1) * width + column]).max(best[row * width + column - 1]);
        }
    }
    let mut matched = Vec::new();
    let (mut row, mut column) = (rows, columns);
    while row > 0 && column > 0 {
        let score = similarity[(row - 1) * columns + column - 1];
        let here = best[row * width + column];
        if score > 0.0 && (here - (best[(row - 1) * width + column - 1] + score)).abs() < 1e-6 {
            matched.push((olds.start + row - 1, news.start + column - 1));
            row -= 1;
            column -= 1;
        } else if best[(row - 1) * width + column] >= best[row * width + column - 1] {
            row -= 1;
        } else {
            column -= 1;
        }
    }
    matched.reverse();
    matched
}

/// How much of two texts' words they share, 0 to 1: spaces and punctuation, which any two
/// sentences share, do not count. Two texts without words (rules, symbols) are the same.
fn similarity(old: &str, new: &str) -> f32 {
    let words = |text: &str| -> Vec<String> {
        prefix(text, MAX_COMPARED_BYTES)
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let (old, new) = (words(old), words(new));
    if old.is_empty() || new.is_empty() {
        return if old.is_empty() && new.is_empty() { 1.0 } else { 0.0 };
    }
    let deadline = std::time::Instant::now() + WORD_DIFF_TIMEOUT;
    let shared: usize =
        similar::capture_diff_slices_deadline(Algorithm::Patience, &old, &new, Some(deadline))
            .iter()
            .map(|op| match op {
                DiffOp::Equal { len, .. } => *len,
                _ => 0,
            })
            .sum();
    2.0 * shared as f32 / (old.len() + new.len()) as f32
}

/// Byte ranges of the words `new` changed, in `old` and in `new`. Changed words separated only by
/// spaces form one range, so an edited phrase reads as one mark.
pub(crate) fn changed_words(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    if old == new || old.len() > MAX_MARKED_BYTES || new.len() > MAX_MARKED_BYTES {
        return (Vec::new(), Vec::new());
    }
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Patience)
        .timeout(WORD_DIFF_TIMEOUT)
        .diff_unicode_words(old, new);
    let (mut old_words, mut new_words) = (Vec::new(), Vec::new());
    let (mut old_offset, mut new_offset) = (0, 0);
    for change in diff.iter_all_changes() {
        let length = change.value().len();
        match change.tag() {
            ChangeTag::Equal => {
                old_offset += length;
                new_offset += length;
            }
            ChangeTag::Delete => {
                push_word(&mut old_words, old_offset..old_offset + length, old);
                old_offset += length;
            }
            ChangeTag::Insert => {
                push_word(&mut new_words, new_offset..new_offset + length, new);
                new_offset += length;
            }
        }
    }
    (trimmed(old_words, old), trimmed(new_words, new))
}

fn push_word(ranges: &mut Vec<Range<usize>>, range: Range<usize>, text: &str) {
    if let Some(last) = ranges.last_mut()
        && last.end <= range.start
        && text[last.end..range.start].chars().all(|character| character == ' ')
    {
        last.end = range.end;
        return;
    }
    ranges.push(range);
}

/// Marks without leading or trailing whitespace, and none that are whitespace only.
fn trimmed(ranges: Vec<Range<usize>>, text: &str) -> Vec<Range<usize>> {
    ranges
        .into_iter()
        .filter_map(|range| {
            let slice = &text[range.clone()];
            let start = range.start + (slice.len() - slice.trim_start().len());
            let end = range.end - (slice.len() - slice.trim_end().len());
            (start < end).then_some(start..end)
        })
        .collect()
}

fn prefix(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changes(old: &str, new: &str) -> Vec<(Option<usize>, Option<usize>, Change)> {
        let (old, new) = (Units::parse(old, None), Units::parse(new, None));
        align(&old, &new).into_iter().map(|pair| (pair.old, pair.new, pair.change)).collect()
    }

    #[test]
    fn equal_blocks_pair_and_new_ones_are_added() {
        let old = "# Title\n\nFirst paragraph.\n\nLast paragraph.\n";
        let new = "# Title\n\nFirst paragraph.\n\nA new paragraph in between.\n\nLast paragraph.\n";
        assert_eq!(
            changes(old, new),
            [
                (Some(0), Some(0), Change::Equal),
                (Some(1), Some(1), Change::Equal),
                (None, Some(2), Change::Added),
                (Some(2), Some(3), Change::Equal),
            ]
        );
    }

    #[test]
    fn removed_blocks_have_no_new_side() {
        let old = "One.\n\nTwo.\n\nThree.\n";
        let new = "One.\n\nThree.\n";
        assert_eq!(
            changes(old, new),
            [
                (Some(0), Some(0), Change::Equal),
                (Some(1), None, Change::Removed),
                (Some(2), Some(1), Change::Equal)
            ]
        );
    }

    #[test]
    fn edited_blocks_pair_with_their_old_version_and_mark_the_changed_words() {
        let old = "Intro.\n\nThe quick brown fox jumps over the lazy dog.\n\nOutro.\n";
        let new = "Intro.\n\nThe quick red fox jumps over the sleepy dog.\n\nOutro.\n";
        let (old_units, new_units) = (Units::parse(old, None), Units::parse(new, None));
        let pairs = align(&old_units, &new_units);
        assert_eq!(pairs[1].change, Change::Modified);
        let words = |text: &str, ranges: &[Range<usize>]| {
            ranges.iter().map(|range| text[range.clone()].to_owned()).collect::<Vec<_>>()
        };
        assert_eq!(words(&old_units.units[1].text, &pairs[1].old_words), ["brown", "lazy"]);
        assert_eq!(words(&new_units.units[1].text, &pairs[1].new_words), ["red", "sleepy"]);
    }

    #[test]
    fn replaced_blocks_and_other_kinds_do_not_pair() {
        let old = "Alpha beta gamma.\n";
        let new = "Something else entirely here.\n";
        assert_eq!(
            changes(old, new),
            [(Some(0), None, Change::Removed), (None, Some(0), Change::Added)]
        );
        let heading_to_paragraph = changes("# Same words\n", "Same words\n");
        assert_eq!(
            heading_to_paragraph,
            [(Some(0), None, Change::Removed), (None, Some(0), Change::Added)]
        );
    }

    #[test]
    fn list_items_line_up_one_by_one() {
        let old = "- apples\n- pears\n- plums\n";
        let new = "- apples\n- cherries\n- pears\n- plums\n";
        assert_eq!(
            changes(old, new),
            [
                (Some(0), Some(0), Change::Equal),
                (None, Some(1), Change::Added),
                (Some(1), Some(2), Change::Equal),
                (Some(2), Some(3), Change::Equal),
            ]
        );
    }

    #[test]
    fn a_run_pairs_the_edited_block_and_keeps_the_rest_one_sided() {
        let old = "Keep.\n\nThis sentence gets a small edit today.\n\nDropped block.\n\nEnd.\n";
        let new =
            "Keep.\n\nBrand new block first.\n\nThis sentence gets a tiny edit today.\n\nEnd.\n";
        assert_eq!(
            changes(old, new),
            [
                (Some(0), Some(0), Change::Equal),
                (None, Some(1), Change::Added),
                (Some(1), Some(2), Change::Modified),
                (Some(2), None, Change::Removed),
                (Some(3), Some(3), Change::Equal),
            ]
        );
    }

    #[test]
    #[expect(
        clippy::single_range_in_vec_init,
        reason = "one changed word: a list of one range, not a range of indices"
    )]
    fn changed_words_of_code_and_phrases() {
        let (old, new) = changed_words("let total = price * 2;", "let total = price * 3;");
        assert_eq!((old, new), (vec![20..21], vec![20..21]));
        let (old, _) = changed_words("keep this old phrase here", "keep this here");
        assert_eq!(old, [10..20], "one mark over the removed phrase");
    }
}
