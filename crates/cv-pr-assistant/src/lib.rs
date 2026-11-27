//! The pull request assistant: questions about the pull request, answered by the user's own,
//! signed-in Claude Code or Codex CLI (`review-agent`), reading a detached checkout of the head.
//!
//! A card at the end of the Overview's side column (below Reviewers, Labels and Linked), which
//! Alt+A or its expand button widens into a full-height panel; the conversation is the same in
//! both. One conversation per pull request and agent, kept across restarts
//! ([`review_store::AssistantChats`], written on the background by [`transcript::saver`]); the next
//! question continues the CLI's own session.
//!
//! A question walks through: checkout of the head ([`checkout`], under the cache folder) → the
//! CLI found and its models listed ([`cv_services::agents`]) → a session started with the
//! pull request's [`brief`] → the answer streaming in ([`transcript`]). Each step runs on the
//! background ([`runner`]); [`Assistant::update`] advances them every frame on every tab, so an
//! answer keeps arriving while the Files tab shows. A step that cannot go on (no CLI, no sign-in,
//! a failed checkout, a stopped agent) shows its own fix ([`Block`]).
//!
//! Answers cite code as `path:line`; [`citations`] checks each against the checkout and links the
//! ones that exist into the Files tab, where the [`mini`] chat can keep the answer beside the code.
//! The assistant never posts to the host.
//!
//! Settings can turn the assistant off: then the page shows neither card nor panel, drops every
//! request to open it, ends a running CLI ([`Assistant::turn_off`]) and starts nothing. Views
//! outside the page ask [`cv_pr_session::assistant_link::is_enabled`] before they offer it. The
//! conversations stay on disk.

#![forbid(unsafe_code)]
// RS-LINT-02: egui lays out in f32 points; its margins are i8 and its clock is f64.
#![expect(
    clippy::cast_possible_truncation,
    reason = "egui pixel math converts between f64 time, f32 points and i8 margins"
)]
#![cfg_attr(
    test,
    expect(clippy::cast_precision_loss, reason = "tests measure text widths in characters")
)]

mod actions;
mod brief;
mod checkout;
mod chip;
pub mod citations;
pub mod mini;
mod runner;
mod title;
mod transcript;
mod turns;
mod view;
mod wheel;

use cv_pr_session::session::PullRequestSession;
use cv_runtime::background::Background;
use cv_runtime::route::PullRequestKey;
use cv_runtime::toasts::ToastAction;
use cv_services::Services;
use cv_services::agents::Detection;
use review_agent::{AgentError, AgentKind, ModelChoice, SessionConfig};
use review_store::{AssistantChats, Chat, ChatQuote, TurnEnd, chats_dir};

use self::checkout::Checkout;
use self::citations::links;
use self::runner::{Runner, RunnerEvent};
use self::transcript::Applied;
use self::transcript::saver::Saver;

pub use self::title::{TitleLine, card_action};
#[cfg(any(test, feature = "test-support"))]
pub use self::view::CARD_LABEL;
pub use self::view::{TITLE, card, card_drawn, panel};

/// The command that signs the agent's CLI in, run in a terminal.
fn sign_in_command(kind: AgentKind) -> (String, Vec<String>) {
    match kind {
        AgentKind::Claude => ("claude".to_owned(), Vec::new()),
        AgentKind::Codex => ("codex".to_owned(), vec!["login".to_owned()]),
    }
}

/// Why the conversation cannot go on, each with its own fix in the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// The install guide, and "Check again".
    NotInstalled(AgentKind),
    /// A terminal to sign in, and "Check again".
    NotSignedIn(AgentKind),
    /// Git's message, and "Retry".
    Checkout(String),
    /// The agent's process ended: why, and "Restart".
    Stopped(String),
}

/// What the user did in the card or panel this frame.
#[derive(Debug, Clone, PartialEq)]
enum Action {
    Ask(String),
    Stop,
    /// Ask the last, failed question again.
    Retry,
    SwitchAgent(AgentKind),
    SwitchModel(ModelChoice),
    NewConversation,
    /// Brings back the conversation "New conversation" put away.
    UndoNewConversation,
    ContinueInTerminal,
    SignIn,
    CheckAgain,
    /// Clears the block and goes on where it stopped.
    Resume,
    UpdateAgent,
    Expand,
    Collapse,
    DropQuote,
}

