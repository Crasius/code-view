//! Selecting, copying and commenting on several lines in the diff, driven with the pointer and
//! keyboard like a user.

use cv_testkit::{Clicks, ScratchDir};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{
    DiffSide, FileCategory, LineRange, NewThread, ReviewThread, ThreadAnchor, ThreadStatus,
};
use review_git::{ChangeKind, ChangedFile, LineKind};
use review_store::DiffLayout;

use crate::tests::services_in;
use cv_base::theme;
use cv_code::diff_rows::{Layout, RangeRows};
use cv_code::line_source::Pane;
use cv_pr_diff::diff_view::{COMMENT_SHORTCUT, DiffEvent, TextArea};
use cv_pr_diff::testing::{
    self as check, Fixture, NEW, OLD, PATH, fixture, harness, line_of, settle, side,
};
use cv_pr_rendered::file_header::{self, HeaderInputs};
use cv_pr_rendered::preview::Preview;
use cv_pr_threads::samples;
use cv_services::Services;
use cv_widgets::text_selection;

fn button(pos: Pos2, button: PointerButton, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton { pos, button, pressed, modifiers }
}

/// The comments the view asked to create, in order.
fn new_threads(events: &[DiffEvent]) -> impl Iterator<Item = &NewThread> {
    events.iter().filter_map(|event| match event {
        DiffEvent::Create { thread, .. } => Some(thread),
        _ => None,
    })
}

/// Moves to `pos` and clicks there while holding `modifiers`.
fn click<State>(harness: &mut Harness<'_, State>, pos: Pos2, modifiers: Modifiers) {
    harness.hover_at(pos);
    harness.event(Event::ModifiersChanged(modifiers));
    harness.event(button(pos, PointerButton::Primary, true, modifiers));
    harness.event(button(pos, PointerButton::Primary, false, modifiers));
    harness.event(Event::ModifiersChanged(Modifiers::NONE));
    harness.step();
}

/// Clicks `count` times at `pos` within one frame: the harness advances a quarter second per
/// frame, longer than a double-click may take.
fn multi_click<State>(harness: &mut Harness<'_, State>, pos: Pos2, count: usize) {
    harness.hover_at(pos);
    harness.step();
    for _ in 0..count {
        harness.input_mut().events.push(button(pos, PointerButton::Primary, true, Modifiers::NONE));
        harness.input_mut().events.push(button(
            pos,
            PointerButton::Primary,
            false,
            Modifiers::NONE,
        ));
    }
    harness.step();
}

fn right_click<State>(harness: &mut Harness<'_, State>, pos: Pos2) {
    harness.hover_at(pos);
    harness.event(button(pos, PointerButton::Secondary, true, Modifiers::NONE));
    harness.event(button(pos, PointerButton::Secondary, false, Modifiers::NONE));
    harness.step();
}

/// Presses at `from`, moves through `path` and lets go at its last point; the pointer stays there.
fn drag<State>(harness: &mut Harness<'_, State>, from: Pos2, path: &[Pos2]) {
    harness.hover_at(from);
    harness.step();
    harness.event(button(from, PointerButton::Primary, true, Modifiers::NONE));
    harness.step();
    for point in path {
        harness.hover_at(*point);
        harness.step();
    }
    let to = path.last().copied().unwrap_or(from);
    harness.event(button(to, PointerButton::Primary, false, Modifiers::NONE));
    harness.step();
}

/// Sends the copy event egui-winit makes of Ctrl+C and returns what went to the clipboard.
fn copy<State>(harness: &mut Harness<'_, State>) -> Option<String> {
    harness.event(Event::Copy);
    harness.step();
    harness.output().platform_output.commands.iter().find_map(|command| match command {
        egui::OutputCommand::CopyText(text) => Some(text.clone()),
        _ => None,
    })
}

/// What a screen reader hears for the diff area, including the selected lines.
fn diff_label<State>(harness: &Harness<'_, State>) -> String {
    harness
        .query_all_by_role(Role::Document)
        .filter_map(|node| node.accesskit_node().label())
        .find(|label| label.starts_with("Diff of"))
        .expect("the diff area")
}

/// Writes `body` into the open comment and posts it.
fn post_comment<State>(harness: &mut Harness<'_, State>, body: &str) {
    harness.event(Event::Text(body.to_owned()));
    harness.step();
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Comment").rect());
    settle(harness);
}

/// 1-based line `number` of `text`.
fn line(text: &str, number: u32) -> &str {
    text.lines().nth(number as usize - 1).expect("line in the sample")
}

fn row(fixture: &Fixture, old_line: Option<u32>, new_line: Option<u32>) -> usize {
    fixture.view.row_of(&fixture.diff, old_line, new_line)
}

/// A thread on lines `start..=end` of the head file.
fn range_thread(
    start: (Option<u32>, Option<u32>),
    end: (Option<u32>, Option<u32>),
    body: &str,
) -> ReviewThread {
    let anchor = ThreadAnchor {
        path: PATH.into(),
        old_path: None,
        old_line: end.0,
        new_line: end.1,
        range: Some(LineRange {
            start_old_line: start.0,
            start_new_line: start.1,
            side: DiffSide::New,
            detail: None,
        }),
    };
    let comments = vec![samples::comment("c1", &samples::mara(), 0, body)];
    samples::thread("range-thread", ThreadStatus::Active, Some(anchor), comments)
}

