//! How the answers' citation links show and open. Around the page's own resolver (pull request
//! links, shorthand references) the assistant adds its citation targets: a file the pull request
//! changes opens in the Files tab at the line, any other file of the checkout on the platform at
//! the head commit. Ctrl+click opens the platform's page for both.

use std::collections::HashSet;
use std::sync::Arc;

use cv_markdown::{self as markdown, InApp, LinkResolver, ResolvedLink};
use cv_pr_session::session::PullRequestSession;
use cv_pr_session::web_link;
use cv_runtime::background::{Task, TaskPoll};
use cv_services::Services;
use review_core::ProviderKind;
use review_git::ChangedFile;
use review_store::TurnEnd;

use crate::{Assistant, checkout, citations};

/// The changed file list the paths were made from (kept, so its address cannot be reused by
/// another list), and the paths.
type ChangedPaths = (Option<Arc<Vec<ChangedFile>>>, Arc<HashSet<String>>);

/// The changed paths, kept for the file list they were made from.
#[derive(Default)]
pub struct Links {
    changed: Option<ChangedPaths>,
}

impl Links {
    /// Makes the answers drawn after this call resolve citation targets, and [`Self::is_changed`]
    /// read the pull request's current file list.
    pub fn publish(&mut self, ctx: &egui::Context, session: &PullRequestSession) {
        let changed = self.changed_paths(session.files());
        let Some(detail) = session.detail.ready() else {
            return;
        };
        let resolver = CitationLinks {
            inner: markdown::current_links(ctx),
            changed,
            kind: session.kind,
            pull_request_url: detail.summary.web_url.clone(),
            head: detail.head_sha.clone(),
        };
        markdown::publish_links(ctx, Arc::new(resolver));
    }

    /// The paths of `files`, made once per file list.
    fn changed_paths(&mut self, files: Option<&Arc<Vec<ChangedFile>>>) -> Arc<HashSet<String>> {
        if let Some((kept, paths)) = &self.changed {
            let is_same = match (kept, files) {
                (None, None) => true,
                (Some(kept), Some(files)) => Arc::ptr_eq(kept, files),
                _ => false,
            };
            if is_same {
                return paths.clone();
            }
        }
        let paths: HashSet<String> = files
            .into_iter()
            .flat_map(|files| files.iter().map(|file| file.path.clone()))
            .collect();
        let paths = Arc::new(paths);
        self.changed = Some((files.cloned(), paths.clone()));
        paths
    }

    /// Whether the pull request changes `path`, as of the last [`Self::publish`].
    pub(crate) fn is_changed(&self, path: &str) -> bool {
        self.changed.as_ref().is_some_and(|(_, paths)| paths.contains(path))
    }
}

struct CitationLinks {
    inner: Option<Arc<dyn LinkResolver>>,
    changed: Arc<HashSet<String>>,
    kind: ProviderKind,
    pull_request_url: String,
    head: String,
}

impl LinkResolver for CitationLinks {
    fn resolve(&self, target: &str) -> Option<ResolvedLink> {
        let Some((path, start, end)) = citations::parse_target(target) else {
            return self.inner.as_ref().and_then(|inner| inner.resolve(target));
        };
        let web_url =
            web_link::line_url(self.kind, &self.pull_request_url, &path, &self.head, start)
                .unwrap_or_else(|| self.pull_request_url.clone());
        let lines =
            if start == end { format!("line {start}") } else { format!("lines {start}–{end}") };
        let (in_app, title) = if self.changed.contains(&path) {
            (InApp::Yes, format!("{path}, {lines}: open in the Files tab"))
        } else {
            (InApp::No, format!("{path}, {lines}: not changed here, opens {}", self.kind.label()))
        };
        Some(ResolvedLink { web_url, in_app, icon: None, title: Some(title) })
    }
}

/// Answers whose citations were checked: Markdown with the citations that exist linked, kept
/// with the answer they were made from. A turn's answer changes when Retry asks again; the old
/// links then no longer show, even when their check finishes after the retry.
#[derive(Default)]
pub struct LinkedAnswers {
    by_turn: Vec<Option<Linked>>,
    pub to_check: Vec<usize>,
    checking: Option<Task<Vec<(usize, Linked)>>>,
}

