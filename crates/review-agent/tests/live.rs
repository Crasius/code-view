//! Checks against the real, signed-in CLIs on this machine. They spend a tiny turn of the owner's
//! subscription each and need the network, so they only run by hand:
//! `cargo test -p review-agent --test live -- --ignored --test-threads=1`.

use std::path::PathBuf;
use std::time::Duration;

use review_agent::{
    AgentEvent, AgentKind, ModelChoice, Session, SessionConfig, detect, list_models, resolve_model,
};

const WAIT: Duration = Duration::from_secs(180);

/// An empty folder outside any repository, so the agent reads nothing of ours.
fn empty_folder(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("review-agent-live-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create the folder");
    path
}

async fn one_turn(kind: AgentKind) {
    let cli = detect(kind).await.expect("the CLI is installed");
    let models = list_models(&cli).await.expect("the CLI lists its models");
    assert!(!models.is_empty(), "{kind:?} lists models");
    let pick = resolve_model(kind, &ModelChoice::Fastest, &models);
    let workdir = empty_folder(kind.label());
    let config = SessionConfig {
        cli,
        pick,
        workdir: workdir.clone(),
        brief: "Whatever the user writes, answer only with the single word BANANA.".to_owned(),
        resume: None,
    };
    let (session, mut events) = Session::start(config).expect("a runtime is running");
    session.ask("Say hello.");
    let mut seen = Vec::new();
    loop {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("an event in time")
            .expect("events until Closed");
        let done = matches!(event, AgentEvent::TurnDone(_) | AgentEvent::Closed);
        seen.push(event);
        if done {
            break;
        }
    }
    drop(session);
    // The process holds its working directory until it is gone.
    while !matches!(events.recv().await, Some(AgentEvent::Closed) | None) {}
    std::fs::remove_dir_all(&workdir).expect("the CLI let go of its folder");

    assert!(matches!(seen.first(), Some(AgentEvent::Started { .. })), "{seen:?}");
    let text: String = seen
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.contains("BANANA"), "the brief reached the model: {seen:?}");
    assert!(matches!(seen.last(), Some(AgentEvent::TurnDone(_))), "{seen:?}");
}

#[tokio::test]
#[ignore = "spends a turn of the owner's Claude subscription"]
async fn claude_answers_with_the_brief_on_the_fastest_model() {
    one_turn(AgentKind::Claude).await;
}

#[tokio::test]
#[ignore = "spends a turn of the owner's Codex subscription"]
async fn codex_answers_with_the_brief_on_the_fastest_model() {
    one_turn(AgentKind::Codex).await;
}