#[test]
fn side_by_side_selects_and_copies_code_in_one_pane() {
    let mut harness = harness(fixture(Layout::SideBySide));
    settle(&mut harness);
    let (old_total, new_total) = (line_of(OLD, "pub fn total("), line_of(NEW, "pub fn total("));
    let (signature, body) = (
        row(harness.state(), Some(old_total), Some(new_total)),
        row(harness.state(), None, Some(new_total + 1)),
    );

    // A drag over the head pane selects characters across both lines.
    let start = harness.state().view.text_point(Pane::Right, signature, 4);
    let end = harness.state().view.text_point(Pane::Right, body, 20);
    let middle = start.lerp(end, 0.5);
    drag(&mut harness, start, &[middle, end]);
    let expected = format!("{}\n{}", &line(NEW, new_total)[4..], &line(NEW, new_total + 1)[..20]);
    assert_eq!(
        copy(&mut harness).as_deref(),
        Some(expected.as_str()),
        "copies the characters, without numbers, markers or gutter"
    );
    assert!(
        diff_label(&harness).ends_with(&format!("lines {new_total}–{} selected", new_total + 1)),
        "{}",
        diff_label(&harness)
    );
    settle(&mut harness);

    // Dragging on into the base pane keeps the selection in the head pane.
    let into_base = harness.state().view.text_point(Pane::Left, body, 10);
    drag(&mut harness, start, &[into_base]);
    assert_eq!(
        copy(&mut harness),
        Some(format!("{}\n", &line(NEW, new_total)[4..])),
        "the selection stays in the pane it started in"
    );

    // The base pane selects its own lines: the deleted loop start.
    let deleted = line(OLD, old_total + 1);
    let from = harness.state().view.text_point(Pane::Left, body, 8);
    let to = harness.state().view.text_point(Pane::Left, body, deleted.chars().count());
    drag(&mut harness, from, &[to]);
    assert_eq!(copy(&mut harness).as_deref(), Some(deleted.trim_start()));
}

#[test]
fn clicks_pick_words_and_lines_and_keys_pick_everything() {
    let mut harness = harness(fixture(Layout::Inline));
    settle(&mut harness);
    let (old_total, new_total) = (line_of(OLD, "pub fn total("), line_of(NEW, "pub fn total("));
    let signature = row(harness.state(), Some(old_total), Some(new_total));
    let on_word = harness.state().view.text_point(Pane::Inline, signature, 12);

    multi_click(&mut harness, on_word, 2);
    assert_eq!(copy(&mut harness).as_deref(), Some("total"), "a double-click selects the word");
    multi_click(&mut harness, on_word, 3);
    assert_eq!(
        copy(&mut harness),
        Some(format!("{}\n", line(NEW, new_total))),
        "a triple-click selects the line"
    );
    assert!(diff_label(&harness).ends_with(&format!("line {new_total} selected")));

    // Ctrl+A takes every line of the pane: base and head lines alike.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.step();
    let everything = copy(&mut harness).expect("all lines copied");
    let lines = harness.state().diff.lines();
    let shown: Vec<u32> = lines
        .iter()
        .filter(|line| line.kind != LineKind::Hunk && line.kind != LineKind::Deleted)
        .map(|line| line.new_line)
        .collect();
    let (first, last) =
        (shown.iter().min().copied().unwrap_or(0), shown.iter().max().copied().unwrap_or(0));
    assert!(
        everything.contains("let mut total = 0;") && everything.contains(".sum()"),
        "both versions are copied: {everything}"
    );
    assert_eq!(
        everything.lines().count(),
        lines.iter().filter(|line| line.kind != LineKind::Hunk).count()
    );
    assert!(
        diff_label(&harness).ends_with(&format!("lines {first}–{last} selected")),
        "{}",
        diff_label(&harness)
    );

    // Esc lets go of the selection.
    harness.key_press(Key::Escape);
    harness.step();
    assert!(!diff_label(&harness).contains("selected"));
    assert_eq!(copy(&mut harness), None, "nothing is copied without a selection");
}

#[test]
fn line_numbers_pick_lines_to_comment_on() {
    let mut harness = harness(fixture(Layout::Inline));
    settle(&mut harness);
    let first_new = line_of(NEW, "pub fn add(");
    let last_new = line_of(NEW, "pub fn total(") + 1;
    let (first, last) =
        (row(harness.state(), None, Some(first_new)), row(harness.state(), None, Some(last_new)));

    let (first_number, last_number) = (
        harness.state().view.gutter_point(Pane::Inline, first),
        harness.state().view.gutter_point(Pane::Inline, last),
    );
    click(&mut harness, first_number, Modifiers::NONE);
    assert!(diff_label(&harness).ends_with(&format!("line {first_new} selected")));
    click(&mut harness, last_number, Modifiers::SHIFT);
    let lines = format!("lines {first_new}–{last_new}");
    assert!(
        diff_label(&harness).ends_with(&format!("{lines} selected")),
        "Shift+click extends to the line"
    );
    let comment_label = format!("Comment on {lines}");
    assert!(
        harness.query_by_label(&comment_label).is_some(),
        "the + at the last line offers the comment"
    );
    settle(&mut harness);

    // The context menu offers copying and commenting on the selection.
    let on_last = harness.state().view.text_point(Pane::Inline, last, 10);
    right_click(&mut harness, on_last);
    settle(&mut harness);
    assert!(harness.query_by_label_contains("Copy selection").is_some());
    // The menu opens at the pointer; the selection's own button sits in the gutter.
    let entry = harness
        .query_all_by_label(&comment_label)
        .find(|node| node.rect().left() >= on_last.x - 1.0)
        .expect("the menu's comment entry")
        .rect();
    harness.click_at(entry);
    settle(&mut harness);
    assert!(harness.query_by_label(&comment_label).is_some(), "the comment card names the lines");
    assert_eq!(
        harness.state().view.range_rows(),
        [RangeRows { start: first, end: last, side: DiffSide::New, is_draft: true }],
        "the lines being commented on stay marked"
    );

    post_comment(&mut harness, "Both paths could share one helper.");
    let events = &harness.state().events;
    let created: Vec<&NewThread> = new_threads(events).collect();
    assert_eq!(created.len(), 1, "{events:?}");
    let anchor = created[0].anchor.as_ref().expect("an inline comment");
    assert_eq!(
        (anchor.old_line, anchor.new_line),
        (None, Some(last_new)),
        "the comment sits on the last line"
    );
    let range = anchor.range.as_ref().expect("a range");
    assert_eq!(
        (range.start_old_line, range.start_new_line, range.side),
        (None, Some(first_new), DiffSide::New)
    );
    let detail = range.detail.expect("positions for GitLab and Azure DevOps");
    assert_eq!((detail.start.new, detail.end.new), (first_new, last_new));
    // Added lines count the base lines before them: the three replaced `add` lines and the five loop lines.
    let (old_add, old_total) = (line_of(OLD, "pub fn add("), line_of(OLD, "pub fn total("));
    assert_eq!((detail.start.old, detail.end.old), (old_add + 3, old_total + 6));
    assert_eq!(
        (detail.start_offset, detail.end_offset),
        (1, line(NEW, last_new).chars().count() as u32 + 1),
        "whole lines"
    );
}

