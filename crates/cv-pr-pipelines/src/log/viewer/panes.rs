//! What the log pane shows in place of a log: an item without one (yet), a log on its way, and
//! a download that failed, each with its way forward.

use cv_base::theme::{self, icons};
use cv_pr_session::session::PullRequestSession;
use cv_runtime::intents;
use cv_widgets::provider_icon;
use cv_widgets::skeleton;
use cv_widgets::states::{self, Action};
use egui::{RichText, Ui};
use review_core::{PipelineJob, ProviderError, ProviderKind, RunStatus};

use crate::job_tree::LogTarget;

/// The most of the platform's raw answer kept behind "Details".
const MAX_DETAILS_CHARS: usize = 4_000;

/// What the log pane says about an item without a log, or with one the platform does not serve
/// yet.
#[derive(Debug, PartialEq, Eq)]
pub struct NoLog {
    icon: &'static str,
    title: String,
    detail: String,
    /// The item runs: the way out is watching it live on the platform.
    watch_live: bool,
}

impl NoLog {
    pub fn of(status: RunStatus, name: &str, platform: ProviderKind) -> Self {
        let platform_name = platform.label();
        let (icon, title, detail) = match status {
            RunStatus::Queued if platform == ProviderKind::AzureDevOps => (
                icons::CLOCK,
                "Waiting for an agent",
                format!(
                    "{name} waits for an agent or for the jobs it depends on. Its log appears here when it starts."
                ),
            ),
            RunStatus::Queued => (
                icons::CLOCK,
                "Waiting to start",
                format!("{name} has not started yet. Its log appears here when it starts."),
            ),
            RunStatus::Running if platform == ProviderKind::AzureDevOps => (
                icons::CIRCLE_NOTCH,
                "Log not served yet",
                format!(
                    "{name} is running. Azure DevOps serves a task's log once the agent has uploaded it, usually when the task finishes; it loads here on its own then."
                ),
            ),
            RunStatus::Running => (
                icons::CIRCLE_NOTCH,
                "No log yet",
                format!(
                    "{name} is running; {platform_name} has not started its log yet. It appears here as soon as it exists and follows the job live."
                ),
            ),
            RunStatus::Skipped if platform == ProviderKind::AzureDevOps => (
                icons::SKIP_FORWARD,
                "Skipped",
                format!("{name} was skipped due to conditions, so it has no log."),
            ),
            RunStatus::Skipped => {
                (icons::SKIP_FORWARD, "Skipped", format!("{name} was skipped, so it has no log."))
            }
            RunStatus::Canceled => (
                icons::PROHIBIT,
                "Canceled",
                format!("{name} was canceled before it ran, so it has no log."),
            ),
            RunStatus::Succeeded | RunStatus::Failed | RunStatus::Neutral => {
                (icons::INFO, "No log", format!("{platform_name} reports no log for {name}."))
            }
        };
        Self { icon, title: title.to_owned(), detail, watch_live: status == RunStatus::Running }
    }

    /// The platform serves the log only once its job finished, and `owner` (the job) still runs.
    /// A step of it is named: its log is part of the job's.
    pub fn after_finish(
        target: &LogTarget,
        owner: Option<&PipelineJob>,
        platform: ProviderKind,
    ) -> Self {
        let platform_name = platform.label();
        let job = owner.map_or(target.title.as_str(), |job| job.name.as_str());
        let detail = if owner.is_some_and(|job| job.id != target.job_id) {
            format!(
                "The {platform_name} API serves the log of {job}, with all its steps, once the job has finished. {job} is still running, so the log of {step} loads here on its own when it is done.",
                step = target.title
            )
        } else {
            format!(
                "The {platform_name} API serves a job's log once the job has finished. {job} is still running; its log loads here on its own when it is done."
            )
        };
        Self {
            icon: icons::CIRCLE_NOTCH,
            title: format!("Live log only {}", states::on_platform(platform)),
            detail,
            watch_live: true,
        }
    }

    /// The item's state, and the way to watch or open it on the platform.
    pub fn show(&self, ui: &mut Ui, target: &LogTarget, platform: ProviderKind) {
        let label = if self.watch_live {
            format!("Watch live {}", states::on_platform(platform))
        } else {
            states::open_on_platform(platform)
        };
        let actions: Vec<Action<'_>> = target
            .web_url
            .iter()
            .map(|_| Action::primary(provider_icon::glyph(platform), &label))
            .collect();
        if states::message_with_actions(ui, self.icon, &self.title, &self.detail, &actions)
            .is_some()
            && let Some(url) = &target.web_url
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    }
}

