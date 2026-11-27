//! Commits tab: the pull request's commits, newest first. A click opens that commit's own changes.

use egui::{Align2, Id, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use review_core::{SHORT_SHA_LEN, UserIdentity, short_sha};
use review_git::CommitInfo;

use cv_base::theme::{self, icons, motion, style};
use cv_pr_session::session::{CompareRange, PullRequestSession};
use cv_widgets::states::{self, Action};
use cv_widgets::{avatar, buttons, elide, glyph, row, skeleton, time_text};

const ROW_HEIGHT: f32 = 56.0;
const SKELETON_ROWS: usize = 8;
const AVATAR_SIZE: f32 = 28.0;
const PAGE_MARGIN: egui::Margin = egui::Margin::same(style::PAGE_MARGIN_X);
/// The widest outputs of `time_text::relative`, which size the time column.
const TIME_SAMPLES: [&str; 4] = ["2026-12-30", "59 min ago", "23 h ago", "29 d ago"];

pub(crate) enum CommitsAction {
    /// Show this range in the Files tab.
    Show(CompareRange),
    Retry,
}

pub(crate) fn show(ui: &mut Ui, session: &PullRequestSession) -> Option<CommitsAction> {
    let palette = theme::palette(ui.ctx());
    let opacity = motion::reveal(
        ui.ctx(),
        Id::unique(("commits", &session.key)),
        session.commits.ready().is_some(),
    );
    let commits = match (session.commits.ready(), session.commits.error()) {
        (Some(commits), _) => commits,
        (None, Some(message)) => return retry_bar(ui, message),
        (None, None) => {
            if let Some(message) = session.mirror.error().or(session.detail.error()) {
                return retry_bar(ui, message);
            }
            egui::Frame::new().inner_margin(PAGE_MARGIN).show(ui, |ui| {
                style::card_frame(&palette).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for seed in 0..SKELETON_ROWS {
                        let (rect, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), ROW_HEIGHT),
                            Sense::hover(),
                        );
                        skeleton::list_row(ui, rect, seed);
                    }
                });
            });
            return None;
        }
    };
    let mut picked = None;
    egui::Frame::new().inner_margin(PAGE_MARGIN).show(ui, |ui| {
        ui.multiply_opacity(opacity);
        if let CompareRange::Commit { sha, .. } = &session.range {
            ui.horizontal(|ui| {
                let note = format!("Files shows commit {}", short_sha(sha));
                let label = ui.label(
                    RichText::new(format!("{}  {note}", icons::INFO)).color(palette.text_secondary),
                );
                glyph::speak_as(&label, &note);
                if buttons::subtle(ui, icons::GIT_DIFF, "Show all changes").clicked() {
                    picked = Some(CommitsAction::Show(CompareRange::AllChanges));
                }
            });
            ui.add_space(style::SPACE_S);
        }
        if commits.is_empty() {
            states::message(
                ui,
                icons::GIT_COMMIT,
                "No commits",
                "The source branch has no commits beyond the target.",
            );
            return;
        }
        style::card_frame(&palette).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("commits").auto_shrink([false, true]).show_rows(
                ui,
                ROW_HEIGHT,
                commits.len(),
                |ui, range| {
                    let last = commits.len() - 1;
                    for (offset, commit) in commits[range.clone()].iter().enumerate() {
                        let is_last = range.start + offset == last;
                        if let Some(range) = commit_row(ui, session, commit, is_last) {
                            picked = Some(CommitsAction::Show(range));
                        }
                    }
                },
            );
        });
    });
    picked
}

fn retry_bar(ui: &mut Ui, message: &str) -> Option<CommitsAction> {
    let retry = [Action::secondary(icons::ARROW_CLOCKWISE, "Retry")];
    egui::Frame::new()
        .inner_margin(PAGE_MARGIN)
        .show(ui, |ui| states::error_with_actions(ui, message, &retry))
        .inner
        .map(|_| CommitsAction::Retry)
}

