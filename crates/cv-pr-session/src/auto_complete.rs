//! Auto-complete on the merge button: what a click runs ([`Mode`]), and how each platform names it
//! (Azure DevOps "Set auto-complete", GitHub "Enable auto-merge", GitLab "Set to auto-merge").
//!
//! While policies still block a pull request ([`is_waiting`]), the button sets auto-complete with
//! the chosen strategy and options instead of completing, unless the user turned that off in the
//! dropdown for this page (an admin who bypasses policies completes at once). Once auto-complete is
//! set, the button cancels it. A pull request that can complete now keeps the plain complete.

use review_core::{
    AutoComplete, MergeStatus, MergeStrategy, PipelineRun, ProviderKind, PullRequestDetail,
    ReviewVote,
};

use crate::merge_choice;

/// What the merge button's main part runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Complete the pull request now.
    Now,
    /// Let the platform complete it once its policies pass.
    SetAutoComplete,
    /// Auto-complete is set: turn it off.
    CancelAutoComplete,
}

/// The mode for `detail`; `runs` are the pull request's pipelines as far as loaded, `wants_auto`
/// the page's choice in the dropdown (`None`: not made, auto-complete while waiting).
pub fn mode(
    detail: &PullRequestDetail,
    runs: Option<&[PipelineRun]>,
    kind: ProviderKind,
    wants_auto: Option<bool>,
) -> Mode {
    if detail.auto_complete.is_some() {
        Mode::CancelAutoComplete
    } else if is_waiting(detail, runs, kind) && wants_auto.unwrap_or(true) {
        Mode::SetAutoComplete
    } else {
        Mode::Now
    }
}

/// Policies still block completing. GitHub and GitLab say so in the merge state they compute
/// (required checks and reviews included); Azure DevOps reports only git's merge there, so its
/// running pipelines and required reviewers without an approval count too.
pub fn is_waiting(
    detail: &PullRequestDetail,
    runs: Option<&[PipelineRun]>,
    kind: ProviderKind,
) -> bool {
    if detail.merge_status == MergeStatus::Blocked {
        return true;
    }
    if kind != ProviderKind::AzureDevOps || detail.merge_status == MergeStatus::Conflicts {
        return false;
    }
    let is_running =
        runs.unwrap_or_default().iter().any(|run| run.is_head && !run.status.is_finished());
    let lacks_approval = detail.summary.reviewers.iter().any(|reviewer| {
        reviewer.is_required
            && !matches!(reviewer.vote, ReviewVote::Approved | ReviewVote::ApprovedWithSuggestions)
    });
    is_running || lacks_approval
}

/// "Auto-complete" or "Auto-merge".
pub fn noun(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::AzureDevOps => "Auto-complete",
        ProviderKind::GitHub | ProviderKind::GitLab => "Auto-merge",
    }
}

/// The button that turns it on.
pub fn set_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::AzureDevOps => "Set auto-complete",
        ProviderKind::GitHub => "Enable auto-merge",
        ProviderKind::GitLab => "Set to auto-merge",
    }
}

/// The button that turns it off.
pub fn cancel_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::AzureDevOps => "Cancel auto-complete",
        ProviderKind::GitHub => "Disable auto-merge",
        ProviderKind::GitLab => "Cancel auto-merge",
    }
}

/// The armed button's label in `mode`, e.g. "Click again to set auto-complete".
pub fn confirm_label(mode: Mode, kind: ProviderKind) -> Option<String> {
    let label = match mode {
        Mode::Now => return None,
        Mode::SetAutoComplete => set_label(kind),
        Mode::CancelAutoComplete => cancel_label(kind),
    };
    Some(format!("Click again to {}", lower_first(label)))
}

/// The main part's label in `mode`, armed or not; `None` for [`Mode::Now`], whose label names the
/// strategy.
pub fn label(mode: Mode, kind: ProviderKind, is_armed: bool) -> Option<String> {
    match mode {
        Mode::Now => None,
        _ if is_armed => confirm_label(mode, kind),
        Mode::SetAutoComplete => Some(set_label(kind).to_owned()),
        Mode::CancelAutoComplete => Some(cancel_label(kind).to_owned()),
    }
}

