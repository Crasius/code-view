//! Commits tab: the pull request's commits, newest first. A click opens that commit's own changes.

use egui::{Align2, Id, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use review_core::UserIdentity;
use review_git::CommitInfo;

use cv_base::theme::{self, icons, motion, style};
use cv_pr_session::session::{CompareRange, PullRequestSession};
use cv_widgets::states::{self, Action};
use cv_widgets::{avatar, buttons, glyph, row, skeleton, time_text};

const ROW_HEIGHT: f32 = 56.0;
const SKELETON_ROWS: usize = 8;
const AVATAR_SIZE: f32 = 28.0;
/// Length of `CommitInfo::short_sha`.
const SHA_CHARS: usize = 8;
/// The widest outputs of `time_text::relative`, which size the time column.
const TIME_SAMPLES: [&str; 4] = ["2026-12-30", "59 min ago", "23 h ago", "29 d ago"];

pub enum CommitsAction {
    /// Show this range in the Files tab.
    Show(CompareRange),
    Retry,
}

pub fn show(ui: &mut Ui, session: &PullRequestSession) -> Option<CommitsAction> {
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
            egui::Frame::new().inner_margin(egui::Margin::same(24)).show(ui, |ui| {
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
    egui::Frame::new().inner_margin(egui::Margin::same(24)).show(ui, |ui| {
        ui.multiply_opacity(opacity);
        if let CompareRange::Commit { sha, .. } = &session.range {
            ui.horizontal(|ui| {
                let short = &sha[..sha.len().min(8)];
                let note = format!("Files shows commit {short}");
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
        .inner_margin(egui::Margin::same(24))
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
    let sha_column =
        painter.layout_no_wrap("0".repeat(SHA_CHARS), sha_font.clone(), palette.accent).size().x
            + 16.0;
    let chip = Rect::from_min_size(
        pos2(rect.right() - style::SPACE_L - sha_column, rect.center().y - 11.0),
        vec2(sha_column, 22.0),
    );
    painter.rect_filled(chip, style::RADIUS_CONTROL, palette.surface_subtle);
    painter.text(
        chip.center(),
        Align2::CENTER_CENTER,
        commit.short_sha(),
        sha_font,
        palette.accent,
    );

    let small = egui::FontId::proportional(style::SMALL_SIZE);
    let time_column = TIME_SAMPLES
        .iter()
        .map(|sample| {
            painter
                .layout_no_wrap((*sample).to_owned(), small.clone(), palette.text_secondary)
                .size()
                .x
        })
        .fold(0.0, f32::max);
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
    let subject = truncated(
        ui,
        &commit.subject,
        egui::FontId::proportional(style::BODY_SIZE),
        palette.text,
        text_width,
    );
    painter.galley(
        pos2(text_left, rect.center().y - 9.0 - subject.size().y / 2.0),
        subject,
        palette.text,
    );
    let author = truncated(ui, &commit.author_name, small, palette.text_secondary, text_width);
    painter.galley(
        pos2(text_left, rect.center().y + 10.0 - author.size().y / 2.0),
        author,
        palette.text_secondary,
    );
    cv_widgets::focus::ring(ui, &response, rect);
    let response = response.on_hover_text("Show this commit's changes");
    let parent = commit.parents.first()?;
    response
        .clicked()
        .then(|| CompareRange::Commit { sha: commit.sha.clone(), parent: parent.clone() })
}

/// One line of text, cut with an ellipsis at `width`.
fn truncated(
    ui: &Ui,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}