pub struct Assistant {
    key: PullRequestKey,
    chats: AssistantChats,
    /// Writes [`Self::chats`] on the background.
    saver: Saver,
    chat: Chat,
    runner: Runner,
    /// The question being written, one draft for the card, the panel and the [`mini`] chat. It
    /// lives here, not on the per-agent [`Chat`]: switching agent or model or starting a new
    /// conversation keeps it; only a question that went out empties it.
    composer: String,
    /// Diff lines the next question is about.
    quote: Option<ChatQuote>,
    /// The prompt waiting for the checkout and the session.
    pending: Option<String>,
    checkout: Option<Checkout>,
    /// "Update the agent": the head the checkout moves to before the agent is told.
    move_to: Option<String>,
    /// What the session answers with, as the CLI reported it.
    model_in_use: Option<String>,
    block: Option<Block>,
    /// What the conversation waits for, while a question waits.
    waiting: Option<&'static str>,
    is_stopping: bool,
    is_expanded: bool,
    focus_composer: bool,
    /// The conversation "New conversation" put away, until the next question goes out.
    previous: Option<Chat>,
    /// Whether the card's conversation takes the mouse wheel.
    wheel: wheel::CardWheel,
    links: links::LinkedAnswers,
    /// Resolves the answers' citation links.
    resolver: links::Links,
    /// The answer kept beside the code over the Files tab.
    mini: mini::MiniChat,
}

impl Assistant {
    /// The conversation kept for `key`, or a new one with the default agent and model.
    pub fn new(key: &PullRequestKey, services: &mut Services) -> Self {
        let chats = AssistantChats::load(&chats_dir(&services.paths), &key.storage_key())
            .unwrap_or_else(|error| {
                let folder = ToastAction::open_folder(chats_dir(&services.paths));
                let text = format!("Could not read the assistant's conversation: {error}");
                services.toasts.error(text, folder);
                AssistantChats::in_memory()
            });
        let agent = chats.current().unwrap_or(services.settings.assistant.agent);
        let mut assistant = Self {
            key: key.clone(),
            chat: Chat::default(),
            chats,
            saver: Saver::new(&services.background),
            runner: Runner::default(),
            composer: String::new(),
            quote: None,
            pending: None,
            checkout: None,
            move_to: None,
            model_in_use: None,
            block: None,
            waiting: None,
            is_stopping: false,
            is_expanded: false,
            focus_composer: false,
            previous: None,
            wheel: wheel::CardWheel::default(),
            links: links::LinkedAnswers::default(),
            resolver: links::Links::default(),
            mini: mini::MiniChat::default(),
        };
        assistant.show_conversation(agent, services);
        assistant
    }

    /// Shows the kept conversation with `agent`, or a new one.
    fn show_conversation(&mut self, agent: AgentKind, services: &Services) {
        self.chat = self.chats.chat(agent).cloned().unwrap_or_else(|| Chat {
            agent,
            model: services.settings.assistant.model(agent).clone(),
            ..Chat::default()
        });
        transcript::settle_loaded(&mut self.chat);
        self.previous = None;
        self.runner.end();
        self.pending = None;
        self.move_to = None;
        self.block = None;
        self.model_in_use = None;
        self.is_stopping = false;
        self.links = links::LinkedAnswers::default();
        self.links.to_check = (0..self.chat.turns.len()).collect();
        self.mini.close();
    }

    pub fn is_expanded(&self) -> bool {
        self.is_expanded
    }

    /// Opens the assistant (expanded, the composer focused), with `quote` attached when given.
    pub fn open(&mut self, quote: Option<ChatQuote>) {
        self.is_expanded = true;
        self.focus_composer = true;
        if quote.is_some() {
            self.quote = quote;
        }
    }

    /// A question waits or is being answered.
    fn is_busy(&self) -> bool {
        self.chat.turns.last().is_some_and(|turn| turn.end == TurnEnd::Running)
    }

    /// Advances the conversation; runs every frame on every tab.
    pub fn update(&mut self, session: &PullRequestSession, services: &mut Services) {
        self.take_in_saves(services);
        let finished = self.runner.poll(&services.background);
        self.take_in_all(finished, services);
        self.drive(session, services);
        links::check_links(self, services);
    }

    /// The assistant was turned off in Settings: the panel collapses, the CLI process ends and a
    /// waiting or running question ends stopped. Runs every frame while it is off; does nothing
    /// once nothing runs. The conversation stays on disk for when it is turned on again.
    pub fn turn_off(&mut self, services: &mut Services) {
        self.is_expanded = false;
        self.focus_composer = false;
        self.take_in_saves(services);
        let is_running = !self.runner.is_idle() || self.pending.is_some() || self.is_busy();
        if !is_running {
            return;
        }
        self.runner.abandon();
        self.pending = None;
        self.move_to = None;
        self.waiting = None;
        self.is_stopping = false;
        transcript::stop(&mut self.chat);
        self.save(services);
    }

