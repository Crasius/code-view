//! Keeps the user's place when the diff of the open file is replaced (a live update brought new
//! commits, another comparison was picked): the line at the top of the view, and the line a new
//! comment is being written on, are found again in the new diff by their text and the lines above
//! them, so edits higher up that shift the numbers do not move the user.

use review_git::{DiffLine, FileDiff, LineKind};

/// Lines above the anchor compared to tell repeated lines (`}`, blank lines) apart.
const CONTEXT: usize = 3;

/// One line of a diff, remembered well enough to find it in another diff of the same file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineAnchor {
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// The line's text when the diff has it (revealed unchanged lines carry only numbers).
    pub text: Option<String>,
    /// Texts of the diff lines right above, nearest first.
    pub above: Vec<String>,
}

/// Where an anchor's line is in the new diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Target {
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// The same text was found; false when the line itself changed and the closest line stands in.
    pub is_exact: bool,
}

impl LineAnchor {
    /// The diff line at `index` (into [`FileDiff::lines`]).
    pub(crate) fn of_line(diff: &FileDiff, index: usize) -> Option<Self> {
        let lines = diff.lines();
        let line = lines.get(index).filter(|line| line.kind != LineKind::Hunk)?;
        let above = lines[..index]
            .iter()
            .rev()
            .filter(|line| line.kind != LineKind::Hunk)
            .take(CONTEXT)
            .map(|line| diff.text(line).to_owned())
            .collect();
        let (old_line, new_line) = numbers(line);
        Some(Self { old_line, new_line, text: Some(diff.text(line).to_owned()), above })
    }

    /// A line known only by its numbers (revealed from a gap).
    pub(crate) fn of_numbers(old_line: Option<u32>, new_line: Option<u32>) -> Self {
        Self { old_line, new_line, text: None, above: Vec::new() }
    }

