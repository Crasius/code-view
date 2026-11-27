//! What the popover says when a checkout cannot simply go ahead, each with its way on: the clone
//! has changes, no clone is known, the branch is checked out already, the pull request comes from
//! a fork, the checkout failed; and for a done checkout, removing its worktree and an update that
//! changed nothing.

use std::path::Path;

use egui::{Color32, RichText, Ui};
use review_git::checkout::{Blocked, Survey};
use review_store::checkouts::{CheckoutRecord, OpenWith};

use super::choose::folder_name;
use super::{Act, View, progress, settings_link};
use cv_base::theme::{self, icons, style};
use cv_runtime::intents::SettingsSection;
use cv_services::checkouts::{Failure, FolderPurpose, Job, RemovalState, Status, plural, reason};
use cv_widgets::buttons;
use cv_widgets::progress_log::{self, redact};

/// Files or commits a card lists before "and N more".
const MAX_LISTED: usize = 8;

/// The card's small label and its title.
fn head(ui: &mut Ui, label: &str, title: &str, tone: Color32) {
    ui.label(RichText::new(label).font(theme::fonts::semibold_font(style::SMALL_SIZE)).color(tone));
    ui.label(RichText::new(title).font(theme::fonts::semibold_font(style::SECTION_SIZE)));
}

fn body(ui: &mut Ui, text: &str) {
    let palette = theme::palette(ui.ctx());
    ui.label(RichText::new(text).color(palette.text_secondary));
}

/// Paths, file lines or commits in the code font, the first [`MAX_LISTED`] of them.
fn listing(ui: &mut Ui, lines: &[String]) {
    let palette = theme::palette(ui.ctx());
    let frame = egui::Frame::new()
        .fill(palette.surface_subtle)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(style::RADIUS_CARD)
        .inner_margin(egui::Margin::symmetric(10, 8));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = style::SPACE_XS;
        for line in lines.iter().take(MAX_LISTED) {
            ui.add(
                egui::Label::new(
                    RichText::new(redact(line)).font(theme::fonts::code_font(style::SMALL_SIZE)),
                )
                .truncate(),
            );
        }
        if lines.len() > MAX_LISTED {
            ui.label(
                RichText::new(format!("and {} more", lines.len() - MAX_LISTED))
                    .size(style::SMALL_SIZE)
                    .color(palette.text_secondary),
            );
        }
    });
}

/// A primary and a secondary button; returns the act of the one clicked.
fn two_buttons(ui: &mut Ui, primary: (&str, Act), secondary: (&str, Act)) -> Option<Act> {
    let mut act = None;
    ui.horizontal(|ui| {
        if buttons::primary(ui, primary.0).clicked() {
            act = Some(primary.1);
        }
        if buttons::secondary(ui, secondary.0).clicked() {
            act = Some(secondary.1);
        }
    });
    act
}

/// The clone has uncommitted changes, so switching it is refused.
pub(super) fn dirty(ui: &mut Ui, clone: &Path, files: &[String]) -> Option<Act> {
    let palette = theme::palette(ui.ctx());
    let count = files.len();
    let title = format!("{} has {count} uncommitted {}", folder_name(clone), plural(count, "file"));
    head(ui, "YOUR CLONE HAS CHANGES", &title, palette.warning);
    body(
        ui,
        "Switching the branch there would carry them along or fail. A worktree leaves them where they are.",
    );
    listing(ui, files);
    two_buttons(
        ui,
        ("Use a new worktree", Act::UseWorktree),
        ("Open the clone", Act::OpenFolder(clone.to_path_buf())),
    )
}

