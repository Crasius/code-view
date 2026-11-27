//! The rows of a diff and what is worked out with them: the changes to step through, the collapsed
//! gaps and the lines revealed in them, the threads that found no line, the lines of multi-line
//! threads, the overview strip and the columns each row needs. They are built again only when what
//! they are built from changes ([`BuildKey`] and the threads), not every frame.

use std::sync::Arc;

use egui::{Pos2, Rect};
use review_core::{DiffSide, ReviewThread, ThreadAnchor, ThreadStatus};
use review_git::{FileDiff, LineKind};

use super::DiffInputs;
use crate::diff_composer;
use crate::diff_overview::{self, Overview};
use crate::line_anchor::{self, LineAnchor};
use crate::word_highlights::WordHighlights;
use cv_code::code_job;
use cv_code::contents::SideContents;
use cv_code::diff_paint::{Invisibles, LineStyle, Metrics, RangeMark};
use cv_code::diff_rows::{self, ComposerAnchor, Layout, RangeRows, Row, RowInputs, RowKind};
use cv_code::gaps::{self, Expand, Gap, Reveal};
use cv_code::line_source::{self, Pane};

/// What the rows were built from, besides the threads (see [`ThreadsKey`]).
#[derive(PartialEq, Eq, Clone)]
pub(super) struct BuildKey {
    diff: usize,
    layout: Layout,
    composer: Option<ComposerAnchor>,
    new_line_count: Option<u32>,
    can_expand: bool,
    reveal_version: u64,
}

/// The threads the rows were built for (see [`ThreadKey`]). Compared in place every frame, so an
/// unchanged set of threads allocates nothing.
#[derive(Default)]
struct ThreadsKey(Vec<ThreadKey>);

/// What the rows take from a thread: its id, its comment count, whether it is resolved (an
/// active thread's lines are marked, a resolved one's are not) or outdated (it shows above the
/// rows), and its anchor (the line its card follows). A reload can move a thread to other lines
/// or make it outdated while the diff stays.
struct ThreadKey {
    id: String,
    comments: usize,
    is_resolved: bool,
    is_outdated: bool,
    anchor: Option<ThreadAnchor>,
}

impl ThreadKey {
    fn of(thread: &ReviewThread) -> Self {
        Self {
            id: thread.id.clone(),
            comments: thread.comments.len(),
            is_resolved: is_resolved(thread),
            is_outdated: thread.is_outdated,
            anchor: thread.anchor.clone(),
        }
    }

    fn matches(&self, thread: &ReviewThread) -> bool {
        self.id == thread.id
            && self.comments == thread.comments.len()
            && self.is_resolved == is_resolved(thread)
            && self.is_outdated == thread.is_outdated
            && self.anchor == thread.anchor
    }
}

impl ThreadsKey {
    fn of(threads: &[&ReviewThread]) -> Self {
        Self(threads.iter().map(|thread| ThreadKey::of(thread)).collect())
    }

    fn matches(&self, threads: &[&ReviewThread]) -> bool {
        self.0.len() == threads.len()
            && self.0.iter().zip(threads).all(|(key, thread)| key.matches(thread))
    }
}

fn is_resolved(thread: &ReviewThread) -> bool {
    thread.status == ThreadStatus::Resolved
}

/// How the next build differs from the rows built last.
pub(super) struct Change {
    pub is_new_diff: bool,
    /// The card of a new comment comes into the rows.
    pub card_appears: bool,
    pub keep_place: KeepPlace,
}

/// Whether the line at the top of the view keeps its place on screen across a rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeepPlace {
    /// Another layout (inline, side by side).
    Always,
    /// New commits or comments arrived; rows inside another view's scrolling leave that to the
    /// view, which keeps its cards in place.
    UnlessEmbedded,
    Never,
}

