//! The pull request assistant end to end: the real app against the mocked GitHub host
//! ([`super::github_mock`]) with scripted agents ([`cv_services::agents::FakeAgents`]) in place
//! of the Claude Code and Codex CLIs. A question streams its answer with a tool step, a cited line
//! opens in the Files tab, Alt+A on selected diff lines quotes them, a missing or signed-out CLI
//! shows its fix, the default model set in Settings starts the next conversation, the conversation
//! survives a restart, the expanded panel keeps the side column and its composer at the bottom,
//! the card fills the side column until its splitter sets the conversation's height (the window's,
//! not what the sliding header lends), the card's
//! conversation takes the mouse wheel only after a click into it, the assistant turned off in
//! Settings leaves the page and starts nothing, a new conversation takes one click and can be
//! undone, the question being written survives switching agent and model, and the mini chat on an
//! earlier answer waits for the running one. Needs `git` on PATH, no network, no real CLI.

use egui::{Key, Modifiers, PointerButton, Pos2, Role, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_agent::{
    AgentEvent, AgentKind, FakeAgent, ModelChoice, ModelInfo, SessionConfig, StepKind, ToolStep,
    Usage,
};
use review_store::AppSettings;

use super::ReviewApp;
use super::github_mock::{FILE, GitHubHost, TITLE, open_diff, open_pull_request};
use super::walkthrough::{
    app, click_now, press, shows_file_view, type_into, wait_for, wait_until, wheel,
};
use cv_pr_assistant::CARD_LABEL;
use cv_pr_assistant::mini::{self, ANSWERING, FOLLOW_UP};
use cv_runtime::route::Route;
use cv_services::agents::{FakeCli, FakeTerminal};
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;
use cv_widgets::row;

const CARD: &str = "Ask about this PR";
/// The question asked in the plain composer; Enter sends it.
const QUESTION: &str = "What does this PR change?";
/// The citation stands alone in its paragraph, so a click on the paragraph's middle hits it.
const ANSWER: &str = "It renames `greet` to `welcome`, here:

src/lib.rs:1";
const CITATION: &str = "src/lib.rs:1";

fn models() -> Vec<ModelInfo> {
    let model = |id: &str, name: &str| ModelInfo {
        id: id.to_owned(),
        display_name: name.to_owned(),
        description: String::new(),
        is_default: false,
        efforts: Vec::new(),
    };
    vec![model("claude-haiku", "Haiku"), model("claude-sonnet", "Sonnet")]
}

fn set_claude(harness: &Harness<'_, ReviewApp>, is_signed_in: bool) {
    let cli = FakeCli::Installed { models: models(), is_signed_in };
    harness.state().services.agents.fake.set(AgentKind::Claude, cli);
}

/// A question typed into the composer (Shift+Enter for a second line, Enter to send) starts the
/// agent in a checkout with the pull request's brief; its answer streams in with the tool step and
/// the token counts, and the cited line opens in the Files tab.
#[test]
fn assistant_answers_a_typed_question_and_a_citation_opens_the_line() {
    let scratch = ScratchDir::new("assistant-answer");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    set_claude(&harness, true);
    open_pull_request(&mut harness);
    wait_until(&mut harness, "the assistant's card", |harness| {
        harness.query_by_label(CARD).is_some()
    });
    type_into(&mut harness, PLACEHOLDER, QUESTION);
    harness.key_press_modifiers(Modifiers::SHIFT, Key::Enter);
    harness.step();
    harness
        .get_by(|node| {
            node.role() == Role::MultilineTextInput
                && node.value().is_some_and(|text| text.starts_with(QUESTION))
        })
        .type_text("Briefly.");
    harness.step();
    harness.key_press(Key::Enter);
    harness.step();

    let (config, mut agent) = wait_for_session(&mut harness);
    assert!(config.workdir.join(FILE).is_file(), "the agent reads a checkout of the head");
    assert!(config.brief.contains(TITLE), "the brief names the pull request: {}", config.brief);
    assert_eq!(config.resume, None);
    assert_eq!(wait_for_prompts(&mut harness, &mut agent), [Some(format!("{QUESTION}\nBriefly."))]);
    answer(&agent, ANSWER);

    wait_until(&mut harness, "the streamed answer", |harness| {
        harness.query_by_label_contains("Read 1 file").is_some()
            && harness.query_by_label_contains("12k cached").is_some()
    });
    assert_centered(&harness, "Question 1", QUESTION);
    // Checked against the checkout, the citation becomes a link into the Files tab.
    let center = harness.query_all_by_label(CITATION).next().expect("the citation").rect().center();
    wait_for(
        &mut harness,
        "the cited file",
        |harness| shows_file_view(harness, FILE).then_some(()),
        |harness| click_now(harness, center),
    );
}

/// A finished answer ends with its token counts, no buttons under it; the conversation is still
/// there after a restart.
#[test]
fn assistant_conversation_survives_a_restart() {
    let scratch = ScratchDir::new("assistant-restart");
    let host = GitHubHost::with_pull_request(scratch.path());
    {
        let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
        set_claude(&harness, true);
        open_pull_request(&mut harness);
        ask(&mut harness, QUESTION);
        let (_, mut agent) = wait_for_session(&mut harness);
        wait_for_prompts(&mut harness, &mut agent);
        answer(&agent, ANSWER);
        wait_until(&mut harness, "the answer", |harness| {
            harness.query_by_label_contains("12k cached").is_some()
        });
        for gone in ["Draft a comment", "Copy"] {
            let button = harness
                .query_all_by_role(Role::Button)
                .find(|node| node.accesskit_node().label().as_deref() == Some(gone));
            assert!(button.is_none(), "an answer offers {gone}");
        }
    }

    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    wait_until(&mut harness, "the kept conversation", |harness| {
        harness.query_all_by_label(QUESTION).next().is_some()
            && harness.query_all_by_label_contains("It renames greet").next().is_some()
    });
}

/// Alt+A on lines selected in the diff opens the assistant with them attached; the question names
/// them for the agent.
#[test]
fn assistant_alt_a_quotes_the_selected_lines() {
    let scratch = ScratchDir::new("assistant-quote");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    set_claude(&harness, true);
    open_pull_request(&mut harness);
    let diff = open_diff(&mut harness);

    // The first line number of the right (head) pane picks line 1.
    click_now(&mut harness, pos2(diff.center().x + 12.0, diff.top() + 4.0));
    harness.key_press_modifiers(Modifiers::ALT, Key::A);
    harness.step();
    let attached = format!("{FILE}, line 1 attached");
    wait_until(&mut harness, "the attached line", |harness| {
        harness.query_by_label_contains(&attached).is_some()
    });
    assert_centered(&harness, "Attached lines", &attached);
    type_into(&mut harness, PLACEHOLDER, "Why the new name?");
    // One line of text sits centered in the composer, with room above and below.
    let composer = harness
        .get_by(|node| {
            node.role() == Role::Group && node.label().as_deref() == Some("Question composer")
        })
        .rect();
    let input = harness
        .get_by(|node| {
            node.role() == Role::MultilineTextInput
                && node.value().as_deref() == Some("Why the new name?")
        })
        .rect();
    row::assert_centered_in(composer, input);
    let ask = harness.get_by_role_and_label(Role::Button, "Ask").rect().center();
    click_now(&mut harness, ask);

    let (_, mut agent) = wait_for_session(&mut harness);
    let prompts = wait_for_prompts(&mut harness, &mut agent);
    let prompt = prompts.first().cloned().flatten().unwrap_or_default();
    assert!(prompt.starts_with(&format!("About {FILE}, line 1 of the head version:")), "{prompt}");
    assert!(prompt.ends_with("Why the new name?"), "{prompt}");
}

/// Without the CLI the card links its install guide; installed but signed out it offers a terminal
/// to sign in; each "Check again" goes on with the waiting question.
#[test]
fn assistant_missing_and_signed_out_clis_show_their_fix() {
    let scratch = ScratchDir::new("assistant-states");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    ask(&mut harness, QUESTION);
    wait_until(&mut harness, "the install guide", |harness| {
        harness.query_by_label_contains("Install Claude Code").is_some()
            && harness.query_by_label_contains("Check again").is_some()
    });

    set_claude(&harness, false);
    press(&mut harness, "Check again");
    wait_until(&mut harness, "the sign-in fix", |harness| {
        harness.query_by_label_contains("Sign in in a terminal").is_some()
    });

    set_claude(&harness, true);
    press(&mut harness, "Check again");
    let (_, mut agent) = wait_for_session(&mut harness);
    assert_eq!(wait_for_prompts(&mut harness, &mut agent), [Some(QUESTION.to_owned())]);
}

/// The default model picked in Settings is saved and starts the next conversation.
#[test]
fn assistant_default_model_from_settings_starts_the_next_conversation() {
    let scratch = ScratchDir::new("assistant-model");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    set_claude(&harness, true);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Comma);
    press(&mut harness, "Assistant");
    wait_until(&mut harness, "the models of Claude Code", |harness| {
        harness.query_by_label_contains("Installed, 9.9.9").is_some()
    });
    let target =
        harness.query_all_by_role(Role::ComboBox).next().expect("the Claude Code model").rect();
    harness.click_at(target);
    harness.step();
    press(&mut harness, "Sonnet");
    let expected = ModelChoice::Named("claude-sonnet".to_owned());
    assert_eq!(harness.state().services.settings.assistant.claude_model, expected);
    let paths = scratch.app_paths();
    let saved = AppSettings::load(&paths.settings_file()).expect("settings file");
    assert_eq!(saved.assistant.claude_model, expected, "saved");

    harness.state_mut().navigator.go(Route::MyPullRequests);
    open_pull_request(&mut harness);
    ask(&mut harness, QUESTION);
    let (config, _) = wait_for_session(&mut harness);
    assert_eq!(config.pick.model.as_deref(), Some("claude-sonnet"));
}