#[test]
fn dragging_the_plus_comments_on_the_lines_it_passes() {
    let mut harness = harness(fixture(Layout::SideBySide));
    settle(&mut harness);
    let first_new = line_of(NEW, "pub fn apply_discount(");
    let last_new = first_new + 3;
    let (first, last) =
        (row(harness.state(), None, Some(first_new)), row(harness.state(), None, Some(last_new)));
    let marker = |harness: &Harness<'_, Fixture>, row: usize| {
        harness.state().view.marker_point(Pane::Right, row)
    };

    harness.hover_at(marker(&harness, first));
    harness.step();
    assert!(harness.query_by_label("Add comment").is_some(), "hovering a line shows its +");
    let path: Vec<Pos2> = (first + 1..=last).map(|row| marker(&harness, row)).collect();
    let plus = marker(&harness, first);
    drag(&mut harness, plus, &path);
    settle(&mut harness);
    assert!(
        harness.query_by_label(&format!("Comment on lines {first_new}–{last_new}")).is_some(),
        "letting go opens the comment on the lines"
    );
    assert_eq!(
        harness.state().view.range_rows(),
        [RangeRows { start: first, end: last, side: DiffSide::New, is_draft: true }]
    );

    // Esc closes the comment; a click on the + still comments on its one line.
    harness.key_press(Key::Escape);
    settle(&mut harness);
    assert!(harness.query_by_label("Cancel").is_none(), "Esc closes the comment");
    let single = marker(&harness, first);
    harness.hover_at(single);
    harness.step();
    click(&mut harness, single, Modifiers::NONE);
    settle(&mut harness);
    assert!(harness.query_by_label(&format!("Comment on line {first_new}")).is_some());
    assert_eq!(
        harness.state().view.range_rows(),
        [RangeRows { start: first, end: first, side: DiffSide::New, is_draft: true }],
        "one line is marked as its own range"
    );
}

#[test]
fn multi_line_threads_mark_their_lines() {
    let mut fixture = fixture(Layout::SideBySide);
    let (old_total, new_total) = (line_of(OLD, "pub fn total("), line_of(NEW, "pub fn total("));
    let end = new_total + 2;
    fixture.threads.push(range_thread(
        (Some(old_total), Some(new_total)),
        (None, Some(end)),
        "The whole function reads better as one expression.",
    ));
    let mut harness = harness(fixture);
    settle(&mut harness);
    let start_row = row(harness.state(), Some(old_total), Some(new_total));
    let ranges = harness.state().view.range_rows().to_vec();
    assert_eq!(ranges.len(), 1);
    assert_eq!(
        (ranges[0].start, ranges[0].side, ranges[0].is_draft),
        (start_row, DiffSide::New, false)
    );
    assert!(
        ranges[0].end >= row(harness.state(), None, Some(end)),
        "the card follows the last line"
    );
}

#[test]
fn github_ranges_stay_within_one_block_of_changes() {
    let mut fixture = fixture(Layout::Inline);
    // A second change at the top gives the diff two blocks with collapsed lines between them.
    let head = NEW.replacen("use std::fmt;", "use core::fmt;", 1);
    fixture.diff = check::diff_of(OLD, &head);
    fixture.contents.new = Some(side(&head));
    fixture.can_comment_outside_hunks = false;
    let mut harness = harness(fixture);
    settle(&mut harness);
    let (old_total, new_total) = (line_of(OLD, "pub fn total("), line_of(NEW, "pub fn total("));
    let (first, last) = (
        row(harness.state(), None, Some(2)),
        row(harness.state(), Some(old_total), Some(new_total)),
    );

    let (first_number, last_number) = (
        harness.state().view.gutter_point(Pane::Inline, first),
        harness.state().view.gutter_point(Pane::Inline, last),
    );
    click(&mut harness, first_number, Modifiers::NONE);
    click(&mut harness, last_number, Modifiers::SHIFT);
    let label = format!("Comment on lines 2–{new_total}");
    let plus = harness.get_by_label(&label);
    assert!(plus.accesskit_node().is_disabled(), "GitHub takes no comment across collapsed lines");
    plus.hover();
    settle(&mut harness);
    assert!(
        harness.query_by_label_contains("only within one block of changes").is_some(),
        "the muted + says what to select instead"
    );

    harness.key_press_modifiers(Modifiers::ALT, Key::C);
    settle(&mut harness);
    assert!(harness.query_by_label("Cancel").is_none(), "Alt+C opens no comment either");
    assert!(harness.query_by_label(&label).is_some(), "the selection stays for another try");
}

/// The files tab around one diff: its header above the view.
struct Page {
    fixture: Fixture,
    services: Services,
    preview: Preview,
    file: ChangedFile,
}

/// An added or deleted file, on the data folders below `scratch`.
fn page(kind: ChangeKind, chosen: DiffLayout, scratch: &ScratchDir) -> Page {
    let (whole, old, new) = match kind {
        ChangeKind::Added => (DiffSide::New, "", NEW),
        _ => (DiffSide::Old, OLD, ""),
    };
    let mut services = services_in(scratch);
    services.settings.diff_layout = chosen;
    let file = ChangedFile {
        additions: Some(new.lines().count() as u32),
        deletions: Some(old.lines().count() as u32),
        category: FileCategory::Code,
        ..ChangedFile::sample(PATH, kind)
    };
    Page { fixture: check::whole_file_fixture(whole), services, preview: Preview::default(), file }
}

