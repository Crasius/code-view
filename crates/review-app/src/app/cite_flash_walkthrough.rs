//! Lines the assistant cited flash in the Files tab once they are on screen, end to end: the real
//! app against the mocked GitHub host ([`super::github_mock`]) with a scripted agent. A click on a
//! citation opens the file, shows the cited lines from their collapsed gap, scrolls to them and
//! only then flashes them, on the cited side alone in side by side; the flash fades and ends on
//! its own and stops asking for frames. Needs `git` on PATH, no network, no real CLI.

use std::fmt::Write as _;

use egui::{Key, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_agent::{AgentEvent, AgentKind, FakeAgent, ModelInfo, Usage};
use review_store::DiffLayout;

use super::ReviewApp;
use super::action_e2e::{app, open_pull_request, type_into, wait_for, wait_for_link};
use super::github_mock::{ExtraFile, GitHubHost};
use cv_code::diff_paint::{flash_asks_for_frames, flashed_lines};
use cv_code::line_source::Pane;
use cv_services::agents::FakeCli;
use cv_testkit::ScratchDir;

const PLACEHOLDER: &str = "Ask anything about this pull request";
/// A file whose only change is on line 2: the cited lines sit in the collapsed gap far below.
const LONG: &str = "src/long.rs";
const LINES: usize = 120;

/// Line `line` of the long file; the head changes line 2.
fn long_file(head: bool) -> Vec<u8> {
    (1..=LINES)
        .fold(String::new(), |mut text, line| {
            let word = if head && line == 2 { "Changed" } else { "Line" };
            writeln!(text, "/// {word} {line} of the long file.").unwrap();
            text
        })
        .into_bytes()
}

/// Inline: the three cited lines flash in the only pane, once they are on screen.
#[test]
fn cited_lines_flash_once_they_arrive_inline() {
    let lines = cite_and_flash(DiffLayout::Inline, "60-62");
    assert_eq!(lines, [60, 61, 62]);
}

/// Side by side: only the head pane's cited range flashes.
#[test]
fn a_cited_range_flashes_on_its_side_in_split() {
    let lines = cite_and_flash(DiffLayout::SideBySide, "80-84");
    assert_eq!(lines, [80, 81, 82, 83, 84]);
}

/// Opens the pull request in `layout`, has the agent cite `src/long.rs:{range}`, clicks the
/// citation and follows the flash frame by frame to its end. Returns the head lines that flashed
/// when they arrived.
fn cite_and_flash(layout: DiffLayout, range: &str) -> Vec<u32> {
    let scratch = ScratchDir::new(&format!("cite-flash-{range}"));
    let long = ExtraFile { path: LONG, base: long_file(false), head: long_file(true) };
    let host = GitHubHost::with_pull_request_files(scratch.path(), &[long]);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    let cli = FakeCli::Installed { models: models(), is_signed_in: true };
    harness.state().services.agents.fake.set(AgentKind::Claude, cli);
    harness.state_mut().services.settings.diff_layout = layout;
    open_pull_request(&mut harness);

    type_into(&mut harness, PLACEHOLDER, "Where are the docs?");
    harness.key_press(Key::Enter);
    harness.step();
    let citation = format!("{LONG}:{range}");
    let mut agent = wait_for(
        &mut harness,
        "a session",
        |harness| harness.state().services.agents.fake.take_session().map(|(_, agent)| agent),
        |_| {},
    );
    wait_for(
        &mut harness,
        "the prompt",
        |_| Some(agent.take_prompts()).filter(|asked| !asked.is_empty()),
        |_| {},
    );
    answer(&agent, &format!("Here:\n\n{citation}"));
    wait_for_link(&mut harness, &citation);
    // The answer's layout settles before the click.
    for _ in 0..10 {
        harness.step();
    }
    assert!(flashed_lines(&harness.ctx).is_empty(), "nothing flashes before the click");

    // Frame by frame: the flash starts on the frame the lines are painted, at full strength.
    let center = harness.get_by_label(&citation).rect().center();
    primary_click(&mut harness, center);
    let diff = format!("Diff of {LONG}");
    let arrived = wait_for(
        &mut harness,
        "the flash",
        |harness| {
            let painted = flashed_lines(&harness.ctx);
            (!painted.is_empty()).then_some(painted)
        },
        |_| {},
    );
    assert!(harness.query_by_label(&diff).is_some(), "the Files tab shows the cited file");
    let pane = arrived[0].0;
    assert!(
        arrived.iter().all(
            |(painted, _, strength)| *painted == pane && (*strength - 1.0).abs() < f32::EPSILON
        ),
        "one pane, at full strength on arrival: {arrived:?}"
    );
    let expected_pane = match layout {
        DiffLayout::SideBySide => Pane::Right,
        DiffLayout::Inline | DiffLayout::NewOnly => Pane::Inline,
    };
    assert_eq!(pane, expected_pane, "the head side's pane");

    // It fades, then ends without asking for more frames.
    let mut last = 1.0;
    let mut frames = 0;
    loop {
        harness.step();
        frames += 1;
        let painted = flashed_lines(&harness.ctx);
        let Some(strength) = painted.first().map(|(_, _, strength)| *strength) else {
            break;
        };
        assert!(strength <= last, "the flash never brightens: {strength} after {last}");
        assert!(flash_asks_for_frames(&harness.ctx), "a running flash asks for the next frame");
        last = strength;
        assert!(frames < 20, "the flash ends");
    }
    assert!(last < 1.0, "it faded before it ended");
    for _ in 0..3 {
        harness.step();
        assert!(flashed_lines(&harness.ctx).is_empty(), "the flash stays gone");
    }
    assert!(!flash_asks_for_frames(&harness.ctx), "the ended flash asks for no more frames");
    arrived.iter().map(|(_, line, _)| *line).collect()
}

fn models() -> Vec<ModelInfo> {
    vec![ModelInfo {
        id: "claude-haiku".into(),
        display_name: "Haiku".into(),
        description: String::new(),
        is_default: false,
        efforts: Vec::new(),
    }]
}

/// The agent's whole answer to the running question: `text`, and the turn's end.
fn answer(agent: &FakeAgent, text: &str) {
    agent.emit(AgentEvent::Started { session_id: "s-1".into(), model: "claude-haiku".into() });
    agent.emit(AgentEvent::Text(text.to_owned()));
    agent.emit(AgentEvent::TurnDone(Usage::default()));
}

fn primary_click(harness: &mut Harness<'_, ReviewApp>, position: Pos2) {
    harness.hover_at(position);
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: position,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        harness.step();
    }
}
