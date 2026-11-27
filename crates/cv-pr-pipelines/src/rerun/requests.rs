//! Reruns asked of the platform: started one after another off the UI thread, confirmed with a
//! toast and a reload, or kept on screen with their ways out when one did not start. The reruns
//! that started before it are confirmed and reloaded all the same; Retry starts the one that did
//! not start and those not tried yet.

use std::future::Future;

use cv_base::theme::{icons, style};
use cv_pr_session::session::PullRequestSession;
use cv_runtime::intents;
use cv_runtime::load::Load;
use cv_runtime::toasts::ToastAction;
use cv_services::Services;
use cv_shell::focus_mode;
use cv_widgets::provider_icon;
use cv_widgets::states::{self, Action};
use egui::Ui;
use review_core::{PipelineRun, RerunAction};

/// The widest a rerun's failure floats over the log in focus mode.
const FOCUS_FAILURE_WIDTH: f32 = 520.0;

/// How a batch of reruns went.
struct RerunOutcome {
    /// The runs whose reruns started.
    started: Vec<String>,
    /// The rerun that did not start; the ones after it were not tried.
    failure: Option<RerunFailure>,
}

struct RerunFailure {
    message: String,
    /// 401/403: the token is missing a permission.
    is_denied: bool,
    action: RerunAction,
    run_id: String,
    /// The reruns after this one, not tried; Retry starts them too.
    rest: Vec<(String, RerunAction)>,
}

/// Why one rerun did not start: the message, and whether the token is missing a permission.
type Refusal = (String, bool);

/// Starts `reruns` one after another with `rerun` until one does not start.
async fn in_turn<F, Fut>(reruns: Vec<(String, RerunAction)>, mut rerun: F) -> RerunOutcome
where
    F: FnMut(&str, &RerunAction) -> Fut,
    Fut: Future<Output = Result<(), Refusal>>,
{
    let mut started = Vec::new();
    let mut pending = reruns.into_iter();
    while let Some((run_id, action)) = pending.next() {
        if let Err((message, is_denied)) = rerun(&run_id, &action).await {
            let rest = pending.collect();
            let failure = RerunFailure { message, is_denied, action, run_id, rest };
            return RerunOutcome { started, failure: Some(failure) };
        }
        started.push(run_id);
    }
    RerunOutcome { started, failure: None }
}

/// The ways out of a rerun that did not start.
enum FailureChoice<'a> {
    Retry,
    FixToken,
    Open(&'a str),
    Dismiss,
}

#[derive(Default)]
pub(crate) struct RerunRequests {
    running: Load<RerunOutcome>,
    failure: Option<RerunFailure>,
}

impl RerunRequests {
    pub(crate) fn is_busy(&self) -> bool {
        self.running.is_loading()
    }

    /// Whether the last rerun that did not start belongs to the run with `run_id`.
    pub(crate) fn has_failed(&self, run_id: &str) -> bool {
        self.failure.as_ref().is_some_and(|failure| failure.run_id == run_id)
    }

