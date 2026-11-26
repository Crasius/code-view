//! The panes beside the file view: the file tree at the window edge the user picked (or its
//! skeleton beside the loading log while the pull request is prepared), with a splitter on its
//! inner edge, and the all-threads panel right of the diff with a splitter on its left edge. Both
//! widths are saved.

use review_git::ChangedFile;
use review_store::{PaneSizes, TreeSide};

use super::super::notices;
use cv_pr_session::actions::ActionRunner;
use cv_pr_session::prepare_progress;
use cv_pr_session::session::PullRequestSession;
use cv_pr_session::web_link;
use cv_pr_threads::{Jump, PanelEvent, PanelInputs, ThreadsUi};
use cv_pr_tree::{FileTree, TreeEvent, TreeInputs, side_panel};
use cv_services::Services;
use cv_widgets::splitter::{self, Splitter};

/// The file tree keeps at least 220 points and can grow until only the diff's grab strip is left.
fn tree_width_range(available: f32) -> std::ops::RangeInclusive<f32> {
    splitter::pane_range(220.0, available)
}

/// The tree's saved width, within what the window leaves it.
fn tree_width(ui: &egui::Ui, services: &Services) -> (f32, std::ops::RangeInclusive<f32>) {
    let range = tree_width_range(ui.available_width());
    (services.settings.panes.file_tree.clamp(*range.start(), *range.end()), range)
}

/// The file tree's place at the window edge of the setting, reserved before the diff is laid out
/// and filled after it, so Tab reaches the diff first on either side and a flip changes no widget
/// ids: the open file keeps its scroll position and the focus.
pub struct TreeArea {
    rect: egui::Rect,
    side: TreeSide,
    width: f32,
    range: std::ops::RangeInclusive<f32>,
}

impl TreeArea {
    /// The tree's saved width at its side of what `ui` has left.
    pub fn reserve(ui: &egui::Ui, services: &Services) -> Self {
        let (width, range) = tree_width(ui, services);
        let side = services.settings.file_tree_side;
        let available = ui.available_rect_before_wrap();
        let mut rect = available;
        match side {
            TreeSide::Left => rect.max.x = available.left() + width,
            TreeSide::Right => rect.min.x = available.right() - width,
        }
        Self { rect, side, width, range }
    }

    pub fn rect(&self) -> egui::Rect {
        self.rect
    }

    /// What `ui` has left beside the tree.
    pub fn rest(&self, ui: &egui::Ui) -> egui::Rect {
        let mut rest = ui.available_rect_before_wrap();
        match self.side {
            TreeSide::Left => rest.min.x = self.rect.right(),
            TreeSide::Right => rest.max.x = self.rect.left(),
        }
        rest
    }
}

/// The file tree with its filters in its reserved `area`. Returns what the user did in the tree.
pub fn show_tree(
    ui: &mut egui::Ui,
    area: &TreeArea,
    tree: &mut FileTree,
    inputs: &TreeInputs<'_>,
    is_counting_lines: bool,
) -> Option<TreeEvent> {
    tree.place(area.side);
    side_panel::show(ui, area.width, tree, inputs, is_counting_lines)
}

/// The splitter on the tree's inner edge, after the diff and the tree, so it sits on top of the
/// diff's scroll bar (or its left edge) along that edge and takes the drag.
pub fn show_tree_splitter(ui: &mut egui::Ui, area: TreeArea, services: &mut Services) {
    let TreeArea { rect, side, range, .. } = area;
    // The tree grows when its inner edge moves towards the diff.
    let (edge, per_point) = match side {
        TreeSide::Left => (rect.right(), 1.0),
        TreeSide::Right => (rect.left(), -1.0),
    };
    let resize = Splitter::new("file_tree_splitter", "Resize file tree")
        .default_value(PaneSizes::DEFAULT.file_tree)
        .range(range)
        .per_point(per_point)
        .draws_line(false)
        .show(ui, edge, rect.y_range(), &mut services.settings.panes.file_tree);
    if resize.committed {
        services.save_settings();
    }
}