#[derive(Default)]
pub(super) struct DiffRows {
    pub rows: Vec<Row>,
    /// What the rows were built from; `None` before the first build.
    key: Option<BuildKey>,
    threads: ThreadsKey,
    /// The diff the rows were built from, to find the user's place in its successor.
    pub shown_diff: Option<Arc<FileDiff>>,
    /// Threads whose line is in no row, by their index.
    pub detached: Vec<usize>,
    /// The first row of each change.
    pub changes: Vec<u32>,
    pub gaps: Vec<Gap>,
    pub reveals: Vec<Reveal>,
    reveal_version: u64,
    /// Columns of the widest line per pane, and of every code row per pane (for wrapping).
    pub widest: [usize; 2],
    pub row_columns: Vec<[u32; 2]>,
    pub max_line_number: u32,
    /// Changed words of the deleted ↔ added line pairs.
    word_highlights: WordHighlights,
    pub overview: Overview,
    /// Lines of the multi-line threads and of a multi-line draft, and the thread of each.
    pub ranges: Vec<RangeRows>,
    range_threads: Vec<Option<u32>>,
    /// Counts row rebuilds, so the comment target is only worked out again when rows or selection change.
    pub generation: u64,
    /// The row of the new comment's card, if the rows hold one.
    pub composer_row: Option<usize>,
    /// Some row shows code (New only of a deleted or emptied file has none).
    has_code: bool,
}

impl DiffRows {
    /// What rows for `inputs` and the new comment's card at `composer` are built from.
    pub fn key_for(&self, inputs: &DiffInputs<'_>, composer: Option<ComposerAnchor>) -> BuildKey {
        BuildKey {
            diff: Arc::as_ptr(inputs.diff) as usize,
            layout: inputs.layout,
            composer,
            new_line_count: inputs
                .contents
                .and_then(|contents| contents.new.as_ref())
                .map(|side| side.text.line_count() as u32),
            can_expand: inputs.can_expand(),
            reveal_version: self.reveal_version,
        }
    }

    /// How rows built from `key` and `threads` would differ; `None` when they are these rows.
    pub fn change_to(&self, key: &BuildKey, threads: &[&ReviewThread]) -> Option<Change> {
        let same_threads = self.threads.matches(threads);
        let built = self.key.as_ref();
        if built == Some(key) && same_threads {
            return None;
        }
        let keep_place = match built {
            Some(built) if built.layout != key.layout => KeepPlace::Always,
            Some(built) if built.diff != key.diff || !same_threads => KeepPlace::UnlessEmbedded,
            _ => KeepPlace::Never,
        };
        Some(Change {
            is_new_diff: built.is_none_or(|built| built.diff != key.diff),
            card_appears: key.composer.is_some()
                && built.is_none_or(|built| built.composer.is_none()),
            keep_place,
        })
    }

    /// Builds the rows from `key` and returns the rows they replace.
    pub fn rebuild(&mut self, inputs: &DiffInputs<'_>, key: BuildKey) -> Vec<Row> {
        let diff = inputs.diff.as_ref();
        if self.key.as_ref().is_none_or(|built| built.diff != key.diff) {
            self.word_highlights = WordHighlights::new(diff);
        }
        let gaps = gaps::find(diff, key.new_line_count);
        // Revealed lines stay revealed when the gaps are found again: the trailing one appears, a
        // new version of the file moves the hunks.
        self.reveals = gaps::carry_reveals(&self.gaps, &self.reveals, &gaps);
        self.gaps = gaps;
        let built = diff_rows::build(&RowInputs {
            diff,
            layout: inputs.layout,
            gaps: &self.gaps,
            reveals: &self.reveals,
            threads: inputs.threads,
            composer: key.composer.as_ref(),
        });
        let old_rows = std::mem::replace(&mut self.rows, built.rows);
        self.generation += 1;
        self.shown_diff = Some(inputs.diff.clone());
        self.detached = built.detached_threads;
        self.changes = built.changes;
        self.ranges = built.ranges;
        self.range_threads = built.range_threads;
        self.overview = diff_overview::build(diff, &self.rows, inputs.layout);
        self.composer_row = self.rows.iter().position(|row| row.kind == RowKind::Composer);
        self.has_code = self.rows.iter().any(|row| {
            !matches!(row.kind, RowKind::Removal | RowKind::Thread(_) | RowKind::Composer)
        });
        self.measure_columns(inputs);
        self.threads = ThreadsKey::of(inputs.threads);
        self.key = Some(key);
        old_rows
    }

