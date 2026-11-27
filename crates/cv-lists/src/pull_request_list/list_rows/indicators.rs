//! A row's checks and approvals indicators.
//!
//! Checks: one icon for the row's verdict, each state its own shape (check, cross, open ring,
//! dash, warning), so colour is never the only signal; wide cards add "passed/counted", where
//! skipped and neutral checks count neither way ([`review_core::RowStatus::passed_of`]). Its
//! tooltip and accessible name list every check with its state, required or optional where the
//! platform says.
//! A click (or Enter) opens the checks on the platform, also when they could not be loaded.
//!
//! Approvals: up to [`MAX_AVATARS`] reviewers, blocking votes first (changes requested, waiting
//! for the author), then approvals, then those who have not voted, the rest in a "+n" circle. A
//! vote rings its avatar in the vote's color and adds a badge of its own shape
//! ([`avatar::vote_mark`]), so it reads at list density without hover. Reviewers without a vote
//! stay muted and unringed, so a row without approvals is quiet. Its tooltip and accessible name
//! list the approvals the platform requires and every reviewer with their vote; a click opens the
//! pull request.

use cv_base::theme::{Palette, icons, style};
use cv_widgets::{avatar, focus};
use egui::{Align2, Color32, FontId, Id, Painter, Pos2, Rect, Sense, Ui, pos2};
use review_core::{Check, CheckState, ProviderKind, ReviewVote, Reviewer};

use super::super::statuses::ChecksShown;

/// Reviewer avatars shown at most; more fold into a "+n" circle.
pub(super) const MAX_AVATARS: usize = 3;
pub(super) const REVIEWER_SIZE: f32 = 24.0;
pub(super) const REVIEWER_STEP: f32 = 22.0;
/// A reviewer's avatar with its vote ring, cut out of the row's fill.
const REVIEWER_REACH: f32 = REVIEWER_SIZE / 2.0 + avatar::RING_REACH;
/// The last slot's center from the column's right edge: room for a vote badge there.
const RIGHT_INSET: f32 = avatar::badge_reach(REVIEWER_SIZE);
/// Room for [`MAX_AVATARS`] avatars with their rings and badges, and the "+n" circle.
pub(super) const APPROVALS_WIDTH: f32 =
    REVIEWER_REACH + REVIEWER_STEP * MAX_AVATARS as f32 + RIGHT_INSET;
/// The checks column with its "passed/all" count, and icon only on narrow cards.
pub(super) const CHECKS_WIDE: f32 = 56.0;
pub(super) const CHECKS_NARROW: f32 = 24.0;
const CHECKS_ICON: f32 = 16.0;
/// Opacity of reviewers who have not voted yet.
const MUTED: f32 = 0.45;

/// Where one row's indicators go and what they name.
pub(crate) struct Indicators<'a> {
    pub id: Id,
    /// The row's middle.
    pub y: f32,
    /// The pull request as its platform writes it ("#7", "!7").
    pub number: &'a str,
    pub kind: ProviderKind,
    pub web_url: &'a str,
}

/// The checks indicator in `(left, right)`; with `with_count` it adds "passed/all". A click opens
/// the checks on the platform.
pub(crate) fn paint_checks(
    ui: &Ui,
    painter: &Painter,
    palette: &Palette,
    (left, right): (f32, f32),
    row: &Indicators<'_>,
    shown: ChecksShown<'_>,
    with_count: bool,
) {
    let (icon, color) = checks_icon(shown, palette);
    let count = match shown {
        ChecksShown::Known(status) if with_count => {
            let (passed, total) = status.passed_of();
            (total > 0).then(|| format!("{passed}/{total}"))
        }
        _ => None,
    };
    let icon_x =
        if count.is_some() { left + CHECKS_ICON / 2.0 } else { f32::midpoint(left, right) };
    painter.text(
        pos2(icon_x, row.y),
        Align2::CENTER_CENTER,
        icon,
        FontId::proportional(CHECKS_ICON),
        color,
    );
    if let Some(count) = count {
        painter.text(
            pos2(left + CHECKS_ICON + style::SPACE_XS, row.y),
            Align2::LEFT_CENTER,
            count,
            FontId::proportional(style::SMALL_SIZE),
            palette.text_secondary,
        );
    }
    let hit = Rect::from_x_y_ranges(left..=right, (row.y - 14.0)..=(row.y + 14.0));
    let response = ui.interact(hit, row.id.with("checks"), Sense::click());
    let text = || checks_text(row.number, shown, row.kind);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, text()));
    focus::ring(ui, &response, hit);
    let response = response.on_hover_ui(|ui| {
        ui.label(text());
    });
    if response.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(checks_url(row.kind, row.web_url)));
    }
}