/// "Continue in terminal" sits on the title line of card and panel. Before the first question it
/// is disabled and says what to do; once the checkout is there it opens the CLI fresh in it, after
/// the first answer it resumes the session. Tests only record the terminal.
#[test]
fn assistant_terminal_button_opens_the_cli_in_the_checkout() {
    let scratch = ScratchDir::new("assistant-terminal");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    set_claude(&harness, true);
    open_pull_request(&mut harness);
    // The chip names the model once the CLI's models are in; the card is laid out then.
    wait_until(&mut harness, "the terminal button and the chip", |harness| {
        harness.query_by_label(TERMINAL).is_some()
            && harness.query_by_label_contains("haiku").is_some()
    });
    harness.run_steps(10);
    let button = harness.get_by_label(TERMINAL);
    assert!(button.accesskit_node().is_disabled(), "no checkout yet");
    button.hover();
    let hint = "Ask a first question to prepare the checkout";
    wait_until(&mut harness, "the disabled button's hint", |harness| {
        harness.query_by_label(hint).is_some()
    });

    ask(&mut harness, QUESTION);
    let (config, mut agent) = wait_for_session(&mut harness);
    wait_for_prompts(&mut harness, &mut agent);
    let fake = harness.state().services.agents.fake.clone();
    press_terminal(&mut harness);
    let fresh =
        FakeTerminal { dir: config.workdir.clone(), program: "claude".into(), args: vec![] };
    assert_eq!(fake.take_terminals(), [fresh], "a fresh session in the checkout");

    answer(&agent, ANSWER);
    wait_until(&mut harness, "the answer", |harness| {
        harness.query_by_label_contains("12k cached").is_some()
    });
    press_terminal(&mut harness);
    let resumed = FakeTerminal {
        dir: config.workdir,
        program: "claude".into(),
        args: vec!["--resume".into(), "s-1".into()],
    };
    assert_eq!(fake.take_terminals(), std::slice::from_ref(&resumed), "the conversation's session");

    harness.key_press_modifiers(Modifiers::ALT, Key::A);
    wait_until(&mut harness, "the panel", |harness| has_group(harness, PANEL));
    press_terminal(&mut harness);
    assert_eq!(fake.take_terminals(), [resumed], "the panel's button");
}

const TERMINAL: &str = "Continue in terminal";

/// Clicks the terminal button (its tooltip shares the label).
fn press_terminal(harness: &mut Harness<'_, ReviewApp>) {
    harness.click_at(harness.get_by_role_and_label(Role::Button, TERMINAL).rect());
    harness.step();
}

/// The empty composer's placeholder.
const PLACEHOLDER: &str = "Ask anything about this pull request";