/// No clone of the repository in the search folders.
pub(super) fn no_clone(
    ui: &mut Ui,
    view: &View<'_>,
    picked: Option<&Path>,
    is_asking: bool,
    has_mirror: bool,
) -> Option<Act> {
    let palette = theme::palette(ui.ctx());
    let title = format!("Code View knows no clone of {}", view.key.repository_key);
    head(ui, "NO LOCAL CLONE FOUND", &title, palette.text_secondary);
    body(
        ui,
        "Pick the folder of an existing clone once, or let Code View clone it from its mirror, which needs no second download.",
    );
    if let Some(picked) = picked {
        ui.label(
            RichText::new(format!(
                "{} is not a clone of {}. Pick the folder that holds its .git folder.",
                picked.display(),
                view.key.repository_key
            ))
            .color(palette.error),
        );
    }
    let key = view.key.clone();
    let mut act = None;
    if !has_mirror {
        ui.label(
            RichText::new("Getting the pull request's commits into the mirror first…")
                .size(style::SMALL_SIZE)
                .color(palette.text_secondary),
        );
    }
    // A new clone comes from the mirror, so it waits for it; picking a clone does not.
    ui.add_enabled_ui(!is_asking, |ui| {
        ui.horizontal(|ui| {
            let clone =
                ui.add_enabled_ui(has_mirror, |ui| buttons::primary(ui, "Clone into a folder…"));
            if clone.inner.clicked() {
                let purpose = FolderPurpose::CloneInto(key.clone());
                act = Some(Act::AskFolder(purpose, "Folder for the new clone"));
            }
            if buttons::secondary(ui, "Pick existing clone…").clicked() {
                let purpose = FolderPurpose::PickClone(key);
                act = Some(Act::AskFolder(purpose, "Your clone of the repository"));
            }
        });
    });
    act.or_else(|| {
        settings_link(
            ui,
            "Search folders in Settings › Local checkouts",
            SettingsSection::LocalCheckouts,
        )
    })
}

/// The branch is checked out in another working tree, which git allows once only.
pub(super) fn checked_out(ui: &mut Ui, path: &Path) -> Option<Act> {
    let palette = theme::palette(ui.ctx());
    head(ui, "ALREADY CHECKED OUT", "This branch is already in a worktree", palette.accent_text);
    body(
        ui,
        "git allows a branch in one worktree only. Code View opens that one instead of making a second.",
    );
    listing(ui, &[path.display().to_string()]);
    two_buttons(
        ui,
        ("Open it", Act::Adopt { path: path.to_path_buf(), then_update: false }),
        ("Update it", Act::Adopt { path: path.to_path_buf(), then_update: true }),
    )
}

/// The pull request's branch is not on the clone's remote: no upstream, its own local name.
pub(super) fn fork(ui: &mut Ui, view: &View<'_>, survey: &Survey) -> Option<Act> {
    let palette = theme::palette(ui.ctx());
    let source = &view.detail.summary.source_branch;
    let title = format!("The branch {source} lives in a fork");
    head(ui, "PULL REQUEST FROM A FORK", &title, palette.accent_text);
    body(
        ui,
        &format!(
            "The commits come from the pull request ref, so no remote is added. The local branch is named {} and has no upstream.",
            survey.branch
        ),
    );
    two_buttons(ui, ("Check out", Act::ConfirmFork), ("Cancel", Act::Close))
}

/// A checkout that stopped: the card of its reason, or git's failure with the steps and the log.
pub(super) fn failed(ui: &mut Ui, view: &View<'_>, job: &Job, failure: &Failure) -> Option<Act> {
    match failure {
        Failure::Dirty(files) => dirty(ui, &job.plan.clone, files),
        Failure::CheckedOut(path) => checked_out(ui, path),
        Failure::Cancelled => {
            let palette = theme::palette(ui.ctx());
            head(ui, "CANCELLED", "The checkout was cancelled", palette.text_secondary);
            body(ui, "What it had made is removed again.");
            two_buttons(ui, ("Check out again", Act::Dismiss), ("Close", Act::Close))
        }
        Failure::Git { title, message } => git_failed(ui, view, job, title, message),
    }
}

/// git failed: its title and message, the steps and the log, with Retry.
fn git_failed(ui: &mut Ui, view: &View<'_>, job: &Job, title: &str, message: &str) -> Option<Act> {
    let palette = theme::palette(ui.ctx());
    head(ui, "FAILED", title, palette.error);
    let progress = job.progress();
    progress::steps(ui, &progress, std::time::Instant::now());
    listing(ui, &message.lines().map(str::to_owned).collect::<Vec<_>>());
    body(
        ui,
        &format!(
            "Nothing of #{} is left behind: what the checkout made is removed.",
            view.key.number
        ),
    );
    let mut act = None;
    ui.horizontal(|ui| {
        if buttons::primary_icon(ui, icons::ARROW_CLOCKWISE, "Retry").clicked() {
            act = Some(Act::Retry);
        }
        progress_log::copy_button(ui, &progress.log);
        if buttons::secondary(ui, "Cancel").clicked() {
            act = Some(Act::Dismiss);
        }
    });
    act
}

