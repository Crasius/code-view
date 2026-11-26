//! The branch policies of list rows ([`review_core::Provider::row_statuses`]).
//!
//! The pull request listing carries the reviewers with their votes but no checks. Each pull
//! request's policy evaluations are one request
//! (<https://learn.microsoft.com/en-us/rest/api/azure/devops/policy/evaluations/list?view=azure-devops-rest-7.1>,
//! `7.1-preview.1`, read with "Code (read)"): build validation, linked work items, comment
//! resolution and status policies, each with whether it blocks completion, are the checks; the
//! reviewer policies are the row's required approvals, no check. The
//! evaluations name the pull request by its project's GUID (`artifactId`), which the listing's
//! repository key does not hold: one repository lookup per project finds it, then it is kept.
//!
//! The documentation leaves an evaluation's `context` undescribed; a build policy's definition name
//! is read from it where present, else the policy's own display name or type stands in.
//! Reading policies needs sign-in even on public projects, so anonymous connections ask nothing.

use std::collections::HashMap;

use reqwest::Method;
use review_core::{Check, CheckState, Result, RowStatus};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::client::{Client, RepoKey};
use crate::dto::ValueList;
use crate::dto::repository::Repository;

const EVALUATIONS_API_VERSION: &str = "7.1-preview.1";

/// Project GUIDs by `organization/project`.
#[derive(Default)]
pub(crate) struct ProjectIds {
    by_project: Mutex<HashMap<String, String>>,
}

