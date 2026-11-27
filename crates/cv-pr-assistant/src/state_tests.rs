//! The conversation's state as the agent's events and the user's actions move it, without a page:
//! the events go in as the runner hands them out, the actions through their functions.

use cv_runtime::route::PullRequestKey;
use cv_services::Services;
use cv_testkit::{ScratchDir, poll_until};
use review_agent::{AgentError, AgentEvent, AgentKind, Usage};
use review_store::{TurnEnd, TurnKind};

use super::checkout::{self, Checkout};
use super::runner::RunnerEvent;
use super::{Assistant, Block, actions, links, transcript};

const REPOSITORY: &str = "Acme/app";
const NUMBER: u64 = 7;

fn scene(name: &str) -> (ScratchDir, Services, Assistant) {
    let scratch = ScratchDir::new(name);
    let mut services =
        Services::new(egui::Context::default(), scratch.app_paths(), cv_providers::create_provider);
    let key = PullRequestKey {
        connection_id: "gh".into(),
        repository_key: REPOSITORY.into(),
        number: NUMBER,
    };
    let assistant = Assistant::new(&key, &mut services);
    (scratch, services, assistant)
}

/// Asks `question` and hands its prompt to the agent, as the drive does once a session is there.
fn ask_agent(assistant: &mut Assistant, question: &str) -> String {
    assert!(actions::ask(assistant, question, true));
    assistant.pending.take().expect("the question waits for the agent")
}

fn agent(events: Vec<AgentEvent>) -> Vec<RunnerEvent> {
    events.into_iter().map(RunnerEvent::Agent).collect()
}

/// A sign-in failure keeps the question for after the sign-in; the end of the failed turn that
/// comes with it belongs to the ended session and must not finish the kept question.
#[test]
fn a_question_kept_for_the_sign_in_outlives_the_end_of_its_failed_turn() {
    let (_scratch, mut services, mut assistant) = scene("assistant-sign-in-kept");
    let prompt = ask_agent(&mut assistant, "Why?");
    let failed = AgentEvent::Failed(AgentError::NotSignedIn(AgentKind::Claude));
    let events = agent(vec![failed, AgentEvent::TurnDone(Usage::default())]);
    assistant.take_in_all(events, &mut services);

    assert_eq!(assistant.block, Some(Block::NotSignedIn(AgentKind::Claude)));
    assert_eq!(assistant.chat.turns[0].end, TurnEnd::Running, "it waits for the sign-in");
    assert_eq!(assistant.pending, Some(prompt), "asked again once signed in");
}

/// The links checked for an answer belong to that answer: Retry starts a new one, which shows as
/// it streams, never as the old one with its links.
#[test]
fn a_retried_answer_never_shows_the_links_of_the_answer_before() {
    let (_scratch, mut services, mut assistant) = scene("assistant-retry-links");
    let dir = checkout::dir(&services.paths, REPOSITORY, NUMBER);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "fn greet() {}\n").unwrap();
    ask_agent(&mut assistant, "Where?");
    let events = agent(vec![
        AgentEvent::Text("See src/lib.rs:1 for".into()),
        AgentEvent::Failed(AgentError::Turn("cut off".into())),
        AgentEvent::TurnDone(Usage::default()),
    ]);
    assistant.take_in_all(events, &mut services);
    poll_until("the failed answer's citation's link", || {
        links::check_links(&mut assistant, &services);
        assistant.links.get(0, &assistant.chat.turns[0].answer).is_some()
    });

    actions::retry(&mut assistant);
    assistant.take_in_all(agent(vec![AgentEvent::Text("Nothing".into())]), &mut services);
    let turn = &assistant.chat.turns[0];
    assert_eq!(turn.answer, "Nothing");
    assert_eq!(assistant.links.get(0, &turn.answer), None, "the old answer stays gone");
}

/// "Update the agent" moves the checkout for the conversation that asked; another conversation
/// shown meanwhile, or this one shown again, keeps its own head, so its agent is never left
/// believing the checkout is at commits it was not told about.
#[test]
fn moving_to_new_commits_is_dropped_with_the_conversation_that_asked_for_it() {
    let (_scratch, mut services, mut assistant) = scene("assistant-move-dropped");
    let (old, new) = ("a".repeat(40), "b".repeat(40));
    let first = assistant.chat.agent;
    let second = AgentKind::ALL.into_iter().find(|kind| *kind != first).unwrap();
    let dir = checkout::dir(&services.paths, REPOSITORY, NUMBER);
    assistant.chat.head = Some(old.clone());
    // What "Update the agent" leaves while the checkout moves.
    let note = super::brief::update_note(&old, &new);
    transcript::begin(&mut assistant.chat, TurnKind::Update, "Moved", None, note.clone());
    assistant.move_to = Some(new.clone());
    assistant.pending = Some(note);

    assistant.save(&mut services);
    assistant.show_conversation(second, &services);
    let moved = Checkout { dir: dir.clone(), head: new.clone() };
    assistant.take_in(RunnerEvent::CheckoutReady(moved), &mut services);
    assert_eq!(assistant.chat.head.as_deref(), Some(new.as_str()), "a new conversation starts");
    assistant.save(&mut services);
    assistant.show_conversation(first, &services);
    // A checkout landing now (the next question's) does not move this conversation on.
    assistant.take_in(RunnerEvent::CheckoutReady(Checkout { dir, head: new }), &mut services);

    assert_eq!(assistant.chat.head, Some(old));
}

/// After the agent's process ended, Retry on the failed answer asks again at once: it restarts
/// the agent as Restart does instead of waiting behind the block.
#[test]
fn retry_after_the_agent_stopped_asks_again() {
    let (_scratch, mut services, mut assistant) = scene("assistant-retry-stopped");
    let prompt = ask_agent(&mut assistant, "Why?");
    let exited = AgentError::Exited { code: Some(1), stderr_tail: "crashed".into() };
    let events = agent(vec![AgentEvent::Failed(exited), AgentEvent::Closed]);
    assistant.take_in_all(events, &mut services);
    assert!(matches!(assistant.block, Some(Block::Stopped(_))));
    assert!(matches!(assistant.chat.turns[0].end, TurnEnd::Failed(_)));

    actions::retry(&mut assistant);

    assert_eq!(assistant.block, None);
    assert_eq!(assistant.pending, Some(prompt));
    assert_eq!(assistant.chat.turns[0].end, TurnEnd::Running);
}

/// Stopping "Update the agent" before the checkout moved drops the move with the note: the
/// checkout landing afterwards leaves the conversation at the head its agent knows.
#[test]
fn a_stopped_move_to_new_commits_keeps_the_head_the_agent_knows() {
    let (_scratch, mut services, mut assistant) = scene("assistant-move-stopped");
    let (old, new) = ("a".repeat(40), "b".repeat(40));
    let dir = checkout::dir(&services.paths, REPOSITORY, NUMBER);
    assistant.chat.head = Some(old.clone());
    let note = super::brief::update_note(&old, &new);
    transcript::begin(&mut assistant.chat, TurnKind::Update, "Moved", None, note.clone());
    assistant.move_to = Some(new.clone());
    assistant.pending = Some(note);

    actions::stop(&mut assistant);
    assistant.take_in(RunnerEvent::CheckoutReady(Checkout { dir, head: new }), &mut services);

    assert_eq!(assistant.chat.turns[0].end, TurnEnd::Stopped);
    assert_eq!(assistant.chat.head, Some(old));
}