/// Removing the checkout's worktree: the first question, the second for one with changes.
/// `kept` says why the worktree of a finished pull request did not go by itself.
pub(super) fn remove(
    ui: &mut Ui,
    record: &CheckoutRecord,
    state: Option<&RemovalState>,
    kept: Option<&str>,
) -> Option<Act> {
    let palette = theme::palette(ui.ctx());
    match state {
        None => {
            head(ui, "REMOVE WORKTREE", "Remove this worktree?", palette.warning);
            if let Some(kept) = kept {
                body(ui, &format!("It stayed when the pull request was done: {kept}."));
            }
            body(
                ui,
                &format!(
                    "Deletes the folder below. The branch {} stays unless nothing was added to it.",
                    record.branch
                ),
            );
            listing(ui, &[record.path.display().to_string()]);
            danger_or_keep(ui, "Remove worktree", Act::Remove { force: false })
        }
        Some(RemovalState::Removing) => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(icons::SIZE - 2.0).color(palette.text_secondary));
                ui.label("Removing the worktree…");
            });
            None
        }
        Some(RemovalState::Changed(files)) => {
            let count = files.len();
            let title = format!("{count} {} would be lost", plural(count, "change"));
            head(ui, "WORKTREE HAS CHANGES", &title, palette.error);
            body(ui, "The worktree has files that are not committed. Removing it deletes them.");
            listing(ui, files);
            danger_or_keep(ui, "Remove anyway", Act::Remove { force: true })
        }
        Some(RemovalState::Failed(message)) => {
            head(ui, "NOT REMOVED", "The worktree could not be removed", palette.error);
            listing(ui, &message.lines().map(str::to_owned).collect::<Vec<_>>());
            danger_or_keep(ui, "Try again", Act::Remove { force: false })
        }
    }
}

fn danger_or_keep(ui: &mut Ui, label: &str, danger: Act) -> Option<Act> {
    let mut act = None;
    ui.horizontal(|ui| {
        if buttons::danger(ui, icons::TRASH, label).clicked() {
            act = Some(danger);
        }
        if buttons::secondary(ui, "Keep").clicked() {
            act = Some(Act::Keep);
        }
    });
    act
}

/// An update by hand changed nothing: the local commits it would drop, or the reason.
pub(super) fn blocked(
    ui: &mut Ui,
    view: &View<'_>,
    record: &CheckoutRecord,
    status: Option<&Status>,
) -> Option<Act> {
    let palette = theme::palette(ui.ctx());
    let Some(Status { blocked: Some(blocked), behind, .. }) = status else {
        head(ui, "UP TO DATE", "The checkout has the pull request's head", palette.success);
        return two_buttons(ui, ("Close", Act::Close), ("Show log", Act::ShowLog));
    };
    if let Blocked::LocalCommits(commits) = blocked {
        let count = commits.len();
        let title = format!("Update would drop {count} local {}", plural(count, "commit"));
        head(ui, "LOCAL COMMITS", &title, palette.warning);
        body(
            ui,
            "Your checkout has a commit that is not in the pull request. Update only fast-forwards, so it stops here and shows you the commit.",
        );
        listing(ui, commits);
        return two_buttons(
            ui,
            ("Show in terminal", Act::Open(OpenWith::Terminal)),
            ("Keep as is", Act::Close),
        );
    }
    let title = format!("#{} was not updated", view.key.number);
    head(ui, "NOT UPDATED", &title, palette.warning);
    body(ui, &reason(blocked, *behind));
    if let Blocked::Changed(files) = blocked {
        listing(ui, files);
    }
    two_buttons(ui, ("Open folder", Act::OpenFolder(record.path.clone())), ("Close", Act::Close))
}