/// One commit; returns the range to show when clicked.
fn commit_row(
    ui: &mut Ui,
    session: &PullRequestSession,
    commit: &CommitInfo,
    is_last: bool,
) -> Option<CompareRange> {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &commit.subject));
    let is_current =
        matches!(&session.range, CompareRange::Commit { sha, .. } if *sha == commit.sha);
    row::list_background(ui, &response, rect.shrink(1.0), is_current);
    if !is_last {
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, palette.border));
    }
    let author = UserIdentity {
        id: commit.author_email.clone(),
        display_name: commit.author_name.clone(),
        login: None,
        avatar_url: None,
    };
    let painter = ui.painter_at(rect);
    let avatar_center = pos2(rect.left() + style::SPACE_L + AVATAR_SIZE / 2.0, rect.center().y);
    avatar::paint(&painter, avatar_center, AVATAR_SIZE, &author);

    // Right-hand columns have fixed widths, so SHAs and times line up from row to row.
    let sha_font = theme::fonts::code_font(12.0);
    let sha_column = elide::text_width(ui, &"0".repeat(SHORT_SHA_LEN), &sha_font) + 16.0;
    let chip = Rect::from_min_size(
        pos2(rect.right() - style::SPACE_L - sha_column, rect.center().y - 11.0),
        vec2(sha_column, 22.0),
    );
    painter.rect_filled(chip, style::RADIUS_CONTROL, palette.surface_subtle);
    painter.text(
        chip.center(),
        Align2::CENTER_CENTER,
        short_sha(&commit.sha),
        sha_font,
        palette.accent,
    );

    let small = egui::FontId::proportional(style::SMALL_SIZE);
    let time_column =
        TIME_SAMPLES.iter().map(|sample| elide::text_width(ui, sample, &small)).fold(0.0, f32::max);
    let time_right = chip.left() - style::SPACE_L;
    let time = time_text::relative(time_text::from_unix(commit.authored_at));
    painter.text(
        pos2(time_right, rect.center().y),
        Align2::RIGHT_CENTER,
        time,
        small.clone(),
        palette.text_secondary,
    );

    let text_left = avatar_center.x + AVATAR_SIZE / 2.0 + style::SPACE_M;
    let text_width = (time_right - time_column - style::SPACE_L - text_left).max(0.0);
    let subject = elide::line(
        &painter,
        &commit.subject,
        egui::FontId::proportional(style::BODY_SIZE),
        palette.text,
        text_width,
    );
    let author =
        elide::line(&painter, &commit.author_name, small, palette.text_secondary, text_width);
    let tooltip = hover_text(commit, subject.elided, author.elided);
    painter.galley(
        pos2(text_left, rect.center().y - 9.0 - subject.size().y / 2.0),
        subject,
        palette.text,
    );
    painter.galley(
        pos2(text_left, rect.center().y + 10.0 - author.size().y / 2.0),
        author,
        palette.text_secondary,
    );
    cv_widgets::focus::ring(ui, &response, rect);
    let response = response.on_hover_text(tooltip);
    let parent = commit.parents.first()?;
    response
        .clicked()
        .then(|| CompareRange::Commit { sha: commit.sha.clone(), parent: parent.clone() })
}

/// The row's tooltip: what a click does, after the full subject when the subject or the author is
/// cut, and the full author when that is.
fn hover_text(commit: &CommitInfo, is_subject_cut: bool, is_author_cut: bool) -> String {
    const HINT: &str = "Show this commit's changes";
    match (is_subject_cut, is_author_cut) {
        (false, false) => HINT.to_owned(),
        (true, false) => format!("{}\n{HINT}", commit.subject),
        (_, true) => format!("{}\n{}\n{HINT}", commit.subject, commit.author_name),
    }
}

#[cfg(test)]
mod tests {
    use review_git::CommitInfo;

    use super::hover_text;

    #[test]
    fn a_cut_row_tells_its_subject_and_author_in_full_on_hover() {
        let commit = CommitInfo {
            sha: "0123456789abcdef".to_owned(),
            parents: Vec::new(),
            author_name: "Ada Lovelace".to_owned(),
            author_email: "ada@example.com".to_owned(),
            authored_at: 0,
            subject: "Teach the engine to weave".to_owned(),
        };
        let cases = [
            ((false, false), "Show this commit's changes"),
            ((true, false), "Teach the engine to weave\nShow this commit's changes"),
            ((false, true), "Teach the engine to weave\nAda Lovelace\nShow this commit's changes"),
            ((true, true), "Teach the engine to weave\nAda Lovelace\nShow this commit's changes"),
        ];
        for ((is_subject_cut, is_author_cut), expected) in cases {
            assert_eq!(
                hover_text(&commit, is_subject_cut, is_author_cut),
                expected,
                "subject cut {is_subject_cut}, author cut {is_author_cut}"
            );
        }
    }
}
