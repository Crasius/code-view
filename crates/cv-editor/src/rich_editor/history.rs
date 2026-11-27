//! Undo and redo. An entry is a snapshot of the block pointers and the selection before an edit,
//! so it costs one pointer per block, not a copy of the text. Typing and deleting group: chars
//! typed into one block within a second of each other undo together, a group breaks at a word
//! (a space after a non-space), at a caret move, a click, a formatting action and a change of
//! the edit kind. At most `LIMIT` entries are kept, the oldest dropped first.

use std::collections::VecDeque;
use std::sync::Arc;

use super::model::{Block, Document, Selection};

/// Our budget for undo steps per editor.
const LIMIT: usize = 300;
/// Seconds between two keystrokes that still undo together.
const GROUP_SECONDS: f64 = 1.0;

/// What kind of edit is recorded, for grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// A char typed; `is_space` says whether it was whitespace.
    Typing {
        is_space: bool,
    },
    Deleting,
    /// Everything else: a step of its own.
    Other,
}

#[derive(Clone)]
struct Entry {
    blocks: Vec<Arc<Block>>,
    selection: Selection,
}

#[derive(Clone, Default)]
pub struct History {
    undo: VecDeque<Entry>,
    redo: Vec<Entry>,
    /// The open group: its kind, block and the time of its last edit.
    group: Option<(EditKind, usize, f64)>,
    /// The last entry was pushed for an input rule; Backspace now undoes it.
    rule_armed: bool,
}

impl History {
    /// Records an edit: `doc` and `selection` are the state before it. Call it only when the edit
    /// changed something; a refused command calls `break_group` instead.
    pub fn record(
        &mut self,
        doc: &Document,
        selection: Selection,
        kind: EditKind,
        block: usize,
        now: f64,
    ) {
        self.rule_armed = false;
        self.redo.clear();
        let continues = match (self.group, kind) {
            (
                Some((EditKind::Typing { is_space: was_space }, at, time)),
                EditKind::Typing { is_space },
            ) => at == block && now - time < GROUP_SECONDS && (!is_space || was_space),
            (Some((EditKind::Deleting, at, time)), EditKind::Deleting) => {
                at == block && now - time < GROUP_SECONDS
            }
            _ => false,
        };
        self.group = (kind != EditKind::Other).then_some((kind, block, now));
        if continues {
            return;
        }
        self.push(doc, selection);
    }

    /// Records the typed text before an input rule rewrites it, and arms Backspace to undo the rule.
    pub fn record_rule(&mut self, doc: &Document, selection: Selection) {
        self.push(doc, selection);
        self.group = None;
        self.rule_armed = true;
    }

    fn push(&mut self, doc: &Document, selection: Selection) {
        self.undo.push_back(Entry { blocks: doc.blocks.clone(), selection });
        if self.undo.len() > LIMIT {
            self.undo.pop_front();
        }
    }

    /// A caret move or click ends the open group.
    pub fn break_group(&mut self) {
        self.group = None;
        self.rule_armed = false;
    }

    /// Whether Backspace should undo the input rule that just fired.
    pub fn takes_backspace(&self) -> bool {
        self.rule_armed
    }

    pub fn undo(&mut self, doc: &mut Document, selection: &mut Selection) -> bool {
        let Some(entry) = self.undo.pop_back() else { return false };
        self.redo.push(Entry {
            blocks: std::mem::replace(&mut doc.blocks, entry.blocks),
            selection: *selection,
        });
        *selection = entry.selection;
        self.break_group();
        true
    }

    pub fn redo(&mut self, doc: &mut Document, selection: &mut Selection) -> bool {
        let Some(entry) = self.redo.pop() else { return false };
        self.undo.push_back(Entry {
            blocks: std::mem::replace(&mut doc.blocks, entry.blocks),
            selection: *selection,
        });
        *selection = entry.selection;
        self.break_group();
        true
    }
}
