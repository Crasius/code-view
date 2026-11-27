//! The background side of one conversation: preparing the checkout, starting the CLI session and
//! receiving its events, each on the [`Background`] so the UI thread never waits.
//!
//! Events are received by a task that waits for the next batch and wakes the UI when it arrives
//! (every finished background task requests a repaint). It is armed only while an answer is
//! expected: from a question until the turn ends, which is its `TurnDone` (a failed turn's comes
//! after its `Failed`) or the session's `Closed`. Between turns the receiver rests here, so an
//! idle conversation runs no task at all; whatever the CLI reports meanwhile (it closed) arrives
//! with the next question.

use std::path::PathBuf;
use std::sync::Arc;

use cv_runtime::background::{Background, Task, TaskPoll};
use cv_services::agents::{AgentDirectory, Started};
use review_agent::{AgentError, AgentEvent, Session, SessionConfig};
use review_git::GitMirror;
use tokio::sync::mpsc::UnboundedReceiver;

use super::checkout::{self, Checkout};

/// Events that arrived together, and the receiver for the next ones.
type Batch = (Vec<AgentEvent>, UnboundedReceiver<AgentEvent>);

/// What finished in the background since the last frame.
pub(crate) enum RunnerEvent {
    CheckoutReady(Checkout),
    CheckoutFailed(String),
    SessionFailed(AgentError),
    Agent(AgentEvent),
}

#[derive(Default)]
pub(crate) struct Runner {
    checkout: Option<Task<Result<Checkout, String>>>,
    starting: Option<Task<Started>>,
    session: Option<Session>,
    events: Option<Task<Batch>>,
    /// The receiver while no answer is expected.
    resting: Option<UnboundedReceiver<AgentEvent>>,
    /// Counts the sessions ended, so events of an ended session can be told apart.
    generation: u64,
}

impl Runner {
    /// Puts the checkout at `dest` on `head`.
    pub(crate) fn prepare_checkout(
        &mut self,
        background: &Background,
        git: Arc<GitMirror>,
        head: String,
        dest: PathBuf,
    ) {
        self.checkout = Some(background.spawn(checkout::prepare(git, head, dest)));
    }

    pub(crate) fn is_preparing(&self) -> bool {
        self.checkout.is_some()
    }

    pub(crate) fn start(
        &mut self,
        agents: &AgentDirectory,
        background: &Background,
        config: SessionConfig,
    ) {
        self.end();
        self.starting = Some(agents.start(background, config));
    }

    pub(crate) fn is_starting(&self) -> bool {
        self.starting.is_some()
    }

    pub(crate) fn has_session(&self) -> bool {
        self.session.is_some()
    }

    /// Sends `prompt` and waits for the answer.
    pub(crate) fn ask(&mut self, background: &Background, prompt: &str) {
        let Some(session) = &self.session else {
            return;
        };
        session.ask(prompt);
        self.arm(background);
    }

    /// Stops the running turn; its end still arrives as an event.
    pub(crate) fn interrupt(&self) {
        if let Some(session) = &self.session {
            session.interrupt();
        }
    }

    /// Ends the session: dropping it ends the CLI process.
    pub(crate) fn end(&mut self) {
        self.generation += 1;
        self.starting = None;
        self.session = None;
        self.events = None;
        self.resting = None;
    }

    /// Changes whenever [`Self::end`] ends a session: events taken from the runner before belong to
    /// the ended one.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Nothing runs: no checkout, no session starting or running.
    pub(crate) fn is_idle(&self) -> bool {
        self.checkout.is_none() && self.starting.is_none() && self.session.is_none()
    }

    /// Ends the session and drops a checkout on its way; the assistant was turned off.
    pub(crate) fn abandon(&mut self) {
        self.end();
        self.checkout = None;
    }

    fn arm(&mut self, background: &Background) {
        if self.events.is_some() {
            return;
        }
        let Some(mut receiver) = self.resting.take() else {
            return;
        };
        self.events = Some(background.spawn(async move {
            let mut batch = Vec::new();
            match receiver.recv().await {
                Some(event) => batch.push(event),
                // Every sender is gone: so is the process.
                None => batch.push(AgentEvent::Closed),
            }
            while let Ok(event) = receiver.try_recv() {
                batch.push(event);
            }
            (batch, receiver)
        }));
    }

