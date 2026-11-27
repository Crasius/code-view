//! The pull request's writes (vote, merge, close, comment status, …): one at a time, a toast on
//! completion. A failure never ends in a fading message: its toast stays with the ways out, Retry
//! for the header's writes ([`Write`]), "Fix token" when the platform refused the token (401/403)
//! and the pull request on its platform. Retry runs the failed write once more through the page's
//! next [`ActionRunner::poll`]; a failed merge also stays on the merge button
//! ([`ActionRunner::take_merge_failure`]). A merge refused because new commits arrived after the
//! review (`MergeOptions::is_head_moved`) offers Reload instead of Retry: running it again would
//! hit the same guard, reloading shows the new commits to review.
//!
//! The merge button asks for its auto-complete writes ([`AutoCompleteWrite`]) by leaving them in
//! the context ([`ask_auto_complete`]); the page's next [`ActionRunner::poll`] starts them, as it
//! starts a Retry. A failed "set" also offers the platform's merge settings, where a repository
//! that refuses auto-merge allows it.
//!
//! Every write names its pull request and [`Effect`] when it starts, and goes into the app-wide log
//! of the user's writes as on its way ([`writes::flights`]): the lists show its row as pending. The
//! task itself lands it, so a success is recorded ([`writes::land`]) by whichever view polls first,
//! also while this page is hidden or after it closed. The page shows it in the frame it polls the
//! result, before the refresh that follows ([`Refresh`]) brings what the effect does not model.

use std::future::Future;

use cv_base::mailbox;
use cv_base::theme::icons;
use cv_runtime::load::Load;
use cv_runtime::route::PullRequestKey;
use cv_runtime::toasts::{self, ProviderFailure, ToastAction, WaysOut};
use cv_services::Services;
use cv_services::writes::{self, Effect};
use review_core::{AutoComplete, MergeOptions, ProviderError, ProviderKind, PullRequestState};

use crate::auto_complete::{self, Mode};
use crate::lifecycle::LifecycleAction;
use crate::session::PullRequestSession;

/// What to reload after a write succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    PullRequest,
    Threads,
}

/// Identifies one started action so its caller can learn the outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ticket(u64);

/// A write of the header (and the reviewer card) that the failure toast's Retry starts again.
#[derive(Clone)]
pub enum Write {
    Lifecycle(LifecycleAction),
    Merge(MergeOptions),
    AutoComplete(AutoCompleteWrite),
}

/// Turning the platform's auto-complete on or off.
#[derive(Clone)]
pub enum AutoCompleteWrite {
    /// With the options the platform completes with; `settings_url` is where the target branch's
    /// merge settings live, for a refusal.
    Set {
        options: MergeOptions,
        settings_url: Option<String>,
    },
    Cancel,
}

impl AutoCompleteWrite {
    fn mode(&self) -> Mode {
        match self {
            AutoCompleteWrite::Set { .. } => Mode::SetAutoComplete,
            AutoCompleteWrite::Cancel => Mode::CancelAutoComplete,
        }
    }
}

impl Write {
    /// Starts the write; `None` when another one still runs.
    pub fn start(
        self,
        session: &PullRequestSession,
        services: &mut Services,
        actions: &mut ActionRunner,
    ) -> Option<Ticket> {
        match self {
            Write::Lifecycle(action) => action.start(session, services, actions),
            Write::Merge(options) => merge(session, services, actions, options),
            Write::AutoComplete(write) => set_auto_complete(session, services, actions, write),
        }
    }

    /// The write's name in "`<name>` failed: `<reason>`".
    fn name(&self, kind: ProviderKind) -> &'static str {
        match self {
            Write::Merge(_) => "Merge",
            Write::Lifecycle(action) => action.name(),
            Write::AutoComplete(AutoCompleteWrite::Set { .. }) => auto_complete::set_label(kind),
            Write::AutoComplete(AutoCompleteWrite::Cancel) => auto_complete::cancel_label(kind),
        }
    }
}

/// Turns the platform's auto-complete on or off. Setting it remembers the strategy as merging does.
fn set_auto_complete(
    session: &PullRequestSession,
    services: &mut Services,
    actions: &mut ActionRunner,
    write: AutoCompleteWrite,
) -> Option<Ticket> {
    let kind = session.kind;
    let provider = session.provider.clone();
    let repository = session.key.repository_key.clone();
    let number = session.key.number;
    let effect = match &write {
        AutoCompleteWrite::Set { options, .. } => {
            services.settings.merge.remember_last_used(
                &session.key.connection_id,
                &session.key.repository_key,
                options.strategy,
            );
            services.save_settings();
            let strategy = auto_complete::set_strategy(options.strategy, kind);
            Effect::AutoComplete(Some(AutoComplete { set_by: None, strategy }))
        }
        AutoCompleteWrite::Cancel => Effect::AutoComplete(None),
    };
    let text = auto_complete::done_text(write.mode(), kind);
    let retry = Write::AutoComplete(write.clone());
    let key = (&session.key, effect);
    let ticket = actions.run(services, text, Refresh::PullRequest, key, async move {
        match &write {
            AutoCompleteWrite::Set { options, .. } => {
                provider.set_auto_complete(&repository, number, options).await
            }
            AutoCompleteWrite::Cancel => provider.cancel_auto_complete(&repository, number).await,
        }
    });
    if ticket.is_some() {
        actions.remember(retry);
    }
    ticket
}