/// One answer as it was checked, and its Markdown with the existing citations linked.
struct Linked {
    answer: String,
    text: String,
}

impl LinkedAnswers {
    /// Turn `index`'s `answer` with its checked citations linked, once they are.
    pub fn get(&self, index: usize, answer: &str) -> Option<&str> {
        let linked = self.by_turn.get(index)?.as_ref()?;
        (linked.answer == answer).then_some(linked.text.as_str())
    }
}

/// Checks the citations of finished answers against the checkout, one batch at a time.
pub fn check_links(assistant: &mut Assistant, services: &Services) {
    if let Some(task) = &mut assistant.links.checking {
        match task.poll() {
            TaskPoll::Pending => return,
            TaskPoll::Done(linked) => {
                let by_turn = &mut assistant.links.by_turn;
                for (index, answer) in linked {
                    by_turn.resize_with(by_turn.len().max(index + 1), || None);
                    if let Some(turn) = by_turn.get_mut(index) {
                        *turn = Some(answer);
                    }
                }
            }
            TaskPoll::Lost => {}
        }
        assistant.links.checking = None;
    }
    if assistant.links.to_check.is_empty() {
        return;
    }
    let answers: Vec<(usize, String)> = std::mem::take(&mut assistant.links.to_check)
        .into_iter()
        .filter_map(|index| {
            let turn = assistant.chat.turns.get(index)?;
            let is_answered = turn.end != TurnEnd::Running && !turn.answer.is_empty();
            is_answered.then(|| (index, turn.answer.clone()))
        })
        .collect();
    let dir = checkout::dir(&services.paths, &assistant.key.repository_key, assistant.key.number);
    let task = services.background.spawn(async move { link_answers(&dir, answers) });
    assistant.links.checking = Some(task);
}

/// The answers whose citations name lines of `checkout`, with those citations linked.
fn link_answers(checkout: &std::path::Path, answers: Vec<(usize, String)>) -> Vec<(usize, Linked)> {
    if !checkout.is_dir() {
        return Vec::new();
    }
    answers
        .into_iter()
        .filter_map(|(index, answer)| {
            let mut found = citations::find(&answer);
            found.retain(|citation| {
                citations::is_in_checkout(checkout, &citation.path, citation.start)
            });
            if found.is_empty() {
                return None;
            }
            let text = citations::link(&answer, &found);
            Some((index, Linked { answer, text }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use cv_testkit::ScratchDir;
    use review_git::ChangeKind;

    use super::*;

    /// A new file list gets its own paths, even when it sits where a dropped list was: the paths
    /// were once kept by the list's address alone, which a later list can reuse.
    #[test]
    fn a_new_file_list_never_gets_the_paths_of_a_dropped_one() {
        let mut links = Links::default();
        let first = Arc::new(vec![ChangedFile::sample("src/old.rs", ChangeKind::Modified)]);
        assert!(links.changed_paths(Some(&first)).contains("src/old.rs"));
        drop(first);
        let second = Arc::new(vec![ChangedFile::sample("src/new.rs", ChangeKind::Added)]);

        let paths = links.changed_paths(Some(&second));

        assert!(paths.contains("src/new.rs"));
        assert!(!paths.contains("src/old.rs"));
        assert!(links.is_changed("src/new.rs"));
    }

    #[test]
    fn answers_get_links_for_the_lines_the_checkout_has() {
        let scratch = ScratchDir::new("assistant-link-answers");
        std::fs::create_dir_all(scratch.path().join("src")).expect("folder");
        std::fs::write(scratch.path().join("src/lib.rs"), "fn a() {}\nfn b() {}\n").expect("file");
        let answers = vec![
            (0, "Here:\n\nsrc/lib.rs:1".to_owned()),
            (1, "Not there: src/lib.rs:9".to_owned()),
        ];
        let linked: Vec<(usize, String)> = link_answers(scratch.path(), answers)
            .into_iter()
            .map(|(index, linked)| (index, linked.text))
            .collect();
        let expected = "Here:\n\n[src/lib.rs:1](<codeview-cite:src/lib.rs#1-1>)".to_owned();
        assert_eq!(linked, [(0, expected)]);
    }
}