/// The log is asked for or on its way: how much arrived, over skeleton lines.
pub fn downloading(ui: &mut Ui, title: &str, received: u64) {
    let text = if received == 0 {
        format!("Requesting the log of {title}…")
    } else {
        format!("Downloading the log of {title}: {}", format_bytes(received))
    };
    waiting(ui, &text);
}

/// The job just finished and the platform has not published its log yet: it is asked for again.
pub fn publishing(ui: &mut Ui, title: &str, platform: ProviderKind) {
    waiting(ui, &format!("{} is publishing the log of {title}…", platform.label()));
}

/// Something on its way to the log pane, `text` saying what: a spinner over skeleton lines.
pub fn waiting(ui: &mut Ui, text: &str) {
    let palette = theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(RichText::new(text).color(palette.text_secondary));
    });
    ui.add_space(8.0);
    skeleton::code_lines(ui, 30, 18.0, false);
}

/// The ways out of a failed log download.
enum FailedLog<'a> {
    Open(&'a str),
    AddToken,
    Retry,
}

/// A download that failed: the cause in a few words with the ways out; the platform's raw
/// answer only behind "Details". True when the user asked to try again.
pub fn failed(
    ui: &mut Ui,
    cause: &str,
    details: Option<&str>,
    target: &LogTarget,
    session: &PullRequestSession,
) -> bool {
    let open_label = format!("Open log {}", states::on_platform(session.kind));
    let mut choices = Vec::new();
    if let Some(url) = &target.web_url {
        choices.push((
            Action::primary(provider_icon::glyph(session.kind), open_label.as_str()),
            FailedLog::Open(url),
        ));
    }
    if !session.has_token {
        choices.push((Action::secondary(icons::GEAR, "Add a token"), FailedLog::AddToken));
    }
    choices.push((Action::secondary(icons::ARROW_CLOCKWISE, "Retry"), FailedLog::Retry));
    let actions: Vec<Action<'_>> = choices.iter().map(|(action, _)| *action).collect();
    let picked = states::error_with_actions(ui, cause, &actions).map(|index| &choices[index].1);
    if let Some(details) = details {
        states::details(ui, "log_failure_details", details);
    }
    match picked {
        Some(FailedLog::Open(url)) => ui.ctx().open_url(egui::OpenUrl::new_tab(*url)),
        Some(FailedLog::AddToken) => {
            intents::request_fix(ui.ctx(), &session.key.connection_id);
        }
        Some(FailedLog::Retry) => return true,
        None => {}
    }
    false
}

