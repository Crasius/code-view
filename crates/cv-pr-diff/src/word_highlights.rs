//! Word-level highlights of a diff's replaced lines: each deleted line paired with the added line
//! that replaced it (as side-by-side rows pair them), its changed words worked out once, the first
//! time either line is on screen. Shared by the file diff and the folder summary's cards.

use std::collections::HashMap;
use std::ops::Range;

use review_git::{DiffLine, FileDiff, LineKind};

use crate::intraline::{self, Highlights};
use cv_code::diff_paint::{Invisibles, LineStyle};
use cv_code::diff_rows;

#[derive(Default)]
pub(crate) struct WordHighlights {
    /// Deleted ↔ added line pairs (indices into the diff lines), both ways.
    partners: HashMap<u32, u32>,
    /// By the deleted line of a pair.
    computed: HashMap<u32, Highlights>,
}

impl WordHighlights {
    pub(crate) fn new(diff: &FileDiff) -> Self {
        let partners = diff_rows::change_pairs(diff)
            .into_iter()
            .flat_map(|(deleted, added)| [(deleted, added), (added, deleted)])
            .collect();
        Self { partners, computed: HashMap::new() }
    }

    /// The line diff line `index` was paired with: the added line that replaced a deleted one, or
    /// the deleted line an added one replaced.
    fn partner(&self, index: u32) -> Option<u32> {
        self.partners.get(&index).copied()
    }

    /// The invisible characters diff line `index` shows: on a changed line paired with the line it
    /// replaced, its spaces and tabs inside the changed words, and its line ending or byte order
    /// mark where they differ from the partner's; on any line, a missing final newline.
    pub(crate) fn invisibles(&self, diff: &FileDiff, index: u32, style: LineStyle) -> Invisibles {
        let lines = diff.lines();
        let line = &lines[index as usize];
        let partner = self.partner(index).map(|partner| &lines[partner as usize]);
        let is_marked =
            style == LineStyle::Diff && matches!(line.kind, LineKind::Added | LineKind::Deleted);
        let differs = |has: fn(&DiffLine) -> bool| {
            is_marked && partner.is_some_and(|partner| has(partner) != has(line))
        };
        Invisibles {
            whitespace: is_marked && partner.is_some(),
            ending: differs(|line| line.has_cr).then_some(if line.has_cr { "CRLF" } else { "LF" }),
            bom: line.has_bom && is_marked && partner.is_none_or(|partner| !partner.has_bom),
            no_newline: line.missing_newline,
        }
    }

    /// Works out the changed words of the pair diff line `index` (a `kind` line) belongs to, once
    /// per pair; [`Self::words`] reads them. Split from the read so that a caller can borrow the
    /// words while it reads its other state (the diff view paints them every frame).
    pub(crate) fn compute(&mut self, diff: &FileDiff, index: u32, kind: LineKind) {
        let Some((deleted, added)) = self.pair(index, kind) else {
            return;
        };
        self.computed.entry(deleted).or_insert_with(|| {
            let lines = diff.lines();
            intraline::compute(
                diff.text(&lines[deleted as usize]),
                diff.text(&lines[added as usize]),
            )
        });
    }

    /// The changed words (byte ranges) of diff line `index`, which is a `kind` line, as
    /// [`Self::compute`] worked them out; empty for a line without a partner and before `compute`.
    pub(crate) fn words(&self, index: u32, kind: LineKind) -> &[Range<usize>] {
        let Some(highlights) =
            self.pair(index, kind).and_then(|(deleted, _)| self.computed.get(&deleted))
        else {
            return &[];
        };
        if kind == LineKind::Deleted { &highlights.old } else { &highlights.new }
    }

    /// The (deleted, added) pair diff line `index`, a `kind` line, belongs to.
    fn pair(&self, index: u32, kind: LineKind) -> Option<(u32, u32)> {
        let partner = self.partner(index)?;
        Some(if kind == LineKind::Deleted { (index, partner) } else { (partner, index) })
    }
}

#[cfg(test)]
mod tests {
    use review_git::{FileDiff, LineKind};

    use super::WordHighlights;

    #[test]
    #[expect(
        clippy::single_range_in_vec_init,
        reason = "one changed word: a list of one range, not a range of indices"
    )]
    fn highlights_both_lines_of_a_replaced_pair_and_nothing_else() {
        let diff = FileDiff::parse_unified(
            b"@@ -1,3 +1,3 @@\n keep\n-let total = 2;\n+let total = 3;\n-gone\n",
        );
        let mut words = WordHighlights::new(&diff);
        assert!(words.words(2, LineKind::Deleted).is_empty(), "nothing before compute");
        words.compute(&diff, 2, LineKind::Deleted);
        assert_eq!(words.words(2, LineKind::Deleted), [12..13]);
        assert_eq!(words.words(3, LineKind::Added), [12..13], "computed once for the pair");
        words.compute(&diff, 4, LineKind::Deleted);
        assert!(words.words(4, LineKind::Deleted).is_empty(), "a deleted line without a partner");
        words.compute(&diff, 1, LineKind::Context);
        assert!(words.words(1, LineKind::Context).is_empty(), "a context line");
    }
}
