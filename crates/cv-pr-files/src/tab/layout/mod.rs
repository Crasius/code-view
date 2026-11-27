//! The Files tab's layout. The frame: the bar (or focus mode's strip), the threads panel, the
//! content beside the file tree, then the tree at the window edge the user picked, with its
//! splitter. The tree comes after the diff on either side, so Tab reaches the diff first and a
//! flip of the side keeps every widget's id (the diff's scroll position, the focus). The panes
//! beside the file view and their splitters are in `panels`; how the review steps from file to
//! file, which the frame calls at its start, around the file and at its end, in `stepping`.

pub(crate) mod panels;

use std::collections::HashMap;
use std::sync::Arc;

use review_core::{ReviewThread, ThreadStatus};
use review_git::ChangedFile;
use review_store::DiffLayout;

use super::review::stepping;
use super::{origin, start};
use crate::{Env, FilesTab, Opened, focus_strip};
use cv_pr_diff::diff_view::Direction;
use cv_pr_session::assistant_link;
use cv_pr_tree::{TreeEvent, TreeInputs};
use cv_runtime::load::Live;
use cv_services::Services;
use cv_shell::focus_mode::{self, Surface};

/// The frame: the bar (or focus mode's strip), the threads panel, then the original version of
/// a thread, the folder summary or the open file, and last the file tree at its side with its
/// splitter. The tree comes after the diff, so Tab reaches the diff first and the splitter sits on
/// top of the diff's edge; what the user did in the tree is carried out at the start of the next
/// frame, before anything reads the open file or the viewed marks. Moving the tree to the other
/// side is carried out at once, and the frame is laid out again before it shows.
pub(crate) fn show(tab: &mut FilesTab, ui: &mut egui::Ui, env: &mut Env<'_>) {
    tab.poll_loads(env.services);
    let Some(files) = env.session.files().cloned() else {
        panels::show_preparing(ui, env.session, env.services);
        return;
    };
    // Before the tree's click from the last frame, which may mark the open file viewed.
    let watched = stepping::watch(tab, &files);
    stepping::go_to_keys(tab, ui.ctx(), &files);
    stepping::take_requests(tab, ui.ctx(), env, &files);
    apply_pending_tree_event(tab, env.services, &files);
    tab.follow_files(env.session, env.services.settings.files_start, &files);
    // Focus mode: the strip at the top of the window stands in for the file header (or the
    // original version's banner), and the file tree shows only when the user asked for it there.
    let layout_before = env.services.settings.diff_layout;
    let is_focused = focus_mode::offer(ui.ctx(), Surface::Files);
    let strip = show_strip(tab, ui, env, &files, is_focused);
    if let Some(range) =
        start::show_vanished_hint(ui, &mut tab.vanished, &mut tab.selected_path, env.session)
    {
        env.session.set_range(env.services, range);
    }
    let selected = tab.selected_index(&files);
    if let Some(rank) =
        selected.and_then(|index| tab.tree.file_order().position(|position| position == index))
    {
        tab.selected_rank = rank;
    }

    let thread_counts = tab.thread_counts.of(&env.session.threads);
    let shows_tree = !is_focused || env.services.settings.focus_file_tree || tab.tree.is_going_to();
    if !shows_tree {
        tab.tree.forget_hover();
    }
    let tree_area = shows_tree.then(|| panels::TreeArea::reserve(ui, env.services));
    let content =
        tree_area.as_ref().map_or_else(|| ui.available_rect_before_wrap(), |area| area.rest(ui));
    assistant_link::publish_room(ui.ctx(), content);
    let frame = Frame { selected, strip, layout_before, thread_counts: &thread_counts };
    ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
        ui.set_clip_rect(content.intersect(ui.clip_rect()));
        show_content(tab, ui, env, &files, frame);
    });
    if let Some(area) = tree_area {
        let event = ui
            .scope_builder(egui::UiBuilder::new().max_rect(area.rect()), |ui| {
                show_tree(tab, ui, env, &files, selected, &thread_counts, &area)
            })
            .inner;
        match event {
            Some(TreeEvent::FlipSide) => flip_side(ui, env.services),
            Some(TreeEvent::Review(step)) => stepping::run(tab, ui.ctx(), env, &files, step),
            Some(event) => {
                tab.pending_tree_event = Some((Arc::clone(&files), event));
                ui.ctx().request_repaint();
            }
            None => {}
        }
        panels::show_tree_splitter(ui, area, env.services);
    }
    stepping::after_viewed(tab, ui.ctx(), env, &files, watched);
}

