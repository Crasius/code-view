//! The find widget of the text diff. The search reads the whole file in the order its rows show:
//! the unchanged lines git left out (from the file's two versions, once they are loaded), the
//! hunks' lines, and side by side each replaced line next to its replacement, so stepping goes
//! down the rows. A hit in a collapsed gap is gone to with a [`LineRequest::Find`], which shows
//! the gap's lines before it scrolls (the path a thread jump takes); any other hit scrolls to the
//! middle of the view unless its row is in view already. Side by side the widget searches the
//! focused pane's version only (see [`super::bar`]).
//!
//! The rows paint each pane line's hits ([`TextFind::marks`]); the overview strip gets a tick
//! per hit at its row (a hidden line's at its gap), worked out again only when the rows are
//! rebuilt or new hits arrive. The other occurrences of the text selected within one line show
//! beside them all the time, in a colour of their own ([`Occurrences`]); where a range is both,
//! the hit shows.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use egui::{Rect, Ui};
use review_core::DiffSide;
use review_git::{FileDiff, LineKind};

use super::bar::{FindBar, Panes};
use super::occurrences::Occurrences;
use super::{Entry, Found, Hit, Hits, MatchOptions, Query, search, spawn};
use crate::diff_overview::Overview;
use crate::diff_view::{DiffInputs, LineRequest};
use crate::selection::Selection;
use cv_code::diff_rows::{Layout, Row, RowKind};
use cv_code::find_paint::{self as paint, FindMark, HitSide, MarkLook};
use cv_code::gaps::{self, Gap};
use cv_code::line_source::{CodeLine, Pane};
use cv_syntax::blob_text::BlobText;

/// The text diff's find widget and where its hits are among the rows.
#[derive(Default)]
pub struct TextFind {
    bar: FindBar,
    /// The panes this frame and the side searched, for the widget painted after the rows.
    focus: Option<(Panes, HitSide)>,
    /// The row of each hit (a hidden line's gap), for the rows of `rows_generation`.
    hit_rows: Vec<Option<u32>>,
    rows_generation: Option<u64>,
    /// A hit became current: its pane scrolls sideways to it once its line is painted.
    wants_reveal: bool,
    /// Where the panes scroll sideways next frame, for the current hit.
    sideways: [Option<f32>; 2],
    occurrences: Occurrences,
    /// The row of each occurrence of the selection, like `hit_rows`.
    occurrence_rows: Vec<Option<u32>>,
}

/// Text selected within one line of the diff, whose other occurrences show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    pub text: String,
    /// The selection is a whole word there, so only whole words match.
    pub is_word: bool,
}

/// The rows the find widget works with this frame.
pub struct TextRows<'a> {
    /// The panes the rows show in.
    pub panes: Panes,
    pub rows: &'a [Row],
    pub gaps: &'a [Gap],
    /// Changes with every rebuild of `rows`.
    pub generation: u64,
    /// The top of each row and the end of the last.
    pub offsets: &'a [f32],
    /// The part of the rows in view, in row coordinates.
    pub view: Rect,
    /// The strip the hits tick on.
    pub overview: &'a mut Overview,
    /// How far each pane is scrolled sideways.
    pub sideways: &'a mut [f32; 2],
    /// The text selected within one line, if any.
    pub selected: Option<Selected>,
    /// What is selected, if anything: its pane has the focus.
    pub selection: Option<Selection>,
}

/// Where the selection's other occurrences are among the rows, for tests.
#[cfg(test)]
pub struct OccurrenceRows {
    /// The row of each occurrence (a hidden line's gap).
    pub rows: Vec<Option<u32>>,
    /// The overview strip's ticks.
    pub ticks: Vec<u32>,
    /// The rows that are collapsed gaps.
    pub gap_rows: Vec<u32>,
}