/// Where the line numbers of two lines of the one pane are.
fn numbers(
    harness: &Harness<'_, Page>,
    first: (Option<u32>, Option<u32>),
    last: (Option<u32>, Option<u32>),
) -> (Pos2, Pos2) {
    let fixture = &harness.state().fixture;
    let point = |(old_line, new_line)| {
        fixture.view.gutter_point(Pane::Inline, row(fixture, old_line, new_line))
    };
    (point(first), point(last))
}

fn page_harness(page: Page) -> Harness<'static, Page> {
    Harness::builder()
        .with_size([1180.0, 760.0])
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::motion::set_reduced(ctx, false);
        })
        .build_ui_state(
            |ui, page: &mut Page| {
                theme::activate(ui.ctx(), page.fixture.theme);
                let whole_file = super::whole_file_side(&page.file);
                page.fixture.layout =
                    super::file_layout(whole_file, page.services.settings.diff_layout);
                let inputs = HeaderInputs {
                    file: &page.file,
                    is_viewed: false,
                    changes: Some(page.fixture.view.change_position()),
                    is_rendered: false,
                    whole_file,
                    hides_whitespace: false,
                    edge_hint: None,
                };
                file_header::show(ui, &inputs, &mut page.services, &mut page.preview);
                check::show_diff(ui, &mut page.fixture);
            },
            page,
        )
}

#[test]
fn added_files_show_their_new_content_whole() {
    let scratch = ScratchDir::new("selection-added");
    let mut harness = page_harness(page(ChangeKind::Added, DiffLayout::SideBySide, &scratch));
    settle(&mut harness);
    assert_eq!(harness.state().fixture.layout, Layout::WholeFile(DiffSide::New));
    assert!(
        harness.query_by_label("New file").is_some(),
        "the header says why there is one version"
    );
    assert!(
        harness.query_by_label("Side by side").is_none(),
        "no layout to pick for a file with one version"
    );
    harness.get_by_label("New file").hover();
    settle(&mut harness);
    assert!(harness.query_by_label_contains("no old version to compare").is_some());
    // Right on the edge between two lines: only one of them offers its +.
    harness.hover_at(Pos2::new(900.0, 700.0));
    settle(&mut harness);
    assert_eq!(harness.query_all_by_label("Add comment").count(), 1);

    // Comments work on the one pane: the + at the end of the picked lines.
    let (first, last) = numbers(&harness, (None, Some(3)), (None, Some(6)));
    click(&mut harness, first, Modifiers::NONE);
    click(&mut harness, last, Modifiers::SHIFT);
    harness.click_at(harness.get_by_label("Comment on lines 3–6").rect());
    settle(&mut harness);
    post_comment(&mut harness, "Should the doc comment mention the currency?");
    let created = new_threads(&harness.state().fixture.events).next().expect("a new comment");
    let anchor = created.anchor.clone().expect("an inline comment");
    assert_eq!((anchor.old_line, anchor.new_line), (None, Some(6)));
    let range = anchor.range.expect("a range");
    assert_eq!(
        (range.start_old_line, range.start_new_line, range.side),
        (None, Some(3), DiffSide::New)
    );
    assert_eq!(
        harness.state().services.settings.diff_layout,
        DiffLayout::SideBySide,
        "the chosen layout stays for the next file"
    );
}

#[test]
fn deleted_files_show_their_old_content_whole() {
    let scratch = ScratchDir::new("selection-deleted");
    let mut harness = page_harness(page(ChangeKind::Deleted, DiffLayout::NewOnly, &scratch));
    settle(&mut harness);
    assert_eq!(harness.state().fixture.layout, Layout::WholeFile(DiffSide::Old));
    assert!(harness.query_by_label("Deleted file").is_some());
    assert!(
        harness.query_by_label("The new version is empty").is_none(),
        "the old content shows instead of an empty new version"
    );

    // Alt+C comments on the selected lines, anchored in the base file.
    let (first, last) = numbers(&harness, (Some(5), None), (Some(8), None));
    click(&mut harness, first, Modifiers::NONE);
    click(&mut harness, last, Modifiers::SHIFT);
    harness.key_press_modifiers(Modifiers::ALT, Key::C);
    settle(&mut harness);
    assert!(harness.query_by_label("Comment on lines 5–8").is_some());
    post_comment(&mut harness, "Is anything still using the cart?");
    let created = new_threads(&harness.state().fixture.events).next().expect("a new comment");
    let anchor = created.anchor.clone().expect("an inline comment");
    assert_eq!(
        (anchor.old_line, anchor.new_line),
        (Some(8), None),
        "deleted lines are commented on in the base file"
    );
    let range = anchor.range.expect("a range");
    assert_eq!(
        (range.start_old_line, range.start_new_line, range.side),
        (Some(5), None, DiffSide::Old)
    );
    assert_eq!(harness.state().services.settings.diff_layout, DiffLayout::NewOnly);
}

/// One layout the selection tests drag over: the lines the drag starts and ends on, as (base, head)
/// line numbers, and the window width.
struct LayoutCase {
    name: &'static str,
    fixture: Fixture,
    pane: Pane,
    first: (Option<u32>, Option<u32>),
    last: (Option<u32>, Option<u32>),
    /// The line the drag of [`a_selection_offers_one_comment_button`] ends on.
    comment_last: (Option<u32>, Option<u32>),
    width: f32,
}

/// Where the drag starts on the first line and ends on the last.
const BLOCK_FROM: usize = 12;
const BLOCK_TO: usize = 5;