fn checks_icon(shown: ChecksShown<'_>, palette: &Palette) -> (&'static str, Color32) {
    let verdict = match shown {
        ChecksShown::Loading => return (icons::CIRCLE_DASHED, palette.text_disabled),
        ChecksShown::Unknown(_) => return (icons::WARNING_CIRCLE, palette.text_secondary),
        ChecksShown::Rollup(state) => Some(state),
        ChecksShown::Known(status) => status.verdict(),
    };
    match verdict {
        None => (icons::MINUS, palette.text_disabled),
        Some(CheckState::Passed) => (icons::CHECK_CIRCLE, palette.success),
        Some(CheckState::Failed) => (icons::X_CIRCLE, palette.error),
        Some(CheckState::Pending) => (icons::CIRCLE_NOTCH, palette.warning),
        Some(CheckState::Neutral) => (icons::MINUS_CIRCLE, palette.text_secondary),
    }
}

/// Where a pull request's checks are on its platform: GitHub's Checks tab, GitLab's pipelines of
/// the merge request, Azure DevOps' overview (it lists the policies).
fn checks_url(kind: ProviderKind, web_url: &str) -> String {
    let base = web_url.trim_end_matches('/');
    match kind {
        ProviderKind::GitHub => format!("{base}/checks"),
        ProviderKind::GitLab => format!("{base}/pipelines"),
        ProviderKind::AzureDevOps => base.to_owned(),
    }
}

/// The checks indicator's tooltip and accessible name: the verdict, every check, and the way on.
pub(super) fn checks_text(number: &str, shown: ChecksShown<'_>, kind: ProviderKind) -> String {
    let platform = kind.label();
    let mut lines = match shown {
        ChecksShown::Loading => vec![format!("Checks of {number}: loading…")],
        ChecksShown::Unknown(reason) => {
            vec![format!("Checks of {number}: unknown"), reason.to_owned()]
        }
        ChecksShown::Rollup(state) => {
            vec![
                format!("Checks of {number}: {}", state_word(state)),
                "Loading each check…".to_owned(),
            ]
        }
        ChecksShown::Known(status) if status.checks.is_empty() && status.unlisted == 0 => {
            vec![format!("Checks of {number}: none configured")]
        }
        ChecksShown::Known(status) => {
            let (passed, total) = status.passed_of();
            let verdict = status.verdict().map_or("none", state_word);
            let neutral = status.neutral();
            let summary = [
                Some(format!("Checks of {number}: {verdict}")),
                (total > 0).then(|| format!("{passed} of {total} passed")),
                (neutral > 0).then(|| format!("{neutral} skipped or neutral")),
            ];
            let mut lines = vec![summary.into_iter().flatten().collect::<Vec<_>>().join(", ")];
            lines.extend(status.checks.iter().map(check_line));
            if status.unlisted > 0 {
                lines.push(format!("and {} more", status.unlisted));
            }
            lines
        }
    };
    lines.push(format!("Click to open the checks on {platform}"));
    lines.join("\n")
}

fn check_line(check: &Check) -> String {
    let mut line = format!("{}: {}", check.name, state_word(check.state));
    match check.is_required {
        Some(true) => line.push_str(" (required)"),
        Some(false) => line.push_str(" (optional)"),
        None => {}
    }
    if let Some(detail) = &check.detail {
        line.push_str(" · ");
        line.push_str(detail);
    }
    line
}

fn state_word(state: CheckState) -> &'static str {
    match state {
        CheckState::Passed => "passed",
        CheckState::Failed => "failed",
        CheckState::Pending => "running",
        CheckState::Neutral => "neutral",
    }
}

/// A row's reviewers with their votes, and the approvals its platform requires.
#[derive(Clone, Copy)]
pub(crate) struct Votes<'a> {
    pub reviewers: &'a [Reviewer],
    pub required: Option<&'a Check>,
}

/// The approvals indicator, right-aligned at `right` so the stacks end in one column. `ring`
/// separates overlapping avatars in the row's own fill. True when clicked.
pub(crate) fn paint_approvals(
    ui: &Ui,
    painter: &Painter,
    palette: &Palette,
    (left, right): (f32, f32),
    row: &Indicators<'_>,
    votes: Votes<'_>,
    ring: Color32,
) -> bool {
    let reviewers = votes.reviewers;
    if reviewers.is_empty() {
        return false;
    }
    let ordered = in_vote_order(reviewers);
    let (shown, folded) = stack(ordered.len());
    let slots = shown + usize::from(folded > 0);
    // Slot 0 is the leftmost; the last slot ends at the column's right edge.
    let center_at = |slot: usize| pos2(slot_x(right, slots - 1 - slot), row.y);
    for (slot, reviewer) in ordered.iter().take(shown).enumerate() {
        paint_reviewer(painter, palette, ring, center_at(slot), reviewer);
    }
    if folded > 0 {
        let center = center_at(shown);
        painter.circle_filled(center, REVIEWER_REACH, ring);
        painter.circle_filled(center, REVIEWER_SIZE / 2.0, palette.surface_subtle);
        painter.text(
            center,
            Align2::CENTER_CENTER,
            format!("+{folded}"),
            FontId::proportional(style::SMALL_SIZE - 1.0),
            palette.text_secondary,
        );
    }
    let stack_left = (center_at(0).x - REVIEWER_REACH).max(left);
    let hit = Rect::from_x_y_ranges(stack_left..=right, (row.y - 14.0)..=(row.y + 14.0));
    let response = ui.interact(hit, row.id.with("approvals"), Sense::click());
    let text = || approvals_text(row.number, votes, row.kind);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, text()));
    focus::ring(ui, &response, hit);
    response
        .on_hover_ui(|ui| {
            ui.label(text());
        })
        .clicked()
}