impl TextFind {
    /// The widget's keys while the diff `owns_keys`, the focused pane, the search over `inputs`
    /// and where to go for the current hit; before the rows paint (the widget paints after them,
    /// [`Self::show_widget`]).
    pub fn prepare(
        &mut self,
        ui: &Ui,
        inputs: &DiffInputs<'_>,
        owns_keys: bool,
        selected: impl FnOnce() -> Option<String>,
        rows: &mut TextRows<'_>,
    ) -> Option<LineRequest> {
        let step = self.bar.keys(ui, owns_keys, selected);
        let selection = rows.selection.filter(|selection| !selection.is_empty());
        let selected_side = selection.map(|selection| side_of(selection.pane));
        let side = self.bar.follow_focus(ui, &rows.panes, selected_side);
        self.focus = Some((rows.panes, side));
        let (source, start) = searcher(inputs);
        let start_find = start.clone();
        self.bar
            .refresh(ui.ctx(), side, source, |query| spawn(ui.ctx(), move || start_find(query)));
        let arrived = self.bar.poll();
        let wanted = occurrence_query(inputs, rows.selected.take());
        let occurred =
            self.occurrences.follow(wanted, source, |query| spawn(ui.ctx(), move || start(query)));
        let is_stale = self.rows_generation != Some(rows.generation);
        if arrived || occurred || is_stale {
            self.place_hits(rows, inputs.diff);
        }
        let jump = match step {
            Some(step) => self.bar.step(step).is_some(),
            None if arrived => {
                self.bar.set_current(self.first_in_view(rows));
                self.bar.current().is_some()
            }
            None => false,
        };
        self.tick(rows.overview);
        self.scroll_sideways(ui, rows.sideways);
        self.wants_reveal |= jump;
        let jump = jump.then(|| self.jump(rows)).flatten();
        if jump.is_some() {
            ui.ctx().request_repaint();
        }
        jump
    }

    /// The widget over the focused pane, after the rows painted; true when it closed.
    pub fn show_widget(&mut self, ui: &mut Ui) -> bool {
        let Some((panes, side)) = self.focus else {
            return false;
        };
        self.bar.show(ui, &panes, side)
    }

    /// The hits `pane` paints on `line`, the current one marked, and the selection's other
    /// occurrences that no hit covers.
    pub fn marks(&self, pane: Pane, line: Option<&CodeLine<'_>>) -> Vec<FindMark> {
        let Some(line) = line else {
            return Vec::new();
        };
        let mut marks = Vec::new();
        if let Some(hits) = self.bar.hits() {
            let current = self.bar.current();
            let look =
                |index| if current == Some(index) { MarkLook::Current } else { MarkLook::Hit };
            let found = on_line(hits, pane, line).into_iter();
            marks.extend(found.map(|(index, bytes)| FindMark { bytes, look: look(index) }));
        }
        let Some(occurrences) = self.occurrences.hits() else {
            return marks;
        };
        let hits = marks.len();
        for (_, bytes) in on_line(occurrences, pane, line) {
            let overlaps =
                |mark: &FindMark| mark.bytes.start < bytes.end && bytes.start < mark.bytes.end;
            if !marks.get(..hits).unwrap_or_default().iter().any(overlaps) {
                marks.push(FindMark { bytes, look: MarkLook::Occurrence });
            }
        }
        marks
    }

    #[cfg(test)]
    pub fn occurrence_rows(&self) -> Option<&[Option<u32>]> {
        self.occurrences.hits().map(|_| self.occurrence_rows.as_slice())
    }

    /// The current hit on a line painted in pane `slot` at `columns`, `advance` wide each, of a
    /// code area `width` wide scrolled `offset`: once it became current, the pane scrolls to it
    /// next frame.
    pub fn reveal(
        &mut self,
        slot: usize,
        columns: std::ops::Range<usize>,
        advance: f32,
        width: f32,
        offset: f32,
    ) {
        if self.wants_reveal
            && let Some(pending) = self.sideways.get_mut(slot)
        {
            *pending = Some(paint::reveal_offset(columns, advance, width, offset));
        }
    }

    /// Scrolls the panes sideways where the current hit asked.
    fn scroll_sideways(&mut self, ui: &Ui, offsets: &mut [f32; 2]) {
        for (offset, pending) in offsets.iter_mut().zip(&mut self.sideways) {
            if let Some(to) = pending.take() {
                self.wants_reveal = false;
                if (*offset - to).abs() > 0.5 {
                    *offset = to;
                    ui.ctx().request_repaint();
                }
            }
        }
    }

