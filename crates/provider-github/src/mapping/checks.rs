//! A pull request's check rollup and review decision as a list row's [`RowStatus`].

use review_core::{Check, CheckState, RowStatus};

use crate::dto::search::{ContextDto, StatusCheckRollupDto};
use crate::mapping::run_status;

/// The check runs and commit statuses of the head commit's rollup, required ones first where the
/// query asked which are required, and the review the branch protection asks for
/// (`reviewDecision`) as the row's required approvals: a review is no check, so it never counts in
/// "passed/all".
pub(crate) fn row_status(
    rollup: Option<StatusCheckRollupDto>,
    review_decision: Option<&str>,
) -> RowStatus {
    let reported =
        rollup.as_ref().map(|rollup| run_status::from_rollup_state(&rollup.state).into());
    let contexts = rollup.and_then(|rollup| rollup.contexts);
    let total = contexts.as_ref().map_or(0, |contexts| contexts.total_count);
    let mut checks: Vec<Check> = contexts
        .into_iter()
        .flat_map(|contexts| contexts.nodes)
        .flatten()
        .filter_map(check)
        .collect();
    let unlisted = total.saturating_sub(checks.len());
    checks.sort_by_key(|check| check.is_required != Some(true));
    let required_approvals = review_decision.and_then(review_check);
    RowStatus { checks, unlisted, reported, reviewers: None, required_approvals }
}

fn check(context: ContextDto) -> Option<Check> {
    match context {
        ContextDto::CheckRun { name, status, conclusion, is_required } => {
            let status = status.map(|status| status.to_ascii_lowercase());
            let conclusion = conclusion.map(|conclusion| conclusion.to_ascii_lowercase());
            let run = run_status::from_check(status.as_deref(), conclusion.as_deref());
            Some(Check::new(name, run.into()).required(is_required))
        }
        ContextDto::StatusContext { context, state, description, is_required } => {
            let run = run_status::from_commit_state(&state.to_ascii_lowercase());
            Some(Check::new(context, run.into()).required(is_required).with_detail(description))
        }
        ContextDto::Other => None,
    }
}

/// The review the branch protection requires, as GitHub decided it.
fn review_check(decision: &str) -> Option<Check> {
    let (state, detail) = match decision {
        "APPROVED" => (CheckState::Passed, "Approved"),
        "CHANGES_REQUESTED" => (CheckState::Failed, "Changes requested"),
        "REVIEW_REQUIRED" => (CheckState::Pending, "Waiting for an approving review"),
        _ => return None,
    };
    Some(
        Check::new("Required review", state)
            .required(Some(true))
            .with_detail(Some(detail.to_owned())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::fixtures::parse;

    #[test]
    fn maps_check_runs_statuses_and_the_review_decision() {
        let rollup: StatusCheckRollupDto = parse(
            r#"{"state": "FAILURE", "contexts": {"totalCount": 4, "nodes": [
                {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "FAILURE", "isRequired": false},
                {"__typename": "CheckRun", "name": "build", "status": "IN_PROGRESS", "conclusion": null, "isRequired": true},
                {"__typename": "StatusContext", "context": "ci/deploy", "state": "SUCCESS", "description": "Preview ready"},
                null
            ]}}"#,
        );
        let status = row_status(Some(rollup), Some("REVIEW_REQUIRED"));
        let checks: Vec<(&str, CheckState, Option<bool>)> = status
            .checks
            .iter()
            .map(|check| (check.name.as_str(), check.state, check.is_required))
            .collect();
        assert_eq!(
            checks,
            [
                ("build", CheckState::Pending, Some(true)),
                ("lint", CheckState::Failed, Some(false)),
                ("ci/deploy", CheckState::Passed, None),
            ],
            "required checks first; the review is no check"
        );
        let review = status.required_approvals.as_ref().expect("the review decision");
        assert_eq!(
            (review.state, review.detail.as_deref()),
            (CheckState::Pending, Some("Waiting for an approving review"))
        );
        assert_eq!(status.checks[2].detail.as_deref(), Some("Preview ready"));
        assert_eq!(status.unlisted, 1, "four in total, three listed");
        assert_eq!(status.reported, Some(CheckState::Failed));
        assert_eq!(
            status.verdict(),
            Some(CheckState::Failed),
            "GitHub's rollup decides with a check unlisted"
        );
    }

    #[test]
    fn no_rollup_and_no_required_review_is_no_check() {
        let status = row_status(None, None);
        assert_eq!(status.checks, []);
        assert_eq!(status.verdict(), None);
        let approved = row_status(None, Some("APPROVED"));
        assert_eq!(approved.verdict(), None, "an approval is no check");
        assert_eq!(
            approved.required_approvals.map(|review| review.state),
            Some(CheckState::Passed)
        );
    }
}
