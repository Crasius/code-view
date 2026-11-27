//! The pull request assistant's conversations: one per pull request and agent, kept across
//! restarts so the transcript shows again and the next question continues the CLI's own session.
//! The CLI keeps the conversation itself; this file keeps what the app shows and the session id
//! that resumes it. One file per pull request, under the cache folder: losing it costs the
//! transcript, not the CLI's session. Comment drafts, which earlier versions asked for, are
//! dropped on load: they never show, and the next save leaves them out of the file.
//!
//! The app writes a [`ChatsSnapshot`] away from the UI thread, often while an answer streams in.
//! Snapshots are numbered as they are taken, and a write never replaces a later snapshot of the
//! same file: writes that land out of order, or a closing page's last write racing the next
//! page's first, leave the file at the latest state.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use review_core::{AgentKind, ModelChoice};
use serde::{Deserialize, Serialize};

use crate::{AppPaths, atomic_file, stable_hash};

/// Numbers the snapshots in the order they are taken, across every store of the process.
static NEXT_SNAPSHOT: AtomicU64 = AtomicU64::new(1);

/// The number of the snapshot each file holds. Held while a file is written, so the check and
/// the write go together; only the background writes take it, never the UI thread.
static WRITTEN: Mutex<BTreeMap<PathBuf, u64>> = Mutex::new(BTreeMap::new());

/// Where the conversations live, one file per pull request.
pub fn chats_dir(paths: &AppPaths) -> PathBuf {
    paths.cache_dir().join("assistant")
}

/// Where the detached checkouts the agents read live, one folder per pull request.
pub fn agent_checkouts_dir(paths: &AppPaths) -> PathBuf {
    paths.cache_dir().join("agent-checkouts")
}

/// The conversations of one pull request.
pub struct AssistantChats {
    /// `None` for a store that is never written.
    file: Option<PathBuf>,
    stored: StoredChats,
}

/// The conversations of one pull request as they were when taken, to write on another thread.
pub struct ChatsSnapshot {
    file: Option<PathBuf>,
    stored: StoredChats,
    number: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoredChats {
    /// The agent the panel showed last.
    current: Option<AgentKind>,
    chats: Vec<Chat>,
}

/// One conversation with one agent.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Chat {
    pub agent: AgentKind,
    pub model: ModelChoice,
    /// The CLI's session id, to continue the conversation.
    pub session_id: Option<String>,
    /// The head commit the agent's checkout is at.
    pub head: Option<String>,
    pub turns: Vec<ChatTurn>,
}

/// What started a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TurnKind {
    /// The user's question.
    #[default]
    Question,
    /// A comment draft, which earlier versions asked for; kept so their saved conversations load,
    /// and dropped there ([`AssistantChats::load`]).
    Draft,
    /// The checkout moved to new commits and the agent was told.
    Update,
}

/// One question and its answer.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatTurn {
    pub kind: TurnKind,
    /// What the user asked, as shown.
    pub question: String,
    /// What was sent: the question with its quoted lines, or the draft and update requests.
    pub prompt: String,
    /// Diff lines the question is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<ChatQuote>,
    /// Markdown, as the agent wrote it.
    pub answer: String,
    pub steps: Vec<ChatStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ChatUsage>,
    pub end: TurnEnd,
}

/// Lines of a file the user asked about.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChatQuote {
    pub path: String,
    /// 1-based, inclusive.
    pub start: u32,
    pub end: u32,
    /// The lines are the base version's (deleted lines).
    pub is_old_side: bool,
    pub text: String,
}

/// One step of the agent's work on a turn: a tool call, or a stretch of thinking. Fields added
/// later default, so chats saved before them load (their steps count as done, without
/// durations). Only the time of thinking is kept, not its text: it would make the files large.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatStep {
    /// The tool's name as the CLI reports it; empty for thinking.
    pub tool: String,
    pub detail: String,
    /// `thinking`, or the tool's kind as `review-agent` names it (`read`, `search`, `run`,
    /// `edit`, `other`); empty in chats saved before kinds.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The CLI's id of the call, which its end names; only while the turn runs.
    #[serde(skip)]
    pub id: String,
    /// When the step started, in Unix milliseconds; only while the turn runs.
    #[serde(skip)]
    pub started_ms: Option<u64>,
    /// How long the step took; none while it runs and when its end never came.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// The CLI reported the call as failed.
    #[serde(skip_serializing_if = "is_false")]
    pub is_failed: bool,
    /// What the model wrote while thinking; only while the turn runs.
    #[serde(skip)]
    pub thinking: String,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
fn is_false(value: &bool) -> bool {
    !value
}