/// Types `question` into the empty composer and sends it with Enter.
fn ask(harness: &mut Harness<'_, ReviewApp>, question: &str) {
    type_into(harness, PLACEHOLDER, question);
    harness.key_press(Key::Enter);
    harness.step();
}

/// Expanding keeps the side column: the panel takes the card's column edges, reaches from the
/// column's top to the window's visible bottom, and the other side cards leave until it collapses
/// again. Its composer stays on the bottom edge, empty and under a long conversation; scrolled up,
/// "Jump to latest" leads back down. The side column's splitter sizes the panel too, and that
/// width survives a restart.
#[test]
fn assistant_panel_keeps_the_side_column_and_the_composer_at_the_bottom() {
    let scratch = ScratchDir::new("assistant-panel");
    let host = GitHubHost::with_pull_request(scratch.path());
    let width = {
        let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
        set_claude(&harness, true);
        open_pull_request(&mut harness);
        wait_until(&mut harness, "the card", |harness| {
            harness.query_by_label(SIDE_SPLITTER).is_some()
                && harness.query_by_label(CARD_SPLITTER).is_some()
        });
        let card = group(&harness, "Question composer").x_range();
        let boundary = harness.get_by_label(SIDE_SPLITTER).rect();
        assert!(has_side_cards(&harness), "the side cards before expanding");

        harness.key_press_modifiers(Modifiers::ALT, Key::A);
        wait_until(&mut harness, "the panel", |harness| has_group(harness, PANEL));
        let shown = panel(&harness);
        let composer = group(&harness, "Question composer").x_range();
        assert!(
            (composer.min - card.min).abs() <= 1.0 && (composer.max - card.max).abs() <= 1.0,
            "the panel's composer spans {composer:?}, the card's {card:?}"
        );
        let now = harness.get_by_label(SIDE_SPLITTER).rect();
        assert!((now.center().x - boundary.center().x).abs() <= 1.0, "the boundary stays");
        assert!((now.top() - boundary.top()).abs() <= 1.0, "the panel starts at the column top");
        assert!((shown.top() - boundary.top()).abs() <= 1.0, "panel top {shown:?}");
        assert!((shown.bottom() - (900.0 - 16.0)).abs() <= 1.0, "panel bottom {shown:?}");
        assert!(!has_side_cards(&harness), "the other side cards leave");
        let gap = composer_gap(&harness);
        assert!(gap <= 24.0, "the composer ends {gap} above the panel's bottom");

        ask(&mut harness, QUESTION);
        let (_, mut agent) = wait_for_session(&mut harness);
        wait_for_prompts(&mut harness, &mut agent);
        let long: Vec<String> =
            (1..=60).map(|line| format!("Point {line} of the answer.")).collect();
        answer(&agent, &long.join("\n\n"));
        wait_until(&mut harness, "the long answer", |harness| {
            harness.query_by_label_contains("12k cached").is_some()
        });
        assert_same_gap(&harness, gap, "under a long conversation");
        assert!(harness.query_by_label("Jump to latest").is_none(), "the answer is followed");

        let over = harness.get_by_label_contains("Point 60").rect().center();
        harness.hover_at(over);
        for _ in 0..4 {
            harness.event(wheel(400.0, egui::TouchPhase::Move, egui::MouseWheelSource::Wheel));
            harness.step();
        }
        press(&mut harness, "Jump to latest");
        wait_until(&mut harness, "the latest line again", |harness| {
            harness.query_by_label("Jump to latest").is_none()
        });

        let before = panel(&harness).width();
        drag(&mut harness, SIDE_SPLITTER, egui::vec2(-100.0, 0.0));
        let wider = panel(&harness).width();
        assert!((wider - before - 100.0).abs() < 1.0, "{before} became {wider}");
        assert_same_gap(&harness, gap, "after widening");
        let saved = AppSettings::load(&harness.state().services.paths.settings_file())
            .expect("settings")
            .panes
            .overview_side;
        assert!((saved - wider).abs() < 1.0, "saved {saved}");

        press(&mut harness, "Collapse (Esc)");
        wait_until(&mut harness, "the side cards again", has_side_cards);
        wider
    };

    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    harness.key_press_modifiers(Modifiers::ALT, Key::A);
    wait_until(&mut harness, "the panel", |harness| has_group(harness, PANEL));
    assert!((panel(&harness).width() - width).abs() < 1.0, "the width survived the restart");
}

const SIDE_SPLITTER: &str = "Resize side column";
const PANEL: &str = "Assistant panel";

/// The side column's other cards (Reviewers, Labels) are on screen.
fn has_side_cards(harness: &Harness<'_, ReviewApp>) -> bool {
    harness.query_all_by_label("Reviewers").next().is_some()
        && harness.query_all_by_label("Labels").next().is_some()
}

fn has_group(harness: &Harness<'_, ReviewApp>, name: &str) -> bool {
    harness
        .query_all_by(|node| node.role() == Role::Group && node.label().as_deref() == Some(name))
        .next()
        .is_some()
}