    fn measure_columns(&mut self, inputs: &DiffInputs<'_>) {
        let diff = inputs.diff.as_ref();
        let panes: &[Pane] = if inputs.layout.is_single_pane() {
            &[Pane::Inline]
        } else {
            &[Pane::Left, Pane::Right]
        };
        self.widest = [0; 2];
        self.row_columns.clear();
        self.row_columns.reserve(self.rows.len());
        let mut max_line_number = 0;
        for row in &self.rows {
            let mut columns = [0u32; 2];
            for (slot, pane) in panes.iter().enumerate() {
                if let Some(line) = line_source::code_line(diff, inputs.contents, row, *pane) {
                    let width = code_job::columns(line.text);
                    self.widest[slot] = self.widest[slot].max(width);
                    columns[slot] = width as u32;
                    max_line_number = max_line_number.max(line.old_line.max(line.new_line));
                }
            }
            self.row_columns.push(columns);
        }
        let file_lines = inputs.contents.map_or(0, |contents| {
            let count = |side: &Option<SideContents>| {
                side.as_ref().map_or(0, |side| side.text.line_count())
            };
            count(&contents.old).max(count(&contents.new))
        });
        self.max_line_number = max_line_number.max(file_lines as u32);
    }

    /// New only: nothing of the head version is left to show (the file was deleted or emptied).
    pub fn has_no_head_lines(&self) -> bool {
        !self.has_code
    }

    /// Shows lines of collapsed gap `gap`; the rows are built again next.
    pub fn expand_gap(&mut self, gap: usize, expand: Expand) {
        if let (Some(gap), Some(reveal)) = (self.gaps.get(gap), self.reveals.get_mut(gap)) {
            reveal.expand(gap, expand);
            self.reveal_version += 1;
        }
    }

    /// The collapsed gap holding the line `old_line` / `new_line`.
    pub fn gap_of(&self, old_line: Option<u32>, new_line: Option<u32>) -> Option<usize> {
        self.gaps
            .iter()
            .zip(&self.reveals)
            .position(|(gap, reveal)| reveal.hidden(gap) > 0 && gap.contains(old_line, new_line))
    }

    /// The row holding the anchor's line in these rows of `diff`, or the collapsed gap hiding it.
    pub fn row_of_anchor(&self, anchor: &LineAnchor, diff: &FileDiff) -> Option<usize> {
        let target = line_anchor::locate(anchor, diff)?;
        diff_rows::find_row(diff, &self.rows, target.old_line, target.new_line).or_else(|| {
            self.rows.iter().position(|row| match row.kind {
                RowKind::Gap(gap) => self
                    .gaps
                    .get(gap as usize)
                    .is_some_and(|gap| gap.contains(target.old_line, target.new_line)),
                _ => false,
            })
        })
    }