    /// The run whose last rerun did not start, among `runs`.
    pub(crate) fn failed_run<'a>(&self, runs: &'a [PipelineRun]) -> Option<&'a PipelineRun> {
        let failure = self.failure.as_ref()?;
        runs.iter().find(|run| run.id == failure.run_id)
    }

    /// Starts the reruns one after another; the first that does not start stops the rest and
    /// stays on screen with its ways out, Retry included, which goes on with the rest.
    pub(crate) fn start(
        &mut self,
        reruns: Vec<(String, RerunAction)>,
        session: &PullRequestSession,
        services: &Services,
    ) {
        if self.running.is_loading() || reruns.is_empty() {
            return;
        }
        self.failure = None;
        let provider = session.provider.clone();
        let repository = session.key.repository_key.clone();
        let number = session.key.number;
        self.running.start(&services.background, async move {
            let outcome = in_turn(reruns, |run_id, action| {
                let (provider, repository) = (provider.clone(), repository.clone());
                let (run_id, action) = (run_id.to_owned(), action.clone());
                async move {
                    provider.rerun(&repository, number, &run_id, &action).await.map_err(|error| {
                        let is_denied = provider.explain_denial(&error).is_some()
                            || matches!(error.status(), Some(401 | 403));
                        (error.to_string(), is_denied)
                    })
                }
            });
            Ok(outcome.await)
        });
    }

    /// A finished rerun request: confirm it and reload so the new attempt shows up, or keep the
    /// failure on screen with its ways out.
    pub(crate) fn poll(&mut self, session: &mut PullRequestSession, services: &mut Services) {
        let Some(result) = self.running.take_finished() else {
            return;
        };
        match result {
            Ok(RerunOutcome { started, failure }) => {
                if !started.is_empty() {
                    services.toasts.success(if started.len() == 1 {
                        "Rerun started".to_owned()
                    } else {
                        format!("{} reruns started", started.len())
                    });
                    session.reload_runs(services);
                    for run_id in started {
                        session.reload_stages(&run_id, services);
                    }
                }
                self.failure = failure;
            }
            // The task stopped without an answer: some reruns may have started, so no Retry; the
            // platform shows which did.
            Err(message) => {
                let known_url =
                    session.detail.ready().map(|detail| detail.summary.web_url.as_str());
                let connection = services.connection(&session.key.connection_id);
                let page = ToastAction::open_pull_request(connection, &session.key, known_url);
                let ways_out =
                    page.unwrap_or_else(|| ToastAction::copy("Copy message", message.clone()));
                services.toasts.error(format!("Reruns may not have started: {message}"), ways_out);
                session.reload_runs(services);
            }
        }
    }

    /// The last rerun of `run` that did not start, with Retry, the token fix and the platform
    /// page.
    pub(crate) fn show_failure(
        &mut self,
        ui: &mut Ui,
        run: &PipelineRun,
        web_url: Option<&str>,
        session: &PullRequestSession,
        services: &Services,
    ) {
        let Some(failure) = self.failure.as_ref().filter(|failure| failure.run_id == run.id) else {
            return;
        };
        let open_label = states::open_on_platform(session.kind);
        let mut choices =
            vec![(Action::primary(icons::ARROW_CLOCKWISE, "Retry"), FailureChoice::Retry)];
        if failure.is_denied || !session.has_token {
            choices.push((Action::secondary(icons::KEY, "Fix token"), FailureChoice::FixToken));
        }
        if let Some(url) = web_url {
            choices.push((
                Action::secondary(provider_icon::glyph(session.kind), open_label.as_str()),
                FailureChoice::Open(url),
            ));
        }
        choices.push((Action::secondary(icons::X, "Dismiss"), FailureChoice::Dismiss));
        let actions: Vec<Action<'_>> = choices.iter().map(|(action, _)| *action).collect();
        let text = format!("The rerun of {} did not start: {}", run.name, failure.message);
        ui.add_space(style::SPACE_S);
        let picked = states::error_with_actions(ui, &text, &actions);
        match picked.map(|index| &choices[index].1) {
            Some(FailureChoice::Retry) => {
                let rerun = (failure.run_id.clone(), failure.action.clone());
                let reruns = std::iter::once(rerun).chain(failure.rest.iter().cloned()).collect();
                self.start(reruns, session, services);
            }
            Some(FailureChoice::FixToken) => {
                intents::request_fix(ui.ctx(), &session.key.connection_id);
            }
            Some(FailureChoice::Open(url)) => ui.ctx().open_url(egui::OpenUrl::new_tab(*url)),
            Some(FailureChoice::Dismiss) => self.failure = None,
            None => {}
        }
    }

    /// In focus mode a rerun's failure floats under the strip's right end, with all its actions:
    /// the log below keeps its place, whether the failure comes or goes.
    pub(crate) fn show_focus_failure(
        &mut self,
        ui: &Ui,
        run: &PipelineRun,
        web_url: Option<&str>,
        session: &PullRequestSession,
        services: &Services,
    ) {
        if !self.has_failed(&run.id) {
            return;
        }
        let page = ui.max_rect();
        let corner = egui::pos2(
            page.right() - style::SPACE_M,
            page.top() + focus_mode::strip::HEIGHT + style::SPACE_S,
        );
        egui::Area::new(ui.scope_id().with("focus_rerun_failure"))
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(corner)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(FOCUS_FAILURE_WIDTH.min(page.width() - style::SPACE_M * 2.0));
                    self.show_failure(ui, run, web_url, session, services);
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use review_core::{RerunAction, RerunKind};

    use super::in_turn;

    #[test]
    fn a_rerun_that_does_not_start_keeps_the_started_ones_and_the_rest() {
        let reruns: Vec<(String, RerunAction)> = ["build", "test", "deploy"]
            .into_iter()
            .map(|run| (run.to_owned(), RerunAction::new(RerunKind::RunFailed, run)))
            .collect();
        let runtime = tokio::runtime::Builder::new_current_thread().build().expect("runtime");
        let outcome = runtime.block_on(in_turn(reruns, |run_id, _| {
            let is_refused = run_id == "test";
            async move { if is_refused { Err(("403 Forbidden".to_owned(), true)) } else { Ok(()) } }
        }));
        assert_eq!(outcome.started, ["build"], "confirmed and reloaded");
        let failure = outcome.failure.expect("the second did not start");
        assert_eq!((failure.run_id.as_str(), failure.is_denied), ("test", true));
        let rest: Vec<&str> = failure.rest.iter().map(|(run_id, _)| run_id.as_str()).collect();
        assert_eq!(rest, ["deploy"], "Retry goes on with it");
    }
}