    /// What finished since the last frame; re-arms the receiver while the turn goes on.
    pub(crate) fn poll(&mut self, background: &Background) -> Vec<RunnerEvent> {
        let mut finished = Vec::new();
        if let Some(task) = &mut self.checkout {
            match task.poll() {
                TaskPoll::Pending => {}
                TaskPoll::Done(Ok(checkout)) => {
                    finished.push(RunnerEvent::CheckoutReady(checkout));
                    self.checkout = None;
                }
                TaskPoll::Done(Err(message)) => {
                    finished.push(RunnerEvent::CheckoutFailed(message));
                    self.checkout = None;
                }
                TaskPoll::Lost => {
                    finished.push(RunnerEvent::CheckoutFailed(
                        "The checkout stopped unexpectedly.".to_owned(),
                    ));
                    self.checkout = None;
                }
            }
        }
        if let Some(task) = &mut self.starting {
            match task.poll() {
                TaskPoll::Pending => {}
                TaskPoll::Done(Ok((session, events))) => {
                    self.session = Some(session);
                    self.resting = Some(events);
                    self.starting = None;
                }
                TaskPoll::Done(Err(error)) => {
                    self.starting = None;
                    finished.push(RunnerEvent::SessionFailed(error));
                }
                TaskPoll::Lost => {
                    self.starting = None;
                    let error = AgentError::Spawn("the start stopped unexpectedly".to_owned());
                    finished.push(RunnerEvent::SessionFailed(error));
                }
            }
        }
        if let Some(task) = &mut self.events {
            match task.poll() {
                TaskPoll::Pending => {}
                TaskPoll::Done((batch, receiver)) => {
                    self.events = None;
                    // A failure is followed by the turn's end or the session's.
                    let goes_on = batch.last().is_some_and(|event| {
                        !matches!(event, AgentEvent::TurnDone(_) | AgentEvent::Closed)
                    });
                    let is_closed = batch.contains(&AgentEvent::Closed);
                    finished.extend(batch.into_iter().map(RunnerEvent::Agent));
                    if is_closed {
                        self.session = None;
                    } else {
                        self.resting = Some(receiver);
                        if goes_on {
                            self.arm(background);
                        }
                    }
                }
                TaskPoll::Lost => {
                    self.events = None;
                    self.session = None;
                    finished.push(RunnerEvent::Agent(AgentEvent::Closed));
                }
            }
        }
        finished
    }
}

#[cfg(test)]
mod tests {
    use cv_runtime::background::Background;
    use cv_testkit::poll_until;
    use review_agent::{AgentError, AgentEvent, Session, Usage};

    use super::{Runner, RunnerEvent};

    /// The agent events the runner hands out until `count` arrived.
    fn agent_events(runner: &mut Runner, background: &Background, count: usize) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        poll_until("the agent's events", || {
            let finished = runner.poll(background).into_iter();
            events.extend(finished.filter_map(|event| match event {
                RunnerEvent::Agent(event) => Some(event),
                _ => None,
            }));
            events.len() >= count
        });
        events
    }

    /// A failed turn still ends with its `TurnDone`: left in the receiver, it would end the next
    /// question before its answer came.
    #[test]
    fn a_failed_turn_keeps_listening_for_its_end() {
        let background = Background::new(egui::Context::default());
        let (session, events, agent) = Session::fake();
        let mut runner =
            Runner { session: Some(session), resting: Some(events), ..Runner::default() };
        runner.ask(&background, "Why?");
        let failed = AgentEvent::Failed(AgentError::Turn("rate limited".to_owned()));
        agent.emit(failed.clone());
        assert_eq!(agent_events(&mut runner, &background, 1), [failed]);

        agent.emit(AgentEvent::TurnDone(Usage::default()));
        assert_eq!(
            agent_events(&mut runner, &background, 1),
            [AgentEvent::TurnDone(Usage::default())]
        );
    }
}