    /// The threads whose cards follow code row `row_index` in `pane`.
    pub fn pane_threads<'s, 'a: 's>(
        &'s self,
        row_index: usize,
        pane: Pane,
        threads: &'s [&'a ReviewThread],
    ) -> impl Iterator<Item = &'a ReviewThread> + 's {
        self.rows[row_index + 1..]
            .iter()
            .map_while(|row| match row.kind {
                RowKind::Thread(index) => Some(Some(threads[index as usize])),
                RowKind::Composer => Some(None),
                _ => None,
            })
            .flatten()
            .filter(move |thread| {
                let side = if thread.anchor.as_ref().is_some_and(diff_composer::is_right) {
                    DiffSide::New
                } else {
                    DiffSide::Old
                };
                range_in_pane(side, pane)
            })
    }

    /// How code row `row_index` is marked in `pane` as a line of a multi-line comment; a thread
    /// that `is_hidden` leaves its lines plain.
    pub fn range_mark(
        &self,
        row_index: usize,
        pane: Pane,
        is_hidden: impl Fn(u32) -> bool,
    ) -> Option<RangeMark> {
        self.ranges
            .iter()
            .zip(&self.range_threads)
            .find(|(range, thread)| {
                range.contains(row_index)
                    && range_in_pane(range.side, pane)
                    && !thread.is_some_and(&is_hidden)
            })
            .map(|(range, _)| if range.is_draft { RangeMark::Draft } else { RangeMark::Thread })
    }

    /// The invisible characters a line shows: on a changed line paired with the line it replaced,
    /// its spaces and tabs inside the changed words, and its line ending or byte order mark where
    /// they differ from the partner's; on any line, a missing final newline.
    pub fn invisibles(
        &self,
        diff: &FileDiff,
        row: &Row,
        pane: Pane,
        style: LineStyle,
    ) -> Invisibles {
        let Some(index) = line_index(row, pane) else {
            return Invisibles::default();
        };
        self.word_highlights.invisibles(diff, index, style)
    }

    /// Works out the word highlights of `row`'s line in `pane` (a `kind` line) and the line it
    /// was paired with; once per pair. [`Self::words`] reads them.
    pub fn compute_words(&mut self, diff: &FileDiff, row: &Row, pane: Pane, kind: LineKind) {
        if let Some(index) = line_index(row, pane) {
            self.word_highlights.compute(diff, index, kind);
        }
    }

    /// The changed words of `row`'s line in `pane` (a `kind` line), from [`Self::compute_words`].
    pub fn words(&self, row: &Row, pane: Pane, kind: LineKind) -> &[std::ops::Range<usize>] {
        line_index(row, pane).map_or(&[], |index| self.word_highlights.words(index, kind))
    }

    /// Whether `pointer` is on the hit area of a removal wedge at the top or bottom edge of code row
    /// `row_index`, painted at `rect`.
    pub fn is_on_wedge(
        &self,
        pointer: Pos2,
        row_index: usize,
        rect: Rect,
        metrics: &Metrics,
    ) -> bool {
        let is_removal = |index: Option<usize>| {
            index
                .and_then(|index| self.rows.get(index))
                .is_some_and(|row| row.kind == RowKind::Removal)
        };
        let above = is_removal(row_index.checked_sub(1))
            && metrics.wedge_rect(rect.left(), rect.top()).contains(pointer);
        let below = is_removal(Some(row_index + 1))
            && metrics.wedge_rect(rect.left(), rect.bottom()).contains(pointer);
        above || below
    }
}

/// The diff line `pane` shows on a code row, by its index in the diff.
pub(super) fn line_index(row: &Row, pane: Pane) -> Option<u32> {
    if row.kind != RowKind::Line {
        return None;
    }
    match pane {
        Pane::Left => row.left,
        Pane::Right => row.right,
        Pane::Inline => row.right.or(row.left),
    }
}