/// The main part's hover in `mode`: what the auto-complete does or who set it.
pub fn hover(
    detail: &PullRequestDetail,
    mode: Mode,
    strategy: Option<MergeStrategy>,
    kind: ProviderKind,
) -> Option<String> {
    match (mode, &detail.auto_complete) {
        (Mode::CancelAutoComplete, Some(auto_complete)) => Some(format!(
            "{}. Click to {}.",
            state_text(auto_complete, kind),
            lower_first(cancel_label(kind))
        )),
        (Mode::SetAutoComplete, _) => {
            let with = strategy
                .map(|strategy| format!(" with {}", merge_choice::button_label(strategy, kind)))
                .unwrap_or_default();
            Some(format!("{}{with}.", waits_for(kind)))
        }
        _ => None,
    }
}

/// The dropdown's choice between auto-complete and completing at once.
pub fn choice_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::AzureDevOps => "Complete automatically when policies pass",
        ProviderKind::GitHub => "Merge automatically when requirements are met",
        ProviderKind::GitLab => "Merge automatically when checks pass",
    }
}

/// The toast once the write succeeded.
pub fn done_text(mode: Mode, kind: ProviderKind) -> &'static str {
    match (mode, kind) {
        (Mode::CancelAutoComplete, ProviderKind::AzureDevOps) => "Auto-complete cancelled.",
        (Mode::CancelAutoComplete, ProviderKind::GitHub) => "Auto-merge disabled.",
        (Mode::CancelAutoComplete, ProviderKind::GitLab) => "Auto-merge cancelled.",
        (_, ProviderKind::AzureDevOps) => "Auto-complete set.",
        (_, ProviderKind::GitHub) => "Auto-merge enabled.",
        (_, ProviderKind::GitLab) => "Set to auto-merge.",
    }
}

/// "Auto-complete set by Ada Lovelace: Squash merge". GitLab merges with the project's method
/// unless it squashes, so only a squash is named there.
pub fn state_text(auto_complete: &AutoComplete, kind: ProviderKind) -> String {
    let verb = match kind {
        ProviderKind::AzureDevOps => "set",
        ProviderKind::GitHub => "enabled",
        ProviderKind::GitLab => "turned on",
    };
    let by = auto_complete
        .set_by
        .as_ref()
        .map(|who| format!(" by {}", who.display_name))
        .unwrap_or_default();
    let with = match auto_complete.strategy {
        Some(strategy) => format!(": {}", merge_choice::button_label(strategy, kind)),
        None if kind == ProviderKind::GitLab => ": the project's merge method".to_owned(),
        None => String::new(),
    };
    format!("{} {verb}{by}{with}", noun(kind))
}

/// What completes once policies pass, as the line under the state says it.
pub fn waits_for(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::AzureDevOps => "Completes once the branch policies pass",
        ProviderKind::GitHub => "Merges once the required checks and reviews pass",
        ProviderKind::GitLab => "Merges once the pipeline and merge checks pass",
    }
}

/// The strategy the platform will show for an auto-complete set with `strategy`.
pub fn set_strategy(strategy: MergeStrategy, kind: ProviderKind) -> Option<MergeStrategy> {
    match kind {
        ProviderKind::GitLab => (strategy == MergeStrategy::Squash).then_some(strategy),
        ProviderKind::AzureDevOps | ProviderKind::GitHub => Some(strategy),
    }
}

/// Whether the dropdown offers deleting the source branch with auto-complete: GitHub's auto-merge
/// takes no such option (the repository's own setting decides).
pub fn offers_branch_deletion(mode: Mode, kind: ProviderKind) -> bool {
    !(mode == Mode::SetAutoComplete && kind == ProviderKind::GitHub)
}

fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_lowercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use review_core::{
        AutoComplete, DiffStat, MergeStatus, MergeStrategy, PipelineRun, ProviderKind,
        PullRequestDetail, PullRequestState, PullRequestSummary, ReviewVote, Reviewer, RunStatus,
        UserIdentity,
    };

    use super::{Mode, mode, state_text};

    fn person(name: &str) -> UserIdentity {
        UserIdentity { id: name.into(), display_name: name.into(), login: None, avatar_url: None }
    }

    fn detail(merge_status: MergeStatus) -> PullRequestDetail {
        PullRequestDetail {
            summary: PullRequestSummary {
                repository_key: "o/p/r".into(),
                repository_name: "r".into(),
                number: 7,
                title: "T".into(),
                author: person("author"),
                state: PullRequestState::Open,
                is_draft: false,
                source_branch: "feature".into(),
                target_branch: "main".into(),
                created_at: time::OffsetDateTime::UNIX_EPOCH,
                updated_at: None,
                web_url: String::new(),
                labels: Vec::new(),
                reviewers: Vec::new(),
                checks: None,
                diff_stat: DiffStat::default(),
                head_sha: None,
                target_sha: None,
            },
            description: String::new(),
            target_sha: "base".into(),
            head_sha: "head".into(),
            merge_status,
            git_fetch_refs: Vec::new(),
            closed_at: None,
            auto_complete: None,
        }
    }

    fn run(status: RunStatus, is_head: bool) -> PipelineRun {
        PipelineRun {
            id: "1".into(),
            name: "CI".into(),
            status,
            description: None,
            web_url: None,
            started_at: None,
            finished_at: None,
            has_jobs: true,
            is_head,
            reruns: Vec::new(),
        }
    }

    #[test]
    fn a_blocked_pull_request_sets_auto_complete_unless_the_page_chose_otherwise() {
        let kinds = [ProviderKind::AzureDevOps, ProviderKind::GitHub, ProviderKind::GitLab];
        for kind in kinds {
            let blocked = detail(MergeStatus::Blocked);
            assert_eq!(mode(&blocked, None, kind, None), Mode::SetAutoComplete, "{kind:?}");
            assert_eq!(mode(&blocked, None, kind, Some(false)), Mode::Now, "{kind:?}");
            let ready = detail(MergeStatus::Mergeable);
            assert_eq!(mode(&ready, None, kind, None), Mode::Now, "{kind:?}");
            let mut set = detail(MergeStatus::Mergeable);
            set.auto_complete = Some(AutoComplete { set_by: None, strategy: None });
            assert_eq!(mode(&set, None, kind, Some(false)), Mode::CancelAutoComplete);
        }
    }

    /// Azure DevOps' merge status says nothing of policies: running builds and missing required
    /// approvals wait too. Elsewhere the platform's own merge state decides.
    #[test]
    fn azure_devops_waits_for_running_builds_and_required_reviewers() {
        let ado = ProviderKind::AzureDevOps;
        let ready = detail(MergeStatus::Mergeable);
        let running = [run(RunStatus::Running, true)];
        assert_eq!(mode(&ready, Some(&running), ado, None), Mode::SetAutoComplete);
        assert_eq!(mode(&ready, Some(&running), ProviderKind::GitHub, None), Mode::Now);
        let older = [run(RunStatus::Running, false), run(RunStatus::Succeeded, true)];
        assert_eq!(mode(&ready, Some(&older), ado, None), Mode::Now, "an older push's run");
        let mut required = detail(MergeStatus::Mergeable);
        required.summary.reviewers.push(Reviewer {
            identity: person("lead"),
            vote: ReviewVote::None,
            is_required: true,
            is_group: false,
        });
        assert_eq!(mode(&required, None, ado, None), Mode::SetAutoComplete);
        required.summary.reviewers[0].vote = ReviewVote::Approved;
        assert_eq!(mode(&required, None, ado, None), Mode::Now);
        let conflicts = detail(MergeStatus::Conflicts);
        assert_eq!(mode(&conflicts, Some(&running), ado, None), Mode::Now, "conflicts first");
    }

    #[test]
    fn the_state_names_who_set_it_and_the_strategy_in_the_platforms_words() {
        let squash =
            AutoComplete { set_by: Some(person("Ada")), strategy: Some(MergeStrategy::Squash) };
        assert_eq!(
            state_text(&squash, ProviderKind::AzureDevOps),
            "Auto-complete set by Ada: Squash merge"
        );
        assert_eq!(
            state_text(&squash, ProviderKind::GitHub),
            "Auto-merge enabled by Ada: Squash and merge"
        );
        let method = AutoComplete { set_by: None, strategy: None };
        assert_eq!(
            state_text(&method, ProviderKind::GitLab),
            "Auto-merge turned on: the project's merge method"
        );
    }
}