    /// Works out the row of every hit in `rows`: its line's row, or the collapsed gap hiding it.
    fn place_hits(&mut self, rows: &TextRows<'_>, diff: &FileDiff) {
        self.rows_generation = Some(rows.generation);
        self.hit_rows.clear();
        self.occurrence_rows.clear();
        if self.bar.hits().is_none() && self.occurrences.hits().is_none() {
            return;
        }
        let lines = LineRows::of(rows.rows, rows.gaps, diff);
        let place = |hits: Option<&Hits>| -> Vec<Option<u32>> {
            hits.map_or_else(Vec::new, |hits| {
                hits.list.iter().map(|hit| lines.row_of(hit)).collect()
            })
        };
        self.hit_rows = place(self.bar.hits());
        self.occurrence_rows = place(self.occurrences.hits());
    }

    /// The first hit at or below the top of the view, else the first one.
    fn first_in_view(&self, rows: &TextRows<'_>) -> Option<usize> {
        let top_of = |row: u32| rows.offsets.get(row as usize).copied();
        let below = self
            .hit_rows
            .iter()
            .position(|row| row.and_then(top_of).is_some_and(|top| top >= rows.view.top() - 0.5));
        below.or_else(|| (!self.hit_rows.is_empty()).then_some(0))
    }

    /// Where to go for the current hit; nothing while its row is in view already.
    fn jump(&self, rows: &TextRows<'_>) -> Option<LineRequest> {
        let current = self.bar.current()?;
        let hit = self.bar.hits()?.list.get(current)?;
        let row = self.hit_rows.get(current).copied().flatten();
        let is_code = row
            .and_then(|row| rows.rows.get(row as usize))
            .is_some_and(|row| matches!(row.kind, RowKind::Line | RowKind::Expanded));
        let span = row.and_then(|row| {
            Some((*rows.offsets.get(row as usize)?, *rows.offsets.get(row as usize + 1)?))
        });
        let is_in_view = span.is_some_and(|(top, bottom)| {
            is_code && top >= rows.view.top() + self.bar.cover() && bottom <= rows.view.bottom()
        });
        if is_in_view {
            return None;
        }
        Some(match (hit.new, hit.old) {
            (Some(line), _) => LineRequest::Find { side: DiffSide::New, line },
            (None, Some(line)) => LineRequest::Find { side: DiffSide::Old, line },
            (None, None) => return None,
        })
    }

    /// The overview strip's ticks of the hits, the current one marked, and of the occurrences.
    fn tick(&self, overview: &mut Overview) {
        overview.finds.clear();
        overview.current_find = None;
        overview.occurrences.clear();
        if self.occurrences.hits().is_some() {
            overview.occurrences.extend(self.occurrence_rows.iter().flatten());
        }
        let Some(hits) = self.bar.hits() else {
            return;
        };
        for (index, (hit, row)) in hits.list.iter().zip(&self.hit_rows).enumerate() {
            let Some(row) = *row else {
                continue;
            };
            overview.finds.push((row, hit.side));
            if self.bar.current() == Some(index) {
                overview.current_find = Some(row);
            }
        }
    }
}

/// The hits among `hits` that `pane` paints on `line`: their index and bytes.
fn on_line(hits: &Hits, pane: Pane, line: &CodeLine<'_>) -> Vec<(usize, Range<usize>)> {
    let is_old = pane == Pane::Left || (pane == Pane::Inline && line.kind == LineKind::Deleted);
    let found: Vec<(usize, &Hit)> = match is_old {
        true if line.old_line > 0 && line.kind != LineKind::Added => hits
            .on_old(line.old_line)
            .filter(|(_, hit)| pane == Pane::Inline || hit.side != HitSide::New)
            .collect(),
        false if line.new_line > 0 && line.kind != LineKind::Deleted => hits
            .on_new(line.new_line)
            .filter(|(_, hit)| pane == Pane::Inline || hit.side != HitSide::Old)
            .collect(),
        _ => return Vec::new(),
    };
    found
        .into_iter()
        .filter(|(_, hit)| {
            line.text.is_char_boundary(hit.bytes.start) && line.text.is_char_boundary(hit.bytes.end)
        })
        .map(|(index, hit)| (index, hit.bytes.clone()))
        .collect()
}

/// The rows showing each line of the old and the new version, and the collapsed gaps.
struct LineRows {
    old: HashMap<u32, u32>,
    new: HashMap<u32, u32>,
    gaps: Vec<(Gap, u32)>,
}