/// Token counts of a turn as the CLI reported them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChatUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TurnEnd {
    /// Still answering; a turn stored like this was cut off by the app closing.
    #[default]
    Running,
    Done,
    /// The user stopped it.
    Stopped,
    /// The CLI's own message.
    Failed(String),
}

impl AssistantChats {
    /// The conversations of `pull_request_key` (`connection|repository|number`), without old
    /// comment drafts; a missing file means none.
    pub fn load(dir: &Path, pull_request_key: &str) -> io::Result<Self> {
        let file = dir.join(format!("{}.json", stable_hash(pull_request_key)));
        let mut stored: StoredChats = atomic_file::read_json(&file)?.unwrap_or_default();
        for chat in &mut stored.chats {
            chat.turns.retain(|turn| turn.kind != TurnKind::Draft);
        }
        Ok(Self { file: Some(file), stored })
    }

    /// A store that is never written, for when the file could not be read.
    pub fn in_memory() -> Self {
        Self { file: None, stored: StoredChats::default() }
    }

    /// The agent shown last, if any conversation was kept.
    pub fn current(&self) -> Option<AgentKind> {
        self.stored.current
    }

    pub fn chat(&self, agent: AgentKind) -> Option<&Chat> {
        self.stored.chats.iter().find(|chat| chat.agent == agent)
    }

    /// Keeps `chat` as its agent's conversation, which becomes the one shown.
    pub fn put(&mut self, chat: Chat) {
        self.stored.current = Some(chat.agent);
        match self.stored.chats.iter_mut().find(|kept| kept.agent == chat.agent) {
            Some(kept) => *kept = chat,
            None => self.stored.chats.push(chat),
        }
    }

    /// Forgets the conversation with `agent` ("New conversation").
    pub fn remove(&mut self, agent: AgentKind) {
        self.stored.chats.retain(|chat| chat.agent != agent);
    }