    /// Takes in what finished since the last frame, in order. Once an event ended the session (a
    /// missing sign-in, the process gone), the agent events after it belong to the ended session:
    /// the end of its failed turn would otherwise finish the question kept for after the fix.
    fn take_in_all(&mut self, finished: Vec<RunnerEvent>, services: &mut Services) {
        let generation = self.runner.generation();
        for event in finished {
            let is_ended = self.runner.generation() != generation;
            if !(is_ended && matches!(event, RunnerEvent::Agent(_))) {
                self.take_in(event, services);
            }
        }
    }

    fn take_in(&mut self, event: RunnerEvent, services: &mut Services) {
        match event {
            RunnerEvent::CheckoutReady(checkout) => {
                if self.chat.head.is_none() || self.move_to.take().is_some() {
                    self.chat.head = Some(checkout.head.clone());
                }
                self.checkout = Some(checkout);
                self.links.to_check = (0..self.chat.turns.len()).collect();
            }
            RunnerEvent::CheckoutFailed(message) => {
                self.block = Some(Block::Checkout(message));
            }
            RunnerEvent::SessionStarted => {}
            RunnerEvent::SessionFailed(error) => self.fail(&error),
            RunnerEvent::Agent(event) => match transcript::apply(&mut self.chat, event) {
                Applied::Progress => {}
                Applied::Started { model } => self.model_in_use = Some(model),
                Applied::TurnDone => self.turn_done(),
                Applied::Failed(error) => self.fail(&error),
                Applied::Closed => {
                    self.runner.end();
                    self.is_stopping = false;
                    if self.pending.is_none() && !self.is_busy() {
                        self.block = Some(Block::Stopped("The agent's process ended.".to_owned()));
                    }
                }
            },
        }
        self.save(services);
    }

    fn turn_done(&mut self) {
        let index = self.chat.turns.len().saturating_sub(1);
        if self.is_stopping
            && let Some(turn) = self.chat.turns.last_mut()
        {
            turn.end = TurnEnd::Stopped;
        }
        self.is_stopping = false;
        self.links.to_check.push(index);
    }

    /// A failure of the start, the session or the turn. A missing CLI or sign-in blocks and keeps
    /// the question to ask again once fixed; anything else ends the question with its message.
    fn fail(&mut self, error: &AgentError) {
        self.is_stopping = false;
        let block = match error {
            AgentError::NotInstalled(kind) => Some(Block::NotInstalled(*kind)),
            AgentError::NotSignedIn(kind) => Some(Block::NotSignedIn(*kind)),
            AgentError::Spawn(_)
            | AgentError::Protocol(_)
            | AgentError::Turn(_)
            | AgentError::Exited { .. }
            | AgentError::Timeout(_) => None,
        };
        if let Some(block) = block {
            self.runner.end();
            self.block = Some(block);
            if let Some(turn) = self.chat.turns.last_mut()
                && matches!(turn.end, TurnEnd::Running | TurnEnd::Failed(_))
                && self.pending.is_none()
            {
                turn.end = TurnEnd::Running;
                turn.answer.clear();
                self.pending = Some(turn.prompt.clone());
            }
            return;
        }
        self.pending = None;
        match transcript::running(&mut self.chat) {
            Some(turn) => turn.end = TurnEnd::Failed(error.to_string()),
            None if !self.runner.has_session() => {
                self.block = Some(Block::Stopped(error.to_string()));
            }
            None => {}
        }
    }

    /// Moves a waiting question on: checkout, then session, then the prompt.
    fn drive(&mut self, session: &PullRequestSession, services: &mut Services) {
        self.waiting = None;
        if self.block.is_some() || self.pending.is_none() {
            return;
        }
        if self.runner.is_preparing() {
            self.waiting = Some("Preparing the checkout…");
            return;
        }
        let Some(detail) = session.detail.ready() else {
            self.waiting = Some("Loading the pull request…");
            return;
        };
        let wanted = self
            .move_to
            .clone()
            .or_else(|| self.checkout.is_none().then(|| detail.head_sha.clone()));
        if let Some(head) = wanted {
            self.start_checkout(session, &services.background, &services.paths, head);
            return;
        }
        if self.runner.has_session() {
            if let Some(prompt) = self.pending.take() {
                self.runner.ask(&services.background, &prompt);
            }
            return;
        }
        if !self.runner.is_starting() {
            self.start_session(session, services);
        }
    }