/// Until the user drags it, the card's conversation fills the side column's room: the card ends
/// at the column's bottom edge, at any window height, and never goes below its least height (the
/// column scrolls then). A drag or a key step keeps a height, taller than that room too (up to
/// the column's visible height); it survives a restart; the reset (Enter on the splitter) forgets
/// it and the card fills again.
#[test]
fn assistant_card_fills_the_side_column_until_dragged() {
    let scratch = ScratchDir::new("assistant-card-height");
    let host = GitHubHost::with_pull_request(scratch.path());
    {
        let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
        open_pull_request(&mut harness);
        wait_until(&mut harness, "the card's splitter", |harness| {
            harness.query_by_label(CARD_SPLITTER).is_some()
        });
        for height in [1_000.0, 1_200.0] {
            resize(&mut harness, height);
            assert_card_fills(&harness, height);
        }
        resize(&mut harness, 600.0);
        assert_eq!(shown_height(&harness), 260.0, "the least height in a small window");
        assert_eq!(stored_height(&harness), None, "nothing stored without a drag");

        resize(&mut harness, 1_200.0);
        let fill = shown_height(&harness);
        let (bar, gap) = bar_and_composer(&harness);
        drag(&mut harness, CARD_SPLITTER, egui::vec2(0.0, -100.0));
        assert_eq!(stored_height(&harness), Some(fill - 100.0), "a drag keeps the height");
        let (higher, higher_gap) = bar_and_composer(&harness);
        assert!((bar - higher - 100.0).abs() < 1.0, "the bar went from {bar} to {higher}");
        assert!((higher_gap - gap).abs() < 1.0, "the composer stays under the bar");
        // Regression: the drag stopped at the start height; the card grows past it, the column
        // scrolls.
        drag(&mut harness, CARD_SPLITTER, egui::vec2(0.0, 300.0));
        let taller = stored_height(&harness).expect("a drag keeps the height");
        assert!((taller - (fill + 200.0)).abs() < 1.0, "{taller}, not 200 above the start {fill}");
        assert_eq!(shown_height(&harness), taller, "the card shows the height it was dragged to");
        let settings = AppSettings::load(&harness.state().services.paths.settings_file());
        assert_eq!(settings.expect("settings").assistant.card_height, Some(taller), "saved");
        // The bar is below the window now, out of the pointer's reach; the keys take it on.
        harness.get_by_label(CARD_SPLITTER).focus();
        harness.step();
        harness.key_press(Key::End);
        harness.step();
        let tallest = stored_height(&harness).expect("a key step keeps the height");
        assert!(tallest > taller && tallest < 1_200.0, "up to the column's height: {tallest}");
        harness.key_press(Key::Home);
        harness.step();
        assert_eq!(stored_height(&harness), Some(260.0), "at least the least height");
        harness.key_press(Key::ArrowDown);
        harness.step();
        assert_eq!(stored_height(&harness), Some(268.0), "↓ steps by 8 points");
        let settings = AppSettings::load(&harness.state().services.paths.settings_file());
        assert_eq!(settings.expect("settings").assistant.card_height, Some(268.0), "saved");
    }

    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    wait_until(&mut harness, "the card's splitter", |harness| {
        harness.query_by_label(CARD_SPLITTER).is_some()
    });
    resize(&mut harness, 1_200.0);
    assert_eq!(shown_height(&harness), 268.0, "the height survived the restart");
    harness.get_by_label(CARD_SPLITTER).focus();
    harness.step();
    harness.key_press(Key::Enter);
    harness.run_steps(4);
    assert_eq!(stored_height(&harness), None, "the reset forgets the height");
    assert_card_fills(&harness, 1_200.0);
    let settings = AppSettings::load(&harness.state().services.paths.settings_file());
    assert_eq!(settings.expect("settings").assistant.card_height, None, "saved");
}

const CARD_SPLITTER: &str = "Resize assistant conversation";

fn stored_height(harness: &Harness<'_, ReviewApp>) -> Option<f32> {
    harness.state().services.settings.assistant.card_height
}

/// The conversation's height as the card shows it: the value its splitter reports.
fn shown_height(harness: &Harness<'_, ReviewApp>) -> f32 {
    let value = harness.get_by_label(CARD_SPLITTER).accesskit_node().numeric_value();
    value.expect("the splitter's value") as f32
}

/// The card ends on the side column's bottom edge: the window's bottom less the page padding and
/// the card's own padding below the composer, with the page scrolled to the top.
fn assert_card_fills(harness: &Harness<'_, ReviewApp>, window_height: f32) {
    let bottom = group(harness, "Question composer").bottom();
    let expected = window_height - 16.0 - 16.0;
    assert!((bottom - expected).abs() <= 1.0, "the composer ends at {bottom}, not {expected}");
}

fn resize(harness: &mut Harness<'_, ReviewApp>, height: f32) {
    harness.set_size(egui::vec2(1_440.0, height));
    harness.run_steps(4);
}

/// The card's splitter line and how far below it the composer starts.
fn bar_and_composer(harness: &Harness<'_, ReviewApp>) -> (f32, f32) {
    let bar = harness.get_by_label(CARD_SPLITTER).rect().center().y;
    (bar, group(harness, "Question composer").top() - bar)
}

fn panel(harness: &Harness<'_, ReviewApp>) -> egui::Rect {
    group(harness, PANEL)
}

fn group(harness: &Harness<'_, ReviewApp>, name: &str) -> egui::Rect {
    harness
        .get_by(|node| node.role() == Role::Group && node.label().as_deref() == Some(name))
        .rect()
}

/// How far the composer ends above the panel's bottom.
fn composer_gap(harness: &Harness<'_, ReviewApp>) -> f32 {
    panel(harness).bottom() - group(harness, "Question composer").bottom()
}

fn assert_same_gap(harness: &Harness<'_, ReviewApp>, gap: f32, when: &str) {
    let now = composer_gap(harness);
    assert!((now - gap).abs() <= 1.0, "{when} the composer sits {now} above the bottom, not {gap}");
}

