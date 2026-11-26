//! Which file the tab shows when nothing else decides: the first file the first time the files
//! show (the user's "First file" choice), and the file at the open one's place when an update
//! takes the open one out of the list, with a hint that leads back to it.

use review_git::ChangedFile;
use review_store::FilesStart;

use cv_base::theme::{self, icons};
use cv_pr_session::session::{CompareRange, PullRequestSession};
use cv_pr_tree::FileTree;
use cv_widgets::{buttons, glyph};

/// What the Files tab opened on: undecided until its files show for the first time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    Undecided,
    /// A file, a comment's version or a folder was asked for before the files showed.
    Asked,
    Decided,
}

/// The first time the files show, "First file" (the user's choice) opens the first file in the
/// tree's order, before anything is drawn, so the folder view never flashes. A file, a comment or
/// a folder asked for before that wins; the folder view stays a click on a folder away.
pub fn open_start_file(
    start: &mut Start,
    tree: &mut FileTree,
    selected_path: &mut Option<String>,
    files: &[ChangedFile],
    choice: FilesStart,
) {
    let was = std::mem::replace(start, Start::Decided);
    if was != Start::Undecided || selected_path.is_some() || choice != FilesStart::FirstFile {
        return;
    }
    let first = tree.file_order().next();
    if let Some(index) = first {
        *selected_path = Some(files[index].path.clone());
        tree.scroll_to(index);
    }
}

/// The open file is no longer in the list (an update reverted it, another comparison): the file at
/// its place in the tree opens instead. Returns the path that left.
pub fn stay_near_vanished_file(
    tree: &mut FileTree,
    selected_path: &mut Option<String>,
    rank: usize,
    files: &[ChangedFile],
) -> Option<String> {
    let path = selected_path.clone()?;
    if files.iter().any(|file| file.path == path) {
        return None;
    }
    let order: Vec<usize> = tree.file_order().collect();
    let near = order.get(rank.min(order.len().saturating_sub(1))).copied();
    if let Some(index) = near {
        tree.scroll_to(index);
    }
    *selected_path = near.map(|index| files[index].path.clone());
    Some(path)
}

/// "x.rs is no longer changed" with a way back to it in the version before the update. Returns the
/// comparison to switch to.
pub fn show_vanished_hint(
    ui: &mut egui::Ui,
    vanished: &mut Option<String>,
    selected_path: &mut Option<String>,
    session: &PullRequestSession,
) -> Option<CompareRange> {
    let path = vanished.clone()?;
    let palette = theme::palette(ui.ctx());
    let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
    let previous = session.previous_version().filter(|range| *range != session.range);
    let mut picked = None;
    let mut dismiss = false;
    egui::Frame::new()
        .fill(palette.surface_subtle)
        .inner_margin(egui::Margin::symmetric(12, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                glyph::decorative(
                    ui,
                    egui::RichText::new(icons::INFO).color(palette.text_secondary),
                );
                ui.label(
                    egui::RichText::new(format!("{name} is no longer changed in this comparison."))
                        .color(palette.text_secondary),
                )
                .on_hover_text(&path);
                if let Some(range) = previous
                    && buttons::subtle(
                        ui,
                        icons::ARROW_COUNTER_CLOCKWISE,
                        "Show it in the previous version",
                    )
                    .clicked()
                {
                    picked = Some(range);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    dismiss = buttons::icon(ui, icons::X, "Dismiss").clicked();
                });
            });
        });
    if picked.is_some() {
        *selected_path = Some(path);
    }
    if picked.is_some() || dismiss {
        *vanished = None;
    }
    picked
}
