//! The assistant's activity line end to end: the real app against the mocked GitHub host with a
//! scripted Claude ([`cv_services::agents::FakeAgents`]). A running turn shows "Working",
//! expanded, with its thinking and one row per tool call (the running one with a spinner); once
//! done it folds into the summary line, which a click or Enter opens and closes. A chat saved
//! before steps had kinds, durations and outcomes still shows its summary. Needs `git` on PATH,
//! no network, no real CLI.

use egui::{Key, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{AccessKitNode, NodeT, Queryable};
use review_agent::{AgentEvent, FakeAgent, StepKind, StepOutcome, ToolStep, Usage};
use review_store::chats_dir;
use serde_json::Value;

use super::ReviewApp;
use super::assistant_script::{ask, install_claude, wait_for_prompts, wait_for_session};
use super::github_mock::{FILE, GitHubHost, open_pull_request};
use super::walkthrough::{app, wait_until};
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;
use cv_widgets::buttons;

const QUESTION: &str = "Would a hunk without the marker still parse?";
const COMMAND: &str = "cargo test -p review-diff hunk";
const THINKING: &str = "The parser branches on the first byte.";

/// The agent's work on the running question: thinking, a read that finished, a command that runs.
fn work(agent: &FakeAgent) {
    let step = |id: &str, tool: &str, kind: StepKind, detail: &str| {
        let (id, tool, detail) = (id.to_owned(), tool.to_owned(), detail.to_owned());
        AgentEvent::Tool(ToolStep { id, tool, kind, detail })
    };
    agent.emit(AgentEvent::Started { session_id: "s-1".into(), model: "claude-haiku".into() });
    agent.emit(AgentEvent::Thinking(String::new()));
    agent.emit(AgentEvent::Thinking(THINKING.into()));
    agent.emit(AgentEvent::ThinkingDone);
    agent.emit(step("t1", "Read", StepKind::Read, FILE));
    agent.emit(AgentEvent::ToolDone { id: "t1".into(), outcome: StepOutcome::Ok });
    agent.emit(step("t2", "Bash", StepKind::Run, COMMAND));
}

/// The command fails, the answer comes, the turn ends.
fn finish(agent: &FakeAgent) {
    agent.emit(AgentEvent::ToolDone { id: "t2".into(), outcome: StepOutcome::Failed });
    agent.emit(AgentEvent::Text("Yes: the last line keeps its newline.".into()));
    agent.emit(AgentEvent::TurnDone(Usage { input: 10, output: 5, cache_read: 0, cache_write: 0 }));
}

/// A running turn: "Working" expanded, the thinking excerpt and both tool rows, the command's
/// with the spinner. Done: the summary line alone, closed; a click opens it with the failed
/// command, Enter on the focused line closes it again.
#[test]
fn shows_the_running_steps_then_folds_into_the_summary() {
    let scratch = ScratchDir::new("activity-running");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    install_claude(&harness, true);
    open_pull_request(&mut harness);
    ask(&mut harness, QUESTION);
    let (_, mut agent) = wait_for_session(&mut harness);
    wait_for_prompts(&mut harness, &mut agent);
    work(&agent);

    let read = format!("Read {FILE}");
    let running = format!("Running {COMMAND}");
    wait_until(&mut harness, "the working steps", |harness| {
        line(harness, "Working").is_some_and(|(_, open)| open == Some(true))
            && step(harness, &read).as_deref() == Some("done")
            && step(harness, &running).as_deref() == Some("running")
            && harness.query_by_label_contains("first byte").is_some()
    });
    let (rect, _) = line(&harness, "Working").expect("the line");
    assert!(rect.height() >= buttons::SUBTLE_HEIGHT, "the line is a full touch target");

    finish(&agent);
    let summary = "read 1 file · ran 1 command";
    wait_until(&mut harness, "the folded summary", |harness| {
        line(harness, "Thought").is_some_and(|(_, open)| open == Some(false))
            && label_of(harness, "Thought").is_some_and(|label| label.ends_with(summary))
            && step(harness, &read).is_none()
    });

    harness.click_at(harness.get_by(|node| is_line(node, "Thought")).rect());
    let ran = format!("Ran {COMMAND}");
    wait_until(&mut harness, "the opened steps", |harness| {
        line(harness, "Thought").is_some_and(|(_, open)| open == Some(true))
            && step(harness, &read).as_deref() == Some("done")
            && step(harness, &ran).as_deref() == Some("failed")
    });
    assert!(harness.query_by_label_contains("first byte").is_none(), "no thinking once done");

    harness.get_by(|node| is_line(node, "Thought")).focus();
    harness.step();
    harness.key_press(Key::Enter);
    wait_until(&mut harness, "the closed steps", |harness| {
        line(harness, "Thought").is_some_and(|(_, open)| open == Some(false))
    });
}

/// A chat saved before steps kept a kind, a duration or an outcome loads: its step counts by its
/// tool's name and shows as done.
#[test]
fn of_an_older_chat_shows_its_summary() {
    let scratch = ScratchDir::new("activity-older");
    let host = GitHubHost::with_pull_request(scratch.path());
    let dir = {
        let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
        install_claude(&harness, true);
        open_pull_request(&mut harness);
        ask(&mut harness, QUESTION);
        let (_, mut agent) = wait_for_session(&mut harness);
        wait_for_prompts(&mut harness, &mut agent);
        let read = ToolStep {
            id: "t1".into(),
            tool: "Read".into(),
            kind: StepKind::Read,
            detail: FILE.into(),
        };
        agent.emit(AgentEvent::Tool(read));
        agent.emit(AgentEvent::ToolDone { id: "t1".into(), outcome: StepOutcome::Failed });
        agent.emit(AgentEvent::TurnDone(Usage::default()));
        wait_until(&mut harness, "the finished turn", |harness| {
            line(harness, "Read 1 file").is_some()
        });
        chats_dir(&harness.state().services.paths)
    };
    strip_new_step_fields(&dir);

    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    wait_until(&mut harness, "the kept summary", |harness| {
        label_of(harness, "Read").as_deref() == Some("Read 1 file")
    });
    harness.click_at(harness.get_by(|node| is_line(node, "Read 1 file")).rect());
    let read = format!("Read {FILE}");
    wait_until(&mut harness, "the kept step", |harness| {
        step(harness, &read).as_deref() == Some("done")
    });
}

/// Rewrites every saved chat in `dir` as the app wrote it before steps had kinds, durations and
/// outcomes.
fn strip_new_step_fields(dir: &std::path::Path) {
    let files =
        std::fs::read_dir(dir).expect("the chats folder").map(|entry| entry.unwrap().path());
    for file in files {
        let mut stored: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        let chats = stored["chats"].as_array_mut().unwrap();
        for step in chats.iter_mut().flat_map(|chat| {
            chat["turns"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .flat_map(|turn| turn["steps"].as_array_mut().unwrap().iter_mut())
        }) {
            let step = step.as_object_mut().unwrap();
            for key in ["kind", "duration_ms", "is_failed"] {
                step.remove(key);
            }
            assert_eq!(step.len(), 2, "only tool and detail stay: {step:?}");
        }
        std::fs::write(&file, serde_json::to_vec(&stored).unwrap()).unwrap();
    }
}

fn is_line(node: &AccessKitNode<'_>, start: &str) -> bool {
    node.role() == Role::Button && node.label().is_some_and(|label| label.starts_with(start))
}

/// The activity line whose text starts with `start`: its rect and whether it is expanded.
fn line(harness: &Harness<'_, ReviewApp>, start: &str) -> Option<(egui::Rect, Option<bool>)> {
    let node = harness.query_by(|node| is_line(node, start))?;
    Some((node.rect(), node.accesskit_node().data().is_expanded()))
}

fn label_of(harness: &Harness<'_, ReviewApp>, start: &str) -> Option<String> {
    harness.query_by(|node| is_line(node, start)).and_then(|node| node.accesskit_node().label())
}

/// The status of the step row called `label` ("done", "running", "failed"), if it shows.
fn step(harness: &Harness<'_, ReviewApp>, label: &str) -> Option<String> {
    harness
        .query_by(|node| node.role() == Role::ListItem && node.label().as_deref() == Some(label))
        .and_then(|node| node.accesskit_node().description())
}
