//! How the review moves on through the files: J/K and Shift+J/K (the next file, the next one not
//! viewed yet), Ctrl+P's go to file, the next unviewed file opening after the open one was marked
//! viewed (Settings, on by default), and N or P past the last or first change, which first says
//! so under the change arrows and on a second press opens the next (or previous) file at its
//! first (or last) change. Where the steps end, a toast says why and offers the way on: the vote
//! and the Overview once every file is viewed, clearing the filters when they hide what is left.
//! The toasts' buttons and the progress line's links ask through [`ReviewStep`].

use egui::{Context, Ui};
use review_core::{PullRequestState, ReviewVote};
use review_git::ChangedFile;

use super::super::super::{Env, FilesTab};
use super::keys::{self, Unviewed};
use super::viewed::ViewedToggle;
use cv_base::mailbox;
use cv_base::theme::icons;
use cv_pr_diff::diff_view::{Direction, LineRequest};
use cv_pr_session::lifecycle::{LifecycleAction, author_refusal};
use cv_pr_tree::{FileFilter, ReviewStep, TreeEvent, VoteOffer, goto};
use cv_runtime::route::PullRequestKey;
use cv_runtime::toasts::ToastAction;
use cv_services::self_review::{self, Standing};
use cv_widgets::badges;

/// How long the note after a step past the last or first change shows, and a second press of the
/// same key goes on into the next file, in seconds.
const EDGE_SECONDS: f64 = 4.0;

/// The tab's part of the review's steps.
pub struct Stepping {
    /// Where the toasts' buttons leave their [`ReviewStep`] for this pull request's tab.
    requests: egui::Id,
    /// The last step past the last or first change, while its note shows.
    edge: Option<Edge>,
    /// "Overview" was asked for: the page switches tabs.
    wants_overview: bool,
}

/// A step that found no change further that way.
struct Edge {
    path: String,
    direction: Direction,
    /// When, in egui's input time.
    since: f64,
    /// The change position then: a step back or a scroll away ends the note.
    position: (usize, usize),
}

impl Stepping {
    pub fn new(key: &PullRequestKey) -> Self {
        Self {
            requests: egui::Id::unique(("files_review_step", key)),
            edge: None,
            wants_overview: false,
        }
    }

    /// Whether "Overview" was asked for since the last call.
    pub fn take_overview_request(&mut self) -> bool {
        std::mem::take(&mut self.wants_overview)
    }
}

/// The file after or before `selected` in the tree's order; with none open, "next" is the first.
pub fn neighbour(tab: &FilesTab, selected: Option<usize>, direction: Direction) -> Option<usize> {
    keys::neighbour(tab.tree.file_order(), selected, direction)
}

/// The tab's keys ([`keys`]): a step to the next or previous file (or the next one not viewed),
/// or the viewed mark of the open file, else of the folder whose summary shows.
pub fn keyboard(
    tab: &mut FilesTab,
    ui: &Ui,
    selected: Option<usize>,
    env: &mut Env<'_>,
    files: &[ChangedFile],
) -> Option<TreeEvent> {
    let pressed = keys::read(ui, env.services);
    if pressed.toggle_viewed {
        match selected {
            Some(index) => return Some(TreeEvent::ToggleViewed(index)),
            None if !tab.summary.folder().is_empty() => {
                return Some(TreeEvent::ToggleFolderViewed(tab.summary.folder().to_owned()));
            }
            None => {}
        }
    }
    if let Some(direction) = pressed.unviewed {
        let next = next_unviewed(tab, ui.ctx(), env, files, direction)?;
        tab.tree.reveal(&files[next].path);
        return Some(TreeEvent::Select(next));
    }
    let direction = pressed.step?;
    neighbour(tab, selected, direction).map(TreeEvent::Select)
}

/// Ctrl+P's go to file: opens what Enter picked.
pub fn go_to_keys(tab: &mut FilesTab, ctx: &Context, files: &[ChangedFile]) {
    if let Some(index) = goto::keys(&mut tab.tree, ctx, files) {
        open(tab, files, index);
    }
}