impl ProjectIds {
    async fn get(&self, client: &Client, key: RepoKey<'_>) -> Result<String> {
        let name = format!("{}/{}", key.organization, key.project);
        let mut cache = self.by_project.lock().await;
        if let Some(id) = cache.get(&name) {
            return Ok(id.clone());
        }
        let repository: Repository = client.get(&client.repository_resource_api(key), &[]).await?;
        cache.insert(name, repository.project.id.clone());
        Ok(repository.project.id)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Evaluation {
    /// `queued`, `running`, `approved`, `rejected`, `notApplicable`, `broken`.
    #[serde(default)]
    status: String,
    #[serde(default)]
    configuration: Configuration,
    #[serde(default)]
    context: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Configuration {
    #[serde(default)]
    is_blocking: bool,
    #[serde(default = "enabled_by_default")]
    is_enabled: bool,
    #[serde(default)]
    is_deleted: bool,
    #[serde(rename = "type", default)]
    policy_type: Option<PolicyType>,
    #[serde(default)]
    settings: Option<Value>,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            is_blocking: false,
            is_enabled: true,
            is_deleted: false,
            policy_type: None,
            settings: None,
        }
    }
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolicyType {
    #[serde(default)]
    id: String,
    #[serde(default)]
    display_name: String,
}

/// The policy types that ask for approvals: "Minimum number of reviewers" and "Required
/// reviewers" (<https://learn.microsoft.com/en-us/azure/devops/repos/git/branch-policies>).
const APPROVAL_POLICY_TYPES: [&str; 2] =
    ["fa4e907d-c16b-4a4c-9dfa-4906e5d171dd", "fd2167ab-b0be-447a-8ec8-39368250530e"];

pub(crate) async fn row_statuses(
    client: &Client,
    projects: &ProjectIds,
    repo: &str,
    numbers: &[u64],
) -> Result<Vec<(u64, RowStatus)>> {
    client.require_token("Reading branch policies")?;
    let key = RepoKey::parse(repo)?;
    let project_id = projects.get(client, key).await?;
    let url = client.project_api(key, "policy/evaluations");
    let mut statuses = Vec::new();
    for &number in numbers {
        let artifact = format!("vstfs:///CodeReview/CodeReviewId/{project_id}/{number}");
        let request = client
            .request_versioned(Method::GET, &url, EVALUATIONS_API_VERSION)
            .query(&[("artifactId", artifact)]);
        let evaluations: ValueList<Evaluation> = client.json(request).await?;
        statuses.push((number, row_status(evaluations.value)));
    }
    Ok(statuses)
}

/// Enabled policies, blocking ones first. The reviewer policies are no checks: the blocking ones
/// that apply make the row's required approvals, the rest show nowhere (votes show the reviews).
fn row_status(evaluations: Vec<Evaluation>) -> RowStatus {
    let mut approvals = Vec::new();
    let mut checks = Vec::new();
    for evaluation in evaluations {
        let configuration = &evaluation.configuration;
        if !configuration.is_enabled || configuration.is_deleted {
            continue;
        }
        let asks_approvals = configuration
            .policy_type
            .as_ref()
            .is_some_and(|policy_type| APPROVAL_POLICY_TYPES.contains(&policy_type.id.as_str()));
        let is_blocking = configuration.is_blocking;
        let check = check(evaluation);
        match (asks_approvals, is_blocking) {
            // A policy that does not apply (`notApplicable`, neutral) asks for nothing.
            (true, true) if check.state != CheckState::Neutral => approvals.push(check),
            (true, _) => {}
            (false, _) => checks.push(check),
        }
    }
    checks.sort_by_key(|check| check.is_required != Some(true));
    RowStatus { checks, required_approvals: combined(&approvals), ..RowStatus::default() }
}

/// Several reviewer policies as one: the state of the one furthest from done, every name with its
/// detail.
fn combined(approvals: &[Check]) -> Option<Check> {
    let state = [CheckState::Failed, CheckState::Pending, CheckState::Passed]
        .into_iter()
        .find(|state| approvals.iter().any(|check| check.state == *state))?;
    let detail = approvals
        .iter()
        .map(|check| match &check.detail {
            Some(detail) => format!("{}: {detail}", check.name),
            None => check.name.clone(),
        })
        .collect::<Vec<_>>()
        .join("; ");
    Some(Check::new("Reviewer policies", state).required(Some(true)).with_detail(Some(detail)))
}

fn check(evaluation: Evaluation) -> Check {
    let state = match evaluation.status.as_str() {
        "approved" => CheckState::Passed,
        "rejected" | "broken" => CheckState::Failed,
        "queued" | "running" => CheckState::Pending,
        _ => CheckState::Neutral,
    };
    let settings = evaluation.configuration.settings.as_ref();
    let context = evaluation.context.as_ref();
    let text = |value: Option<&Value>, field: &str| {
        value
            .and_then(|value| value.get(field))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let type_name = evaluation
        .configuration
        .policy_type
        .map(|policy_type| policy_type.display_name)
        .filter(|name| !name.is_empty());
    let status_name =
        text(settings, "statusName").map(|name| match text(settings, "statusGenre") {
            Some(genre) => format!("{genre}/{name}"),
            None => name,
        });
    let name = text(settings, "displayName")
        .or_else(|| text(context, "buildDefinitionName"))
        .or(status_name)
        .or_else(|| type_name.clone())
        .unwrap_or_else(|| "Policy".to_owned());
    let detail = settings
        .and_then(|settings| settings.get("minimumApproverCount"))
        .and_then(Value::as_u64)
        .map(|count| {
            format!("{count} {} required", if count == 1 { "approval" } else { "approvals" })
        })
        .or_else(|| {
            let is_expired =
                context.and_then(|context| context.get("isExpired")).and_then(Value::as_bool);
            (is_expired == Some(true)).then(|| "Expired".to_owned())
        })
        .or_else(|| type_name.filter(|type_name| *type_name != name));
    Check::new(name, state).required(Some(evaluation.configuration.is_blocking)).with_detail(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_policies_blocking_first_and_skips_disabled_ones() {
        let evaluations: ValueList<Evaluation> = serde_json::from_str(
            r#"{"count": 5, "value": [
                {"status": "approved", "configuration": {"isBlocking": false, "isEnabled": true, "type": {"displayName": "Work item linking"}}},
                {"status": "running", "configuration": {"isBlocking": true, "isEnabled": true, "type": {"displayName": "Build"}, "settings": {"buildDefinitionId": 3}},
                 "context": {"buildDefinitionName": "web-ci", "buildId": 91}},
                {"status": "rejected", "configuration": {"isBlocking": true, "isEnabled": true, "type": {"id": "fa4e907d-c16b-4a4c-9dfa-4906e5d171dd", "displayName": "Minimum number of reviewers"}, "settings": {"minimumApproverCount": 2}}},
                {"status": "approved", "configuration": {"isBlocking": true, "isEnabled": false, "type": {"displayName": "Comment requirements"}}},
                {"status": "queued", "configuration": {"isBlocking": true, "type": {"displayName": "Status"}, "settings": {"statusGenre": "sonar", "statusName": "quality-gate"}}}
            ]}"#,
        )
        .expect("fixture");
        let status = row_status(evaluations.value);
        let checks: Vec<(&str, CheckState, Option<bool>, Option<&str>)> = status
            .checks
            .iter()
            .map(|check| {
                (check.name.as_str(), check.state, check.is_required, check.detail.as_deref())
            })
            .collect();
        assert_eq!(
            checks,
            [
                ("web-ci", CheckState::Pending, Some(true), Some("Build")),
                ("sonar/quality-gate", CheckState::Pending, Some(true), Some("Status")),
                ("Work item linking", CheckState::Passed, Some(false), None),
            ],
            "the reviewer policy is no check"
        );
        assert_eq!(status.verdict(), Some(CheckState::Pending));
        assert_eq!(status.passed_of(), (1, 3));
        let approvals = status.required_approvals.expect("the reviewer policy");
        assert_eq!(
            (approvals.state, approvals.detail.as_deref()),
            (CheckState::Failed, Some("Minimum number of reviewers: 2 approvals required"))
        );
    }

    #[test]
    fn reviewer_policies_decide_the_required_approvals() {
        let required = |policies: &str| {
            let evaluations: ValueList<Evaluation> =
                serde_json::from_str(&format!(r#"{{"count": 2, "value": [{policies}]}}"#))
                    .expect("fixture");
            row_status(evaluations.value).required_approvals.map(|approvals| approvals.state)
        };
        let policy = |status: &str, id: &str, is_blocking: bool| {
            format!(
                r#"{{"status": "{status}", "configuration": {{"isBlocking": {is_blocking}, "type": {{"id": "{id}"}}}}}}"#
            )
        };
        let minimum = "fa4e907d-c16b-4a4c-9dfa-4906e5d171dd";
        let named = "fd2167ab-b0be-447a-8ec8-39368250530e";
        let both =
            format!("{},{}", policy("approved", minimum, true), policy("running", named, true));
        assert_eq!(required(&both), Some(CheckState::Pending), "a named reviewer is missing");
        let met =
            format!("{},{}", policy("approved", minimum, true), policy("approved", named, true));
        assert_eq!(required(&met), Some(CheckState::Passed));
        let optional = policy("running", minimum, false);
        assert_eq!(required(&optional), None, "an optional policy asks for nothing");
        let elsewhere = policy("notApplicable", named, true);
        assert_eq!(required(&elsewhere), None, "nor does one that does not apply");
        let build = policy("approved", "0609b952-1397-4640-95ec-e00a01b2c241", true);
        assert_eq!(required(&build), None, "a build policy is no approval");
    }
}