/// Leaves `write` for the page's next [`ActionRunner::poll`] of the pull request `session` shows.
pub fn ask_auto_complete(
    ctx: &egui::Context,
    session: &PullRequestSession,
    write: AutoCompleteWrite,
) {
    mailbox::post(ctx, asked_id(session), write);
}

/// Where the merge button leaves an auto-complete write for the page of `session`.
fn asked_id(session: &PullRequestSession) -> egui::Id {
    egui::Id::unique(("auto_complete_asked", &session.key))
}

/// Completes the pull request and remembers the strategy as this repository's last used.
fn merge(
    session: &PullRequestSession,
    services: &mut Services,
    actions: &mut ActionRunner,
    options: MergeOptions,
) -> Option<Ticket> {
    services.settings.merge.remember_last_used(
        &session.key.connection_id,
        &session.key.repository_key,
        options.strategy,
    );
    services.save_settings();
    cv_services::checkouts::merging_here(services, &session.key);
    let provider = session.provider.clone();
    let repository = session.key.repository_key.clone();
    let number = session.key.number;
    let retry = Write::Merge(options.clone());
    let effect = Effect::State(PullRequestState::Merged);
    let text = "Pull request merged.";
    let write = (&session.key, effect);
    let ticket = actions.run(services, text, Refresh::PullRequest, write, async move {
        provider.merge(&repository, number, &options).await
    });
    if ticket.is_some() {
        actions.remember(retry);
    }
    ticket
}

/// Why a write failed, and whether it was a merge refused for commits pushed after the review.
struct WriteFailure {
    failure: ProviderFailure,
    is_head_moved: bool,
}

impl From<ProviderError> for WriteFailure {
    fn from(error: ProviderError) -> Self {
        Self { is_head_moved: matches!(error, ProviderError::HeadMoved), failure: error.into() }
    }
}

/// What the failure toast's Reload leaves in the context; its own type keeps the id's slot apart.
#[derive(Clone, Copy)]
struct ReloadAsked;

/// One write at a time per pull request (vote, comment, merge, …), with a toast on completion.
#[derive(Default)]
pub struct ActionRunner {
    running: Load<Result<(), WriteFailure>>,
    success_text: String,
    refresh: Option<Refresh>,
    next_ticket: u64,
    /// The last finished action and its failure message, `None` when it succeeded.
    finished: Option<(Ticket, Option<String>)>,
    /// The running write, kept for Retry should it fail.
    write: Option<Write>,
    /// The failed write the failure toast's Retry starts again, once.
    retry: Option<(Ticket, Write)>,
    /// Why the last merge failed, until the merge button took it.
    merge_failure: Option<String>,
}

impl ActionRunner {
    pub fn is_busy(&self) -> bool {
        self.running.is_loading()
    }

    /// Starts `future`, the write that does the effect on the pull request `key`, unless another
    /// action runs; `None` means it was not started.
    pub fn run<F>(
        &mut self,
        services: &Services,
        success_text: &str,
        refresh: Refresh,
        (key, effect): (&PullRequestKey, Effect),
        future: F,
    ) -> Option<Ticket>
    where
        F: Future<Output = Result<(), ProviderError>> + Send + 'static,
    {
        if self.is_busy() {
            return None;
        }
        success_text.clone_into(&mut self.success_text);
        self.refresh = Some(refresh);
        self.next_ticket += 1;
        self.write = None;
        let flight = services.writes.flights.start(key, &effect);
        self.running.start(&services.background, async move {
            let result = future.await;
            // Before the page gets the result: the log is the one place every view reads it from.
            flight.land(result.is_ok());
            Ok(result.map_err(WriteFailure::from))
        });
        Some(Ticket(self.next_ticket))
    }

    /// The action just started is `write`: a failure offers Retry.
    pub(crate) fn remember(&mut self, write: Write) {
        self.write = Some(write);
    }

    /// `Some(true)` once the action behind `ticket` succeeded, `Some(false)` when it failed.
    pub fn outcome(&self, ticket: Ticket) -> Option<bool> {
        match &self.finished {
            Some((finished, failure)) if *finished == ticket => Some(failure.is_none()),
            _ => None,
        }
    }

    /// Why the action behind `ticket` failed, while it is the last one that finished.
    pub fn failure(&self, ticket: Ticket) -> Option<&str> {
        match &self.finished {
            Some((finished, Some(message))) if *finished == ticket => Some(message),
            _ => None,
        }
    }

