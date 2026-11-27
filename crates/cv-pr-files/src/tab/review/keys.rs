//! The Files tab's keys: Alt+↓/↑ or J/K step to the next or previous file in the tree's order,
//! Shift+J/K to the next or previous file not viewed yet, Alt+V marks the open file viewed (with no
//! file open, every file of the open folder); Alt+Z (wrap lines), Alt+W (ignore whitespace) and
//! Alt+H (comments in the diff) switch and save their setting at once.

use egui::{Key, Modifiers};

use cv_pr_diff::diff_view::Direction;
use cv_services::Services;

/// The keys that act on the open file or move to another one, pressed this frame.
pub(crate) struct Pressed {
    pub toggle_viewed: bool,
    pub step: Option<Direction>,
    /// Shift+J or Shift+K: the next or previous file not viewed yet.
    pub unviewed: Option<Direction>,
}

/// Consumes the tab's keys and switches the settings they stand for. J and K only while no text
/// field has the keyboard.
pub(crate) fn read(ui: &egui::Ui, services: &mut Services) -> Pressed {
    let is_typing = ui.ctx().text_edit_focused();
    if ui.input_mut(|input| input.consume_key(Modifiers::ALT, Key::W)) {
        super::super::choices::toggle_ignore_whitespace(services);
    }
    if ui.input_mut(|input| input.consume_key(Modifiers::ALT, Key::H)) {
        super::super::choices::toggle_comments_in_diff(services);
    }
    // Before J and K, which take a Shift+J or Shift+K as well.
    let unviewed = ui.input_mut(|input| {
        let next = !is_typing && input.consume_key(Modifiers::SHIFT, Key::J);
        let previous = !is_typing && input.consume_key(Modifiers::SHIFT, Key::K);
        if next { Some(Direction::Next) } else { previous.then_some(Direction::Previous) }
    });
    let (next, previous, toggle_viewed, wrap) = ui.input_mut(|input| {
        let next = input.consume_key(Modifiers::ALT, Key::ArrowDown)
            || (!is_typing && input.consume_key(Modifiers::NONE, Key::J));
        let previous = input.consume_key(Modifiers::ALT, Key::ArrowUp)
            || (!is_typing && input.consume_key(Modifiers::NONE, Key::K));
        (
            next,
            previous,
            input.consume_key(Modifiers::ALT, Key::V),
            input.consume_key(Modifiers::ALT, Key::Z),
        )
    });
    if wrap {
        services.settings.wrap_lines = !services.settings.wrap_lines;
        services.save_settings();
    }
    let step = if next { Some(Direction::Next) } else { previous.then_some(Direction::Previous) };
    Pressed { toggle_viewed, step, unviewed }
}

/// The file after or before `selected` in the tree's `order`; with none open, "next" is the first.
pub(crate) fn neighbour(
    mut order: impl Iterator<Item = usize>,
    selected: Option<usize>,
    direction: Direction,
) -> Option<usize> {
    let Some(index) = selected else {
        return if direction == Direction::Next { order.next() } else { None };
    };
    let mut previous = None;
    loop {
        let candidate = order.next()?;
        if candidate == index {
            break;
        }
        previous = Some(candidate);
    }
    match direction {
        Direction::Next => order.next(),
        Direction::Previous => previous,
    }
}

/// Where the next file still to review is, in the tree's `order` from `selected`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unviewed {
    /// This file (by index) is the next one not viewed.
    Open(usize),
    /// The open file is the only one left.
    OnlyThisOne,
    /// Every file is viewed.
    AllViewed,
    /// Files are left, but the filters hide all of them.
    Hidden,
}

