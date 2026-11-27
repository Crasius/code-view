//! The coding agent CLIs on this computer (Claude Code, Codex), for the pull request assistant and
//! Settings. Each CLI is looked for once per app run, and its models are asked once after it was
//! found; "Check again" asks anew. Everything runs on the [`Background`]; the views read what
//! arrived.
//!
//! Tests never start a real CLI: in test builds (and with the `test-support` feature) the
//! directory answers from `FakeAgents`, which reports every agent as not installed until a test
//! sets it up, hands out scripted sessions and records the terminals the app would open.

use review_agent::{
    AgentCli, AgentError, AgentEvent, AgentKind, ModelInfo, Session, SessionConfig,
};
use tokio::sync::mpsc::UnboundedReceiver;

#[cfg(any(test, feature = "test-support"))]
pub(crate) use crate::agents_fake::FakeAgents;
#[cfg(any(test, feature = "test-support"))]
pub use crate::agents_fake::{FakeCli, FakeTerminal};
use cv_runtime::background::{Background, Task, TaskPoll};

/// A started session and its events, or why it did not start.
pub type Started = Result<(Session, UnboundedReceiver<AgentEvent>), AgentError>;

/// Whether an agent's CLI is there.
pub enum Detection<'a> {
    Checking,
    Installed(&'a AgentCli),
    Missing(&'a AgentError),
}

/// One answer that is asked for once.
enum Probe<T> {
    Idle,
    Running(Task<T>),
    Done(T),
}

impl<T: Send + 'static> Probe<T> {
    fn poll(&mut self) {
        let Probe::Running(task) = self else {
            return;
        };
        match task.poll() {
            TaskPoll::Pending => {}
            TaskPoll::Done(value) => *self = Probe::Done(value),
            TaskPoll::Lost => *self = Probe::Idle,
        }
    }

    fn done(&self) -> Option<&T> {
        match self {
            Probe::Done(value) => Some(value),
            Probe::Idle | Probe::Running(_) => None,
        }
    }
}

struct AgentProbes {
    cli: Probe<Result<AgentCli, AgentError>>,
    models: Probe<Result<Vec<ModelInfo>, AgentError>>,
}

impl AgentProbes {
    fn new() -> Self {
        Self { cli: Probe::Idle, models: Probe::Idle }
    }
}

pub struct AgentDirectory {
    claude: AgentProbes,
    codex: AgentProbes,
    /// What the agents answer in tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fake: std::sync::Arc<FakeAgents>,
}

impl Default for AgentDirectory {
    fn default() -> Self {
        Self {
            claude: AgentProbes::new(),
            codex: AgentProbes::new(),
            #[cfg(any(test, feature = "test-support"))]
            fake: std::sync::Arc::default(),
        }
    }
}

impl AgentDirectory {
    fn probes(&mut self, kind: AgentKind) -> &mut AgentProbes {
        match kind {
            AgentKind::Claude => &mut self.claude,
            AgentKind::Codex => &mut self.codex,
        }
    }