    /// The action behind `ticket` is the one running now.
    pub fn is_running(&self, ticket: Ticket) -> bool {
        self.is_busy() && ticket == Ticket(self.next_ticket)
    }

    /// Why the last merge failed, once.
    pub fn take_merge_failure(&mut self) -> Option<String> {
        self.merge_failure.take()
    }

    /// Starts a write the failure toast's Retry asked for, then returns what to reload when an
    /// action finished successfully this frame or the failure toast's Reload was clicked. A write
    /// that succeeded shows on the page at once.
    pub fn poll(
        &mut self,
        services: &mut Services,
        session: &mut PullRequestSession,
    ) -> Option<Refresh> {
        self.retry_requested(services, session);
        if let Some(write) = mailbox::take(&services.background.ctx(), asked_id(session)) {
            Write::AutoComplete(write).start(session, services, self);
        }
        if take_reload(&services.background.ctx(), reload_id(session)) {
            return Some(Refresh::PullRequest);
        }
        let finished = self.running.take_finished()?;
        let ticket = Ticket(self.next_ticket);
        let write = self.write.take();
        let failure = match finished {
            Ok(Ok(())) => {
                self.finished = Some((ticket, None));
                services.toasts.success(std::mem::take(&mut self.success_text));
                // Unless a list recorded it already.
                writes::land(services);
                session.catch_up_writes(services);
                return self.refresh.take();
            }
            Ok(Err(failure)) => failure,
            Err(message) => {
                WriteFailure { failure: ProviderFailure::local(message), is_head_moved: false }
            }
        };
        let WriteFailure { failure, is_head_moved } = failure;
        let text = match &write {
            Some(write) => format!("{} failed: {}", write.name(session.kind), failure.message),
            None => failure.message.clone(),
        };
        let settings = match &write {
            Some(Write::AutoComplete(AutoCompleteWrite::Set {
                settings_url: Some(url), ..
            })) => Some(merge_settings(session.kind, url.clone())),
            _ => None,
        };
        if let Some(Write::Merge(_)) = &write {
            self.merge_failure = Some(failure.message.clone());
        }
        let retry = if is_head_moved {
            Some(reload(reload_id(session)))
        } else {
            write.map(|write| {
                self.retry = Some((ticket, write));
                ToastAction::retry(retry_id(session, ticket))
            })
        };
        let known_url = session.detail.ready().map(|detail| detail.summary.web_url.as_str());
        let connection = services.connection(&session.key.connection_id);
        let page = ToastAction::open_pull_request(connection, &session.key, known_url);
        // Only a connection removed while the write ran leaves none of them.
        let ways_out = failure.ways_out(retry, &session.key.connection_id, page).map_or_else(
            || WaysOut::new(ToastAction::copy("Copy message", text.clone())),
            |ways_out| ways_out.and_some(settings),
        );
        services.toasts.error(text, ways_out);
        self.finished = Some((ticket, Some(failure.message)));
        None
    }

    /// Runs the failed write again when its toast's Retry was clicked, once; a Retry of an older
    /// failure, or one clicked while another write runs, waits or is dropped.
    fn retry_requested(&mut self, services: &mut Services, session: &PullRequestSession) {
        if self.is_busy() {
            return;
        }
        let ctx = services.background.ctx();
        let Some((_, write)) =
            self.retry.take_if(|(ticket, _)| toasts::take_retry(&ctx, retry_id(session, *ticket)))
        else {
            return;
        };
        write.start(session, services, self);
    }
}

/// The platform's merge settings of the repository or target branch, at `url`.
fn merge_settings(kind: ProviderKind, url: String) -> ToastAction {
    let label = match kind {
        ProviderKind::AzureDevOps => "Branch policies",
        ProviderKind::GitHub => "Repository settings",
        ProviderKind::GitLab => "Merge request settings",
    };
    ToastAction::new(icons::GEAR, label, move |ctx| {
        ctx.open_url(egui::OpenUrl::new_tab(url));
    })
}

/// "Reload": leaves `id` for [`ActionRunner::poll`], which reloads the pull request.
fn reload(id: egui::Id) -> ToastAction {
    ToastAction::new(icons::ARROW_CLOCKWISE, "Reload", move |ctx| {
        mailbox::post(ctx, id, ReloadAsked);
    })
}

/// Whether a toast's Reload asked for `id` since the last call; true once per click.
fn take_reload(ctx: &egui::Context, id: egui::Id) -> bool {
    mailbox::take::<ReloadAsked>(ctx, id).is_some()
}

/// Where the failure toast's Reload asks for the pull request of `session` again.
fn reload_id(session: &PullRequestSession) -> egui::Id {
    egui::Id::unique(("reload_after_write", &session.key))
}

/// Where the failure toast's Retry of the write behind `ticket` asks for it on the page of
/// `session`.
fn retry_id(session: &PullRequestSession, ticket: Ticket) -> egui::Id {
    egui::Id::unique(("retry_write", &session.key, ticket))
}