/// Drags the splitter called `splitter` by `by`.
fn drag(harness: &mut Harness<'_, ReviewApp>, splitter: &str, by: egui::Vec2) {
    let from = harness.get_by_label(splitter).rect().center();
    let to = from + by;
    harness.hover_at(from);
    harness.step();
    for (at, pressed) in [(from, true), (to, false)] {
        harness.hover_at(at);
        harness.step();
        harness.event(egui::Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        harness.step();
    }
    for _ in 0..4 {
        harness.step();
    }
}

/// The text containing `text` sits in the bubble called `group` with even insets above and below.
fn assert_centered(harness: &Harness<'_, ReviewApp>, group: &str, text: &str) {
    let row = harness
        .get_by(|node| node.role() == Role::Group && node.label().as_deref() == Some(group))
        .rect();
    let block = harness
        .query_all_by_label_contains(text)
        .map(|node| node.rect())
        .find(|rect| row.contains_rect(*rect))
        .unwrap_or_else(|| panic!("no \"{text}\" in {group}"));
    row::assert_centered_in(row, block);
}

/// The agent's whole answer to the running question: a tool step, `text`, and the turn's end.
fn answer(agent: &FakeAgent, text: &str) {
    agent.emit(AgentEvent::Started { session_id: "s-1".into(), model: "claude-haiku".into() });
    let (id, tool, detail) = ("t1".into(), "Read".into(), FILE.into());
    agent.emit(AgentEvent::Tool(ToolStep { id, tool, kind: StepKind::Read, detail }));
    agent.emit(AgentEvent::Text(text.to_owned()));
    let usage = Usage { input: 1_200, output: 340, cache_read: 12_000, cache_write: 0 };
    agent.emit(AgentEvent::TurnDone(usage));
}

fn wait_for_session(harness: &mut Harness<'_, ReviewApp>) -> (SessionConfig, FakeAgent) {
    let take =
        |harness: &Harness<'_, ReviewApp>| harness.state().services.agents.fake.take_session();
    wait_for(harness, "a session", take, |_| {})
}

fn wait_for_prompts(
    harness: &mut Harness<'_, ReviewApp>,
    agent: &mut FakeAgent,
) -> Vec<Option<String>> {
    let take =
        |_: &Harness<'_, ReviewApp>| Some(agent.take_prompts()).filter(|asked| !asked.is_empty());
    wait_for(harness, "the prompt", take, |_| {})
}

/// Over the un-armed card the wheel scrolls the page and leaves the conversation where it is; a
/// click into the card arms it and the wheel scrolls the conversation; a click outside gives the
/// wheel back to the page.
#[test]
fn assistant_card_takes_the_wheel_only_after_a_click_into_it() {
    let scratch = ScratchDir::new("assistant-wheel");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    set_claude(&harness, true);
    open_pull_request(&mut harness);
    // The side column is a little taller than this window: the page scrolls by about 125
    // points, and the card's conversation stays on screen.
    resize(&mut harness, 800.0);
    ask(&mut harness, QUESTION);
    let (_, mut agent) = wait_for_session(&mut harness);
    wait_for_prompts(&mut harness, &mut agent);
    let long: Vec<String> = (1..=60).map(|line| format!("Point {line} of the answer.")).collect();
    answer(&agent, &long.join("\n\n"));
    wait_until(&mut harness, "the long answer", |harness| {
        harness.query_by_label_contains("12k cached").is_some()
    });
    assert!(is_armed(&harness), "typing the question armed the card");

    click_outside(&mut harness);
    assert!(!is_armed(&harness), "a click outside disarms");
    let start = wheel_over_conversation(&mut harness, -100.0);
    assert!(start.page < -10.0, "the wheel scrolls the page: {start:?}");
    assert!(start.conversation.abs() < 1.0, "the conversation stays: {start:?}");
    let back = wheel_over_conversation(&mut harness, 100.0);
    assert!(back.page > 10.0, "the page scrolls back: {back:?}");
    assert!(back.conversation.abs() < 1.0, "the conversation stays: {back:?}");

    let inside = conversation_point(&harness);
    click_now(&mut harness, inside);
    assert!(is_armed(&harness), "a click into the card arms it");
    let armed = wheel_over_conversation(&mut harness, 100.0);
    assert!(armed.conversation > 10.0, "the conversation scrolls: {armed:?}");
    assert!(armed.page.abs() < 1.0, "the page stays: {armed:?}");

    click_outside(&mut harness);
    assert!(!is_armed(&harness), "a click outside disarms again");
    let page = wheel_over_conversation(&mut harness, -100.0);
    assert!(page.page < -10.0, "the page takes the wheel again: {page:?}");
    assert!(page.conversation.abs() < 1.0, "the conversation stays: {page:?}");
}

/// Turned off in Settings, the assistant leaves the Overview, Alt+A opens nothing, the other
/// Assistant rows read as inactive and no CLI is looked for or started; turned on again, it is
/// back without a restart.
#[test]
fn assistant_turned_off_in_settings_leaves_the_page_and_starts_nothing() {
    let scratch = ScratchDir::new("assistant-off");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    set_claude(&harness, true);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Comma);
    press(&mut harness, "Assistant");
    switch_assistant(&mut harness);
    assert!(!harness.state().services.settings.assistant.enabled);
    let saved = AppSettings::load(&harness.state().services.paths.settings_file());
    assert!(!saved.expect("settings").assistant.enabled, "saved");
    let model = harness.query_all_by_role(Role::ComboBox).next().expect("the Claude Code model");
    assert!(model.accesskit_node().is_disabled(), "the model rows read as inactive");
    // Settings looked for the CLIs while the assistant was on; from here on nothing may.
    let agents = &mut harness.state_mut().services.agents;
    for kind in AgentKind::ALL {
        agents.check_again(kind);
    }

    harness.state_mut().navigator.go(Route::MyPullRequests);
    open_pull_request(&mut harness);
    harness.run_steps(20);
    assert!(harness.query_by_label(CARD).is_none(), "no card");
    assert!(!has_group(&harness, CARD_LABEL), "no card");
    harness.key_press_modifiers(Modifiers::ALT, Key::A);
    harness.run_steps(10);
    assert!(!has_group(&harness, PANEL), "Alt+A opens nothing");
    assert!(harness.state().services.agents.is_untouched(), "no CLI looked for");
    assert!(harness.state().services.agents.fake.take_session().is_none(), "no agent started");

    harness.key_press_modifiers(Modifiers::COMMAND, Key::Comma);
    switch_assistant(&mut harness);
    assert!(harness.state().services.settings.assistant.enabled);
    harness.state_mut().navigator.go(Route::MyPullRequests);
    open_pull_request(&mut harness);
    wait_until(&mut harness, "the card again", |harness| harness.query_by_label(CARD).is_some());
    harness.key_press_modifiers(Modifiers::ALT, Key::A);
    wait_until(&mut harness, "the panel", |harness| has_group(harness, PANEL));
}

const SWITCH: &str = "Show the assistant";

fn switch_assistant(harness: &mut Harness<'_, ReviewApp>) {
    wait_until(harness, SWITCH, |harness| {
        harness.query_by_role_and_label(Role::CheckBox, SWITCH).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::CheckBox, SWITCH).rect());
    harness.run_steps(4);
}

