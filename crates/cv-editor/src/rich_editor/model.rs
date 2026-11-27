//! The rich editor's document: a flat list of blocks, each a leaf (paragraph, heading, code,
//! rule, raw HTML, table) that names the containers it sits in (quotes, list items), outermost
//! first. A list item that holds a paragraph and a nested list is two blocks: the paragraph with
//! `[Item(head)]` and the nested item's text with `[Item, Item(head)]`. Positions are a block
//! index plus a char offset into that block's text, so the caret never needs a tree path, layout
//! caches per block, and a viewport shows a contiguous range of blocks.
//!
//! Inline content is a run list. Runs with equal marks merge; an atom (mention, image, raw HTML
//! tag) is a run of one placeholder char, so offsets count it as one character. A soft line break
//! from the source is `\n`, a hard break `HARD_BREAK`.
//!
//! Blocks sit behind `Arc`: an undo snapshot copies pointers, an edit clones only the blocks it
//! touches (`Arc::make_mut`), and the layout cache knows a block is unchanged by its pointer.
//! The model has no egui types, so parse, serialize and edit are tested without a context.

mod inline;

use std::sync::Arc;

pub use self::inline::{Inline, Run};

/// A hard line break inside a block's text (`\` + newline in Markdown, Shift+Enter).
pub const HARD_BREAK: char = '\u{2028}';
/// The char an atom occupies in a block's text.
pub const ATOM_CHAR: char = '\u{FFFC}';

/// An inline mark besides links.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mark {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
    Subscript,
    Superscript,
    Kbd,
}

impl Mark {
    pub const ALL: [Mark; 8] = [
        Mark::Bold,
        Mark::Italic,
        Mark::Underline,
        Mark::Strike,
        Mark::Code,
        Mark::Subscript,
        Mark::Superscript,
        Mark::Kbd,
    ];

    const fn bit(self) -> u16 {
        1 << self as u16
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Link {
    pub url: String,
    pub title: String,
}

/// The marks of a run: a set of [`Mark`]s and an optional link.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Marks {
    bits: u16,
    pub link: Option<Arc<Link>>,
}

impl Marks {
    pub fn has(&self, mark: Mark) -> bool {
        self.bits & mark.bit() != 0
    }

    pub fn set(&mut self, mark: Mark, is_on: bool) {
        if is_on {
            self.bits |= mark.bit();
        } else {
            self.bits &= !mark.bit();
        }
    }

    /// [`Self::set`] for a toggle: subscript and superscript exclude each other.
    pub fn switch(&mut self, mark: Mark, is_on: bool) {
        self.set(mark, is_on);
        if is_on {
            match mark {
                Mark::Subscript => self.set(Mark::Superscript, false),
                Mark::Superscript => self.set(Mark::Subscript, false),
                _ => {}
            }
        }
    }

    #[must_use]
    pub fn with(mut self, mark: Mark) -> Self {
        self.set(mark, true);
        self
    }

    #[must_use]
    pub fn without(mut self, mark: Mark) -> Self {
        self.set(mark, false);
        self
    }

    pub fn is_plain(&self) -> bool {
        self.bits == 0 && self.link.is_none()
    }
}

/// A run that stands for one char of text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Atom {
    /// `key` is the login (GitHub, GitLab) or the identity id (Azure DevOps).
    Mention {
        key: String,
    },
    Image {
        src: String,
        alt: String,
        title: String,
    },
    /// Source kept as it was written: an HTML tag outside the supported subset, a footnote
    /// reference. Serialized verbatim, so nothing of a comment is lost.
    Raw(String),
}

/// A GitHub alert (`> [!NOTE]`) a quote starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Alert {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListKind {
    Bullet,
    /// With the list's first number.
    Ordered(u64),
}

/// A container a block sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Container {
    Quote(Option<Alert>),
    /// `head` marks the first block of the item (where its marker is); `task` is the item's
    /// checkbox, on every block of the item.
    Item {
        list: ListKind,
        task: Option<bool>,
        head: bool,
    },
}

impl Container {
    /// Whether a block in `self` belongs to the same container as one in `previous`: same quote,
    /// or the same item (not a new item's head) of the same kind of list.
    pub fn continues(&self, previous: &Container) -> bool {
        match (self, previous) {
            (Container::Quote(alert), Container::Quote(before)) => alert == before,
            (Container::Item { list, head: false, .. }, Container::Item { list: before, .. }) => {
                same_list(*list, *before)
            }
            _ => false,
        }
    }