    /// The conversations as they are now, to write with [`ChatsSnapshot::write`].
    pub fn snapshot(&self) -> ChatsSnapshot {
        ChatsSnapshot {
            file: self.file.clone(),
            stored: self.stored.clone(),
            number: NEXT_SNAPSHOT.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl ChatsSnapshot {
    /// Writes the conversations, unless the file already holds a later snapshot; a snapshot of
    /// an in-memory store writes nothing. Blocks on disk: call it off the UI thread.
    pub fn write(self) -> io::Result<()> {
        let Some(file) = self.file else {
            return Ok(());
        };
        // A write that panicked left no number behind, so the map is still right.
        let mut written = WRITTEN.lock().unwrap_or_else(PoisonError::into_inner);
        if written.get(&file).is_some_and(|&held| held >= self.number) {
            return Ok(());
        }
        atomic_file::write_json(&file, &self.stored)?;
        written.insert(file, self.number);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDir;

    fn chat(agent: AgentKind) -> Chat {
        Chat {
            agent,
            model: ModelChoice::Named("claude-haiku-5-5".into()),
            session_id: Some("s-1".into()),
            head: Some("a".repeat(40)),
            turns: vec![ChatTurn {
                kind: TurnKind::Question,
                question: "What changed?".into(),
                prompt: "About src/a.rs …

What changed?"
                    .into(),
                quote: Some(ChatQuote {
                    path: "src/a.rs".into(),
                    start: 3,
                    end: 4,
                    is_old_side: false,
                    text: "let a = 1;\nlet b = 2;".into(),
                }),
                answer: "See `src/a.rs:3`.".into(),
                steps: vec![ChatStep {
                    tool: "Read".into(),
                    detail: "src/a.rs".into(),
                    kind: "read".into(),
                    duration_ms: Some(1_250),
                    is_failed: true,
                    ..ChatStep::default()
                }],
                usage: Some(ChatUsage { input: 10, output: 5, cache_read: 900, cache_write: 0 }),
                end: TurnEnd::Failed("rate limited".into()),
            }],
        }
    }

    #[test]
    fn conversations_survive_a_restart_per_agent() {
        let temp = TestDir::new("assistant-chats");
        let mut chats = AssistantChats::load(temp.path(), "gh|acme/widgets|7").unwrap();
        assert_eq!(chats.current(), None, "a missing file means no conversation");
        chats.put(chat(AgentKind::Codex));
        chats.put(chat(AgentKind::Claude));
        chats.snapshot().write().unwrap();

        let mut loaded = AssistantChats::load(temp.path(), "gh|acme/widgets|7").unwrap();
        assert_eq!(loaded.current(), Some(AgentKind::Claude), "the agent put last shows");
        assert_eq!(loaded.chat(AgentKind::Claude), Some(&chat(AgentKind::Claude)));
        assert_eq!(loaded.chat(AgentKind::Codex), Some(&chat(AgentKind::Codex)));
        loaded.remove(AgentKind::Codex);
        assert_eq!(loaded.chat(AgentKind::Codex), None);
        let other = AssistantChats::load(temp.path(), "gh|acme/widgets|8").unwrap();
        assert_eq!(other.current(), None, "each pull request has its own file");
    }

    #[test]
    fn a_corrupt_file_is_an_error_and_fields_added_later_default() {
        let temp = TestDir::new("assistant-chats-corrupt");
        let key = "gh|acme/widgets|7";
        let file = temp.path().join(format!("{}.json", stable_hash(key)));
        std::fs::write(&file, b"{ not json").unwrap();
        assert!(AssistantChats::load(temp.path(), key).is_err());
        let older = br#"{"chats":[{"agent":"Codex","turns":[
            {"question":"Why?","steps":[{"tool":"Read","detail":"src/a.rs"}]}]}]}"#;
        std::fs::write(&file, older).unwrap();
        let loaded = AssistantChats::load(temp.path(), key).unwrap();
        let chat = loaded.chat(AgentKind::Codex).unwrap();
        assert_eq!(chat.model, ModelChoice::Fastest);
        assert_eq!(chat.turns[0].end, TurnEnd::Running);
        let step =
            ChatStep { tool: "Read".into(), detail: "src/a.rs".into(), ..ChatStep::default() };
        assert_eq!(chat.turns[0].steps, [step], "a step without kind, duration or outcome");

        let thought = ChatStep {
            kind: "thinking".into(),
            duration_ms: Some(6_000),
            thinking: "The parser branches on the first byte.".into(),
            ..ChatStep::default()
        };
        let saved = serde_json::to_string(&thought).unwrap();
        assert_eq!(saved, r#"{"tool":"","detail":"","kind":"thinking","duration_ms":6000}"#);
    }

    /// Comment drafts of earlier versions neither show nor stay in the file.
    #[test]
    fn old_comment_drafts_are_dropped_on_load() {
        let temp = TestDir::new("assistant-chats-drafts");
        let key = "gh|acme/widgets|7";
        let file = temp.path().join(format!("{}.json", stable_hash(key)));
        let turns = r#"[
            {"kind":"Question","question":"Why?","answer":"Because."},
            {"kind":"Draft","question":"Draft a review comment from this answer","answer":"Nit."},
            {"kind":"Question","question":"And then?","answer":"Done."}
        ]"#;
        let json = format!(r#"{{"chats":[{{"agent":"Claude","turns":{turns}}}]}}"#);
        std::fs::write(&file, json).unwrap();
        let loaded = AssistantChats::load(temp.path(), key).unwrap();
        let questions: Vec<&str> = loaded
            .chat(AgentKind::Claude)
            .unwrap()
            .turns
            .iter()
            .map(|turn| turn.question.as_str())
            .collect();
        assert_eq!(questions, ["Why?", "And then?"]);
        loaded.snapshot().write().unwrap();
        let saved = std::fs::read_to_string(&file).unwrap();
        assert!(!saved.contains("Draft"), "{saved}");
    }

    fn answers(chats: &AssistantChats) -> Vec<&str> {
        let chat = chats.chat(AgentKind::Claude).unwrap();
        chat.turns.iter().map(|turn| turn.answer.as_str()).collect()
    }

    /// Writes landing out of order, also a closing page's last one landing after the next page's
    /// first, leave the file at the snapshot taken last.
    #[test]
    fn a_snapshot_never_replaces_a_later_one_on_disk() {
        let temp = TestDir::new("assistant-chats-order");
        let key = "gh|acme/widgets|7";
        let mut closing = AssistantChats::load(temp.path(), key).unwrap();
        let mut conversation = chat(AgentKind::Claude);
        conversation.turns[0].answer = "Partial".into();
        closing.put(conversation.clone());
        let partial = closing.snapshot();
        conversation.turns[0].answer = "Whole".into();
        closing.put(conversation.clone());
        let whole = closing.snapshot();
        let last_of_closing = closing.snapshot();
        let mut next = AssistantChats::load(temp.path(), key).unwrap();
        conversation.turns[0].answer = "Next page".into();
        next.put(conversation);
        let next_page = next.snapshot();

        whole.write().unwrap();
        partial.write().unwrap();
        let loaded = AssistantChats::load(temp.path(), key).unwrap();
        assert_eq!(answers(&loaded), ["Whole"], "the earlier snapshot landed last");
        next_page.write().unwrap();
        last_of_closing.write().unwrap();
        let loaded = AssistantChats::load(temp.path(), key).unwrap();
        assert_eq!(answers(&loaded), ["Next page"], "the closed page's write landed last");
    }
}