    /// Head lines are matched in the head file; lines only in the base, in the base.
    fn wanted(&self) -> Option<(Side, u32)> {
        match (self.new_line, self.old_line) {
            (Some(line), _) => Some((Side::New, line)),
            (None, Some(line)) => Some((Side::Old, line)),
            (None, None) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Old,
    New,
}

/// The anchor's line in `diff`: the line with the same text whose lines above match best, the
/// nearest one among equals; the nearest line on the same side when the text is gone; `None` for
/// an empty diff.
pub(crate) fn locate(anchor: &LineAnchor, diff: &FileDiff) -> Option<Target> {
    let (side, wanted) = anchor.wanted()?;
    let lines = diff.lines();
    let on_side = |line: &DiffLine| match side {
        Side::New => line.kind != LineKind::Hunk && line.new_line != 0,
        Side::Old => line.kind != LineKind::Hunk && line.old_line != 0,
    };
    let number = |line: &DiffLine| match side {
        Side::New => line.new_line,
        Side::Old => line.old_line,
    };
    let distance = |line: &DiffLine| number(line).abs_diff(wanted);
    let target = |line: &DiffLine, is_exact: bool| {
        let (old_line, new_line) = numbers(line);
        Target { old_line, new_line, is_exact }
    };
    let Some(text) = &anchor.text else {
        // A revealed line outside the hunks keeps its number; the view finds the row or gap holding it.
        let line = lines.iter().find(|line| on_side(line) && number(line) == wanted);
        return Some(line.map_or(
            Target { old_line: anchor.old_line, new_line: anchor.new_line, is_exact: true },
            |line| target(line, true),
        ));
    };
    let best = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| on_side(line) && diff.text(line) == text)
        .max_by_key(|(index, line)| {
            (matching_above(diff, *index, &anchor.above), std::cmp::Reverse(distance(line)))
        });
    if let Some((_, line)) = best {
        return Some(target(line, true));
    }
    let nearest = lines.iter().filter(|line| on_side(line)).min_by_key(|line| distance(line))?;
    Some(target(nearest, false))
}

/// The anchor of the line numbered like a comment anchor: from the diff when it holds that line,
/// else by its numbers.
pub(crate) fn for_numbers(
    diff: &FileDiff,
    old_line: Option<u32>,
    new_line: Option<u32>,
) -> LineAnchor {
    let index = diff
        .lines()
        .iter()
        .position(|line| line.kind != LineKind::Hunk && numbers(line) == (old_line, new_line));
    index
        .and_then(|index| LineAnchor::of_line(diff, index))
        .unwrap_or_else(|| LineAnchor::of_numbers(old_line, new_line))
}

/// How many of the lines above `index` read like `above`, counted from the nearest.
fn matching_above(diff: &FileDiff, index: usize, above: &[String]) -> usize {
    let lines = diff.lines();
    lines[..index]
        .iter()
        .rev()
        .filter(|line| line.kind != LineKind::Hunk)
        .zip(above)
        .take_while(|(line, text)| diff.text(line) == text.as_str())
        .count()
}

fn numbers(line: &DiffLine) -> (Option<u32>, Option<u32>) {
    let some = |number: u32| (number != 0).then_some(number);
    match line.kind {
        LineKind::Added => (None, some(line.new_line)),
        LineKind::Deleted => (some(line.old_line), None),
        LineKind::Context | LineKind::Hunk => (some(line.old_line), some(line.new_line)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "fn main() {\n    let a = 1;\n    let b = 2;\n    println!(\"{a}\");\n}\n\nfn helper() {\n    let a = 1;\n    todo!()\n}\n";

    fn diff(old: &str, new: &str) -> FileDiff {
        let unified =
            similar::TextDiff::from_lines(old, new).unified_diff().context_radius(20).to_string();
        FileDiff::parse_unified(unified.as_bytes())
    }

    fn index_of(diff: &FileDiff, new_line: u32) -> usize {
        diff.lines()
            .iter()
            .position(|line| line.kind != LineKind::Hunk && line.new_line == new_line)
            .expect("line in the diff")
    }

    #[test]
    fn follows_a_line_that_moved_down() {
        let head = BASE.replace("    let b = 2;", "    let b = 2;\n    let c = 3;");
        let before = diff(BASE, &head);
        let anchor = LineAnchor::of_line(&before, index_of(&before, 10)).expect("todo line");
        assert_eq!(anchor.text.as_deref(), Some("    todo!()"));
        // The newer push adds two lines at the top: the line is two further down now.
        let newer = format!("// one\n// two\n{head}");
        let after = diff(BASE, &newer);
        let target = locate(&anchor, &after).expect("found");
        assert_eq!((target.new_line, target.is_exact), (Some(12), true));
    }

    #[test]
    fn tells_repeated_lines_apart_by_the_lines_above() {
        let head = BASE.replace("todo!()", "unimplemented!()");
        let before = diff(BASE, &head);
        // `    let a = 1;` appears twice; the anchor is the second one (line 8, in helper).
        let anchor = LineAnchor::of_line(&before, index_of(&before, 8)).expect("second let");
        let after = diff(BASE, &format!("// header\n{head}"));
        let target = locate(&anchor, &after).expect("found");
        assert_eq!(target.new_line, Some(9), "the one in helper, not the one in main");
    }

    #[test]
    fn stands_in_with_the_nearest_line_when_the_line_changed() {
        let head = BASE.replace("    let b = 2;", "    let b = 20;");
        let before = diff(BASE, &head);
        let anchor = LineAnchor::of_line(&before, index_of(&before, 3)).expect("changed line");
        let after = diff(BASE, &BASE.replace("    let b = 2;", "    let b = 200;"));
        let target = locate(&anchor, &after).expect("near");
        assert_eq!((target.new_line, target.is_exact), (Some(3), false));
    }

    #[test]
    fn keeps_revealed_lines_by_number() {
        let after = diff(BASE, &BASE.replace("todo!()", "unimplemented!()"));
        let target = locate(&LineAnchor::of_numbers(Some(2), Some(2)), &after).expect("kept");
        assert_eq!((target.old_line, target.new_line, target.is_exact), (Some(2), Some(2), true));
        assert_eq!(locate(&LineAnchor::of_numbers(None, None), &after), None);
    }
}