    fn start_checkout(
        &mut self,
        session: &PullRequestSession,
        background: &Background,
        paths: &review_store::AppPaths,
        head: String,
    ) {
        if let Some(message) = session.mirror.error() {
            self.block = Some(Block::Checkout(message.to_owned()));
            return;
        }
        let Some(mirror) = session.mirror.ready() else {
            self.waiting = Some("Fetching the pull request…");
            return;
        };
        self.waiting = Some("Preparing the checkout…");
        let dir = checkout::dir(paths, &self.key.repository_key, self.key.number);
        self.runner.prepare_checkout(background, mirror.git.clone(), head, dir);
    }

    fn start_session(&mut self, session: &PullRequestSession, services: &mut Services) {
        let kind = self.chat.agent;
        self.waiting = Some("Starting the agent…");
        let cli = match services.agents.detection(kind, &services.background) {
            Detection::Checking => return,
            Detection::Missing(error) => {
                let error = error.clone();
                self.fail(&error);
                return;
            }
            Detection::Installed(cli) => cli.clone(),
        };
        let pick = match services.agents.models(kind, &services.background) {
            None => return,
            Some(Ok(models)) => review_agent::resolve_model(kind, &self.chat.model, models),
            // Without the list, Fastest cannot be picked: the CLI's own default answers.
            Some(Err(_)) => review_agent::resolve_model(kind, &self.chat.model, &[]),
        };
        let (Some(detail), Some(checkout)) = (session.detail.ready(), &self.checkout) else {
            return;
        };
        if session.files().is_none() && session.files_fast.error().is_none() {
            self.waiting = Some("Listing the changed files…");
            return;
        }
        let facts = brief::Facts {
            detail,
            merge_base: session.mirror.ready().map(|mirror| mirror.merge_base.as_str()),
            files: session.files().map_or(&[], |files| files.as_slice()),
            threads: session.threads.ready().map_or(&[], Vec::as_slice),
            runs: session.runs.ready().map_or(&[], AsRef::as_ref),
        };
        let config = SessionConfig {
            cli,
            pick,
            workdir: checkout.dir.clone(),
            brief: brief::brief(&facts),
            resume: self.chat.session_id.clone(),
        };
        self.runner.start(&services.agents, &services.background, config);
    }

    /// New commits arrived since the conversation started: how many is not known, only that.
    fn has_new_commits(&self, session: &PullRequestSession) -> bool {
        let head = session.detail.ready().map(|detail| detail.head_sha.as_str());
        !self.chat.turns.is_empty()
            && self.move_to.is_none()
            && self.chat.head.as_deref().is_some_and(|seen| Some(seen) != head)
            && head.is_some()
    }

    /// Why "Continue in terminal" cannot open the agent's CLI yet, or `None` when it can: it
    /// resumes this conversation's session, or before the first answer starts a fresh one in the
    /// checkout, which the first question prepares.
    pub fn terminal_block(&self) -> Option<&'static str> {
        let can_open = self.chat.session_id.is_some() || self.checkout.is_some();
        (!can_open).then_some("Ask a first question to prepare the checkout")
    }

    /// What the title line of the card or panel shows besides its buttons.
    pub fn title_line(&self, services: &mut Services) -> TitleLine {
        TitleLine::new(self, services)
    }

    /// A citation was followed into the Files tab: the [`mini`] chat shows its answer there.
    pub fn follow_citation(&mut self, path: &str, start: u32, end: u32) {
        mini::follow(self, path, start, end);
    }

    /// The [`mini`] chat over the Files tab, drawn after the tab; the lines it asks the diff for.
    pub fn mini_chat(
        &mut self,
        ui: &egui::Ui,
        session: &PullRequestSession,
        services: &mut Services,
    ) -> Option<mini::Jump> {
        mini::show(self, ui.ctx(), session, services)
    }

    /// Keeps the conversation shown and writes it on the background ([`transcript::saver`]).
    fn save(&mut self, services: &mut Services) {
        self.chats.put(self.chat.clone());
        self.saver.changed();
        self.take_in_saves(services);
    }

    /// Starts the waiting write and shows the failure of a finished one.
    fn take_in_saves(&mut self, services: &mut Services) {
        if let Err(error) = self.saver.poll(&self.chats, &services.background) {
            let folder = ToastAction::open_folder(chats_dir(&services.paths));
            let text = format!("Could not save the assistant's conversation: {error}");
            services.toasts.error(text, folder);
        }
    }
}

impl Drop for Assistant {
    /// The page closes: what no write took yet still goes to disk, on the background.
    fn drop(&mut self) {
        self.saver.finish(&self.chats);
    }
}

#[cfg(test)]
mod state_tests;
