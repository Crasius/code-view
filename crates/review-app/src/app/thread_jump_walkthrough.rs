//! A click on a thread in the Files tab's threads panel brings the thread into view, end to end:
//! the real app against the mocked GitHub host ([`super::github_mock`]). The thread's file was
//! never opened, and the file open at the click is of the same kind (a text diff before a code
//! file, a rendered Markdown diff before a Markdown file), far down below a collapsed unchanged
//! run, or inside one. Needs `git` on PATH, no network.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use egui::{Rect, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::ReviewApp;
use super::action_e2e::{app, open_pull_request, shows_file_view, wait_until};
use super::github_mock::{ExtraFile, FILE, GitHubHost, MockThread};
use cv_testkit::Clicks;
use cv_testkit::{self as test_support, ScratchDir};

const TIMEOUT: Duration = Duration::from_secs(30);
const CODE: &str = "src/long.rs";
const INTRO: &str = "docs/intro.md";
const SOURCES: &str = "docs/build-sources.md";
const CODE_THREAD: &str = "Does this value need the offset?";
const FAR_THREAD: &str = "Is this source still mirrored?";
const FOLDED_THREAD: &str = "Who owns this unchanged paragraph?";

/// 600 lines; the head changes lines 300 to 520, so lines 1 to 296 collapse into one gap.
fn code_file() -> ExtraFile {
    let text = |head: bool| {
        (1..=600).fold(String::new(), |mut text, line| {
            let changed = head && (300..=520).contains(&line);
            let tail = if changed { " + 1" } else { "" };
            writeln!(text, "    let value_{line} = compute({line}){tail};").unwrap();
            text
        })
    };
    ExtraFile { path: CODE, base: text(false).into_bytes(), head: text(true).into_bytes() }
}

/// 400 lines of one-line paragraphs; the head revises every paragraph from line 181 to 381, so
/// the paragraphs above collapse into one unchanged run, as in the owner's file.
fn sources_file() -> ExtraFile {
    let text = |head: bool| {
        (1..=400).fold(String::new(), |mut text, line| {
            if line % 2 == 0 {
                text.push('\n');
            } else if head && (181..=381).contains(&line) {
                writeln!(text, "Paragraph {line} of the build sources, revised.").unwrap();
            } else {
                writeln!(text, "Paragraph {line} of the build sources.").unwrap();
            }
            text
        })
    };
    ExtraFile { path: SOURCES, base: text(false).into_bytes(), head: text(true).into_bytes() }
}

fn intro_file() -> ExtraFile {
    ExtraFile {
        path: INTRO,
        base: b"# Intro\n\nThe old introduction.\n".to_vec(),
        head: b"# Intro\n\nThe new introduction.\n".to_vec(),
    }
}

#[test]
fn a_thread_in_a_file_never_opened_comes_into_view() {
    let scratch = ScratchDir::new("thread-jump");
    let files = [code_file(), sources_file(), intro_file()];
    let host = GitHubHost::with_pull_request_files(scratch.path(), &files);
    host.add_threads(&[
        MockThread { id: "PRRT_code", path: CODE, lines: 500..=500, text: CODE_THREAD },
        MockThread { id: "PRRT_far", path: SOURCES, lines: 369..=372, text: FAR_THREAD },
        MockThread { id: "PRRT_folded", path: SOURCES, lines: 101..=101, text: FOLDED_THREAD },
    ]);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    harness.state_mut().services.settings.threads_panel.is_open = true;
    open_pull_request(&mut harness);
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::Tab
                    && node.label().is_some_and(|label| label.starts_with("Files"))
            })
            .rect(),
    );

    // A text diff is open; the code thread's file was never opened.
    open_in_tree(&mut harness, FILE);
    wait_until(&mut harness, "the open text diff", |harness| shows_file_view(harness, FILE));
    click_row(&mut harness, CODE_THREAD);
    let card = wait_for_card(&mut harness, CODE_THREAD);
    assert!(shows_file_view(&harness, CODE), "the thread's file opened");
    let diff = harness.get_by_label(&format!("Diff of {CODE}")).rect();
    assert!(diff.contains_rect(card), "the code thread {card:?} inside the diff {diff:?}");

    // A rendered Markdown diff is open; the far thread's file was never opened.
    open_in_tree(&mut harness, INTRO);
    wait_until(&mut harness, "the rendered intro", |harness| {
        harness.query_by_label("The new introduction.").is_some()
    });
    click_row(&mut harness, FAR_THREAD);
    wait_for_card(&mut harness, FAR_THREAD);
    assert!(
        harness.query_by_label(&format!("Diff of {SOURCES}")).is_none(),
        "the Markdown file shows rendered"
    );

    // A thread inside the collapsed run of the same file: the run opens and the thread shows.
    click_row(&mut harness, FOLDED_THREAD);
    wait_for_card(&mut harness, FOLDED_THREAD);
}

fn open_in_tree(harness: &mut Harness<'_, ReviewApp>, path: &str) {
    wait_until(harness, "the file's row in the tree", |harness| {
        harness.query_by_role_and_label(Role::TreeItem, path).is_some()
    });
    harness.click_at(harness.get_by_role_and_label(Role::TreeItem, path).rect());
    harness.step();
}

/// Clicks the threads panel's row of the thread reading `text`.
fn click_row(harness: &mut Harness<'_, ReviewApp>, text: &str) {
    wait_until(harness, "the thread's row", |harness| row(harness, text).is_some());
    harness.click_at(row(harness, text).expect("the thread's row").rect());
    harness.step();
}

fn row<'h>(harness: &'h Harness<'_, ReviewApp>, text: &str) -> Option<egui_kittest::Node<'h>> {
    harness.query_all_by_role(Role::Button).find(|node| {
        node.accesskit_node().label().is_some_and(|label| label.ends_with(&format!(": {text}")))
    })
}

/// Steps until the card of the thread reading `text` shows in the diff, left of the threads
/// panel and inside the window; returns its place.
fn wait_for_card(harness: &mut Harness<'_, ReviewApp>, text: &str) -> Rect {
    let started = Instant::now();
    loop {
        harness.step();
        if card(harness, text).is_some() {
            // The view settles before the place is read.
            for _ in 0..10 {
                harness.step();
            }
            let card = card_rect(harness, text).expect("the card stays in view");
            let window = harness.ctx.content_rect();
            assert!(window.contains_rect(card), "the card {card:?} inside the window {window:?}");
            return card;
        }
        assert!(
            started.elapsed() < TIMEOUT,
            "the card of {text:?} never came into view; the diff stayed where it was"
        );
        test_support::let_threads_run(Duration::from_millis(20));
    }
}

/// The card's place while it shows in the diff (not the panel's row of it) inside the window.
fn card(harness: &Harness<'_, ReviewApp>, text: &str) -> Option<Rect> {
    card_rect(harness, text).filter(|card| harness.ctx.content_rect().contains_rect(*card))
}

fn card_rect(harness: &Harness<'_, ReviewApp>, text: &str) -> Option<Rect> {
    let panel = row(harness, text)?.rect();
    harness
        .query_all_by_label_contains(text)
        .map(|node| node.rect())
        .find(|rect| rect.right() <= panel.left() + 1.0)
}