fn is_armed(harness: &Harness<'_, ReviewApp>) -> bool {
    let card = harness
        .get_by(|node| node.role() == Role::Group && node.label().as_deref() == Some(CARD_LABEL));
    card.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

/// Clicks the Description card's title in the main column.
fn click_outside(harness: &mut Harness<'_, ReviewApp>) {
    let title = harness.query_all_by_label("Description").next().expect("the title").rect();
    click_now(harness, title.center());
    harness.run_steps(4);
}

/// A point in the card's conversation, above its composer.
fn conversation_point(harness: &Harness<'_, ReviewApp>) -> Pos2 {
    let composer = group(harness, "Question composer");
    pos2(composer.center().x, composer.top() - 80.0)
}

/// How far a wheel turn moved the page (the composer) and the conversation within the card.
#[derive(Debug)]
struct Moved {
    page: f32,
    conversation: f32,
}

/// Turns the wheel by `delta` points over the conversation and lets the scrolling settle. The
/// line nearest the pointer marks the conversation's offset: lines scrolled out of view leave the
/// accessibility tree, and `delta` stays below half the conversation's least height.
fn wheel_over_conversation(harness: &mut Harness<'_, ReviewApp>, delta: f32) -> Moved {
    let at = conversation_point(harness);
    let line = harness
        .query_all_by(|node| {
            node.label().or_else(|| node.value()).is_some_and(|text| text.starts_with("Point "))
        })
        .min_by(|a, b| {
            let distance = |rect: egui::Rect| (rect.center().y - at.y).abs();
            distance(a.rect()).total_cmp(&distance(b.rect()))
        })
        .and_then(|node| node.accesskit_node().label().or_else(|| node.accesskit_node().value()))
        .expect("a line of the answer on screen");
    let places = |harness: &Harness<'_, ReviewApp>| {
        let composer = group(harness, "Question composer").top();
        let shown = harness
            .query_all_by(|node| {
                node.label().or_else(|| node.value()).as_deref() == Some(line.as_str())
            })
            .next()
            .expect("the line stays on screen");
        (composer, shown.rect().top() - composer)
    };
    let (page, conversation) = places(harness);
    harness.hover_at(at);
    harness.step();
    harness.event(wheel(delta, egui::TouchPhase::Move, egui::MouseWheelSource::Wheel));
    harness.run_steps(30);
    let (page_now, conversation_now) = places(harness);
    Moved { page: page_now - page, conversation: conversation_now - conversation }
}

