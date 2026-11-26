//! Unchanged lines git left out around the hunks, and how much of each gap the user revealed.

use review_git::{FileDiff, LineKind};

/// Lines one click on "expand up/down" reveals.
pub const EXPAND_STEP: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    /// First hidden line in the base and in the head file (1-based).
    pub old_start: u32,
    pub new_start: u32,
    pub len: u32,
    /// The hunk header line (index into [`FileDiff::lines`]) right below; `None` for the gap after the last hunk.
    pub hunk: Option<u32>,
}

impl Gap {
    /// The gap at the top of the file only grows upwards from its hunk.
    pub fn can_expand_down(&self) -> bool {
        self.old_start > 1 || self.new_start > 1
    }

    /// The gap after the last hunk only grows downwards.
    pub fn can_expand_up(&self) -> bool {
        self.hunk.is_some()
    }

    /// Whether head line `new_line` (or base line `old_line`) lies in this gap.
    pub fn contains(&self, old_line: Option<u32>, new_line: Option<u32>) -> bool {
        let in_range = |start: u32, line: u32| line >= start && line < start + self.len;
        match (new_line, old_line) {
            (Some(line), _) => in_range(self.new_start, line),
            (None, Some(line)) => in_range(self.old_start, line),
            (None, None) => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expand {
    /// Reveal the lines right below the previous hunk.
    Down,
    /// Reveal the lines right above the next hunk.
    Up,
    All,
}

/// Revealed lines at the top (below the previous hunk) and bottom (above the next hunk) of a gap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reveal {
    pub top: u32,
    pub bottom: u32,
}

impl Reveal {
    pub fn hidden(self, gap: &Gap) -> u32 {
        gap.len.saturating_sub(self.top + self.bottom)
    }

    pub fn expand(&mut self, gap: &Gap, expand: Expand) {
        let hidden = self.hidden(gap);
        match expand {
            Expand::Down => self.top += hidden.min(EXPAND_STEP),
            Expand::Up => self.bottom += hidden.min(EXPAND_STEP),
            Expand::All => self.top += hidden,
        }
    }
}

/// What `reveals` showed of `gaps` carried over to `new_gaps`, the gaps of the same file found
/// again (its trailing gap appeared, a new version moved the hunks): each new gap shows the base
/// lines at its ends that showed before. By line, not by index: a hunk more above shifts every
/// gap's index, and the revealed lines would show in another gap.
pub fn carry_reveals(gaps: &[Gap], reveals: &[Reveal], new_gaps: &[Gap]) -> Vec<Reveal> {
    // The revealed base lines, in file order: the gaps are, and so are their two ends.
    let mut shown: Vec<std::ops::Range<u32>> = Vec::new();
    for (gap, reveal) in gaps.iter().zip(reveals) {
        let end = gap.old_start + gap.len;
        let top = reveal.top.min(gap.len);
        let bottom = reveal.bottom.min(gap.len - top);
        for range in [gap.old_start..gap.old_start + top, end - bottom..end] {
            if !range.is_empty() {
                shown.push(range);
            }
        }
    }
    let shown_at = |line: u32| {
        let index = shown.partition_point(|range| range.end <= line);
        shown.get(index).filter(|range| range.start <= line)
    };
    new_gaps
        .iter()
        .map(|gap| {
            let (start, end) = (gap.old_start, gap.old_start + gap.len);
            let top = shown_at(start).map_or(0, |range| range.end.min(end) - start);
            let bottom = shown_at(end - 1)
                .filter(|_| top < gap.len)
                .map_or(0, |range| end - range.start.max(start + top));
            Reveal { top, bottom }
        })
        .collect()
}

/// The gaps of `diff` in file order. The gap after the last hunk needs the head file's line count.
pub fn find(diff: &FileDiff, new_line_count: Option<u32>) -> Vec<Gap> {
    let lines = diff.lines();
    let mut gaps = Vec::new();
    // Last line of the previous hunk in base and head (0 before the first hunk).
    let mut old_end = 0u32;
    let mut new_end = 0u32;
    for (index, line) in lines.iter().enumerate() {
        match line.kind {
            LineKind::Hunk => {
                // A hunk with no lines on a side (`@@ -10,0 +11,2 @@` of a diff without context)
                // names the line before it there, not its first line.
                let body = lines[index + 1..].iter().take_while(|line| line.kind != LineKind::Hunk);
                let (mut has_old, mut has_new) = (false, false);
                for line in body {
                    has_old |= line.kind != LineKind::Added;
                    has_new |= line.kind != LineKind::Deleted;
                }
                let old_before =
                    if has_old { line.old_line.saturating_sub(1) } else { line.old_line };
                let new_before =
                    if has_new { line.new_line.saturating_sub(1) } else { line.new_line };
                // An empty side (added or deleted file) hides nothing: its count is 0.
                let len =
                    new_before.saturating_sub(new_end).min(old_before.saturating_sub(old_end));
                if len > 0 {
                    gaps.push(Gap {
                        old_start: old_end + 1,
                        new_start: new_end + 1,
                        len,
                        hunk: Some(index as u32),
                    });
                }
                // The hunk's lines move the ends on; a side it has no lines on ends right before it.
                old_end = old_before;
                new_end = new_before;
            }
            LineKind::Context => {
                old_end = line.old_line;
                new_end = line.new_line;
            }
            LineKind::Added => new_end = line.new_line,
            LineKind::Deleted => old_end = line.old_line,
        }
    }
    let has_hunks = diff.lines().first().is_some_and(|line| line.kind == LineKind::Hunk);
    if let Some(count) = new_line_count
        && has_hunks
        && new_end > 0
        && count > new_end
    {
        gaps.push(Gap {
            old_start: old_end + 1,
            new_start: new_end + 1,
            len: count - new_end,
            hunk: None,
        });
    }
    gaps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_hunks() -> FileDiff {
        let text = concat!(
            "@@ -5,3 +5,4 @@ fn first()\n",
            " a\n",
            "+b\n",
            " c\n",
            " d\n",
            "@@ -20,3 +21,2 @@ fn second()\n",
            " e\n",
            "-f\n",
            " g\n",
        );
        FileDiff::parse_unified(text.as_bytes())
    }

    #[test]
    fn finds_gaps_before_between_and_after_hunks() {
        let gaps = find(&two_hunks(), Some(40));
        assert_eq!(
            gaps,
            [
                Gap { old_start: 1, new_start: 1, len: 4, hunk: Some(0) },
                Gap { old_start: 8, new_start: 9, len: 12, hunk: Some(5) },
                Gap { old_start: 23, new_start: 23, len: 18, hunk: None },
            ]
        );
        assert_eq!(find(&two_hunks(), None).len(), 2, "the trailing gap waits for the file length");
    }

    /// Regression: with 0 lines of context a hunk that only adds (or only deletes) names the line
    /// before it on that side. The gap above it left that line out, and the next gap started its
    /// base (or head) lines at 1.
    #[test]
    fn gaps_around_hunks_without_context_hide_every_unchanged_line_once() {
        // 30 lines; two lines added after line 10, line 20 changed, line 25 deleted.
        let text = concat!(
            "@@ -10,0 +11,2 @@\n",
            "+x\n",
            "+y\n",
            "@@ -20 +22 @@\n",
            "-twenty\n",
            "+TWENTY\n",
            "@@ -25 +26,0 @@\n",
            "-twenty-five\n",
        );
        let gaps = find(&FileDiff::parse_unified(text.as_bytes()), Some(31));
        assert_eq!(
            gaps,
            [
                Gap { old_start: 1, new_start: 1, len: 10, hunk: Some(0) },
                Gap { old_start: 11, new_start: 13, len: 9, hunk: Some(3) },
                Gap { old_start: 21, new_start: 23, len: 4, hunk: Some(6) },
                Gap { old_start: 26, new_start: 27, len: 5, hunk: None },
            ]
        );
    }

    #[test]
    fn new_and_deleted_files_have_no_gaps() {
        let added = FileDiff::parse_unified(b"@@ -0,0 +1,2 @@\n+a\n+b\n");
        assert_eq!(find(&added, Some(2)), [] as [Gap; 0]);
        let deleted = FileDiff::parse_unified(b"@@ -1,2 +0,0 @@\n-a\n-b\n");
        assert_eq!(find(&deleted, None), [] as [Gap; 0]);
    }

    #[test]
    fn expands_in_steps_from_either_side_and_all_at_once() {
        let gap = Gap { old_start: 8, new_start: 9, len: 45, hunk: Some(5) };
        let mut reveal = Reveal::default();
        reveal.expand(&gap, Expand::Down);
        reveal.expand(&gap, Expand::Up);
        assert_eq!(reveal, Reveal { top: EXPAND_STEP, bottom: EXPAND_STEP });
        assert_eq!(reveal.hidden(&gap), 5);
        reveal.expand(&gap, Expand::Up);
        assert_eq!(reveal.hidden(&gap), 0, "a step never reveals more than is hidden");
        let mut all = Reveal::default();
        all.expand(&gap, Expand::All);
        assert_eq!(all.hidden(&gap), 0);
    }

    #[test]
    fn knows_which_lines_it_hides() {
        let gap = Gap { old_start: 8, new_start: 9, len: 12, hunk: Some(5) };
        assert!(gap.contains(None, Some(9)) && gap.contains(None, Some(20)));
        assert!(!gap.contains(None, Some(21)));
        assert!(gap.contains(Some(8), None) && !gap.contains(Some(20), None));
        assert!(!Gap { old_start: 1, new_start: 1, len: 4, hunk: Some(0) }.can_expand_down());
        assert!(!Gap { old_start: 23, new_start: 23, len: 18, hunk: None }.can_expand_up());
    }
}