/// Three added lines in each layout, and in the wrapped inline diff the signature of `total` over
/// its removed loop and its wrapped new body to its brace; the comment's drag there ends on the
/// first line of that body.
fn layout_cases() -> Vec<LayoutCase> {
    let is_empty = line_of(NEW, "pub fn is_empty(");
    let added = ((None, Some(is_empty)), (None, Some(is_empty + 2)));
    let (old_total, new_total) = (line_of(OLD, "pub fn total("), line_of(NEW, "pub fn total("));
    let mut wrapped = fixture(Layout::Inline);
    wrapped.wrap = true;
    let case = |name, fixture, pane| LayoutCase {
        name,
        fixture,
        pane,
        first: added.0,
        last: added.1,
        comment_last: added.1,
        width: 1180.0,
    };
    vec![
        case("inline", fixture(Layout::Inline), Pane::Inline),
        case("side-by-side", fixture(Layout::SideBySide), Pane::Right),
        case("new-only", fixture(Layout::NewOnly), Pane::Inline),
        case("whole-file", check::whole_file_fixture(DiffSide::New), Pane::Inline),
        LayoutCase {
            name: "inline-wrapped",
            fixture: wrapped,
            pane: Pane::Inline,
            first: (Some(old_total), Some(new_total)),
            last: (Some(old_total + 6), Some(new_total + 2)),
            comment_last: (None, Some(new_total + 1)),
            width: 640.0,
        },
    ]
}

/// A drag over three lines (more where removed lines sit between them) paints one block in the
/// selection color, in every layout, wrapped too. Checked on the frame's painted shapes.
#[test]
fn a_text_selection_paints_one_continuous_block() {
    for case in layout_cases() {
        let mut harness = harness(case.fixture);
        harness.set_size(egui::vec2(case.width, 760.0));
        settle(&mut harness);
        let (first, last) = (
            row(harness.state(), case.first.0, case.first.1),
            row(harness.state(), case.last.0, case.last.1),
        );
        let start = harness.state().view.text_point(case.pane, first, BLOCK_FROM);
        let end = harness.state().view.text_point(case.pane, last, BLOCK_TO);
        drag(&mut harness, start, &[start.lerp(end, 0.5), end]);
        let copied = copy(&mut harness).expect("a selection");
        let parts: Vec<&str> = copied.split('\n').collect();
        assert_eq!(
            parts.len(),
            last - first + 1,
            "{}: one selected part per row: {copied:?}",
            case.name
        );
        // The first line is selected from the caret to its end.
        let first_line = line(NEW, case.first.1.expect("a head line"));
        let from = first_line.chars().count() - parts[0].chars().count();
        assert_eq!(from, BLOCK_FROM, "{}: the selection starts where the drag did", case.name);
        settle(&mut harness);
        let fills = selection_fills(&harness, theme::Palette::LIGHT.text_selection);
        let areas: Vec<TextArea> =
            (first..=last).map(|row| harness.state().view.text_area(case.pane, row)).collect();
        assert_block(&fills, &areas, &parts, case.name);
    }
}

/// The rectangles the last frame filled in the selection `color`, each cut to its clip rect.
fn selection_fills(harness: &Harness<'_, Fixture>, color: egui::Color32) -> Vec<egui::Rect> {
    harness
        .output()
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) if rect.fill == color => {
                Some(rect.rect.intersect(clipped.clip_rect))
            }
            _ => None,
        })
        .collect()
}

/// The selection paints `parts` (what was copied, line by line) on the rows `areas` as one block
/// of `fills`: every visual row covered from its first selected column to its last (and a newline
/// past it where the selection goes on), top to bottom, leading whitespace included, and no pixel
/// row between two lines left out.
fn assert_block(fills: &[egui::Rect], areas: &[TextArea], parts: &[&str], name: &str) {
    // The pixel at (x, y) is selected when a fill covers its center.
    let is_selected = |x: i64, y: i64| {
        let center = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
        fills.iter().any(|fill| fill.contains(center))
    };
    let mut above: Option<(f32, std::ops::Range<f32>)> = None;
    for (index, (area, part)) in areas.iter().zip(parts).enumerate() {
        let is_last = index + 1 == parts.len();
        let from = if index == 0 { BLOCK_FROM } else { 0 };
        let columns = from..from + part.chars().count();
        // The first and middle lines are selected to their end, so that is their length.
        let length = if is_last { usize::MAX } else { columns.end };
        let per_row = area.wrap_columns.unwrap_or(usize::MAX);
        let visual_rows = (area.rect.height() / area.line_height).round().max(1.0) as usize;
        for visual in 0..visual_rows {
            let row_columns =
                visual.saturating_mul(per_row)..(visual + 1).saturating_mul(per_row).min(length);
            let (a, b) = (
                columns.start.clamp(row_columns.start, row_columns.end),
                columns.end.clamp(row_columns.start, row_columns.end),
            );
            let newline = if !is_last && visual + 1 == visual_rows {
                text_selection::newline_width(area.char_width)
            } else {
                0.0
            };
            let left = area.rect.left() + (a - row_columns.start) as f32 * area.char_width;
            let right =
                area.rect.left() + (b - row_columns.start) as f32 * area.char_width + newline;
            if right <= left {
                continue;
            }
            let top = area.rect.top() + visual as f32 * area.line_height;
            let bottom =
                if visual + 1 == visual_rows { area.rect.bottom() } else { top + area.line_height };
            let xs = (left + 1.0).ceil() as i64..=(right - 1.0).floor() as i64;
            let (first_y, last_y) = (top.ceil() as i64, bottom.floor() as i64 - 1);
            let what = format!("{name}: line {index}, visual row {visual}");
            assert_row_selected(&is_selected, xs, first_y..=last_y, &what);
            // A whole line: its indentation is selected from the top of the row to its bottom.
            if index > 0 && !is_last && visual == 0 && part.starts_with("    ") {
                let indent = (area.rect.left() + 1.0).ceil() as i64
                    ..=(area.rect.left() + 4.0 * area.char_width - 1.0).floor() as i64;
                let holes = indent
                    .flat_map(|x| (first_y..=last_y).map(move |y| (x, y)))
                    .filter(|(x, y)| !is_selected(*x, *y))
                    .count();
                assert_eq!(
                    holes, 0,
                    "{name}: line {index}: the indentation is not selected all the way"
                );
            }
            // Where this line meets the one above: no pixel row between them is left out.
            if visual == 0
                && let Some((boundary, span)) = above.take()
            {
                let overlap = (left.max(span.start) + 1.0).ceil() as i64
                    ..=(right.min(span.end) - 1.0).floor() as i64;
                for y in (boundary.floor() as i64 - 1)..=(boundary.ceil() as i64) {
                    let gaps: Vec<i64> = overlap.clone().filter(|x| !is_selected(*x, y)).collect();
                    assert!(
                        gaps.is_empty(),
                        "{name}: a gap between lines {} and {index} at y {y}, x {gaps:?}",
                        index - 1
                    );
                }
            }
            if visual + 1 == visual_rows {
                above = Some((bottom, left..right));
            }
        }
    }
}

