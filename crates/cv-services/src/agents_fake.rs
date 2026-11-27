//! The agents of the test builds: no CLI process, no network. A test says which agents are
//! installed (and signed in), and scripts each started session through its [`FakeAgent`]. A
//! terminal the app opens is only recorded.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use review_agent::{AgentCli, AgentError, AgentKind, FakeAgent, ModelInfo, Session, SessionConfig};

use super::agents::Started;

/// How one fake agent answers.
#[derive(Debug, Clone, Default)]
pub enum FakeCli {
    #[default]
    Missing,
    Installed {
        models: Vec<ModelInfo>,
        is_signed_in: bool,
    },
}

#[derive(Default)]
pub struct FakeAgents {
    clis: Mutex<HashMap<AgentKind, FakeCli>>,
    /// Sessions started and not yet taken by the test, with what they were started with.
    sessions: Mutex<VecDeque<(SessionConfig, FakeAgent)>>,
    /// Terminals opened and not yet taken by the test.
    terminals: Mutex<Vec<FakeTerminal>>,
}

/// A terminal the app opened: in which folder, running what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeTerminal {
    pub dir: PathBuf,
    pub program: String,
    pub args: Vec<String>,
}

impl FakeAgents {
    pub fn set(&self, kind: AgentKind, cli: FakeCli) {
        self.clis.lock().unwrap_or_else(PoisonError::into_inner).insert(kind, cli);
    }

    fn cli(&self, kind: AgentKind) -> FakeCli {
        self.clis
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&kind)
            .cloned()
            .unwrap_or_default()
    }

    pub fn detect(&self, kind: AgentKind) -> Result<AgentCli, AgentError> {
        match self.cli(kind) {
            FakeCli::Missing => Err(AgentError::NotInstalled(kind)),
            FakeCli::Installed { .. } => Ok(AgentCli {
                kind,
                program: PathBuf::from(kind.label().to_lowercase()),
                version: "9.9.9 (fake)".to_owned(),
            }),
        }
    }

    pub fn list_models(&self, cli: &AgentCli) -> Result<Vec<ModelInfo>, AgentError> {
        match self.cli(cli.kind) {
            FakeCli::Missing => Err(AgentError::NotInstalled(cli.kind)),
            FakeCli::Installed { models, .. } => Ok(models),
        }
    }

    pub fn start(&self, config: SessionConfig) -> Started {
        let kind = config.cli.kind;
        match self.cli(kind) {
            FakeCli::Missing => Err(AgentError::NotInstalled(kind)),
            FakeCli::Installed { is_signed_in: false, .. } => Err(AgentError::NotSignedIn(kind)),
            FakeCli::Installed { is_signed_in: true, .. } => {
                let (session, events, agent) = Session::fake();
                self.sessions
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push_back((config, agent));
                Ok((session, events))
            }
        }
    }

    pub fn open_terminal(&self, dir: &Path, program: &str, args: &[String]) {
        let terminal = FakeTerminal {
            dir: dir.to_path_buf(),
            program: program.to_owned(),
            args: args.to_vec(),
        };
        self.terminals.lock().unwrap_or_else(PoisonError::into_inner).push(terminal);
    }

    /// The terminals opened since the last call.
    pub fn take_terminals(&self) -> Vec<FakeTerminal> {
        std::mem::take(&mut *self.terminals.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The oldest session started and not taken yet.
    pub fn take_session(&self) -> Option<(SessionConfig, FakeAgent)> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner).pop_front()
    }
}
