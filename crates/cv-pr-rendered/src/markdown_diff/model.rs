//! The two versions parsed and lined up ([`Model`]), and what the rows of a layout hold: blocks
//! ([`Cell`]) marked by how they changed ([`Look`]), folds, removals and comment cards ([`Row`]).

use std::collections::BTreeMap;
use std::ops::Range;

use egui::Color32;
use review_core::DiffSide;
use review_store::DiffLayout;

use super::RenderedInputs;
use super::align::{self, Change, Pair};
use super::threads::ThreadRow;
use cv_markdown::{ImageBase, RenderOptions, Units};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Side {
    Old,
    New,
}

impl Side {
    pub fn of(side: DiffSide) -> Self {
        match side {
            DiffSide::Old => Side::Old,
            DiffSide::New => Side::New,
        }
    }

    pub fn diff_side(self) -> DiffSide {
        match self {
            Side::Old => DiffSide::Old,
            Side::New => DiffSide::New,
        }
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
    Whole(Side),
}

impl Mode {
    pub fn of(has_old: bool, has_new: bool, layout: DiffLayout) -> Self {
        match (has_old, has_new) {
            (false, _) => Mode::Whole(Side::New),
            (true, false) => Mode::Whole(Side::Old),
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
    pub side: Side,
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

/// Both versions parsed and lined up.
pub(super) struct Model {
    pub key: u64,
    pub old: Units,
    pub new: Units,
    pub has_old: bool,
    pub has_new: bool,
    pub pairs: Vec<Pair>,
    /// The pair holding each unit of the old and of the new text.
    pub old_pairs: Vec<usize>,
    pub new_pairs: Vec<usize>,
    /// Pairs added, removed and edited.
    pub counts: [usize; 3],
    /// Where the relative image paths of the old and of the new version point.
    pub images: [Option<ImageBase>; 2],
}

impl Model {
    pub fn new(inputs: &RenderedInputs<'_>) -> Self {
        let platform = Some(inputs.platform);
        let parse = |text: Option<&str>, content: &BTreeMap<String, String>| {
            let mut units = Units::parse(text.unwrap_or_default(), platform);
            units.distinguish_images(content);
            units
        };
        let [old_content, new_content] = &inputs.images.content;
        let (old, new) = (parse(inputs.old, old_content), parse(inputs.new, new_content));
        let (has_old, has_new) = (inputs.old.is_some(), inputs.new.is_some());
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
        let mut counts = [0; 3];
        for (index, pair) in pairs.iter().enumerate() {
            if let Some(unit) = pair.old {
                old_pairs[unit] = index;
            }
            if let Some(unit) = pair.new {
                new_pairs[unit] = index;
            }
            match pair.change {
                Change::Added => counts[0] += 1,
                Change::Removed => counts[1] += 1,
                Change::Modified => counts[2] += 1,
                Change::Equal => {}
            }
        }
        let images = inputs.images.bases;
        Self {
            key: inputs.key,
            old,
            new,
            has_old,
            has_new,
            pairs,
            old_pairs,
            new_pairs,
            counts,
            images,
        }
    }

    /// `options` for the blocks of `side`: its images, framed in `border` (a change color).
    pub fn options(
        &self,
        options: RenderOptions,
        side: Side,
        border: Option<Color32>,
    ) -> RenderOptions {
        let images = match side {
            Side::Old => self.images[0],
            Side::New => self.images[1],
        };
        RenderOptions { images, image_border: border, ..options }
    }

    pub fn units(&self, side: Side) -> &Units {
        match side {
            Side::Old => &self.old,
            Side::New => &self.new,
        }
    }

    pub fn pair_of(&self, side: Side, unit: usize) -> Option<usize> {
        match side {
            Side::Old => self.old_pairs.get(unit).copied(),
            Side::New => self.new_pairs.get(unit).copied(),
        }
    }
}
