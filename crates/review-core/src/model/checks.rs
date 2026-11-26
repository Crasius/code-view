//! What a pull request list row shows beside its title: the checks and branch policies of the head
//! commit, and the reviewers' votes where the listing left them out.
//!
//! Listings carry little of it (only GitHub's search names a combined check state), so providers
//! report it per row through [`crate::Provider::row_statuses`], in as few requests as the platform
//! allows. A check is anything the platform gates or reports a pull request with: a CI run or one of
//! its stages, a commit status, an Azure DevOps branch policy. The approvals a platform requires
//! are no check; they come apart, in [`RowStatus::required_approvals`].

use super::{Reviewer, RunStatus};

/// The outcome of one check, in the four states a row tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckState {
    Passed,
    Failed,
    /// Queued, running or waiting for something (an approval, a manual start).
    Pending,
    /// Finished without pass or fail meaning: skipped, canceled, neutral, not applicable.
    Neutral,
}

impl From<RunStatus> for CheckState {
    fn from(status: RunStatus) -> Self {
        match status {
            RunStatus::Succeeded => CheckState::Passed,
            RunStatus::Failed => CheckState::Failed,
            RunStatus::Queued | RunStatus::Running => CheckState::Pending,
            RunStatus::Canceled | RunStatus::Skipped | RunStatus::Neutral => CheckState::Neutral,
        }
    }
}

/// One check or branch policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub state: CheckState,
    /// Whether merging waits for it; `None` where the platform does not say.
    pub is_required: Option<bool>,
    /// A short note the platform adds ("1 of 2 approvals", a status description).
    pub detail: Option<String>,
}

impl Check {
    pub fn new(name: impl Into<String>, state: CheckState) -> Self {
        Self { name: name.into(), state, is_required: None, detail: None }
    }

    #[must_use]
    pub fn required(mut self, is_required: Option<bool>) -> Self {
        self.is_required = is_required;
        self
    }

    #[must_use]
    pub fn with_detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail.filter(|detail| !detail.trim().is_empty());
        self
    }

    fn is_optional(&self) -> bool {
        self.is_required == Some(false)
    }
}

/// The checks and votes of one list row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RowStatus {
    /// Every check of the head commit, required ones first where the platform says.
    pub checks: Vec<Check>,
    /// Checks the platform has beyond `checks` but did not list (more than one page of them).
    pub unlisted: usize,
    /// The platform's own combined state, where it reports one (GitHub's check rollup); it decides
    /// the verdict while `unlisted` checks are unknown.
    pub reported: Option<CheckState>,
    /// The reviewers with their votes, where the listing had none (GitLab, GitHub's per-repository
    /// listing); `None` keeps the listing's.
    pub reviewers: Option<Vec<Reviewer>>,
    /// The platform's verdict on the approvals it requires, named with a detail ("1 of 2
    /// approvals"): GitHub's review decision, GitLab's approval rules, Azure DevOps' reviewer
    /// policies. Passed when they are in, pending while some are missing, failed when a reviewer
    /// blocks; `None` where no rule asks for approvals or the platform does not say
    /// ([`super::is_approved`] then goes by the votes). A review is no check: it stays out of
    /// `checks`, the verdict and "passed/all".
    pub required_approvals: Option<Check>,
}

impl RowStatus {
    /// The row's one state: failed when a check merging waits for failed, else pending while one
    /// runs, else passed when one passed; neutral when every check finished without meaning.
    /// Optional checks only decide when every check is optional. `None` without any check.
    pub fn verdict(&self) -> Option<CheckState> {
        if self.unlisted > 0
            && let Some(reported) = self.reported
        {
            return Some(reported);
        }
        if self.checks.is_empty() {
            return self.reported;
        }
        let deciding: Vec<&Check> = if self.checks.iter().all(Check::is_optional) {
            self.checks.iter().collect()
        } else {
            self.checks.iter().filter(|check| !check.is_optional()).collect()
        };
        let has = |state: CheckState| deciding.iter().any(|check| check.state == state);
        let verdict = [CheckState::Failed, CheckState::Pending, CheckState::Passed]
            .into_iter()
            .find(|state| has(*state))
            .unwrap_or(CheckState::Neutral);
        Some(verdict)
    }

    /// Passed checks and the checks that count, listed or not. Neutral checks (skipped, canceled)
    /// finished without a result, so they count neither way: four skipped of ten leave "5/6", and
    /// "6/6" once the rest pass.
    pub fn passed_of(&self) -> (usize, usize) {
        let passed = self.checks.iter().filter(|check| check.state == CheckState::Passed).count();
        (passed, self.checks.len() + self.unlisted - self.neutral())
    }

    /// Listed checks that finished without a result: skipped, canceled, neutral.
    pub fn neutral(&self) -> usize {
        self.checks.iter().filter(|check| check.state == CheckState::Neutral).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(checks: Vec<Check>) -> RowStatus {
        RowStatus { checks, ..RowStatus::default() }
    }

    #[test]
    fn a_failed_required_check_wins_and_optional_ones_only_decide_alone() {
        use CheckState::{Failed, Neutral, Passed, Pending};
        let required = |state| Check::new("build", state).required(Some(true));
        let optional = |state| Check::new("lint", state).required(Some(false));
        let unknown = |state| Check::new("status", state);
        let cases: [(Vec<Check>, Option<CheckState>); 7] = [
            (vec![required(Passed), unknown(Failed)], Some(Failed)),
            (vec![required(Passed), optional(Failed)], Some(Passed)),
            (vec![required(Pending), optional(Failed)], Some(Pending)),
            (vec![optional(Failed), optional(Passed)], Some(Failed)),
            (vec![unknown(Neutral), unknown(Passed)], Some(Passed)),
            (vec![unknown(Neutral)], Some(Neutral)),
            (vec![], None),
        ];
        for (checks, expected) in cases {
            assert_eq!(status(checks.clone()).verdict(), expected, "{checks:?}");
        }
    }

    #[test]
    fn unlisted_checks_leave_the_verdict_to_the_platform() {
        let mut row = status(vec![Check::new("build", CheckState::Passed)]);
        row.unlisted = 60;
        row.reported = Some(CheckState::Failed);
        assert_eq!(row.verdict(), Some(CheckState::Failed));
        assert_eq!(row.passed_of(), (1, 61));
        let reported_only =
            RowStatus { reported: Some(CheckState::Pending), ..RowStatus::default() };
        assert_eq!(reported_only.verdict(), Some(CheckState::Pending));
    }

    #[test]
    fn skipped_checks_count_neither_as_passed_nor_as_open() {
        use CheckState::{Neutral, Passed, Pending};
        let mut checks = vec![Check::new("review", Pending)];
        checks.extend((0..5).map(|_| Check::new("build", Passed)));
        checks.extend((0..4).map(|_| Check::new("deploy", Neutral)));
        let mut row = status(checks);
        assert_eq!((row.passed_of(), row.neutral()), ((5, 6), 4));
        row.checks[0].state = Passed;
        assert_eq!(row.passed_of(), (6, 6));
        assert_eq!(status(vec![Check::new("deploy", Neutral)]).passed_of(), (0, 0));
    }
}
