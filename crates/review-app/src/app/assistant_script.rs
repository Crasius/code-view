//! The scripted assistant the assistant walkthroughs share: a Claude Code CLI that is installed
//! with two models ([`cv_services::agents::FakeAgents`] in place of the real CLI), the composer's
//! question, the waits for the started session and its prompts, and a plain answer.

use std::fmt::Write as _;

use egui::Key;
use egui_kittest::Harness;
use review_agent::{AgentEvent, AgentKind, FakeAgent, ModelInfo, SessionConfig, Usage};

use super::ReviewApp;
use super::walkthrough::{type_into, wait_for};
use cv_services::agents::FakeCli;

/// The empty composer's placeholder.
pub(super) const PLACEHOLDER: &str = "Ask anything about this pull request";

/// The models the scripted CLI lists: Haiku, then Sonnet.
pub(super) fn models() -> Vec<ModelInfo> {
    let model = |id: &str, name: &str| ModelInfo {
        id: id.to_owned(),
        display_name: name.to_owned(),
        description: String::new(),
        is_default: false,
        efforts: Vec::new(),
    };
    vec![model("claude-haiku", "Haiku"), model("claude-sonnet", "Sonnet")]
}

/// Claude Code is installed with [`models`], signed in or not.
pub(super) fn install_claude(harness: &Harness<'_, ReviewApp>, is_signed_in: bool) {
    let cli = FakeCli::Installed { models: models(), is_signed_in };
    harness.state().services.agents.fake.set(AgentKind::Claude, cli);
}

/// Types `question` into the empty composer and sends it with Enter.
pub(super) fn ask(harness: &mut Harness<'_, ReviewApp>, question: &str) {
    type_into(harness, PLACEHOLDER, question);
    harness.key_press(Key::Enter);
    harness.step();
}

/// The session the question started, with its agent.
pub(super) fn wait_for_session(harness: &mut Harness<'_, ReviewApp>) -> (SessionConfig, FakeAgent) {
    let take =
        |harness: &Harness<'_, ReviewApp>| harness.state().services.agents.fake.take_session();
    wait_for(harness, "a session", take, |_| {})
}

/// The prompts that reached `agent` since the last call, once there is one.
pub(super) fn wait_for_prompts(
    harness: &mut Harness<'_, ReviewApp>,
    agent: &mut FakeAgent,
) -> Vec<Option<String>> {
    let take =
        |_: &Harness<'_, ReviewApp>| Some(agent.take_prompts()).filter(|asked| !asked.is_empty());
    wait_for(harness, "the prompt", take, |_| {})
}

/// The agent's whole answer to the running question: `text`, and the turn's end.
pub(super) fn answer_text(agent: &FakeAgent, text: &str) {
    agent.emit(AgentEvent::Started { session_id: "s-1".into(), model: "claude-haiku".into() });
    agent.emit(AgentEvent::Text(text.to_owned()));
    agent.emit(AgentEvent::TurnDone(Usage::default()));
}

/// The 120 lines of a long file; the head changes line 2, so lines far below sit in a collapsed
/// gap of the diff.
pub(super) fn long_file(head: bool) -> Vec<u8> {
    (1..=120)
        .fold(String::new(), |mut text, line| {
            let word = if head && line == 2 { "Changed" } else { "Line" };
            writeln!(text, "/// {word} {line} of the long file.").unwrap();
            text
        })
        .into_bytes()
}