/// The next (or previous) file of `order` that is not viewed, from `selected` on and wrapping
/// around once; without an open file in the order, from its start (or its end). `total_left`
/// counts the files not viewed over all changed files, the filtered ones too.
pub(crate) fn next_unviewed(
    order: &[u32],
    selected: Option<usize>,
    direction: Direction,
    is_viewed: impl Fn(usize) -> bool,
    total_left: usize,
) -> Unviewed {
    let start =
        selected.and_then(|index| order.iter().position(|listed| *listed as usize == index));
    let count = order.len();
    // The positions after `start` in `direction`, the open file itself last.
    let steps = (1..=count).map(|step| match (start, direction) {
        (Some(start), Direction::Next) => (start + step) % count,
        (Some(start), Direction::Previous) => (start + count - step % count) % count,
        (None, Direction::Next) => step - 1,
        (None, Direction::Previous) => count - step,
    });
    let found = steps
        .map(|position| order[position] as usize)
        .find(|index| Some(*index) != selected && !is_viewed(*index));
    match found {
        Some(index) => Unviewed::Open(index),
        None if total_left == 0 => Unviewed::AllViewed,
        None if selected.is_some_and(|index| !is_viewed(index)) && total_left == 1 => {
            Unviewed::OnlyThisOne
        }
        None => Unviewed::Hidden,
    }
}

#[cfg(test)]
mod tests {
    use super::{Unviewed, neighbour, next_unviewed};
    use cv_pr_diff::diff_view::Direction;

    /// The tree's order: files by index as the filtered, sorted tree lists them.
    const ORDER: [usize; 4] = [7, 2, 9, 4];

    #[test]
    fn steps_follow_the_tree_order_and_stop_at_its_ends() {
        let step = |selected, direction| neighbour(ORDER.into_iter(), Some(selected), direction);
        assert_eq!(step(2, Direction::Next), Some(9));
        assert_eq!(step(2, Direction::Previous), Some(7));
        assert_eq!(step(4, Direction::Next), None, "the last file has no next one");
        assert_eq!(step(7, Direction::Previous), None, "the first file has no previous one");
    }

    #[test]
    fn without_an_open_file_next_opens_the_first_and_a_hidden_file_has_no_neighbours() {
        assert_eq!(neighbour(ORDER.into_iter(), None, Direction::Next), Some(7));
        assert_eq!(neighbour(ORDER.into_iter(), None, Direction::Previous), None);
        // "Hide viewed" keeps the open file in the order (`FileTree::keep_open`); only a file the
        // search or the categories hide drops out of it.
        assert_eq!(
            neighbour(ORDER.into_iter(), Some(5), Direction::Next),
            None,
            "a file the search hides is not in the order"
        );
    }

    /// Shift+J and Shift+K skip viewed files, wrap around the order once and say why they stop.
    #[test]
    fn next_unviewed_skips_viewed_files_and_wraps_once() {
        const TREE: [u32; 5] = [7, 2, 9, 4, 1];
        let viewed = [2, 4];
        let left = |viewed: &[usize]| {
            TREE.iter().filter(|index| !viewed.contains(&(**index as usize))).count()
        };
        let find = |selected, direction, viewed: &[usize]| {
            next_unviewed(&TREE, selected, direction, |index| viewed.contains(&index), left(viewed))
        };
        assert_eq!(find(Some(7), Direction::Next, &viewed), Unviewed::Open(9), "2 is viewed");
        assert_eq!(find(Some(9), Direction::Next, &viewed), Unviewed::Open(1), "4 is viewed");
        assert_eq!(find(Some(1), Direction::Next, &viewed), Unviewed::Open(7), "wraps around");
        assert_eq!(find(Some(9), Direction::Previous, &viewed), Unviewed::Open(7));
        assert_eq!(find(Some(7), Direction::Previous, &viewed), Unviewed::Open(1), "wraps back");
        assert_eq!(find(None, Direction::Next, &viewed), Unviewed::Open(7), "from the start");
        assert_eq!(find(None, Direction::Previous, &viewed), Unviewed::Open(1), "from the end");

        let all_but_nine = [7, 2, 4, 1];
        assert_eq!(find(Some(9), Direction::Next, &all_but_nine), Unviewed::OnlyThisOne);
        assert_eq!(find(Some(2), Direction::Next, &all_but_nine), Unviewed::Open(9));
        let all = [7, 2, 9, 4, 1];
        assert_eq!(find(Some(9), Direction::Next, &all), Unviewed::AllViewed);
        // A file left to review that the filters hide (index 3, not in the order).
        let hidden =
            next_unviewed(&TREE, Some(9), Direction::Next, |index| all.contains(&index), 1);
        assert_eq!(hidden, Unviewed::Hidden);
        assert_eq!(next_unviewed(&[], None, Direction::Next, |_| false, 2), Unviewed::Hidden);
    }
}