impl LineRows {
    fn of(rows: &[Row], gaps: &[Gap], diff: &FileDiff) -> Self {
        let lines = diff.lines();
        let line = |index: Option<u32>| index.and_then(|index| lines.get(index as usize));
        let mut found = Self { old: HashMap::new(), new: HashMap::new(), gaps: Vec::new() };
        for (index, row) in rows.iter().enumerate() {
            let index = index as u32;
            match row.kind {
                RowKind::Line => {
                    if let Some(left) = line(row.left).filter(|line| line.kind != LineKind::Added) {
                        found.old.insert(left.old_line, index);
                    }
                    if let Some(right) =
                        line(row.right).filter(|line| line.kind != LineKind::Deleted)
                    {
                        found.new.insert(right.new_line, index);
                    }
                }
                RowKind::Removal => {
                    for removed in row.removed_lines().into_iter().flatten() {
                        found.old.extend(lines.get(removed).map(|line| (line.old_line, index)));
                    }
                }
                RowKind::Expanded => {
                    found.old.extend(row.left.map(|line| (line, index)));
                    found.new.extend(row.right.map(|line| (line, index)));
                }
                RowKind::Gap(gap) => {
                    found.gaps.extend(gaps.get(gap as usize).map(|gap| (*gap, index)));
                }
                RowKind::Thread(_) | RowKind::Composer | RowKind::Separator => {}
            }
        }
        found
    }

    /// The row of `hit` among `rows`.
    fn row_of(&self, hit: &Hit) -> Option<u32> {
        let by_new = hit.new.and_then(|line| self.new.get(&line));
        let by_old = hit.old.and_then(|line| self.old.get(&line));
        let in_gap = || {
            let gap = self.gaps.iter().find(|(gap, _)| gap.contains(hit.old, hit.new));
            gap.map(|(_, row)| *row)
        };
        by_new.or(by_old).copied().or_else(in_gap)
    }
}

/// What the search of `inputs` runs over: an id of the text (the diff, the file's two versions
/// and whether replaced lines pair up), and the search itself, to run off the UI thread.
fn searcher(
    inputs: &DiffInputs<'_>,
) -> (u64, impl FnOnce(Query) -> Found + Send + Clone + 'static) {
    let diff = inputs.diff.clone();
    let blob = |side: Option<&cv_code::contents::SideContents>| side.map(|side| side.text.clone());
    let old = blob(inputs.contents.and_then(|contents| contents.old.as_ref()));
    let new = blob(inputs.contents.and_then(|contents| contents.new.as_ref()));
    let paired = matches!(inputs.layout, Layout::SideBySide);
    let source = egui::epaint::util::hash((
        Arc::as_ptr(&diff) as usize,
        old.as_ref().map(|text| Arc::as_ptr(text) as usize),
        new.as_ref().map(|text| Arc::as_ptr(text) as usize),
        paired,
    ));
    let search_all = move |query: Query| {
        let entries = entries(&diff, old.as_deref(), new.as_deref(), paired);
        let hits = search(entries, &query);
        Found::new(query, source, hits)
    };
    (source, search_all)
}

/// The version a pane shows.
fn side_of(pane: Pane) -> HitSide {
    match pane {
        Pane::Left => HitSide::Old,
        Pane::Right => HitSide::New,
        Pane::Inline => HitSide::Both,
    }
}