    /// Whether `self` is a new item in the same list as `previous`.
    pub fn is_next_item_of(&self, previous: &Container) -> bool {
        matches!(
            (self, previous),
            (Container::Item { list, head: true, .. }, Container::Item { list: before, .. })
                if same_list(*list, *before)
        )
    }

    pub fn is_item(&self) -> bool {
        matches!(self, Container::Item { .. })
    }
}

/// Bullets continue bullets and numbers continue numbers, whatever number an item was given.
pub fn same_list(a: ListKind, b: ListKind) -> bool {
    matches!(
        (a, b),
        (ListKind::Bullet, ListKind::Bullet) | (ListKind::Ordered(_), ListKind::Ordered(_))
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    Paragraph,
    Heading(u8),
    /// A fenced code block with its info string.
    Code(String),
    Rule,
    /// An HTML block, kept as written.
    Html,
    /// A table, as canonical Markdown source.
    Table,
}

impl Kind {
    /// Blocks whose content is plain source: no marks, no input rules, Enter adds a line.
    pub fn is_source(&self) -> bool {
        matches!(self, Kind::Code(_) | Kind::Html | Kind::Table)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Block {
    pub kind: Kind,
    pub nesting: Vec<Container>,
    pub content: Inline,
}

impl Block {
    pub fn paragraph(nesting: Vec<Container>, content: Inline) -> Self {
        Self { kind: Kind::Paragraph, nesting, content }
    }

    pub fn len(&self) -> usize {
        self.content.len()
    }

    /// The innermost list item, with its depth in `nesting`.
    pub fn item(&self) -> Option<(usize, Container)> {
        self.nesting
            .iter()
            .enumerate()
            .rev()
            .find(|(_, container)| container.is_item())
            .map(|(depth, container)| (depth, *container))
    }
}

/// A place in the document: a block and a char offset in it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub block: usize,
    pub offset: usize,
}

impl Pos {
    pub const fn new(block: usize, offset: usize) -> Self {
        Self { block, offset }
    }
}

/// The selection: `anchor` stays where it started, `head` moves with the caret.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Pos,
    pub head: Pos,
}

impl Selection {
    pub const fn caret(pos: Pos) -> Self {
        Self { anchor: pos, head: pos }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Start and end, in document order.
    pub fn ordered(&self) -> (Pos, Pos) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub blocks: Vec<Arc<Block>>,
}

impl Default for Document {
    fn default() -> Self {
        Self { blocks: vec![Arc::new(Block::paragraph(Vec::new(), Inline::default()))] }
    }
}

impl Document {
    pub fn block(&self, index: usize) -> &Block {
        &self.blocks[index.min(self.blocks.len() - 1)]
    }

    pub fn block_mut(&mut self, index: usize) -> &mut Block {
        let last = self.blocks.len() - 1;
        Arc::make_mut(&mut self.blocks[index.min(last)])
    }

    pub fn end(&self) -> Pos {
        let last = self.blocks.len() - 1;
        Pos::new(last, self.blocks[last].len())
    }

    /// `pos` moved inside the document.
    pub fn clamp(&self, pos: Pos) -> Pos {
        let block = pos.block.min(self.blocks.len() - 1);
        Pos::new(block, pos.offset.min(self.blocks[block].len()))
    }

    /// Keeps the invariants edits rely on: at least one block, and an item's first block is its
    /// head (a block whose item does not continue the previous block's starts a new item).
    pub fn normalize(&mut self) {
        if self.blocks.is_empty() {
            *self = Self::default();
            return;
        }
        for index in 0..self.blocks.len() {
            let previous = index.checked_sub(1).map(|before| self.blocks[before].nesting.clone());
            let needs_head: Vec<usize> = self.blocks[index]
                .nesting
                .iter()
                .enumerate()
                .filter(|(depth, container)| {
                    let Container::Item { head: false, .. } = container else { return false };
                    let here = &self.blocks[index].nesting;
                    let continued = previous.as_ref().is_some_and(|before| {
                        (0..=*depth).all(|level| match (here.get(level), before.get(level)) {
                            (Some(this), Some(that)) => this.continues(that),
                            _ => false,
                        })
                    });
                    !continued
                })
                .map(|(depth, _)| depth)
                .collect();
            if needs_head.is_empty() {
                continue;
            }
            let block = Arc::make_mut(&mut self.blocks[index]);
            for depth in needs_head {
                if let Some(Container::Item { head, .. }) = block.nesting.get_mut(depth) {
                    *head = true;
                }
            }
        }
    }
}