/// The center of the stack's slot `from_right` (0 is the rightmost) in a column ending at `right`.
pub(super) fn slot_x(right: f32, from_right: usize) -> f32 {
    right - RIGHT_INSET - REVIEWER_STEP * from_right as f32
}

/// Of `count` reviewers: how many show as avatars, and how many fold into the "+n" circle.
fn stack(count: usize) -> (usize, usize) {
    let shown = count.min(MAX_AVATARS);
    (shown, count - shown)
}

fn paint_reviewer(
    painter: &Painter,
    palette: &Palette,
    ring: Color32,
    center: Pos2,
    reviewer: &Reviewer,
) {
    painter.circle_filled(center, REVIEWER_REACH, ring);
    let (identity, vote) = (&reviewer.identity, reviewer.vote);
    if vote == ReviewVote::None {
        let mut muted = painter.clone();
        muted.multiply_opacity(MUTED);
        avatar::paint(&muted, center, REVIEWER_SIZE, identity);
        return;
    }
    avatar::paint_voted(painter, palette, ring, center, REVIEWER_SIZE, identity, vote);
}

/// Blocking votes first, then approvals, then reviewers without a vote (people before groups).
fn in_vote_order(reviewers: &[Reviewer]) -> Vec<&Reviewer> {
    let rank = |reviewer: &Reviewer| match reviewer.vote {
        ReviewVote::ChangesRequested => 0,
        ReviewVote::WaitingForAuthor => 1,
        ReviewVote::Approved => 2,
        ReviewVote::ApprovedWithSuggestions => 3,
        ReviewVote::None if !reviewer.is_group => 4,
        ReviewVote::None => 5,
    };
    let mut ordered: Vec<&Reviewer> = reviewers.iter().collect();
    ordered.sort_by_key(|reviewer| rank(reviewer));
    ordered
}

/// The approvals indicator's tooltip and accessible name: a summary of the votes, the approvals
/// the platform requires, then every reviewer with theirs.
pub(super) fn approvals_text(number: &str, votes: Votes<'_>, kind: ProviderKind) -> String {
    let ordered = in_vote_order(votes.reviewers);
    let cast = [
        ReviewVote::ChangesRequested,
        ReviewVote::WaitingForAuthor,
        ReviewVote::Approved,
        ReviewVote::ApprovedWithSuggestions,
    ];
    let counts: Vec<String> = cast
        .iter()
        .filter_map(|vote| {
            let count = ordered.iter().filter(|reviewer| reviewer.vote == *vote).count();
            (count > 0).then(|| format!("{count} {}", vote_word(*vote, kind).to_lowercase()))
        })
        .collect();
    let has_approval = ordered.iter().any(|reviewer| {
        matches!(reviewer.vote, ReviewVote::Approved | ReviewVote::ApprovedWithSuggestions)
    });
    let mut summary = if counts.is_empty() { "no votes yet".to_owned() } else { counts.join(", ") };
    if !has_approval {
        summary.push_str("; no approvals");
    }
    let mut lines = vec![format!("Approvals of {number}: {summary}")];
    lines.extend(votes.required.map(required_line));
    lines.extend(ordered.iter().map(|reviewer| {
        let mut line =
            format!("{}: {}", reviewer.identity.display_name, vote_word(reviewer.vote, kind));
        if reviewer.is_required {
            line.push_str(" (required)");
        }
        line
    }));
    lines.join("\n")
}

/// The approvals the platform requires, in one line: "Required review: missing · Waiting for an
/// approving review".
pub(super) fn required_line(required: &Check) -> String {
    let state = match required.state {
        CheckState::Passed => "met",
        CheckState::Failed => "blocked",
        CheckState::Pending | CheckState::Neutral => "missing",
    };
    match &required.detail {
        Some(detail) => format!("{}: {state} · {detail}", required.name),
        None => format!("{}: {state}", required.name),
    }
}