/// Opens the file at `index` and shows it in the tree.
fn open(tab: &mut FilesTab, files: &[ChangedFile], index: usize) {
    let Some(file) = files.get(index) else {
        return;
    };
    tab.selected_path = Some(file.path.clone());
    tab.vanished = None;
    tab.origin = None;
    tab.tree.reveal(&file.path);
    tab.tree.scroll_to(index);
}

/// The next (or previous) file not viewed yet, in the tree's order; where there is none, a toast
/// says why and offers the way on.
fn next_unviewed(
    tab: &FilesTab,
    ctx: &Context,
    env: &mut Env<'_>,
    files: &[ChangedFile],
    direction: Direction,
) -> Option<usize> {
    let selected = tab.selected_index(files);
    let viewed = &tab.viewed;
    let left = files.iter().filter(|file| !viewed.is_viewed(file)).count();
    let is_viewed = |index: usize| files.get(index).is_some_and(|file| viewed.is_viewed(file));
    let found = keys::next_unviewed(tab.tree.full_order(), selected, direction, is_viewed, left);
    let id = tab.stepping.requests;
    let action = move |icon, label: &str, step| {
        ToastAction::new(icon, label, move |ctx| mailbox::post(ctx, id, step))
    };
    let toasts = &mut env.services.toasts;
    match found {
        Unviewed::Open(index) => return Some(index),
        Unviewed::OnlyThisOne => toasts.info(
            "This is the last file left to review.",
            vec![action(icons::CHECK_SQUARE, "Mark as viewed", ReviewStep::MarkViewed)],
        ),
        Unviewed::AllViewed => {
            let mut actions = Vec::new();
            if let VoteOffer::Approve { is_enabled: true } = vote_offer(env) {
                actions.push(action(icons::CHECK, "Approve", ReviewStep::Approve));
            }
            actions.push(action(icons::BOOK_OPEN_TEXT, "Overview", ReviewStep::Overview));
            let text = format!("All {} files viewed.", files.len());
            env.services.toasts.info(text, actions);
        }
        Unviewed::Hidden => toasts.info(
            "The filters hide the files left to review.",
            vec![action(icons::FUNNEL, "Clear filters", ReviewStep::ClearFilters)],
        ),
    }
    ctx.request_repaint();
    None
}

/// Carries out what a toast's button asked for since the last frame.
pub fn take_requests(tab: &mut FilesTab, ctx: &Context, env: &mut Env<'_>, files: &[ChangedFile]) {
    if let Some(step) = mailbox::take::<ReviewStep>(ctx, tab.stepping.requests) {
        run(tab, ctx, env, files, step);
    }
}

/// Carries out `step`, from the progress line or a toast.
pub fn run(
    tab: &mut FilesTab,
    ctx: &Context,
    env: &mut Env<'_>,
    files: &[ChangedFile],
    step: ReviewStep,
) {
    match step {
        ReviewStep::NextUnviewed => {
            if let Some(index) = next_unviewed(tab, ctx, env, files, Direction::Next) {
                open(tab, files, index);
            }
        }
        ReviewStep::Approve => {
            if let VoteOffer::Approve { is_enabled: true } = vote_offer(env) {
                let approve = LifecycleAction::Vote { vote: ReviewVote::Approved, comment: None };
                // A write that did not start (another one runs) is visible as the busy header.
                approve.start(env.session, env.services, env.actions);
            }
        }
        ReviewStep::Overview => tab.stepping.wants_overview = true,
        ReviewStep::ClearFilters => tab.tree.filter = FileFilter::default(),
        ReviewStep::MarkViewed => {
            if let Some(index) = tab.selected_index(files) {
                tab.viewed.apply(env.services, files, &ViewedToggle::File(index));
            }
        }
    }
    ctx.request_repaint();
}