/// The selection covers columns `xs` of the pixel rows `ys` of one visual row (`what`) without a
/// hole: the fills lie under the glyphs, so every pixel of the span counts.
fn assert_row_selected(
    is_selected: &impl Fn(i64, i64) -> bool,
    xs: std::ops::RangeInclusive<i64>,
    ys: std::ops::RangeInclusive<i64>,
    what: &str,
) {
    for y in ys {
        let gaps: Vec<i64> = xs.clone().filter(|x| !is_selected(*x, y)).collect();
        assert!(gaps.is_empty(), "{what}: x {gaps:?} at y {y} not selected (span {xs:?})");
    }
}

/// A selection over several lines offers exactly one comment button, "Comment on lines X–Y", on
/// the line the selection was dragged to (as Azure DevOps does): the last line after a drag down,
/// the first after a drag up. It stays the only one wherever the pointer rests, and its hint opens
/// beyond that line, never over the selected code. Shift+↑ takes the button along with the
/// selection's free end. Without a selection the hovered line offers its own button again.
#[test]
fn a_selection_offers_one_comment_button() {
    let is_empty = line_of(NEW, "pub fn is_empty(");
    for case in layout_cases() {
        let head = |(_, new_line): (Option<u32>, Option<u32>)| new_line.expect("a head line");
        let label = format!("Comment on lines {}–{}", head(case.first), head(case.comment_last));
        let mut harness = harness(case.fixture);
        harness.set_size(egui::vec2(case.width, 760.0));
        settle(&mut harness);
        let (first, last) = (
            row(harness.state(), case.first.0, case.first.1),
            row(harness.state(), case.comment_last.0, case.comment_last.1),
        );
        for is_up in [false, true] {
            let name = format!("{}-{}", case.name, if is_up { "up" } else { "down" });
            let (top, bottom) = (
                harness.state().view.text_point(case.pane, first, 8),
                harness.state().view.text_point(case.pane, last, 6),
            );
            let (from, to, focus, origin) =
                if is_up { (bottom, top, first, last) } else { (top, bottom, last, first) };
            drag(&mut harness, from, &[from.lerp(to, 0.5), to]);

            let button = ButtonAt { pane: case.pane, label: &label, focus, origin };
            assert_one_button_wherever_it_rests(&mut harness, &button, &name);
            assert_hint_beyond_the_selection(&mut harness, &button, (first, last), is_up, &name);
            harness.hover_at(harness.state().view.marker_point(case.pane, origin));
            settle(&mut harness);

            // Shift+↑ moves the free end, and the button with it, one line up.
            if case.name == "inline" && is_up {
                harness.key_press_modifiers(Modifiers::SHIFT, Key::ArrowUp);
                settle(&mut harness);
                let grown = format!("Comment on lines {}–{}", is_empty - 1, is_empty + 2);
                assert_eq!(
                    harness.query_all_by_label_contains("Comment on line").count(),
                    1,
                    "{name}: still one button"
                );
                assert_button_on(&harness, &grown, case.pane, first - 1, &name);
            }

            // Without a selection the hovered line offers its own button.
            harness.key_press(Key::Escape);
            harness.hover_at(harness.state().view.marker_point(case.pane, first));
            settle(&mut harness);
            assert_eq!(
                harness.query_all_by_label("Add comment").count(),
                1,
                "{name}: the hovered line's button"
            );
            assert_eq!(
                harness.query_all_by_label_contains("Comment on line").count(),
                0,
                "{name}: no selection, no range button"
            );
        }
    }
}

/// The "Comment on lines X–Y" button `label` of a selection in `pane`, on the `focus` row the
/// drag ended on; the drag started on the `origin` row.
struct ButtonAt<'a> {
    pane: Pane,
    label: &'a str,
    focus: usize,
    origin: usize,
}

/// With the pointer back where the drag started, and on the other pane: still one button, on the
/// line the drag ended on, and no hover button.
fn assert_one_button_wherever_it_rests(
    harness: &mut Harness<'_, Fixture>,
    button: &ButtonAt<'_>,
    name: &str,
) {
    let mut rests = vec![harness.state().view.marker_point(button.pane, button.origin)];
    if button.pane == Pane::Right {
        rests.push(harness.state().view.marker_point(Pane::Left, button.origin));
    }
    for rest in rests {
        harness.hover_at(rest);
        settle(harness);
        assert_eq!(
            harness.query_all_by_label("Add comment").count(),
            0,
            "{name}: no hover button while lines are selected"
        );
        assert_eq!(
            harness.query_all_by_label_contains("Comment on line").count(),
            1,
            "{name}: one comment button"
        );
        assert_button_on(harness, button.label, button.pane, button.focus, name);
    }
}

/// The button's hint names the lines and opens beyond the focus line, clear of the selected rows
/// `first..=last`: above them after a drag up, below after a drag down.
fn assert_hint_beyond_the_selection(
    harness: &mut Harness<'_, Fixture>,
    button: &ButtonAt<'_>,
    (first, last): (usize, usize),
    is_up: bool,
    name: &str,
) {
    harness.hover_at(harness.state().view.marker_point(button.pane, button.focus));
    settle(harness);
    let hint = harness.get_by_label(&format!("{} ({COMMENT_SHORTCUT})", button.label)).rect();
    let selection_top = harness.state().view.text_area(button.pane, first).rect.top();
    let selection_bottom = harness.state().view.text_area(button.pane, last).rect.bottom();
    if is_up {
        assert!(
            hint.bottom() <= selection_top + 0.5,
            "{name}: dragged up, the hint at {hint:?} opens above the first line at {selection_top}"
        );
    } else {
        assert!(
            hint.top() >= selection_bottom - 0.5,
            "{name}: dragged down, the hint at {hint:?} opens below the last line at {selection_bottom}"
        );
    }
}

