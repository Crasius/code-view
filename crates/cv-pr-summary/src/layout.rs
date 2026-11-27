//! Where the summary's cards sit: every card's height from what is known about its file, before
//! and after its diff loads, and the running total of heights, so the card at any scroll position
//! is found by binary search and only the cards on screen are ever looked at.

use std::collections::{HashMap, HashSet};

use review_core::FileCategory;
use review_git::{ChangeKind, ChangedFile};

/// Changes larger than this show a placeholder with the choice to load them anyway: a card that
/// tall is more scrolling than reading, and its file view has the tools for it.
pub const INLINE_MAX_CHANGED_LINES: u32 = 25_000;
/// Rows a card is estimated at while its line counts are not known yet.
const UNKNOWN_ROWS: u32 = 12;
/// Asset extensions git diffs as text: their cards show code like any other file.
const TEXT_ASSET_EXTENSIONS: &[&str] = &["svg"];
/// Changed lines per hunk, on average, for the estimate of how many hunks a change has.
const LINES_PER_HUNK: u32 = 12;
const MAX_ESTIMATED_HUNKS: u32 = 400;

/// The fixed sizes of a card, from the code font's line height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub header: f32,
    pub line: f32,
    pub separator: f32,
    pub placeholder: f32,
    /// Space below each card.
    pub gap: f32,
}

impl Geometry {
    pub fn new(line_height: f32) -> Self {
        Self {
            header: 44.0,
            line: line_height,
            separator: (line_height * 0.9).round(),
            placeholder: 52.0,
            gap: 12.0,
        }
    }

    /// The height of the body below the header.
    pub fn body(&self, shape: Shape) -> f32 {
        match shape {
            Shape::Rows { lines, separators } => {
                lines as f32 * self.line + separators as f32 * self.separator
            }
            Shape::Placeholder => self.placeholder,
        }
    }

    /// A card's slot: header, body unless collapsed, and the gap after it.
    pub fn slot(&self, shape: Shape, is_collapsed: bool) -> f32 {
        let body = if is_collapsed { 0.0 } else { self.body(shape) };
        self.header + body + self.gap
    }
}

/// What a card's body holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Rows {
        lines: u32,
        separators: u32,
    },
    /// One line saying why there is no code here, with its action.
    Placeholder,
}

/// The body a file's card will most likely get, from its kind and line counts alone. Added and
/// deleted files are exact (every line shows); edits assume a hunk per dozen changed lines with
/// `context` unchanged lines on either side. Side by side (`split`) a deleted and an added line
/// share a row, so the changed lines take the larger of the two counts. Before the counts arrive,
/// a file whose name says binary (an image, a font, an archive) is estimated at the placeholder
/// it will get, so its card does not shrink once the counts mark it binary.
pub fn estimate(file: &ChangedFile, context: u32, split: bool) -> Shape {
    if file.is_binary || is_too_large(file) {
        return Shape::Placeholder;
    }
    let (Some(additions), Some(deletions)) = (file.additions, file.deletions) else {
        if is_binary_by_name(&file.path) {
            return Shape::Placeholder;
        }
        return Shape::Rows { lines: UNKNOWN_ROWS, separators: 0 };
    };
    let changed = additions + deletions;
    match file.kind {
        ChangeKind::Added | ChangeKind::Deleted => {
            Shape::Rows { lines: changed.max(1), separators: 0 }
        }
        _ if changed == 0 => Shape::Placeholder,
        _ => {
            let hunks = changed.div_ceil(LINES_PER_HUNK).clamp(1, MAX_ESTIMATED_HUNKS);
            let rows = if split { additions.max(deletions) } else { changed };
            Shape::Rows { lines: rows + hunks * context * 2, separators: hunks - 1 }
        }
    }
}

/// A card's exact shapes once its diff is known: inline and side by side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shapes {
    pub inline: Shape,
    pub split: Shape,
}

impl Shapes {
    pub fn pick(self, split: bool) -> Shape {
        if split { self.split } else { self.inline }
    }
}

/// The file name alone makes it an asset git cannot diff as text. Only the name counts: an image
/// under a generated or test folder is still an image.
fn is_binary_by_name(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let is_text = name.rsplit_once('.').is_some_and(|(_, extension)| {
        TEXT_ASSET_EXTENSIONS.iter().any(|text| extension.eq_ignore_ascii_case(text))
    });
    FileCategory::classify(name) == FileCategory::Assets && !is_text
}

/// More changed lines than a card shows inline.
pub fn is_too_large(file: &ChangedFile) -> bool {
    file.additions.unwrap_or(0).saturating_add(file.deletions.unwrap_or(0))
        > INLINE_MAX_CHANGED_LINES
}

/// A pure rename, copy or mode change: line counts say nothing changed inside.
pub fn is_unchanged(file: &ChangedFile) -> bool {
    file.kind != ChangeKind::Added
        && file.kind != ChangeKind::Deleted
        && file.additions == Some(0)
        && file.deletions == Some(0)
}

/// A card's shape for the layout: the exact one once its diff loaded, all its changed lines when
/// the user asked to see a large change here, else the estimate.
pub fn shape_of(
    shapes: &HashMap<String, Shapes>,
    file: &ChangedFile,
    context: u32,
    forced: &HashSet<String>,
    is_split: bool,
) -> Shape {
    if let Some(shapes) = shapes.get(&file.path) {
        return shapes.pick(is_split);
    }
    if forced.contains(&file.path) && !file.is_binary {
        return shown_here(file, None);
    }
    estimate(file, context, is_split)
}

/// A large change the user asked to see in its card: all its changed lines, as counted or as
/// `measured` from its diff when that loaded before the lines were counted.
pub fn shown_here(file: &ChangedFile, measured: Option<u32>) -> Shape {
    let counted = file.additions.unwrap_or(0) + file.deletions.unwrap_or(0);
    Shape::Rows { lines: measured.unwrap_or(counted), separators: 0 }
}