/// A failed download in a few words with what to do, and the platform's raw answer for "Details".
/// The answer itself (an XML or JSON document) is never the message.
pub fn failure(
    error: &ProviderError,
    title: &str,
    platform: ProviderKind,
) -> (String, Option<String>) {
    let platform_name = platform.label();
    match error {
        ProviderError::Status { status, path, body } => {
            let cause = match status {
                401 | 403 => format!(
                    "{platform_name} did not let this connection read the log of {title} (HTTP {status})."
                ),
                404 => format!("{platform_name} has no log for {title} (HTTP 404)."),
                500.. => format!(
                    "{platform_name} could not serve the log of {title} (HTTP {status}). Retry in a moment."
                ),
                _ => format!("{platform_name} did not serve the log of {title} (HTTP {status})."),
            };
            let body: String = body.trim().chars().take(MAX_DETAILS_CHARS).collect();
            (cause, Some(format!("HTTP {status} from {path}\n{body}")))
        }
        ProviderError::Network(message) => (
            format!(
                "The log of {title} did not download: the connection to {platform_name} failed."
            ),
            Some(message.clone()),
        ),
        ProviderError::Decode(message) => (
            format!("{platform_name} sent the log of {title} in a form the app cannot read."),
            Some(message.clone()),
        ),
        // The app's own sentences: a token is needed, a rate limit, a local file problem.
        ProviderError::Unsupported(_)
        | ProviderError::Local(_)
        | ProviderError::RateLimited { .. }
        | ProviderError::HeadMoved => (error.to_string(), None),
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value >= KIB * KIB {
        format!("{:.1} MB", value / KIB / KIB)
    } else {
        format!("{:.0} KB", (value / KIB).max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use review_core::{PipelineJob, ProviderError, ProviderKind, RunStatus};

    use super::{LogTarget, NoLog, failure, format_bytes};

    #[test]
    fn explains_why_an_item_has_no_log() {
        let azure = ProviderKind::AzureDevOps;
        assert_eq!(NoLog::of(RunStatus::Queued, "Linux", azure).title, "Waiting for an agent");
        assert_eq!(
            NoLog::of(RunStatus::Queued, "lint", ProviderKind::GitLab).title,
            "Waiting to start"
        );
        let running = NoLog::of(RunStatus::Running, "Linux", azure);
        assert!(
            running
                .detail
                .contains("usually when the task finishes; it loads here on its own then"),
            "{}",
            running.detail
        );
        assert!(running.watch_live, "a running task offers watching it live");
        assert!(
            NoLog::of(RunStatus::Running, "rspec", ProviderKind::GitLab)
                .detail
                .contains("follows the job live")
        );
        let skipped = NoLog::of(RunStatus::Skipped, "Publish", azure);
        assert!(!skipped.watch_live);
        assert!(skipped.detail.contains("due to conditions"), "{}", skipped.detail);
        assert_eq!(
            NoLog::of(RunStatus::Canceled, "Deploy", ProviderKind::GitHub).title,
            "Canceled"
        );
        let succeeded = NoLog::of(RunStatus::Succeeded, "trigger", ProviderKind::GitLab);
        assert!(
            succeeded.detail.contains("GitLab") && succeeded.detail.contains("trigger"),
            "{}",
            succeeded.detail
        );
    }

    fn target(job_id: &str, title: &str) -> LogTarget {
        LogTarget {
            job_id: job_id.to_owned(),
            log_id: Some("118225375880".to_owned()),
            title: title.to_owned(),
            web_url: Some(format!(
                "https://github.com/o/r/actions/runs/1/job/118225375880#step:{job_id}"
            )),
        }
    }

    /// GitHub, job running: the pane says where the live log is and names the selected step.
    #[test]
    fn a_running_github_job_points_to_its_live_log() {
        let job = PipelineJob {
            id: "118225375880".to_owned(),
            name: "Backend / Lint and test".to_owned(),
            status: RunStatus::Running,
            ..PipelineJob::default()
        };
        let pane = NoLog::after_finish(
            &target("118225375880", "Backend / Lint and test"),
            Some(&job),
            ProviderKind::GitHub,
        );
        assert_eq!(pane.title, "Live log only on GitHub");
        assert!(
            pane.detail.contains("Backend / Lint and test") && !pane.detail.contains("steps"),
            "names the job, not a step: {}",
            pane.detail
        );
        assert!(pane.watch_live);
        let step = NoLog::after_finish(
            &target("118225375880:5", "Pytest"),
            Some(&job),
            ProviderKind::GitHub,
        );
        assert!(
            step.detail.contains("so the log of Pytest loads here on its own"),
            "{}",
            step.detail
        );
        assert!(step.detail.starts_with(
            "The GitHub API serves the log of Backend / Lint and test, with all its steps"
        ));
    }

    /// The blob storage's XML answer stays behind "Details"; the message is a sentence.
    #[test]
    fn failures_read_as_a_cause_with_the_raw_answer_aside() {
        let blob = ProviderError::Status {
            status: 404,
            path: "/actions-results/7f/workflow-job-run-1/logs/job/job-logs.txt".to_owned(),
            body: "<?xml version=\"1.0\" encoding=\"utf-8\"?><Error><Code>BlobNotFound</Code><Message>The specified blob does not exist.</Message></Error>".to_owned(),
        };
        let (cause, details) = failure(&blob, "Pytest", ProviderKind::GitHub);
        assert_eq!(cause, "GitHub has no log for Pytest (HTTP 404).");
        assert!(!cause.contains('<'));
        assert!(
            details.is_some_and(|details| details.starts_with("HTTP 404 from /actions-results/")
                && details.contains("<Code>BlobNotFound</Code>"))
        );
        let denied = ProviderError::Status {
            status: 403,
            path: "/repos/o/r/actions/jobs/1/logs".to_owned(),
            body: r#"{"message":"Must have admin rights to Repository."}"#.to_owned(),
        };
        assert_eq!(
            failure(&denied, "lint", ProviderKind::GitHub).0,
            "GitHub did not let this connection read the log of lint (HTTP 403)."
        );
        let token = ProviderError::requires_token("Downloading logs");
        assert_eq!(
            failure(&token, "lint", ProviderKind::GitHub),
            (token.to_string(), None),
            "the app's own sentence stays"
        );
    }

    #[test]
    fn formats_download_sizes() {
        assert_eq!(format_bytes(10), "1 KB");
        assert_eq!(format_bytes(300 * 1024), "300 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024 + 512 * 1024), "5.5 MB");
    }
}