/// A vote in its platform's words: Azure DevOps rejects where the others request changes.
fn vote_word(vote: ReviewVote, kind: ProviderKind) -> &'static str {
    match vote {
        ReviewVote::None => "No vote yet",
        ReviewVote::Approved => "Approved",
        ReviewVote::ApprovedWithSuggestions => "Approved with suggestions",
        ReviewVote::WaitingForAuthor => "Waiting for author",
        ReviewVote::ChangesRequested if kind == ProviderKind::AzureDevOps => "Rejected",
        ReviewVote::ChangesRequested => "Changes requested",
    }
}

#[cfg(test)]
mod tests {
    use review_core::{RowStatus, UserIdentity};

    use super::*;

    fn reviewer(name: &str, vote: ReviewVote) -> Reviewer {
        Reviewer {
            identity: UserIdentity {
                id: name.to_owned(),
                display_name: name.to_owned(),
                ..UserIdentity::default()
            },
            vote,
            is_required: name == "Cy",
            is_group: false,
        }
    }

    #[test]
    fn checks_text_lists_each_check_with_its_requirement() {
        let status = RowStatus {
            checks: vec![
                Check::new("Build", CheckState::Passed).required(Some(true)),
                Check::new("Minimum number of reviewers", CheckState::Pending)
                    .required(Some(true))
                    .with_detail(Some("2 approvals required".to_owned())),
                Check::new("lint", CheckState::Failed).required(Some(false)),
                Check::new("Deploy dev", CheckState::Neutral),
            ],
            ..RowStatus::default()
        };
        let text = checks_text("!7", ChecksShown::Known(&status), ProviderKind::AzureDevOps);
        assert_eq!(
            text,
            "Checks of !7: running, 1 of 3 passed, 1 skipped or neutral\nBuild: passed (required)\n\
             Minimum number of reviewers: running (required) · 2 approvals required\n\
             lint: failed (optional)\nDeploy dev: neutral\nClick to open the checks on Azure DevOps"
        );
        let skipped = RowStatus {
            checks: vec![Check::new("Deploy dev", CheckState::Neutral)],
            ..RowStatus::default()
        };
        let skipped = checks_text("#5", ChecksShown::Known(&skipped), ProviderKind::GitHub);
        assert!(skipped.starts_with("Checks of #5: neutral, 1 skipped or neutral\n"), "{skipped}");
        let none =
            checks_text("#3", ChecksShown::Known(&RowStatus::default()), ProviderKind::GitHub);
        assert!(none.starts_with("Checks of #3: none configured\n"), "{none}");
        let denied = checks_text("!4", ChecksShown::Unknown("403 Forbidden"), ProviderKind::GitLab);
        assert_eq!(
            denied,
            "Checks of !4: unknown\n403 Forbidden\nClick to open the checks on GitLab"
        );
    }

    #[test]
    fn checks_open_on_each_platforms_checks_page() {
        assert_eq!(
            checks_url(ProviderKind::GitHub, "https://h/a/b/pull/3/"),
            "https://h/a/b/pull/3/checks"
        );
        assert_eq!(
            checks_url(ProviderKind::GitLab, "https://h/g/p/-/merge_requests/4"),
            "https://h/g/p/-/merge_requests/4/pipelines"
        );
    }

    #[test]
    fn approvals_text_leads_with_blocking_votes_and_says_when_nobody_approved() {
        let reviewers = [
            reviewer("Ann", ReviewVote::Approved),
            reviewer("Dee", ReviewVote::None),
            reviewer("Cy", ReviewVote::ChangesRequested),
            reviewer("Bo", ReviewVote::ApprovedWithSuggestions),
        ];
        let votes = |reviewers| Votes { reviewers, required: None };
        assert_eq!(
            approvals_text("!7", votes(&reviewers), ProviderKind::AzureDevOps),
            "Approvals of !7: 1 rejected, 1 approved, 1 approved with suggestions\n\
             Cy: Rejected (required)\nAnn: Approved\nBo: Approved with suggestions\nDee: No vote yet"
        );
        let waiting =
            [reviewer("Eve", ReviewVote::WaitingForAuthor), reviewer("Fay", ReviewVote::None)];
        assert_eq!(
            approvals_text("#2", votes(&waiting), ProviderKind::GitHub),
            "Approvals of #2: 1 waiting for author; no approvals\nEve: Waiting for author\nFay: No vote yet"
        );
        let rule = Check::new("Approvals", CheckState::Pending)
            .with_detail(Some("1 of 2 approvals".to_owned()));
        let one = [reviewer("Ann", ReviewVote::Approved)];
        assert_eq!(
            approvals_text(
                "!3",
                Votes { reviewers: &one, required: Some(&rule) },
                ProviderKind::GitLab
            ),
            "Approvals of !3: 1 approved\nApprovals: missing · 1 of 2 approvals\nAnn: Approved",
            "the rule the platform requires follows the summary"
        );
    }
}