/// Carries out what the user did in the tree last frame, also when a newer file list replaced the
/// one it was done on since (see [`TreeEvent::carried_to`]): the click is never lost.
pub(crate) fn apply_pending_tree_event(
    tab: &mut FilesTab,
    services: &mut Services,
    files: &[ChangedFile],
) {
    let Some((listed, event)) = tab.pending_tree_event.take() else {
        return;
    };
    let event = event.carried_to(&listed, files);
    if let Some(toggle) = tab.apply_tree_event(event, files) {
        tab.viewed.apply(services, files, &toggle);
    }
}

/// Moves the file tree to the other window edge with one save, and lays the frame out again, so
/// the first frame shown has the tree at its new place at the same width.
fn flip_side(ui: &egui::Ui, services: &mut cv_services::Services) {
    let settings = &mut services.settings;
    settings.file_tree_side = settings.file_tree_side.flipped();
    services.save_settings();
    ui.ctx().request_discard("the file tree moved to the other side");
}

/// The file tree with its viewed marks, thread counts and the open file or folder, in `area`.
fn show_tree(
    tab: &mut FilesTab,
    ui: &mut egui::Ui,
    env: &Env<'_>,
    files: &Arc<Vec<ChangedFile>>,
    selected: Option<usize>,
    thread_counts: &HashMap<String, usize>,
    area: &panels::TreeArea,
) -> Option<TreeEvent> {
    let selected_folder =
        (selected.is_none() && !tab.summary.folder().is_empty()).then_some(tab.summary.folder());
    let vote = stepping::vote_offer(env);
    let viewed = &tab.viewed;
    let is_viewed = |file: &ChangedFile| viewed.is_viewed(file);
    let inputs =
        TreeInputs { files, selected, selected_folder, is_viewed: &is_viewed, thread_counts, vote };
    let is_counting_lines = env.session.is_counting_lines();
    panels::show_tree(ui, area, &mut tab.tree, &inputs, is_counting_lines)
}

/// Beside the file tree: the threads panel, then the original version of a thread, the
/// folder summary or the open file, after the keys.
fn show_content(
    tab: &mut FilesTab,
    ui: &mut egui::Ui,
    env: &mut Env<'_>,
    files: &Arc<Vec<ChangedFile>>,
    frame: Frame<'_>,
) {
    let Frame { selected, strip, layout_before, thread_counts } = frame;
    let (strip_header, strip_step) =
        strip.map_or((None, None), |strip| (Some(strip.header), strip.step_file));
    tab.show_threads_panel(ui, env, files);
    let keyboard_event = stepping::keyboard(tab, ui, selected, env, files);
    let keyboard_event = keyboard_event.or_else(|| {
        let step = strip_step.and_then(|direction| stepping::neighbour(tab, selected, direction));
        step.map(TreeEvent::Select)
    });
    if let Some(TreeEvent::Select(index)) = &keyboard_event {
        tab.tree.scroll_to(*index);
    }
    // Applied once the header and the strip read the viewed marks of this frame.
    let toggled = tab.apply_tree_event(keyboard_event, files);
    if tab.show_origin(ui, env) {
        if let Some(toggle) = &toggled {
            tab.viewed.apply(env.services, files, toggle);
        }
        return;
    }
    let Some(index) = selected else {
        if let Some(toggle) = &toggled {
            tab.viewed.apply(env.services, files, toggle);
        }
        tab.show_summary(ui, env, files, thread_counts);
        return;
    };
    let opened = Opened { header: strip_header, toggled, layout_before };
    tab.show_file(ui, env, files, index, opened);
    stepping::follow_edge(tab, ui, files, index);
}

/// Focus mode's strip: for the original version while one shows, else for the open file or the
/// summary. `None` outside focus mode.
fn show_strip(
    tab: &mut FilesTab,
    ui: &mut egui::Ui,
    env: &mut Env<'_>,
    files: &[ChangedFile],
    is_focused: bool,
) -> Option<focus_strip::StripAction> {
    if !is_focused {
        return None;
    }
    if tab.origin.is_some() {
        origin::show_strip(&mut tab.origin, ui, env.session, env.threads_ui, env.services);
        return Some(focus_strip::StripAction::default());
    }
    Some(show_focus_strip(tab, ui, files, env))
}

