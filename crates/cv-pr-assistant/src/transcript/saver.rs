//! Writes the conversations to disk on the background, never on the UI thread. One write runs at
//! a time; changes that come in meanwhile wait as one, taken as a snapshot only when the running
//! write ends, so a burst of streamed deltas costs one write running and one waiting, and the file
//! ends at the latest state. A snapshot never replaces a later one on disk
//! ([`review_store::ChatsSnapshot::write`]), which keeps the last write a closing page hands off
//! ([`Saver::finish`]) in order with the next page's.

use std::io;

use cv_runtime::background::{Background, Spawner, Task, TaskPoll};
use review_store::AssistantChats;

pub(crate) struct Saver {
    writing: Option<Task<io::Result<()>>>,
    /// The conversations changed after the running write took its snapshot.
    is_dirty: bool,
    /// Runs the last write of a closing page, which has no frame left to take in its outcome.
    spawner: Spawner,
}

impl Saver {
    pub(crate) fn new(background: &Background) -> Self {
        Self { writing: None, is_dirty: false, spawner: background.spawner() }
    }

    /// The conversations changed: [`Self::poll`] writes them once no write runs.
    pub(crate) fn changed(&mut self) {
        self.is_dirty = true;
    }

    /// Takes in the write that finished and starts the next one when changes wait; returns the
    /// failure of the finished one. Runs every frame.
    pub(crate) fn poll(
        &mut self,
        chats: &AssistantChats,
        background: &Background,
    ) -> io::Result<()> {
        let finished = match self.writing.as_mut().map(Task::poll) {
            Some(TaskPoll::Pending) => return Ok(()),
            Some(TaskPoll::Done(outcome)) => outcome,
            Some(TaskPoll::Lost) => Err(io::Error::other("the write stopped before it ended")),
            None => Ok(()),
        };
        self.writing = None;
        if self.is_dirty {
            self.is_dirty = false;
            let snapshot = chats.snapshot();
            self.writing = Some(background.spawn(async move {
                tokio::task::spawn_blocking(move || snapshot.write())
                    .await
                    .unwrap_or_else(|panicked| Err(io::Error::other(panicked)))
            }));
        }
        finished
    }

    /// The page closes: the latest state goes to disk on the background, kept so that an app
    /// exiting right after still waits for it. A write still running would be cancelled with the
    /// app, so it counts as a change too. No one is left to report a failure to.
    pub(crate) fn finish(&mut self, chats: &AssistantChats) {
        if !self.is_dirty && self.writing.is_none() {
            return;
        }
        self.is_dirty = false;
        let snapshot = chats.snapshot();
        self.spawner.spawn_kept(async move {
            let _unreported = tokio::task::spawn_blocking(move || snapshot.write()).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use cv_runtime::background::Background;
    use cv_testkit::{ScratchDir, poll_until};
    use review_store::{AssistantChats, Chat, ChatTurn, chats_dir};

    use super::Saver;

    const KEY: &str = "gh|Acme/app|7";

    fn answered(answer: usize) -> Chat {
        let turn = ChatTurn { answer: answer.to_string(), ..ChatTurn::default() };
        Chat { turns: vec![turn], ..Chat::default() }
    }

    /// Polls until no write runs or waits; the failures the writes reported.
    fn settle(saver: &mut Saver, chats: &AssistantChats, background: &Background) -> usize {
        let mut failures = 0;
        poll_until("the writes' end", || {
            if saver.writing.is_none() && !saver.is_dirty {
                return true;
            }
            failures += usize::from(saver.poll(chats, background).is_err());
            false
        });
        failures
    }

    /// A burst of streamed deltas starts one write and keeps the rest as one waiting change; once
    /// they settle, the file holds the last delta.
    #[test]
    fn a_burst_of_changes_runs_one_write_and_ends_at_the_latest_state() {
        let scratch = ScratchDir::new("assistant-saver-burst");
        let paths = scratch.app_paths();
        let background = Background::new(egui::Context::default());
        let mut chats = AssistantChats::load(&chats_dir(&paths), KEY).unwrap();
        let mut saver = Saver::new(&background);

        for answer in 0..100 {
            chats.put(answered(answer));
            saver.changed();
            saver.poll(&chats, &background).unwrap();
        }
        assert!(saver.writing.is_some(), "the first change's write runs");
        assert!(saver.is_dirty, "the 99 changes after it wait as one");
        assert_eq!(settle(&mut saver, &chats, &background), 0);

        let loaded = AssistantChats::load(&chats_dir(&paths), KEY).unwrap();
        let chat = loaded.chat(answered(0).agent).unwrap();
        assert_eq!(chat.turns[0].answer, "99");
    }

    /// A failed write comes back once, for the error toast, and the next change writes again.
    #[test]
    fn a_failed_write_is_reported() {
        let scratch = ScratchDir::new("assistant-saver-failed");
        let paths = scratch.app_paths();
        let background = Background::new(egui::Context::default());
        let mut chats = AssistantChats::load(&chats_dir(&paths), KEY).unwrap();
        std::fs::create_dir_all(paths.cache_dir()).unwrap();
        // A file where the folder belongs: the write cannot create it.
        std::fs::write(chats_dir(&paths), b"").unwrap();
        let mut saver = Saver::new(&background);

        chats.put(answered(1));
        saver.changed();
        saver.poll(&chats, &background).unwrap();
        assert_eq!(settle(&mut saver, &chats, &background), 1);

        std::fs::remove_file(chats_dir(&paths)).unwrap();
        saver.changed();
        assert_eq!(settle(&mut saver, &chats, &background), 0);
        let loaded = AssistantChats::load(&chats_dir(&paths), KEY).unwrap();
        assert_eq!(loaded.current(), chats.current(), "written once the folder can be made");
    }

    /// A closing page's change that no write took yet still reaches the file.
    #[test]
    fn a_change_left_at_close_reaches_the_file() {
        let scratch = ScratchDir::new("assistant-saver-finish");
        let paths = scratch.app_paths();
        let background = Background::new(egui::Context::default());
        let mut chats = AssistantChats::load(&chats_dir(&paths), KEY).unwrap();
        let mut saver = Saver::new(&background);
        chats.put(answered(1));
        saver.changed();
        saver.poll(&chats, &background).unwrap();
        chats.put(answered(2));
        saver.changed();

        saver.finish(&chats);

        poll_until("the last change in the file", || {
            let loaded = AssistantChats::load(&chats_dir(&paths), KEY);
            let answer = loaded.ok().and_then(|loaded| {
                loaded.chat(answered(0).agent).map(|chat| chat.turns[0].answer.clone())
            });
            answer.as_deref() == Some("2")
        });
    }
}
