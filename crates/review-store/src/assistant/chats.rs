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
use serde::{Deserialize, Deserializer, Serialize, de};

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
    #[serde(deserialize_with = "without_old_drafts")]
    pub turns: Vec<ChatTurn>,
}

/// What started a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TurnKind {
    /// The user's question.
    #[default]
    Question,
    /// The checkout moved to new commits and the agent was told.
    Update,
}

/// The stored turns without the comment drafts of earlier versions.
fn without_old_drafts<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ChatTurn>, D::Error> {
    Vec::<serde_json::Value>::deserialize(deserializer)?
        .into_iter()
        .filter(|turn| turn.get("kind").and_then(serde_json::Value::as_str) != Some("Draft"))
        .map(|turn| ChatTurn::deserialize(turn).map_err(de::Error::custom))
        .collect()
}

/// One question and its answer.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatTurn {
    pub kind: TurnKind,
    /// What the user asked, as shown.
    pub question: String,
    /// What was sent: the question with its quoted lines, or the update request.
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
    /// `None` in chats saved before kinds, and for a kind this version does not know.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::settings::or_default"
    )]
    pub kind: Option<ChatStepKind>,
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

/// What a step was, under the names the chat files store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatStepKind {
    Thinking,
    Read,
    Search,
    Run,
    Edit,
    Other,
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
    /// The conversations of `pull_request_key` (`connection|repository|number`); a missing file
    /// means none.
    pub fn load(dir: &Path, pull_request_key: &str) -> io::Result<Self> {
        let file = dir.join(format!("{}.json", stable_hash(pull_request_key)));
        let stored: StoredChats = atomic_file::read_json(&file)?.unwrap_or_default();
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
                    kind: Some(ChatStepKind::Read),
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
            kind: Some(ChatStepKind::Thinking),
            duration_ms: Some(6_000),
            thinking: "The parser branches on the first byte.".into(),
            ..ChatStep::default()
        };
        let saved = serde_json::to_string(&thought).unwrap();
        assert_eq!(saved, r#"{"tool":"","detail":"","kind":"thinking","duration_ms":6000}"#);
    }

    /// Every kind name a chat file holds loads, and a save writes it back unchanged; an empty, an
    /// unknown or a missing kind loads as none and is left out of the file.
    #[test]
    fn step_kinds_load_and_save_under_their_stored_names() {
        use ChatStepKind::{Edit, Other, Read, Run, Search, Thinking};
        let temp = TestDir::new("assistant-chats-kinds");
        let key = "gh|acme/widgets|7";
        let file = temp.path().join(format!("{}.json", stable_hash(key)));
        let named = ["thinking", "read", "search", "run", "edit", "other"];
        let steps: Vec<String> = named
            .iter()
            .map(|name| format!(r#"{{"tool":"","detail":"","kind":"{name}"}}"#))
            .chain([
                r#"{"tool":"","detail":"","kind":""}"#.to_owned(),
                r#"{"tool":"","detail":"","kind":"teleport"}"#.to_owned(),
                r#"{"tool":"","detail":""}"#.to_owned(),
            ])
            .collect();
        let json = format!(
            r#"{{"chats":[{{"agent":"Claude","turns":[{{"steps":[{}]}}]}}]}}"#,
            steps.join(",")
        );
        std::fs::write(&file, json).unwrap();

        let loaded = AssistantChats::load(temp.path(), key).unwrap();
        let kinds: Vec<Option<ChatStepKind>> = loaded.chat(AgentKind::Claude).unwrap().turns[0]
            .steps
            .iter()
            .map(|step| step.kind)
            .collect();
        let known = [Thinking, Read, Search, Run, Edit, Other].map(Some);
        assert_eq!(kinds, [known.as_slice(), &[None, None, None]].concat());

        loaded.snapshot().write().unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let written: Vec<Option<&str>> = saved["chats"][0]["turns"][0]["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|step| step.get("kind").map(|kind| kind.as_str().unwrap()))
            .collect();
        let expected: Vec<Option<&str>> =
            named.iter().copied().map(Some).chain([None, None, None]).collect();
        assert_eq!(written, expected);
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