/// Focus mode's bar for the open file, or for the summary while none is open.
fn show_focus_strip(
    tab: &FilesTab,
    ui: &mut egui::Ui,
    files: &[ChangedFile],
    env: &mut Env<'_>,
) -> focus_strip::StripAction {
    let selected = tab.selected_index(files);
    let file = selected.map(|index| &files[index]);
    let state = file.map(|file| tab.file_state(file, env.services));
    let threads = env.session.threads.ready().map_or(&[][..], Vec::as_slice);
    let counts = cv_pr_threads::toggle_counts(threads, env.threads_ui);
    let inputs = focus_strip::StripInputs {
        file,
        folder: tab.summary.folder(),
        is_viewed: file.is_some_and(|file| tab.viewed.is_viewed(file)),
        changes: state.and_then(|state| state.changes),
        is_rendered: state.is_some_and(|state| state.rendered_changes.is_some()),
        whole_file: state.and_then(|state| state.whole_file),
        hides_whitespace: state.is_some_and(|state| state.hides_whitespace),
        has_previous_file: stepping::neighbour(tab, selected, Direction::Previous).is_some(),
        has_next_file: stepping::neighbour(tab, selected, Direction::Next).is_some(),
        edge_hint: selected
            .zip(file)
            .and_then(|(index, file)| stepping::edge_hint(tab, ui, index, &file.path)),
        open_threads: counts.active,
        has_news: counts.unread > 0,
    };
    focus_strip::show(ui, &inputs, env.services)
}

/// What the frame worked out before the content beside the tree shows.
struct Frame<'a> {
    /// The open file, as the frame started.
    selected: Option<usize>,
    /// What focus mode's strip did; `None` outside focus mode.
    strip: Option<focus_strip::StripAction>,
    /// The layout when the frame started, for Alt+N to come back to.
    layout_before: DiffLayout,
    thread_counts: &'a HashMap<String, usize>,
}

/// Open threads by the path they are on, counted again only when the threads changed.
#[derive(Default)]
pub(crate) struct ThreadCounts {
    /// The version of the threads counted; `None` while none are loaded.
    counted: Option<u64>,
    counts: Arc<HashMap<String, usize>>,
}

impl ThreadCounts {
    fn of(&mut self, threads: &Live<Vec<ReviewThread>>) -> Arc<HashMap<String, usize>> {
        let version = threads.ready().map(|_| threads.version());
        if version != self.counted {
            self.counted = version;
            self.counts = Arc::new(count_threads(threads.ready().map_or(&[][..], Vec::as_slice)));
        }
        Arc::clone(&self.counts)
    }
}

/// Open threads by the path they are on.
fn count_threads(threads: &[ReviewThread]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for thread in threads {
        let is_open = thread.status == ThreadStatus::Active;
        if let Some(anchor) = &thread.anchor
            && is_open
        {
            *counts.entry(anchor.path.clone()).or_insert(0) += 1;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use review_core::{ReviewThread, ThreadStatus};

    use super::ThreadCounts;
    use cv_pr_threads::samples;
    use cv_runtime::load::Live;

    #[test]
    fn thread_counts_follow_a_reload_and_a_clear_of_the_threads() {
        let on = |id: &str, status| {
            samples::thread(id, status, Some(samples::lines("src/a.rs", 1, 1)), Vec::new())
        };
        let mut threads: Live<Vec<ReviewThread>> = Live::default();
        let mut counts = ThreadCounts::default();
        assert!(counts.of(&threads).is_empty(), "nothing while the threads load");

        threads.set(vec![on("t-1", ThreadStatus::Active), on("t-2", ThreadStatus::Resolved)]);
        assert_eq!(counts.of(&threads).get("src/a.rs"), Some(&1), "open threads only");
        threads.set(vec![on("t-1", ThreadStatus::Active), on("t-2", ThreadStatus::Active)]);
        assert_eq!(counts.of(&threads).get("src/a.rs"), Some(&2), "a reload counts again");
        threads.clear();
        assert!(counts.of(&threads).is_empty(), "cleared threads count nothing");
    }
}