/// Whether lines of `side` are drawn in `pane`: side by side keeps base lines left and head lines right.
pub(super) fn range_in_pane(side: DiffSide, pane: Pane) -> bool {
    match pane {
        Pane::Inline => true,
        Pane::Left => side == DiffSide::Old,
        Pane::Right => side == DiffSide::New,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find::FindInputs;
    use crate::testing::diff_of;
    use cv_code::contents::{Coloring, FileContents};
    use cv_pr_threads::samples;
    use cv_syntax::blob_text::BlobText;
    use cv_syntax::highlight::LineSpans;

    fn thread(id: &str, comments: usize, status: ThreadStatus) -> ReviewThread {
        let comments = (0..comments)
            .map(|index| samples::comment(&format!("{id}-{index}"), &samples::jonas(), 5, "Body"))
            .collect();
        samples::thread(id, status, Some(samples::lines("a.rs", 3, 3)), comments)
    }

    #[test]
    fn the_threads_key_follows_ids_comment_counts_and_resolution() {
        let one = thread("t1", 1, ThreadStatus::Active);
        let key = ThreadsKey::of(&[&one]);
        assert!(key.matches(&[&one]));
        assert!(!key.matches(&[]), "a thread went");
        assert!(!key.matches(&[&one, &one]), "a thread came");
        assert!(!key.matches(&[&thread("t2", 1, ThreadStatus::Active)]), "another thread");
        assert!(!key.matches(&[&thread("t1", 2, ThreadStatus::Active)]), "a reply");
        assert!(!key.matches(&[&thread("t1", 1, ThreadStatus::Resolved)]), "resolved");
    }

    /// Regression: a reload that moved a thread to other lines or made it outdated, while the diff
    /// stayed, left its card under the line it had before.
    #[test]
    fn the_threads_key_follows_a_moved_or_outdated_thread() {
        let one = thread("t1", 1, ThreadStatus::Active);
        let key = ThreadsKey::of(&[&one]);
        let mut moved = one.clone();
        moved.anchor = Some(samples::lines("a.rs", 7, 7));
        assert!(!key.matches(&[&moved]), "on another line");
        let mut outdated = one;
        outdated.is_outdated = true;
        assert!(!key.matches(&[&outdated]), "outdated");
    }

    /// The rows of the 60-line `head` against `BASE`, built after `rows` were.
    fn rebuilt(rows: &mut DiffRows, head: &str) {
        let diff = diff_of(BASE, head);
        let side = |text: &str| SideContents {
            text: Arc::new(BlobText::from_bytes(text.as_bytes().to_vec())),
            spans: Arc::new(LineSpans::default()),
        };
        let contents = FileContents {
            old: Some(side(BASE)),
            new: Some(side(head)),
            coloring: Coloring::Done,
            error: None,
        };
        let inputs = DiffInputs {
            diff: &diff,
            contents: Some(&contents),
            threads: &[],
            layout: Layout::Inline,
            font_size: 13.0,
            wrap: false,
            can_comment: true,
            can_comment_outside_hunks: true,
            path: "a.rs",
            old_path: None,
            line_spans: None,
            show_threads: true,
            find: FindInputs::default(),
        };
        let key = rows.key_for(&inputs, None);
        rows.rebuild(&inputs, key);
    }

    const BASE: &str = concat!(
        "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n17\n18\n19\n20\n",
        "21\n22\n23\n24\n25\n26\n27\n28\n29\n30\n31\n32\n33\n34\n35\n36\n37\n38\n39\n40\n",
        "41\n42\n43\n44\n45\n46\n47\n48\n49\n50\n51\n52\n53\n54\n55\n56\n57\n58\n59\n60\n",
    );

    /// (base lines, hidden lines) of each gap.
    fn gap_shape(rows: &DiffRows) -> Vec<(std::ops::Range<u32>, u32)> {
        rows.gaps
            .iter()
            .zip(&rows.reveals)
            .map(|(gap, reveal)| (gap.old_start..gap.old_start + gap.len, reveal.hidden(gap)))
            .collect()
    }

    /// Regression: a new version of the open file (a live update) with a change further up showed
    /// the lines revealed in one gap in the gap that took its place in the list, and collapsed
    /// the revealed ones again.
    #[test]
    fn revealed_lines_stay_revealed_when_a_new_version_moves_the_gaps() {
        let first = BASE.replace("\n40\n", "\nforty\n");
        let mut rows = DiffRows::default();
        rebuilt(&mut rows, &first);
        assert_eq!(gap_shape(&rows), [(1..37, 36), (44..61, 17)]);
        rows.expand_gap(0, Expand::Down);
        rows.expand_gap(1, Expand::All);

        rebuilt(&mut rows, &first.replace("\n10\n", "\nten\n"));
        assert_eq!(
            gap_shape(&rows),
            [(1..7, 0), (14..37, 16), (44..61, 0)],
            "lines 1 to 20 and the end stay revealed, 21 to 36 stay hidden"
        );
        assert_eq!(rows.reveals[1], Reveal { top: 7, bottom: 0 });
    }
}
