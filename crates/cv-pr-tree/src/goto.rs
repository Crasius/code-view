//! Go to file: Ctrl+P (⌘P) puts the keys in the file tree's filter, also in focus mode, where the
//! tree shows while it runs. ↓/↑ move a highlight through the files the filter lets through, Enter
//! opens the highlighted one and Esc gives up; both put back the search from before and hand the
//! keys back to the diff. A click elsewhere ends it too, and what was typed stays as the filter.

use egui::{Context, Id, Key, Modifiers, Rect, Stroke, StrokeKind, Ui};
use review_git::ChangedFile;

use super::FileTree;
use cv_base::theme::{self, style};

/// The tree filter's id, so the keys know whether it has the keyboard.
pub fn filter_id() -> Id {
    Id::unique("files_tree_filter")
}

/// The go-to's state, kept with the tree it filters.
#[derive(Default)]
pub struct GoTo {
    /// The search from before Ctrl+P, while a go-to runs.
    before: Option<String>,
    /// The file the arrows moved to (by index); the first match until they move.
    highlighted: Option<usize>,
    /// The search the highlight was picked for.
    searched: Option<String>,
    /// The filter takes the keyboard the next time it shows.
    wants_focus: bool,
}

impl GoTo {
    pub fn is_active(&self) -> bool {
        self.before.is_some()
    }

    /// Whether the filter should take the keyboard now (once).
    pub fn take_focus_request(&mut self) -> bool {
        std::mem::take(&mut self.wants_focus)
    }
}

/// Ctrl+P, and while a go-to runs ↓/↑, Enter and Esc; before anything else reads the keys this
/// frame. Returns the file Enter opens, by index into `files`.
pub fn keys(tree: &mut FileTree, ctx: &Context, files: &[ChangedFile]) -> Option<usize> {
    let has_focus = ctx.memory(|memory| memory.has_focus(filter_id()));
    let is_typing_elsewhere = ctx.text_edit_focused() && !has_focus;
    if !is_typing_elsewhere && ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::P))
    {
        let goto = &mut tree.goto;
        if goto.before.is_none() {
            goto.before = Some(std::mem::take(&mut tree.filter.search));
        }
        goto.wants_focus = true;
        goto.searched = None;
        ctx.request_repaint();
        return None;
    }
    if !tree.goto.is_active() {
        return None;
    }
    // egui took the keyboard from the filter at the start of the frame already.
    if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
        finish(tree, ctx);
        return None;
    }
    if !has_focus && !tree.goto.wants_focus {
        // Clicked away: what was typed stays the filter.
        tree.goto = GoTo::default();
        return None;
    }
    let (down, up, enter) = ctx.input_mut(|input| {
        (
            input.consume_key(Modifiers::NONE, Key::ArrowDown),
            input.consume_key(Modifiers::NONE, Key::ArrowUp),
            input.consume_key(Modifiers::NONE, Key::Enter),
        )
    });
    let order = tree.full_order();
    let position = tree
        .goto
        .highlighted
        .and_then(|index| order.iter().position(|listed| *listed as usize == index));
    let moved = match (down, up, position) {
        (true, _, Some(position)) => order.get(position + 1).or(order.get(position)),
        (true, _, None) => order.first(),
        (false, true, Some(position)) => order.get(position.saturating_sub(1)),
        _ => None,
    };
    if let Some(&index) = moved {
        let index = index as usize;
        tree.goto.highlighted = Some(index);
        if let Some(file) = files.get(index) {
            tree.reveal(&file.path);
        }
        tree.scroll_to(index);
        ctx.request_repaint();
    }
    if !enter {
        return None;
    }
    let picked =
        tree.goto.highlighted.or_else(|| tree.full_order().first().map(|index| *index as usize));
    finish(tree, ctx);
    picked
}

/// Puts the search from before back and hands the keys back to the diff.
fn finish(tree: &mut FileTree, ctx: &Context) {
    if let Some(search) = tree.goto.before.take() {
        tree.filter.search = search;
    }
    tree.goto = GoTo::default();
    ctx.memory_mut(|memory| memory.surrender_focus(filter_id()));
    ctx.request_repaint();
}

/// The highlight follows the search: the first match whenever the search changed. Call once the
/// rows are built for the search.
pub fn follow_search(tree: &mut FileTree) {
    if !tree.goto.is_active() || tree.goto.searched.as_deref() == Some(tree.filter.search.as_str())
    {
        return;
    }
    tree.goto.searched = Some(tree.filter.search.clone());
    tree.goto.highlighted = tree.full_order().first().map(|index| *index as usize);
}

/// The file the arrows are on, while a go-to runs.
pub fn highlighted(tree: &FileTree) -> Option<usize> {
    tree.goto.highlighted.filter(|_| tree.goto.is_active())
}

/// Outlines the highlighted row at `rect` as the keyboard's place, like a focus ring.
pub fn paint_highlight(ui: &Ui, rect: Rect) {
    let palette = theme::palette(ui.ctx());
    ui.painter().rect_stroke(
        rect.shrink(1.0),
        style::RADIUS_CONTROL,
        Stroke::new(2.0, palette.accent),
        StrokeKind::Inside,
    );
}