/// Asserts that the button labelled `label` sits on `row` of `pane`, in the gutter left of the code.
fn assert_button_on(
    harness: &Harness<'_, Fixture>,
    label: &str,
    pane: Pane,
    row: usize,
    name: &str,
) {
    let button = harness.get_by_label(label).rect();
    let line = harness.state().view.text_area(pane, row).rect;
    assert!(
        line.y_range().contains(button.center().y),
        "{name}: the button at {button:?} is not on row {row} ({line:?})"
    );
    assert!(
        button.right() <= line.left() + 0.5,
        "{name}: the button at {button:?} covers the code"
    );
}

/// Where the open comment card is on screen: from the top of its "Lines X–Y" pill to the bottom
/// of its Cancel button.
fn card_span(harness: &Harness<'_, Fixture>, label: &str) -> (f32, f32) {
    let pill = harness.get_by_label(label).rect();
    let cancel = harness.get_by_label("Cancel").rect();
    (pill.top(), cancel.bottom())
}

/// The diff area on screen.
fn diff_rect(harness: &Harness<'_, Fixture>) -> egui::Rect {
    harness
        .query_all_by_role(Role::Document)
        .find(|node| {
            node.accesskit_node().label().is_some_and(|label| label.starts_with("Diff of"))
        })
        .expect("the diff area")
        .rect()
}

/// The comment card opens where the drag ended, next to the pointer: below the last line after a
/// drag down, above the first after a drag up. The line the pointer is at keeps its place while
/// the card fits; at the edge of the view the diff scrolls just enough to show the whole card.
/// Inline and side by side alike.
#[test]
fn the_comment_opens_where_the_drag_ended() {
    for drag_case in drag_cases() {
        let DragCase { layout, pane, ref name, is_up, at_edge, first_new, last_new } = drag_case;
        let label = format!("Comment on lines {first_new}–{last_new}");
        let mut harness = harness(fixture(layout));
        // Shorter than the diff, so it scrolls: a card above `is_empty` pushes the code above up.
        harness.set_size(egui::vec2(1180.0, 640.0));
        settle(&mut harness);
        let lines = |harness: &Harness<'_, Fixture>| {
            (
                row(harness.state(), None, Some(first_new)),
                row(harness.state(), None, Some(last_new)),
            )
        };
        let (first, last) = lines(&harness);
        if at_edge && !is_up {
            // The view ends right below the last line: no room for the card under it.
            let bottom = harness.state().view.text_area(pane, last).rect.bottom();
            harness.set_size(egui::vec2(1180.0, bottom + 6.0));
            settle(&mut harness);
        }
        let (top, bottom) = (
            harness.state().view.text_point(pane, first, 8),
            harness.state().view.text_point(pane, last, 6),
        );
        let (from, to) = if is_up { (bottom, top) } else { (top, bottom) };
        drag(&mut harness, from, &[from.lerp(to, 0.5), to]);
        harness.key_press_modifiers(Modifiers::ALT, Key::C);
        settle(&mut harness);
        settle(&mut harness);

        let (first, last) = lines(&harness);
        assert_card_next_to_the_drag(&harness, (first, last), &label, &drag_case, to.y);
        let ranges = harness.state().view.range_rows();
        assert_eq!(ranges.len(), 1, "{name}");
        assert_eq!(
            (ranges[0].start, ranges[0].end, ranges[0].is_draft),
            (first, last, true),
            "{name}: the picked lines stay marked"
        );
    }
}

/// How a comment's lines were picked: in a layout, dragging up or down, in the middle of the view
/// or at its edge, where the diff has to scroll for the card; and the head lines picked.
struct DragCase {
    layout: Layout,
    pane: Pane,
    name: String,
    is_up: bool,
    at_edge: bool,
    first_new: u32,
    last_new: u32,
}

/// Every drag of the comment card tests, inline and side by side.
fn drag_cases() -> Vec<DragCase> {
    let is_empty = line_of(NEW, "pub fn is_empty(");
    let add = line_of(NEW, "pub fn add(");
    let mut cases = Vec::new();
    for (layout, pane, layout_name) in [
        (Layout::Inline, Pane::Inline, "inline"),
        (Layout::SideBySide, Pane::Right, "side-by-side"),
    ] {
        for is_up in [false, true] {
            for at_edge in [false, true] {
                let direction = if is_up { "up" } else { "down" };
                let place = if at_edge { "edge" } else { "middle" };
                // `add` is near the top of the view, `is_empty` further down: dragged up at the
                // edge, the card has no room above `add`; dragged down in the middle, it has room below.
                let (first_new, last_new) =
                    if is_up == at_edge { (add, add + 1) } else { (is_empty, is_empty + 2) };
                cases.push(DragCase {
                    layout,
                    pane,
                    name: format!("{layout_name}-{direction}-{place}"),
                    is_up,
                    at_edge,
                    first_new,
                    last_new,
                });
            }
        }
    }
    cases
}

/// The `card` opened beside the picked lines, from `first_line` to `last_line`, inside the `view`:
/// above the first line after a drag up, below the last after a drag down.
fn assert_card_beside(
    card: egui::Rect,
    first_line: egui::Rect,
    last_line: egui::Rect,
    view: egui::Rect,
    is_up: bool,
    case: &str,
) {
    assert!(
        card.top() >= view.top() - 0.5 && card.bottom() <= view.bottom() + 0.5,
        "{case}: the card {card:?} is outside the view {view:?}"
    );
    if is_up {
        assert!(
            card.bottom() <= first_line.top() + 0.5,
            "{case}: dragged up, the card ends at {}, below the first line at {}",
            card.bottom(),
            first_line.top()
        );
    } else {
        assert!(
            card.top() >= last_line.bottom() - 0.5,
            "{case}: dragged down, the card starts at {}, above the last line at {}",
            card.top(),
            last_line.bottom()
        );
    }
}

