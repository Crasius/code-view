//! GitHub `status` + `conclusion` pairs (workflow runs, jobs, steps, check runs) and commit status states.

use review_core::RunStatus;

/// `status` is `queued`, `requested`, `waiting`, `pending`, `in_progress` or `completed`;
/// `conclusion` only matters once completed.
pub(crate) fn from_check(status: Option<&str>, conclusion: Option<&str>) -> RunStatus {
    match status.unwrap_or("completed") {
        "queued" | "requested" | "waiting" | "pending" => RunStatus::Queued,
        "in_progress" => RunStatus::Running,
        _ => from_conclusion(conclusion),
    }
}

fn from_conclusion(conclusion: Option<&str>) -> RunStatus {
    match conclusion {
        Some("success") => RunStatus::Succeeded,
        Some("failure" | "timed_out" | "startup_failure") => RunStatus::Failed,
        Some("cancelled") => RunStatus::Canceled,
        Some("skipped") => RunStatus::Skipped,
        // neutral, action_required, stale, or a completed run without conclusion.
        _ => RunStatus::Neutral,
    }
}

/// Commit status `state`: error, failure, pending, success.
pub(crate) fn from_commit_state(state: &str) -> RunStatus {
    match state {
        "success" => RunStatus::Succeeded,
        "failure" | "error" => RunStatus::Failed,
        "pending" => RunStatus::Running,
        _ => RunStatus::Neutral,
    }
}

/// GraphQL `StatusState` of a `StatusCheckRollup`: SUCCESS, FAILURE, ERROR, PENDING, EXPECTED.
pub(crate) fn from_rollup_state(state: &str) -> RunStatus {
    match state {
        "SUCCESS" => RunStatus::Succeeded,
        "FAILURE" | "ERROR" => RunStatus::Failed,
        "PENDING" => RunStatus::Running,
        "EXPECTED" => RunStatus::Queued,
        _ => RunStatus::Neutral,
    }
}

#[cfg(test)]
mod tests {
    use super::{from_check, from_commit_state, from_rollup_state};
    use review_core::RunStatus::{Canceled, Failed, Neutral, Queued, Running, Skipped, Succeeded};

    #[test]
    fn pending_states_are_queued_or_running() {
        for status in ["queued", "requested", "waiting", "pending"] {
            assert_eq!(from_check(Some(status), None), Queued, "{status}");
        }
        assert_eq!(from_check(Some("in_progress"), None), Running);
    }

    #[test]
    fn conclusions_map_once_completed() {
        let cases = [
            ("success", Succeeded),
            ("failure", Failed),
            ("timed_out", Failed),
            ("startup_failure", Failed),
            ("cancelled", Canceled),
            ("skipped", Skipped),
            ("neutral", Neutral),
            ("action_required", Neutral),
            ("stale", Neutral),
        ];
        for (conclusion, expected) in cases {
            assert_eq!(from_check(Some("completed"), Some(conclusion)), expected, "{conclusion}");
        }
        assert_eq!(from_check(Some("completed"), None), Neutral);
        assert_eq!(from_check(None, Some("success")), Succeeded);
    }

    #[test]
    fn commit_states() {
        assert_eq!(from_commit_state("success"), Succeeded);
        assert_eq!(from_commit_state("failure"), Failed);
        assert_eq!(from_commit_state("error"), Failed);
        assert_eq!(from_commit_state("pending"), Running);
    }

    #[test]
    fn rollup_states_map_expected_to_queued_and_unknown_to_neutral() {
        let cases = [
            ("SUCCESS", Succeeded),
            ("FAILURE", Failed),
            ("ERROR", Failed),
            ("PENDING", Running),
            ("EXPECTED", Queued),
            ("SOMETHING_NEW", Neutral),
        ];
        for (state, expected) in cases {
            assert_eq!(from_rollup_state(state), expected, "{state}");
        }
    }
}