/// "New conversation" sits on the title line of card and panel, no longer in the chip's menu. One
/// click empties the conversation, Undo brings it back; Ctrl+N does the same in the panel, and
/// the next question ends the chance to undo.
#[test]
fn assistant_new_conversation_takes_one_click_and_can_be_undone() {
    let scratch = ScratchDir::new("assistant-new");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    set_claude(&harness, true);
    open_pull_request(&mut harness);
    ask(&mut harness, QUESTION);
    let (_, mut agent) = wait_for_session(&mut harness);
    wait_for_prompts(&mut harness, &mut agent);
    answer(&agent, ANSWER);
    wait_until(&mut harness, "the answer", |harness| {
        harness.query_by_label_contains("12k cached").is_some()
    });

    press_new_conversation(&mut harness);
    wait_until(&mut harness, "the empty conversation", |harness| {
        harness.query_all_by_label(QUESTION).next().is_none()
            && harness.query_by_label(UNDO_NOTE).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Undo").rect());
    wait_until(&mut harness, "the conversation again", |harness| {
        harness.query_all_by_label(QUESTION).next().is_some()
            && harness.query_by_label(UNDO_NOTE).is_none()
    });

    harness.click_at(harness.get_by_label_contains("Claude · ").rect());
    wait_until(&mut harness, "the chip's menu", |harness| {
        harness.query_by_label("New conversation").is_none()
            && harness.query_by_label("Model, from the next question").is_some()
    });
    harness.key_press(Key::Escape);
    harness.run_steps(4);

    harness.key_press_modifiers(Modifiers::ALT, Key::A);
    wait_until(&mut harness, "the panel", |harness| has_group(harness, PANEL));
    harness.key_press_modifiers(Modifiers::COMMAND, Key::N);
    wait_until(&mut harness, "the empty panel", |harness| {
        harness.query_all_by_label(QUESTION).next().is_none()
            && harness.query_by_label(UNDO_NOTE).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Undo").rect());
    wait_until(&mut harness, "the conversation in the panel", |harness| {
        harness.query_all_by_label(QUESTION).next().is_some()
    });
    press_new_conversation(&mut harness);
    wait_until(&mut harness, "Undo in the panel", |harness| {
        harness.query_by_label(UNDO_NOTE).is_some()
    });
    ask(&mut harness, "A fresh start?");
    harness.run_steps(4);
    assert!(harness.query_by_label(UNDO_NOTE).is_none(), "the next question ends Undo");
}

const UNDO_NOTE: &str = "Started a new conversation.";

/// Regression: the question being written stays through a switch of the agent, of the model and a
/// new conversation, in the card and in the panel; only sending it empties the composer.
#[test]
fn assistant_draft_survives_switching_agent_and_model() {
    let scratch = ScratchDir::new("assistant-draft");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    set_claude(&harness, true);
    let codex = FakeCli::Installed { models: models(), is_signed_in: true };
    harness.state().services.agents.fake.set(AgentKind::Codex, codex);
    open_pull_request(&mut harness);
    wait_until(&mut harness, "the chip with its model", |harness| {
        harness.query_by_label_contains("haiku").is_some()
    });
    type_into(&mut harness, PLACEHOLDER, DRAFT);
    assert_eq!(composer_text(&harness), DRAFT);

    pick_in_chip(&mut harness, "Claude · ", "Codex");
    wait_until(&mut harness, "the Codex conversation", |harness| {
        harness.query_by_label_contains("Codex · ").is_some()
    });
    assert_eq!(composer_text(&harness), DRAFT, "after switching the agent");
    pick_in_chip(&mut harness, "Codex · ", "Sonnet");
    wait_until(&mut harness, "the picked model", |harness| {
        harness.query_by_label_contains("Codex · claude-sonnet").is_some()
    });
    assert_eq!(composer_text(&harness), DRAFT, "after switching the model");
    press_new_conversation(&mut harness);
    harness.run_steps(4);
    assert_eq!(composer_text(&harness), DRAFT, "after a new conversation");

    harness.key_press_modifiers(Modifiers::ALT, Key::A);
    wait_until(&mut harness, "the panel", |harness| has_group(harness, PANEL));
    assert_eq!(composer_text(&harness), DRAFT, "in the panel");
    pick_in_chip(&mut harness, "Codex · ", "CLI default");
    assert_eq!(composer_text(&harness), DRAFT, "after switching the model in the panel");
    pick_in_chip(&mut harness, "Codex · ", "Claude");
    wait_until(&mut harness, "the Claude conversation", |harness| {
        harness.query_by_label_contains("Claude · ").is_some()
    });
    assert_eq!(composer_text(&harness), DRAFT, "after switching the agent in the panel");

    question_input(&harness).focus();
    harness.step();
    harness.key_press(Key::Enter);
    harness.step();
    let (_, mut agent) = wait_for_session(&mut harness);
    assert_eq!(wait_for_prompts(&mut harness, &mut agent), [Some(DRAFT.to_owned())]);
    assert_eq!(composer_text(&harness), "", "sending empties the composer");
}

/// Regression: the mini chat on an earlier answer waits while a later question runs. It offered
/// Send, and the follow-up it took vanished unasked; now it offers Stop until the answer is in,
/// then takes the follow-up.
#[test]
fn assistant_mini_chat_on_an_earlier_answer_waits_for_the_running_one() {
    let scratch = ScratchDir::new("assistant-mini-busy");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    set_claude(&harness, true);
    harness.state_mut().services.settings.assistant.mini_chat = true;
    open_pull_request(&mut harness);
    ask(&mut harness, QUESTION);
    let (_, mut agent) = wait_for_session(&mut harness);
    wait_for_prompts(&mut harness, &mut agent);
    answer(&agent, ANSWER);
    wait_until(&mut harness, "the answer", |harness| {
        harness.query_by_label_contains("12k cached").is_some()
    });
    type_into(&mut harness, "Follow up", "And the tests?");
    harness.key_press(Key::Enter);
    harness.step();
    wait_for_prompts(&mut harness, &mut agent);

    let center = harness.query_all_by_label(CITATION).next().expect("the citation").rect().center();
    wait_for(
        &mut harness,
        "the mini chat on the first answer",
        |harness| harness.query_by(|node| node.placeholder() == Some(ANSWERING)).map(|_| ()),
        |harness| click_now(harness, center),
    );
    assert!(harness.query_by_label(QUESTION).is_some(), "it shows the first answer");
    assert!(harness.query_by_role_and_label(Role::Button, "Stop").is_some(), "Stop, not Send");
    assert!(harness.query_by_role_and_label(Role::Button, "Send").is_none(), "no Send");

    answer(&agent, "With a test.");
    type_into(&mut harness, FOLLOW_UP, DRAFT);
    harness.key_press(Key::Enter);
    assert_eq!(wait_for_prompts(&mut harness, &mut agent), [Some(DRAFT.to_owned())]);
}

/// Regression, in the owner's state: the panel over kept conversations with both agents (an
/// answered one, a failed one with a failed command), loaded from disk. The question being written
/// stays through switching the model and the agent both ways, also while a question runs and after
/// it failed.
#[test]
fn assistant_draft_survives_switching_over_kept_conversations() {
    use review_agent::{AgentError, StepOutcome};
    let scratch = ScratchDir::new("assistant-draft-kept");
    let host = GitHubHost::with_pull_request(scratch.path());
    let setup = |harness: &Harness<'_, ReviewApp>| {
        set_claude(harness, true);
        let codex = FakeCli::Installed { models: codex_models(), is_signed_in: true };
        harness.state().services.agents.fake.set(AgentKind::Codex, codex);
    };
    {
        let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
        setup(&harness);
        open_pull_request(&mut harness);
        ask(&mut harness, QUESTION);
        let (_, mut agent) = wait_for_session(&mut harness);
        wait_for_prompts(&mut harness, &mut agent);
        answer(&agent, ANSWER);
        wait_until(&mut harness, "the answer", |harness| {
            harness.query_by_label_contains("12k cached").is_some()
        });
        pick_in_chip(&mut harness, "Claude · ", "Codex");
        ask(&mut harness, "Run the tests");
        let (_, mut codex) = wait_for_session(&mut harness);
        wait_for_prompts(&mut harness, &mut codex);
        codex.emit(AgentEvent::Started { session_id: "c-1".into(), model: CODEX_MODEL.into() });
        let (id, tool, detail) = ("x".into(), "shell".into(), "cargo test".into());
        codex.emit(AgentEvent::Tool(ToolStep { id, tool, kind: StepKind::Run, detail }));
        codex.emit(AgentEvent::ToolDone { id: "x".into(), outcome: StepOutcome::Failed });
        codex.emit(AgentEvent::Failed(AgentError::Turn("The sandbox denied it.".into())));
        wait_until(&mut harness, "the failed turn", |harness| {
            harness.query_by_label_contains("The sandbox denied it.").is_some()
        });
    }

    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    setup(&harness);
    open_pull_request(&mut harness);
    harness.key_press_modifiers(Modifiers::ALT, Key::A);
    wait_until(&mut harness, "the panel with the Codex conversation", |harness| {
        has_group(harness, PANEL) && harness.query_by_label_contains(CODEX_MODEL).is_some()
    });
    type_by_keys(&mut harness, DRAFT);
    pick_in_chip(&mut harness, "Codex · ", CODEX_MODEL);
    assert_eq!(composer_text(&harness), DRAFT, "after picking the Codex model");
    pick_in_chip(&mut harness, "Codex · ", "Claude");
    wait_until(&mut harness, "the Claude models", |harness| {
        harness.query_by_label_contains("haiku").is_some()
    });
    assert_eq!(composer_text(&harness), DRAFT, "after switching to Claude");
    pick_in_chip(&mut harness, "Claude · ", "Sonnet");
    assert_eq!(composer_text(&harness), DRAFT, "after picking the Claude model");
    pick_in_chip(&mut harness, "Claude · ", "Codex");
    assert_eq!(composer_text(&harness), DRAFT, "after switching back to Codex");

    let input = question_input(&harness).rect().center();
    click_now(&mut harness, input);
    harness.key_press(Key::Enter);
    harness.step();
    let (_, mut codex) = wait_for_session(&mut harness);
    assert_eq!(wait_for_prompts(&mut harness, &mut codex), [Some(DRAFT.to_owned())]);
    assert_eq!(composer_text(&harness), "", "sending empties the composer");
    type_by_keys(&mut harness, "And now?");
    pick_in_chip(&mut harness, "Codex · ", "Claude");
    assert_eq!(composer_text(&harness), "And now?", "switched away while a question runs");
    pick_in_chip(&mut harness, "Claude · ", "Codex");
    assert_eq!(composer_text(&harness), "And now?", "back to the running question");
    codex.emit(AgentEvent::Failed(AgentError::Turn("Again denied.".into())));
    harness.run_steps(10);
    pick_in_chip(&mut harness, "Codex · ", "Claude");
    assert_eq!(composer_text(&harness), "And now?", "switched away after it failed");
}

/// Regression: the mini chat writes the same draft as the card. A follow-up typed there is still
/// in the card's composer after a switch of the agent closed the mini chat.
#[test]
fn assistant_mini_chat_follow_up_survives_switching_the_agent() {
    let scratch = ScratchDir::new("assistant-mini-draft");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    set_claude(&harness, true);
    let codex = FakeCli::Installed { models: codex_models(), is_signed_in: true };
    harness.state().services.agents.fake.set(AgentKind::Codex, codex);
    harness.state_mut().services.settings.assistant.mini_chat = true;
    open_pull_request(&mut harness);
    ask(&mut harness, QUESTION);
    let (_, mut agent) = wait_for_session(&mut harness);
    wait_for_prompts(&mut harness, &mut agent);
    answer(&agent, ANSWER);
    wait_until(&mut harness, "the answer", |harness| {
        harness.query_by_label_contains("12k cached").is_some()
    });
    let center = harness.query_all_by_label(CITATION).next().expect("the citation").rect().center();
    wait_for(
        &mut harness,
        "the mini chat",
        |harness| harness.query_by(|node| node.placeholder() == Some(FOLLOW_UP)).map(|_| ()),
        |harness| click_now(harness, center),
    );
    type_into(&mut harness, FOLLOW_UP, DRAFT);
    pick_in_chip(&mut harness, "Claude · ", "Codex");
    assert!(!has_group(&harness, mini::CARD_LABEL), "the switch closes the mini chat");
    press(&mut harness, "Overview");
    wait_until(&mut harness, "the card", |harness| has_group(harness, CARD_LABEL));
    assert_eq!(composer_text(&harness), DRAFT, "the follow-up is the card's draft");
}

/// The model the Codex CLI lists.
const CODEX_MODEL: &str = "gpt-6.1-sol";

fn codex_models() -> Vec<ModelInfo> {
    vec![ModelInfo {
        id: CODEX_MODEL.to_owned(),
        display_name: CODEX_MODEL.to_owned(),
        description: "Fast".to_owned(),
        is_default: true,
        efforts: vec!["low".to_owned(), "medium".to_owned()],
    }]
}

/// Clicks into the question composer and types `text` key by key, as a keyboard does.
fn type_by_keys(harness: &mut Harness<'_, ReviewApp>, text: &str) {
    let input = question_input(harness).rect().center();
    click_now(harness, input);
    for word in text.split_inclusive(' ') {
        harness.event(egui::Event::Text(word.to_owned()));
        harness.step();
    }
    assert_eq!(composer_text(harness), text, "typed");
}

/// The question written before the switches.
const DRAFT: &str = "Is the rename complete?";

/// The text input in the question composer.
fn question_input<'a>(harness: &'a Harness<'_, ReviewApp>) -> egui_kittest::Node<'a> {
    let composer = group(harness, "Question composer");
    harness
        .query_all_by_role(Role::MultilineTextInput)
        .find(|node| composer.contains_rect(node.rect()))
        .expect("the question's input")
}

/// The text in the question composer.
fn composer_text(harness: &Harness<'_, ReviewApp>) -> String {
    question_input(harness).accesskit_node().value().unwrap_or_default()
}

/// Opens the chip whose label starts with `chip` and picks `item` in its menu, with the pointer.
fn pick_in_chip(harness: &mut Harness<'_, ReviewApp>, chip: &str, item: &str) {
    let at = harness.get_by_label_contains(chip).rect().center();
    click_now(harness, at);
    // The menu opens before the model list arrives, which moves its items, and an agent's item is
    // disabled until its CLI was found, in the background; the item is what gets clicked, once it
    // is enabled and stayed in place for a frame.
    let last = std::cell::Cell::new(None);
    wait_until(harness, "the chip's menu with the item", |harness| {
        let now = harness
            .query_by_role_and_label(Role::MenuItem, item)
            .filter(|node| !node.accesskit_node().is_disabled())
            .map(|node| node.rect());
        harness.query_by_label("Model, from the next question").is_some()
            && now.is_some()
            && last.replace(now) == now
    });
    let at = harness.get_by_role_and_label(Role::MenuItem, item).rect().center();
    click_now(harness, at);
    harness.run_steps(4);
}

/// Clicks the title line's "New conversation" (its label names the shortcut too).
fn press_new_conversation(harness: &mut Harness<'_, ReviewApp>) {
    let find = |harness: &Harness<'_, ReviewApp>| {
        harness
            .query_all_by_role(Role::Button)
            .find(|node| {
                node.accesskit_node()
                    .label()
                    .is_some_and(|label| label.starts_with("New conversation ("))
            })
            .and_then(|node| node.accesskit_node().label())
    };
    wait_until(harness, "New conversation", |harness| find(harness).is_some());
    let label = find(harness).expect("the button");
    harness.click_at(harness.get_by_role_and_label(Role::Button, &label).rect());
    harness.step();
}

/// The filled card's height follows the window, not the header: scrolled until the header slid
/// away, the conversation keeps its height to the point; a taller window still grows it.
#[test]
fn assistant_card_keeps_its_height_while_the_header_slides_away() {
    let scratch = ScratchDir::new("assistant-card-header");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    wait_until(&mut harness, "the card's splitter", |harness| {
        harness.query_by_label(CARD_SPLITTER).is_some()
    });
    resize(&mut harness, 1_100.0);
    let height = shown_height(&harness);
    let tabs = harness.get_by_role_and_label(Role::Tab, "Overview").rect().top();

    // A trackpad gesture over the main column slides the header away.
    let over = harness.query_all_by_label("Description").next().expect("the title").rect().center();
    harness.hover_at(over);
    harness.step();
    let trackpad = |delta: f32, phase| wheel(delta, phase, egui::MouseWheelSource::Trackpad);
    harness.event(trackpad(0.0, egui::TouchPhase::Start));
    harness.event(trackpad(-400.0, egui::TouchPhase::Move));
    harness.step();
    harness.event(trackpad(0.0, egui::TouchPhase::End));
    harness.run_steps(10);
    let moved = tabs - harness.get_by_role_and_label(Role::Tab, "Overview").rect().top();
    assert!(moved > 40.0, "the header slid away: the tab bar moved up by {moved}");
    assert_eq!(shown_height(&harness), height, "the card keeps its height");

    resize(&mut harness, 1_200.0);
    let taller = shown_height(&harness);
    assert!((taller - height - 100.0).abs() < 1.0, "{height} became {taller} in a taller window");
}