/// Running totals of the slot heights: `offsets[i]` is the top of card `i`, the last entry the
/// height of all cards.
#[derive(Debug, Default, Clone)]
pub struct CardIndex {
    offsets: Vec<f32>,
}

impl CardIndex {
    pub fn rebuild(&mut self, heights: impl Iterator<Item = f32>) {
        self.offsets.clear();
        let mut top = 0.0;
        self.offsets.push(top);
        for height in heights {
            top += height;
            self.offsets.push(top);
        }
    }

    pub fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    pub fn total(&self) -> f32 {
        self.offsets.last().copied().unwrap_or(0.0)
    }

    pub fn top(&self, card: usize) -> f32 {
        self.offsets.get(card).copied().unwrap_or_else(|| self.total())
    }

    pub fn bottom(&self, card: usize) -> f32 {
        self.top(card + 1)
    }

    /// The card at content height `y` (the last one below the end).
    pub fn at(&self, y: f32) -> Option<usize> {
        let count = self.len();
        if count == 0 {
            return None;
        }
        Some(self.offsets.partition_point(|top| *top <= y).saturating_sub(1).min(count - 1))
    }

    /// The cards that overlap `top..bottom`.
    pub fn overlapping(&self, top: f32, bottom: f32) -> std::ops::Range<usize> {
        let Some(first) = self.at(top) else {
            return 0..0;
        };
        let end =
            self.offsets.partition_point(|offset| *offset < bottom).max(first + 1).min(self.len());
        first..end
    }
}

#[cfg(test)]
mod tests {
    use review_git::{ChangeKind, ChangedFile};

    use super::{CardIndex, Geometry, Shape, estimate};
    use crate::testing::file;

    #[test]
    fn estimates_whole_files_exactly_and_edits_by_their_hunks() {
        assert_eq!(
            estimate(&file("src/a.rs", ChangeKind::Added, Some((120, 0))), 3, false),
            Shape::Rows { lines: 120, separators: 0 }
        );
        assert_eq!(
            estimate(&file("src/a.rs", ChangeKind::Deleted, Some((0, 7))), 3, true),
            Shape::Rows { lines: 7, separators: 0 },
            "a whole file shows in one pane"
        );
        // 30 changed lines: three hunks of about ten, each with three lines above and below.
        assert_eq!(
            estimate(&file("src/a.rs", ChangeKind::Modified, Some((20, 10))), 3, false),
            Shape::Rows { lines: 30 + 18, separators: 2 }
        );
        assert_eq!(
            estimate(&file("src/a.rs", ChangeKind::Modified, Some((20, 10))), 3, true),
            Shape::Rows { lines: 20 + 18, separators: 2 },
            "deleted lines sit beside added ones"
        );
        assert_eq!(
            estimate(&file("src/a.rs", ChangeKind::Renamed, Some((0, 0))), 3, false),
            Shape::Placeholder,
            "a pure rename"
        );
        assert_eq!(
            estimate(&file("src/a.rs", ChangeKind::Added, Some((30_000, 0))), 3, false),
            Shape::Placeholder,
            "too large to show inline"
        );
        let binary =
            ChangedFile { is_binary: true, ..file("src/a.rs", ChangeKind::Modified, None) };
        assert_eq!(estimate(&binary, 3, true), Shape::Placeholder);
        assert_eq!(
            estimate(&file("src/a.rs", ChangeKind::Modified, None), 3, false),
            Shape::Rows { lines: 12, separators: 0 },
            "not counted yet"
        );
    }

    #[test]
    fn an_image_keeps_its_card_height_when_the_counts_mark_it_binary() {
        let geometry = Geometry::new(20.0);
        let uncounted = ChangedFile {
            path: "web/dist/Logo.PNG".into(),
            ..file("src/a.rs", ChangeKind::Modified, None)
        };
        let counted = ChangedFile { is_binary: true, ..uncounted.clone() };
        for split in [false, true] {
            assert_eq!(
                geometry.body(estimate(&uncounted, 3, split)),
                geometry.body(estimate(&counted, 3, split))
            );
        }
        let svg = ChangedFile {
            path: "icons/logo.svg".into(),
            ..file("src/a.rs", ChangeKind::Modified, None)
        };
        assert_eq!(
            estimate(&svg, 3, false),
            Shape::Rows { lines: 12, separators: 0 },
            "an svg diffs as text"
        );
    }

    #[test]
    fn collapsed_cards_are_exactly_their_header() {
        let geometry = Geometry::new(20.0);
        let shape = Shape::Rows { lines: 10, separators: 2 };
        assert_eq!(geometry.slot(shape, true), geometry.header + geometry.gap);
        assert_eq!(
            geometry.slot(shape, false),
            geometry.header + 200.0 + 2.0 * geometry.separator + geometry.gap
        );
    }

    #[test]
    fn finds_the_cards_on_screen_by_binary_search() {
        let mut index = CardIndex::default();
        index.rebuild([100.0, 50.0, 200.0, 10.0].into_iter());
        assert_eq!(index.total(), 360.0);
        assert_eq!(
            (index.at(0.0), index.at(99.9), index.at(100.0), index.at(1_000.0)),
            (Some(0), Some(0), Some(1), Some(3))
        );
        assert_eq!(index.overlapping(120.0, 160.0), 1..3);
        assert_eq!(
            index.overlapping(0.0, 100.0),
            0..1,
            "a card starting at the bottom edge is not on screen"
        );
        assert_eq!(index.overlapping(355.0, 900.0), 3..4);
        assert_eq!(CardIndex::default().overlapping(0.0, 10.0), 0..0);
    }
}
