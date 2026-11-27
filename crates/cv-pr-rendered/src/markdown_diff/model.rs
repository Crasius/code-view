//! The two versions parsed and lined up ([`MarkdownModel`]), and what the rows of a layout hold:
//! blocks ([`Cell`]) marked by how they changed ([`Look`]), folds, removals and comment cards
//! ([`Row`]), and what they were built from ([`RowsKey`]).

use std::collections::BTreeMap;
use std::ops::Range;

use egui::Color32;
use review_core::{DiffSide, ProviderKind};
use review_store::DiffLayout;

use super::RenderedImages;
use super::align::{self, Change, Pair};
use super::threads::ThreadRow;
use cv_markdown::{ImageBase, RenderOptions, Units};

/// The index of `side` in an `[old, new]` pair.
pub(super) fn slot(side: DiffSide) -> usize {
    match side {
        DiffSide::Old => 0,
        DiffSide::New => 1,
    }
}

/// The other side of a diff.
pub(super) fn other(side: DiffSide) -> DiffSide {
    match side {
        DiffSide::Old => DiffSide::New,
        DiffSide::New => DiffSide::Old,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(super) enum Mode {
    /// Side by side: Before left, After right.
    #[default]
    Columns,
    /// One column, the old version of a changed block above the new one.
    Unified,
    /// One column of the new version, its changes in the accent color, removals as wedges.
    NewOnly,
    /// Only one version exists (an added or a deleted file): shown whole, unmarked.
    Whole(DiffSide),
}

impl Mode {
    pub fn of(has_old: bool, has_new: bool, layout: DiffLayout) -> Self {
        match (has_old, has_new) {
            (false, _) => Mode::Whole(DiffSide::New),
            (true, false) => Mode::Whole(DiffSide::Old),
            (true, true) => match layout {
                DiffLayout::SideBySide => Mode::Columns,
                DiffLayout::Inline => Mode::Unified,
                DiffLayout::NewOnly => Mode::NewOnly,
            },
        }
    }
}

/// How a block is marked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Look {
    Plain,
    Added,
    Removed,
    /// New only: a changed block, in the accent color.
    Focus,
}

/// One block on screen: unit `unit` of `side`'s text, from pair `pair`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Cell {
    pub side: DiffSide,
    pub unit: usize,
    pub pair: usize,
    pub look: Look,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Row {
    /// Side by side: the old block left and the new one right, a hatched gap for a missing one.
    /// One column: the block in `left`.
    Blocks { left: Option<Cell>, right: Option<Cell> },
    /// Unchanged pairs folded behind a bar.
    Folded { pairs: Range<usize> },
    /// New only: removed pairs, a wedge between the rows around it (no height of its own).
    Removal { pairs: Range<usize> },
    /// A comment thread's card under the block row above it (no height while hidden).
    Thread(ThreadRow),
}

impl Row {
    pub fn cells(&self) -> [Option<Cell>; 2] {
        match self {
            Row::Blocks { left, right } => [*left, *right],
            Row::Folded { .. } | Row::Removal { .. } | Row::Thread(_) => [None, None],
        }
    }

    pub fn is_change(&self) -> bool {
        match self {
            Row::Blocks { .. } => {
                self.cells().into_iter().flatten().any(|cell| cell.look != Look::Plain)
            }
            Row::Removal { .. } => true,
            Row::Folded { .. } | Row::Thread(_) => false,
        }
    }

    /// The pairs the row stands for.
    pub fn pairs(&self) -> Range<usize> {
        match self {
            Row::Folded { pairs } | Row::Removal { pairs } => pairs.clone(),
            Row::Blocks { .. } => self
                .cells()
                .into_iter()
                .flatten()
                .map(|cell| cell.pair..cell.pair + 1)
                .next()
                .unwrap_or(0..0),
            Row::Thread(_) => 0..0,
        }
    }
}

/// How many pairs were added, removed and edited.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Counts {
    pub added: usize,
    pub removed: usize,
    pub edited: usize,
}

/// What the rows were built from: the mode, the context, the folds opened and the threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RowsKey {
    pub mode: Mode,
    pub context: u32,
    /// The version of the folds the user opened.
    pub expanded: u64,
    /// The threads' key ([`super::threads::threads_key`]).
    pub threads: u64,
}

/// Both versions of a Markdown file parsed and lined up. The preview's load builds it in the
/// background ([`crate::preview::Loaded`]), so the rendered diff never parses on the UI thread.
pub struct MarkdownModel {
    pub(super) key: u64,
    pub(super) old: Units,
    pub(super) new: Units,
    pub(super) has_old: bool,
    pub(super) has_new: bool,
    pub(super) pairs: Vec<Pair>,
    /// The pair holding each unit of the old and of the new text.
    pub(super) old_pairs: Vec<usize>,
    pub(super) new_pairs: Vec<usize>,
    pub(super) counts: Counts,
    /// Where the relative image paths of the old and of the new version point.
    pub(super) images: [Option<ImageBase>; 2],
}

impl MarkdownModel {
    /// `key` changes whenever `old` or `new` does; `None` is a version the file does not have.
    pub(crate) fn new(
        key: u64,
        old: Option<&str>,
        new: Option<&str>,
        platform: ProviderKind,
        images: &RenderedImages,
    ) -> Self {
        let parse = |text: Option<&str>, content: &BTreeMap<String, String>| {
            let mut units = Units::parse(text.unwrap_or_default(), Some(platform));
            units.distinguish_images(content);
            units
        };
        let [old_content, new_content] = &images.content;
        let (has_old, has_new) = (old.is_some(), new.is_some());
        let (old, new) = (parse(old, old_content), parse(new, new_content));
        let pairs = match (has_old, has_new) {
            (true, true) => align::align(&old, &new),
            (false, _) => (0..new.len())
                .map(|unit| Pair::one_sided(None, Some(unit), Change::Added))
                .collect(),
            (true, false) => (0..old.len())
                .map(|unit| Pair::one_sided(Some(unit), None, Change::Removed))
                .collect(),
        };
        let (mut old_pairs, mut new_pairs) = (vec![0; old.len()], vec![0; new.len()]);
        let mut counts = Counts::default();
        for (index, pair) in pairs.iter().enumerate() {
            if let Some(unit) = pair.old {
                old_pairs[unit] = index;
            }
            if let Some(unit) = pair.new {
                new_pairs[unit] = index;
            }
            match pair.change {
                Change::Added => counts.added += 1,
                Change::Removed => counts.removed += 1,
                Change::Modified => counts.edited += 1,
                Change::Equal => {}
            }
        }
        Self {
            key,
            old,
            new,
            has_old,
            has_new,
            pairs,
            old_pairs,
            new_pairs,
            counts,
            images: images.bases,
        }
    }

    /// `options` for the blocks of `side`: its images, framed in `border` (a change color).
    pub fn options(
        &self,
        options: RenderOptions,
        side: DiffSide,
        border: Option<Color32>,
    ) -> RenderOptions {
        RenderOptions { images: self.images[slot(side)], image_border: border, ..options }
    }

    pub fn units(&self, side: DiffSide) -> &Units {
        match side {
            DiffSide::Old => &self.old,
            DiffSide::New => &self.new,
        }
    }

    pub fn pair_of(&self, side: DiffSide, unit: usize) -> Option<usize> {
        match side {
            DiffSide::Old => self.old_pairs.get(unit).copied(),
            DiffSide::New => self.new_pairs.get(unit).copied(),
        }
    }
}