/// What the review can offer for the vote: "Approve" on an open pull request whose platform
/// takes approvals, with a token; the user's vote once the platform lists one, so a second
/// approval never goes out. Nothing where the platform refuses the author's vote.
pub fn vote_offer(env: &Env<'_>) -> VoteOffer {
    let session = &*env.session;
    let Some(detail) = session.detail.ready() else {
        return VoteOffer::None;
    };
    let can_approve = session.has_token
        && detail.summary.state == PullRequestState::Open
        && session.capabilities.votes.contains(&ReviewVote::Approved);
    if !can_approve {
        return VoteOffer::None;
    }
    let me = env.services.identities.known(&session.key.connection_id);
    let standing = self_review::standing(&detail.summary, me.as_ref());
    if author_refusal(session.kind, standing).is_some() {
        return VoteOffer::None;
    }
    match standing {
        Standing::Reviewer(vote) if vote != ReviewVote::None => {
            VoteOffer::Voted(badges::vote_label(vote))
        }
        Standing::Reviewer(_) | Standing::Unknown | Standing::Author | Standing::Outside => {
            VoteOffer::Approve { is_enabled: !env.actions.is_busy() }
        }
    }
}

/// The open file and whether it is viewed, before the frame acts on it.
pub fn watch(tab: &FilesTab, files: &[ChangedFile]) -> Option<(usize, bool)> {
    let index = tab.selected_index(files)?;
    Some((index, tab.viewed.is_viewed(&files[index])))
}

/// After the frame: the open file (`watched` before it) just became viewed, and Settings asks for
/// the next unviewed file to open then.
pub fn after_viewed(
    tab: &mut FilesTab,
    ctx: &Context,
    env: &mut Env<'_>,
    files: &[ChangedFile],
    watched: Option<(usize, bool)>,
) {
    let Some((index, false)) = watched else {
        return;
    };
    let is_still_open = tab.selected_index(files) == Some(index);
    let became_viewed = is_still_open && tab.viewed.is_viewed(&files[index]);
    if !became_viewed || !env.services.settings.open_next_unviewed {
        return;
    }
    if let Some(next) = next_unviewed(tab, ctx, env, files, Direction::Next) {
        open(tab, files, next);
        ctx.request_repaint();
    }
}

/// After the open file showed (`index`): a step past its last (or first) change notes so; the
/// same step again while the note shows opens the next (or previous) file at its first (or last)
/// change. Any other step, another file or the note's time ends it.
pub fn follow_edge(tab: &mut FilesTab, ui: &Ui, files: &[ChangedFile], index: usize) {
    let now = ui.input(|input| input.time);
    let path = &files[index].path;
    let position = tab.view.change_position();
    let ran_out = tab.view.take_ran_out();
    let is_over = tab.stepping.edge.as_ref().is_some_and(|edge| {
        edge.path != *path || now - edge.since > EDGE_SECONDS || edge.position != position
    });
    if is_over {
        tab.stepping.edge = None;
        ui.ctx().request_repaint();
    }
    let Some(direction) = ran_out else {
        if let Some(edge) = &tab.stepping.edge {
            let left = (EDGE_SECONDS - (now - edge.since)).max(0.0);
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(left + 0.05));
        }
        return;
    };
    ui.ctx().request_repaint();
    let is_again = tab.stepping.edge.take().is_some_and(|edge| edge.direction == direction);
    let next = neighbour(tab, Some(index), direction);
    if let Some(next) = next.filter(|_| is_again) {
        open(tab, files, next);
        let file = &files[next];
        if !tab.preview.is_active(file) {
            tab.line_request = Some((file.path.clone(), LineRequest::Change(direction)));
        }
        return;
    }
    tab.stepping.edge = Some(Edge { path: path.clone(), direction, since: now, position });
}

/// The note under the change arrows of the open file (`index`) while it shows.
pub fn edge_hint(tab: &FilesTab, ui: &Ui, index: usize, path: &str) -> Option<&'static str> {
    let now = ui.input(|input| input.time);
    let edge = tab
        .stepping
        .edge
        .as_ref()
        .filter(|edge| edge.path == path && now - edge.since <= EDGE_SECONDS)?;
    let goes_on = neighbour(tab, Some(index), edge.direction).is_some();
    Some(match (edge.direction, goes_on) {
        (Direction::Next, true) => "Last change. N again: next file",
        (Direction::Previous, true) => "First change. P again: previous file",
        (Direction::Next, false) => "Last change of the last file",
        (Direction::Previous, false) => "First change of the first file",
    })
}