/// The card labelled `label` for the rows `first..=last` opened next to where the drag ended
/// (`pointer_y`) and inside the view, which scrolled no more than the card needed.
fn assert_card_next_to_the_drag(
    harness: &Harness<'_, Fixture>,
    (first, last): (usize, usize),
    label: &str,
    drag: &DragCase,
    pointer_y: f32,
) {
    let DragCase { pane, is_up, at_edge, .. } = *drag;
    let case = drag.name.as_str();
    let view = diff_rect(harness);
    let (card_top, card_bottom) = card_span(harness, label);
    let card = egui::Rect::from_x_y_ranges(view.x_range(), card_top..=card_bottom);
    let first_line = harness.state().view.text_area(pane, first).rect;
    let last_line = harness.state().view.text_area(pane, last).rect;
    assert!(
        first_line.top() >= view.top() - 0.5 && last_line.bottom() <= view.bottom() + 0.5,
        "{case}: the picked lines stay in view"
    );
    assert_card_beside(card, first_line, last_line, view, is_up, case);
    let focus_after = if is_up { first_line.center().y } else { last_line.center().y };
    if at_edge {
        // Scrolled just enough: the card ends near the edge it would have crossed.
        let left_over = if is_up { card_top - view.top() } else { view.bottom() - card_bottom };
        assert!(
            left_over < 40.0,
            "{case}: the view scrolled more than the card needed ({left_over} left over)"
        );
    } else {
        assert!(
            (focus_after - pointer_y).abs() < 1.0,
            "{case}: the line at the pointer moved from {pointer_y} to {focus_after}"
        );
    }
}

/// The diff plus what every frame that reached the screen showed (discarded passes left out).
struct Recorder {
    fixture: Fixture,
    is_recording: bool,
    /// Per shown frame: where the comment card was, and whether lines showed as picked.
    frames: Vec<(Option<egui::Rect>, bool)>,
}

fn recording_harness(fixture: Fixture, height: f32) -> Harness<'static, Recorder> {
    Harness::builder()
        .with_size([1180.0, height])
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::motion::set_reduced(ctx, false);
        })
        .build_ui_state(
            |ui, recorder: &mut Recorder| {
                theme::activate(ui.ctx(), recorder.fixture.theme);
                check::show_diff(ui, &mut recorder.fixture);
                if recorder.is_recording && !ui.ctx().will_discard() {
                    let view = &recorder.fixture.view;
                    recorder.frames.push((view.composer_card(), view.marks_lines()));
                }
            },
            Recorder { fixture, is_recording: false, frames: Vec::new() },
        )
}

/// From the frame the "+" is let go on, the comment card shows up once, in its final place, and
/// stays there: no frame paints it at an estimated height or on the other side first, and the
/// picked lines stay marked until it is there. Both drag directions, in the middle of the view
/// and at its edge, where the diff has to scroll for it.
#[test]
fn the_comment_card_opens_in_place_without_moving() {
    for drag_case in drag_cases() {
        let DragCase { layout, pane, is_up, at_edge, first_new, last_new, .. } = drag_case;
        let mut harness = recording_harness(fixture(layout), 640.0);
        settle(&mut harness);
        let rows = |harness: &Harness<'_, Recorder>| {
            let fixture = &harness.state().fixture;
            (
                fixture.view.row_of(&fixture.diff, None, Some(first_new)),
                fixture.view.row_of(&fixture.diff, None, Some(last_new)),
            )
        };
        if at_edge && !is_up {
            let (_, last) = rows(&harness);
            let bottom = harness.state().fixture.view.text_area(pane, last).rect.bottom();
            harness.set_size(egui::vec2(1180.0, bottom + 6.0));
            settle(&mut harness);
        }
        let (first, last) = rows(&harness);
        let marker = |harness: &Harness<'_, Recorder>, row: usize| {
            harness.state().fixture.view.marker_point(pane, row)
        };
        let (from, to) = if is_up {
            (marker(&harness, last), marker(&harness, first))
        } else {
            (marker(&harness, first), marker(&harness, last))
        };
        harness.hover_at(from);
        harness.step();
        harness.event(button(from, PointerButton::Primary, true, Modifiers::NONE));
        harness.step();
        harness.hover_at(from.lerp(to, 0.5));
        harness.step();
        harness.hover_at(to);
        harness.step();
        harness.state_mut().is_recording = true;
        harness.event(button(to, PointerButton::Primary, false, Modifiers::NONE));
        harness.run_steps(12);

        let picked = rows(&harness);
        assert_card_steady(&harness, picked, &drag_case);
    }
}

/// From the first frame that showed the card it stays in one place and size, the picked rows
/// `first..=last` stay marked in every frame, and that place is next to the drag's end (above the
/// rows after a drag up), inside the view.
fn assert_card_steady(
    harness: &Harness<'_, Recorder>,
    (first, last): (usize, usize),
    drag: &DragCase,
) {
    let case = drag.name.as_str();
    let frames = &harness.state().frames;
    let first_card = frames
        .iter()
        .position(|(card, _)| card.is_some())
        .unwrap_or_else(|| panic!("{case}: the card never showed"));
    let shown = frames[first_card].0.expect("a card");
    for (index, (card, _)) in frames.iter().enumerate().skip(first_card) {
        let card =
            card.unwrap_or_else(|| panic!("{case}: frame {index} lost the card: {frames:?}"));
        assert!(
            (card.min - shown.min).length() < 0.5 && (card.size() - shown.size()).length() < 0.5,
            "{case}: the card moved from {shown:?} (frame {first_card}) to {card:?} (frame {index})"
        );
    }
    assert!(
        frames.iter().all(|(_, marked)| *marked),
        "{case}: the picked lines blink unmarked in a frame: {frames:?}"
    );
    // And that first place is the right one: next to the pointer, inside the view.
    let view = &harness.state().fixture.view;
    let (first_line, last_line) =
        (view.text_area(drag.pane, first).rect, view.text_area(drag.pane, last).rect);
    assert_card_beside(shown, first_line, last_line, harness.ctx.content_rect(), drag.is_up, case);
}