/// The tree skeleton at the tree's width beside the live loading log; a failure keeps the log
/// and shows Retry and the platform link above it.
pub fn show_preparing(ui: &mut egui::Ui, session: &mut PullRequestSession, services: &Services) {
    let message = session
        .mirror
        .error()
        .or(session.files_fast.error())
        .or(session.detail.error())
        .map(str::to_owned);
    if let Some(message) = &message {
        session.progress.fail(message);
    }
    let link = web_link::for_pull_request(session);
    let failure = message.as_deref().map(|message| prepare_progress::Failure {
        message,
        open: link.as_ref().map(|link| (link.icon, link.label.as_str())),
    });
    let area = TreeArea::reserve(ui, services);
    let rest = area.rest(ui);
    ui.scope_builder(egui::UiBuilder::new().max_rect(area.rect), |ui| {
        side_panel::show_skeleton(ui, area.width, failure.is_some());
    });
    let action = ui
        .scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| {
            prepare_progress::show(ui, &session.progress, failure.as_ref())
        })
        .inner;
    match action {
        Some(prepare_progress::FailureAction::Retry) => session.refresh(services),
        Some(prepare_progress::FailureAction::Open) => notices::open_link(ui, link.as_ref()),
        None => {}
    }
}

/// What the threads panel works with besides the pull request's state.
pub struct PanelContext<'a> {
    pub tree: &'a FileTree,
    pub files: &'a [ChangedFile],
    pub session: &'a PullRequestSession,
}

/// The all-threads panel on the right with its splitter. Returns where a thread should be shown:
/// in its file, or in the version it was written on.
pub fn show_threads_panel(
    ui: &mut egui::Ui,
    context: &PanelContext<'_>,
    services: &mut Services,
    actions: &mut ActionRunner,
    threads_ui: &mut ThreadsUi,
) -> Option<Jump> {
    let session = context.session;
    let all = session.threads.ready().map_or(&[][..], Vec::as_slice);
    let file_order: Vec<&str> =
        context.tree.file_order().map(|index| context.files[index].path.as_str()).collect();
    let range = threads_width_range(ui.available_width());
    let width = services.settings.panes.threads_panel.clamp(*range.start(), *range.end());
    let inputs = PanelInputs {
        threads: all,
        file_order: &file_order,
        filters: &mut services.settings.threads_panel.filters,
        width,
    };
    let (events, rect) = cv_pr_threads::show_panel(ui, threads_ui, inputs);
    // The panel grows when its left edge moves left.
    let resize = Splitter::new("threads_panel_splitter", "Resize threads panel")
        .default_value(PaneSizes::DEFAULT.threads_panel)
        .range(range)
        .per_point(-1.0)
        .draws_line(false)
        .show(ui, rect.left(), rect.y_range(), &mut services.settings.panes.threads_panel);
    let mut save = resize.committed;
    let mut jump = None;
    for event in events {
        match event {
            PanelEvent::Reveal { path, thread_id } => jump = Some(Jump::Reveal { path, thread_id }),
            PanelEvent::ShowOrigin { thread_id } => jump = Some(Jump::Origin { thread_id }),
            PanelEvent::Close => {
                services.settings.threads_panel.is_open = false;
                save = true;
            }
            PanelEvent::FiltersChanged => save = true,
            PanelEvent::Thread(event) => {
                if let Some(target) = threads_ui.run(event, session, services, actions) {
                    jump = Some(target);
                }
            }
            PanelEvent::PostGeneral => threads_ui.post_general(session, services),
        }
    }
    if save {
        services.save_settings();
    }
    jump
}

/// The threads panel keeps at least 300 points and leaves the diff at least 420.
fn threads_width_range(available: f32) -> std::ops::RangeInclusive<f32> {
    const MIN: f32 = 300.0;
    MIN..=(available - 420.0).clamp(MIN, 640.0)
}