    /// Whether `kind`'s CLI is installed; the first call starts looking.
    pub fn detection(&mut self, kind: AgentKind, background: &Background) -> Detection<'_> {
        #[cfg(any(test, feature = "test-support"))]
        let fake = self.fake.clone();
        let probes = self.probes(kind);
        if matches!(probes.cli, Probe::Idle) {
            #[cfg(not(any(test, feature = "test-support")))]
            let task = background.spawn(review_agent::detect(kind));
            #[cfg(any(test, feature = "test-support"))]
            let task = background.spawn(async move { fake.detect(kind) });
            probes.cli = Probe::Running(task);
        }
        probes.cli.poll();
        match probes.cli.done() {
            None => Detection::Checking,
            Some(Ok(cli)) => Detection::Installed(cli),
            Some(Err(error)) => Detection::Missing(error),
        }
    }

    /// Whether `kind`'s CLI was found, if anyone looked already; never starts looking.
    pub fn installed(&mut self, kind: AgentKind) -> Option<bool> {
        let probes = self.probes(kind);
        probes.cli.poll();
        probes.cli.done().map(Result::is_ok)
    }

    /// The installed CLI of `kind`, once found.
    pub(crate) fn cli(&mut self, kind: AgentKind, background: &Background) -> Option<AgentCli> {
        match self.detection(kind, background) {
            Detection::Installed(cli) => Some(cli.clone()),
            Detection::Checking | Detection::Missing(_) => None,
        }
    }

    /// The models `kind`'s CLI offers: `None` while they are asked for, an error when the CLI
    /// could not list them. Asked once the CLI was found.
    pub fn models(
        &mut self,
        kind: AgentKind,
        background: &Background,
    ) -> Option<Result<&[ModelInfo], &AgentError>> {
        let cli = self.cli(kind, background)?;
        #[cfg(any(test, feature = "test-support"))]
        let fake = self.fake.clone();
        let probes = self.probes(kind);
        if matches!(probes.models, Probe::Idle) {
            #[cfg(not(any(test, feature = "test-support")))]
            let task = background.spawn(async move { review_agent::list_models(&cli).await });
            #[cfg(any(test, feature = "test-support"))]
            let task = background.spawn(async move { fake.list_models(&cli) });
            probes.models = Probe::Running(task);
        }
        probes.models.poll();
        probes.models.done().map(|models| models.as_deref())
    }

    /// Nothing looked for a CLI or its models since the start or the last [`Self::check_again`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn is_untouched(&self) -> bool {
        [&self.claude, &self.codex]
            .iter()
            .all(|probes| matches!(probes.cli, Probe::Idle) && matches!(probes.models, Probe::Idle))
    }

    /// Looks for `kind`'s CLI and its models again (after an install or a sign-in).
    pub fn check_again(&mut self, kind: AgentKind) {
        *self.probes(kind) = AgentProbes::new();
    }

    /// Opens the user's terminal in `dir` running `program` with `args` (an agent's CLI, to sign in
    /// or to go on in it). Test builds have no way to open a terminal: the directory records it in
    /// `FakeAgents` instead.
    #[cfg_attr(
        not(any(test, feature = "test-support")),
        expect(clippy::unused_self, reason = "test builds record the terminals they hold")
    )]
    pub fn open_terminal(
        &self,
        dir: &std::path::Path,
        program: &str,
        args: &[String],
    ) -> std::io::Result<()> {
        #[cfg(not(any(test, feature = "test-support")))]
        let opened = cv_base::platform::open_terminal(dir, program, args);
        #[cfg(any(test, feature = "test-support"))]
        let opened = {
            self.fake.open_terminal(dir, program, args);
            Ok(())
        };
        opened
    }

    /// Opens the user's terminal `terminal` with a plain shell in `dir` (a checkout's folder); test
    /// builds record it in `FakeAgents` as the program `shell (<terminal>)`.
    #[cfg_attr(
        not(any(test, feature = "test-support")),
        expect(clippy::unused_self, reason = "test builds record the terminals they hold")
    )]
    pub(crate) fn open_shell(
        &self,
        dir: &std::path::Path,
        terminal: review_store::checkouts::TerminalChoice,
    ) -> std::io::Result<()> {
        #[cfg(not(any(test, feature = "test-support")))]
        let opened = cv_base::platform::open_shell(dir, terminal);
        #[cfg(any(test, feature = "test-support"))]
        let opened = {
            self.fake.open_terminal(dir, &format!("shell ({terminal:?})"), &[]);
            Ok(())
        };
        opened
    }

    /// Starts a session for `config` on the background runtime.
    #[cfg_attr(
        not(any(test, feature = "test-support")),
        expect(clippy::unused_self, reason = "test builds start the scripted sessions it holds")
    )]
    pub fn start(&self, background: &Background, config: SessionConfig) -> Task<Started> {
        #[cfg(not(any(test, feature = "test-support")))]
        let task = background.spawn(async move { Session::start(config) });
        #[cfg(any(test, feature = "test-support"))]
        let task = {
            let fake = self.fake.clone();
            background.spawn(async move { fake.start(config) })
        };
        task
    }
}