/// The file's lines in the order their rows show: the unchanged lines git left out (with both
/// versions loaded), the hunks' lines, and with `paired` each replaced line beside its replacement.
fn entries<'a>(
    diff: &'a FileDiff,
    old: Option<&'a BlobText>,
    new: Option<&'a BlobText>,
    paired: bool,
) -> Vec<Entry<'a>> {
    let lines = diff.lines();
    let both = old.zip(new);
    let gaps =
        both.map_or_else(Vec::new, |(_, new)| gaps::find(diff, Some(new.line_count() as u32)));
    let mut entries = Vec::with_capacity(lines.len());
    let push_gap = |entries: &mut Vec<Entry<'a>>, gap: &Gap| {
        let Some((old, new)) = both else {
            return;
        };
        for offset in 0..gap.len {
            let (old_line, new_line) = (gap.old_start + offset, gap.new_start + offset);
            let texts = old.line(old_line as usize - 1).zip(new.line(new_line as usize - 1));
            if let Some((old_text, new_text)) = texts {
                entries.push(Entry {
                    old: Some((old_line, old_text)),
                    new: Some((new_line, new_text)),
                    is_unchanged: true,
                });
            }
        }
    };
    let (mut deleted, mut added) = (Vec::new(), Vec::new());
    let flush = |entries: &mut Vec<Entry<'a>>, deleted: &mut Vec<usize>, added: &mut Vec<usize>| {
        for pair in 0..deleted.len().max(added.len()) {
            let side = |indices: &[usize], number: fn(&review_git::DiffLine) -> u32| {
                indices.get(pair).map(|index| (number(&lines[*index]), diff.text(&lines[*index])))
            };
            entries.push(Entry {
                old: side(deleted, |line| line.old_line),
                new: side(added, |line| line.new_line),
                is_unchanged: false,
            });
        }
        deleted.clear();
        added.clear();
    };
    let mut next_gap = 0;
    for (index, line) in lines.iter().enumerate() {
        match line.kind {
            LineKind::Hunk => {
                flush(&mut entries, &mut deleted, &mut added);
                while let Some(gap) = gaps
                    .get(next_gap)
                    .filter(|gap| gap.hunk.is_some_and(|hunk| hunk as usize <= index))
                {
                    push_gap(&mut entries, gap);
                    next_gap += 1;
                }
            }
            LineKind::Context => {
                flush(&mut entries, &mut deleted, &mut added);
                let text = diff.text(line);
                entries.push(Entry {
                    old: Some((line.old_line, text)),
                    new: Some((line.new_line, text)),
                    is_unchanged: true,
                });
            }
            LineKind::Deleted => {
                if !added.is_empty() || !paired {
                    flush(&mut entries, &mut deleted, &mut added);
                }
                deleted.push(index);
            }
            LineKind::Added => {
                if !paired {
                    flush(&mut entries, &mut deleted, &mut added);
                }
                added.push(index);
            }
        }
    }
    flush(&mut entries, &mut deleted, &mut added);
    for gap in gaps.get(next_gap..).unwrap_or_default() {
        push_gap(&mut entries, gap);
    }
    entries
}

/// What the selection's other occurrences are searched with: the `selected` text exactly, as a
/// whole word if it is one, on both sides, whatever the widget searches. None while the setting
/// is off or the selection is only blanks.
fn occurrence_query(inputs: &DiffInputs<'_>, selected: Option<Selected>) -> Option<Query> {
    let selected = selected
        .filter(|selected| inputs.find.highlight_occurrences && !selected.text.trim().is_empty())?;
    let options = MatchOptions { match_case: true, whole_word: selected.is_word };
    Some(Query { text: selected.text, options, side: HitSide::Both })
}

#[cfg(test)]
mod tests {
    use cv_code::diff_rows::{self, RowInputs};

    use super::*;

    /// Lines 5 and 6 of ten replaced by three, with one line of context around them: three
    /// unchanged lines above the hunk and three below it stay out of the diff.
    const PATCH: &[u8] = b"@@ -4,4 +4,5 @@\n line 4\n-old five\n-old six\n+new five a\n+new five b\n+new five c\n line 7\n";

    fn versions() -> (BlobText, BlobText) {
        let line = |number: u32| format!("line {number}\n");
        let old: String = (1..=10)
            .map(|number| match number {
                5 => "old five\n".to_owned(),
                6 => "old six\n".to_owned(),
                number => line(number),
            })
            .collect();
        let replaced = ["new five a\n", "new five b\n", "new five c\n"].map(str::to_owned);
        let new: String =
            (1..=4).map(line).chain(replaced).chain((7..=10).map(line)).collect::<String>();
        (BlobText::from_bytes(old.into_bytes()), BlobText::from_bytes(new.into_bytes()))
    }

