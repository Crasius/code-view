//! An added or a deleted file on the Files tab: its header above the one version shown whole, and
//! comments picked on that version's line numbers.

use cv_testkit::{Clicks, ScratchDir};
use egui::{Event, Key, Modifiers, Pos2, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{DiffSide, FileCategory, NewThread};
use review_git::{ChangeKind, ChangedFile};
use review_store::DiffLayout;

use crate::tests::services_in;
use cv_base::theme;
use cv_code::diff_rows::Layout;
use cv_code::line_source::Pane;
use cv_pr_diff::diff_view::DiffEvent;
use cv_pr_diff::testing::{self as check, Fixture, NEW, OLD, PATH, click_with, settle};
use cv_pr_rendered::file_header::{self, HeaderInputs};
use cv_pr_rendered::preview::Preview;
use cv_services::Services;

/// The comments the view asked to create, in order.
fn new_threads(events: &[DiffEvent]) -> impl Iterator<Item = &NewThread> {
    events.iter().filter_map(|event| match event {
        DiffEvent::Create { thread, .. } => Some(thread),
        _ => None,
    })
}

/// Writes `body` into the open comment and posts it.
fn post_comment<State>(harness: &mut Harness<'_, State>, body: &str) {
    harness.event(Event::Text(body.to_owned()));
    harness.step();
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Comment").rect());
    settle(harness);
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
        let row = fixture.view.row_of(&fixture.diff, old_line, new_line);
        fixture.view.gutter_point(Pane::Inline, row)
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
    click_with(&mut harness, first, Modifiers::NONE);
    click_with(&mut harness, last, Modifiers::SHIFT);
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
    click_with(&mut harness, first, Modifiers::NONE);
    click_with(&mut harness, last, Modifiers::SHIFT);
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