    /// Each entry's old and new number and whether it is unchanged.
    fn shape(entries: &[Entry<'_>]) -> Vec<(Option<u32>, Option<u32>, bool)> {
        let number = |side: Option<(u32, &str)>| side.map(|(number, _)| number);
        entries
            .iter()
            .map(|entry| (number(entry.old), number(entry.new), entry.is_unchanged))
            .collect()
    }

    #[test]
    fn entries_follow_the_rows_with_the_hidden_lines_around_the_hunks() {
        let diff = FileDiff::parse_unified(PATCH);
        let (old, new) = versions();
        let above = [(Some(1), Some(1), true), (Some(2), Some(2), true), (Some(3), Some(3), true)];
        let below =
            [(Some(8), Some(9), true), (Some(9), Some(10), true), (Some(10), Some(11), true)];
        let paired = [
            (Some(4), Some(4), true),
            (Some(5), Some(5), false),
            (Some(6), Some(6), false),
            (None, Some(7), false),
            (Some(7), Some(8), true),
        ];
        let inline = [
            (Some(4), Some(4), true),
            (Some(5), None, false),
            (Some(6), None, false),
            (None, Some(5), false),
            (None, Some(6), false),
            (None, Some(7), false),
            (Some(7), Some(8), true),
        ];
        let cases = [
            (
                "side by side: each deleted line beside the added line that replaced it",
                true,
                Some((&old, &new)),
                [&above[..], &paired[..], &below[..]].concat(),
            ),
            (
                "in one pane: every changed line alone",
                false,
                Some((&old, &new)),
                [&above[..], &inline[..], &below[..]].concat(),
            ),
            ("without both versions: no hidden lines", true, None, paired.to_vec()),
        ];
        for (case, is_paired, versions, expected) in cases {
            let (old, new) = (versions.map(|(old, _)| old), versions.map(|(_, new)| new));
            assert_eq!(shape(&entries(&diff, old, new, is_paired)), expected, "{case}");
        }
    }

    #[test]
    fn a_hit_finds_its_line_row_or_the_gap_hiding_it() {
        let diff = FileDiff::parse_unified(PATCH);
        let gaps = gaps::find(&diff, Some(11));
        let hit =
            |side: HitSide, old: Option<u32>, new: Option<u32>| Hit { side, old, new, bytes: 0..4 };
        let cases = [
            (
                "a context line",
                Layout::Inline,
                hit(HitSide::Both, Some(4), Some(4)),
                RowKind::Line,
                Some(1),
            ),
            (
                "a deleted line in one pane",
                Layout::Inline,
                hit(HitSide::Old, Some(5), None),
                RowKind::Line,
                Some(2),
            ),
            (
                "a deleted line in New only",
                Layout::NewOnly,
                hit(HitSide::Old, Some(5), None),
                RowKind::Removal,
                None,
            ),
            (
                "a line hidden above the hunk",
                Layout::Inline,
                hit(HitSide::Both, Some(2), Some(2)),
                RowKind::Gap(0),
                None,
            ),
            (
                "a line hidden below it",
                Layout::Inline,
                hit(HitSide::New, None, Some(10)),
                RowKind::Gap(1),
                None,
            ),
        ];
        for (case, layout, hit, kind, diff_line) in cases {
            let inputs = RowInputs {
                diff: &diff,
                layout,
                gaps: &gaps,
                reveals: &[],
                threads: &[],
                composer: None,
            };
            let rows = diff_rows::build(&inputs).rows;
            let row = LineRows::of(&rows, &gaps, &diff).row_of(&hit).expect(case);
            let row = &rows[row as usize];
            assert_eq!(row.kind, kind, "{case}");
            if let Some(diff_line) = diff_line {
                assert!([row.left, row.right].contains(&Some(diff_line)), "{case}: {row:?}");
            }
        }
    }

    #[test]
    fn each_pane_paints_the_hits_of_the_version_it_shows() {
        let hits = Hits::new(vec![
            Hit { side: HitSide::Old, old: Some(5), new: None, bytes: 0..3 },
            Hit { side: HitSide::New, old: None, new: Some(5), bytes: 0..3 },
            Hit { side: HitSide::New, old: Some(4), new: Some(4), bytes: 5..6 },
        ]);
        let line = |kind: LineKind, old_line: u32, new_line: u32| CodeLine {
            text: "old five",
            kind,
            old_line,
            new_line,
            number: new_line.max(old_line),
            spans: &[],
            has_file_spans: false,
        };
        let cases = [
            (
                "one pane reads a deleted line's old hits",
                Pane::Inline,
                line(LineKind::Deleted, 5, 0),
                vec![(0, 0..3)],
            ),
            (
                "and an added line's new ones",
                Pane::Inline,
                line(LineKind::Added, 0, 5),
                vec![(1, 0..3)],
            ),
            (
                "the right pane paints a hit of the new version",
                Pane::Right,
                line(LineKind::Context, 4, 4),
                vec![(2, 5..6)],
            ),
            ("the left pane leaves it out", Pane::Left, line(LineKind::Context, 4, 4), vec![]),
        ];
        for (case, pane, line, expected) in cases {
            assert_eq!(on_line(&hits, pane, &line), expected, "{case}");
        }
    }
}
