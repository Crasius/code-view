//! Posting a comment hands the composer over to the posted comment without moving the diff: the
//! composer folds into the comment as it will be posted ("Posting…") with an ease, the platform's
//! answer and the reloaded threads swap in at the same height, and a failure brings the composer
//! back with the text, the reason and Retry, again with an ease. Measured frame by frame at 120 Hz
//! on the code line below the comment and on the comment's own text.

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{
    NewThread, ReviewThread, ThreadAnchor, ThreadComment, ThreadStatus, UserIdentity,
};

use cv_base::theme;
use cv_code::diff_rows::Layout;
use cv_code::line_source::Pane;
use cv_pr_diff::diff_view::{DiffEvent, DiffInputs};
use cv_pr_diff::find::FindInputs;
use cv_pr_diff::testing::{Fixture, NEW, PATH, fixture, line_of};
use cv_pr_threads::{Delivery, ThreadEvent, new_thread_key};

const FRAME: f32 = 1.0 / 120.0;
/// Frames the hand-off may take: the ease plus the frame it starts in and one to land.
const EASE_FRAMES: usize = (cv_widgets::settle::DURATION / FRAME) as usize + 3;
const BODY: &str = "Prefer `iter().sum()` here, it reads as the total.";

struct Scene {
    fixture: Fixture,
    /// The threads' version, bumped with every "reload".
    version: u64,
    /// Draft keys of the new threads the view asked to post.
    posted: Vec<(String, NewThread)>,
}

fn me() -> UserIdentity {
    UserIdentity {
        id: "me".into(),
        display_name: "Erika Brandt".into(),
        login: Some("ebrandt".into()),
        avatar_url: None,
    }
}

fn harness() -> Harness<'static, Scene> {
    let mut fixture = fixture(Layout::Inline);
    fixture.threads_ui.set_me(me());
    // No `motion::install`: the eases run whatever the machine's motion setting is.
    Harness::builder()
        .with_size([1180.0, 760.0])
        .with_step_dt(FRAME)
        .with_context_setup(theme::fonts::install)
        .build_ui_state(
            |ui, scene: &mut Scene| {
                theme::activate(ui.ctx(), scene.fixture.theme);
                let fixture = &mut scene.fixture;
                fixture.threads_ui.begin_frame_for_tests(ui.ctx(), &fixture.threads, scene.version);
                let threads: Vec<&ReviewThread> = fixture.threads.iter().collect();
                let inputs = DiffInputs {
                    diff: &fixture.diff,
                    contents: Some(&fixture.contents),
                    threads: &threads,
                    layout: fixture.layout,
                    font_size: 13.0,
                    wrap: false,
                    can_comment: true,
                    can_comment_outside_hunks: true,
                    path: PATH,
                    old_path: None,
                    line_spans: None,
                    show_threads: true,
                    find: FindInputs::default(),
                };
                // What the files tab does with the view's events, without a platform.
                match fixture.view.show(ui, &inputs, &mut fixture.split, &mut fixture.threads_ui) {
                    Some(DiffEvent::Create { key, thread }) => {
                        fixture.threads_ui.post_new_thread_for_tests(&key, thread.clone());
                        scene.posted.push((key, thread));
                    }
                    Some(DiffEvent::Thread(ThreadEvent::Reply { thread_id, body, .. })) => {
                        fixture.threads_ui.reply_for_tests(&thread_id, &body);
                    }
                    _ => {}
                }
            },
            Scene { fixture, version: 1, posted: Vec::new() },
        )
}

/// The head line the comments go on, and the one below it.
fn commented_line() -> u32 {
    line_of(NEW, "pub fn total(&self)")
}

/// Top of the code line right below the commented line and its card.
fn below_y(harness: &Harness<'_, Scene>) -> f32 {
    let fixture = &harness.state().fixture;
    let row = fixture.view.row_of(&fixture.diff, None, Some(commented_line() + 1));
    fixture.view.text_area(Pane::Inline, row).rect.top()
}

/// Top of the commented line: everything above the card.
fn line_y(harness: &Harness<'_, Scene>) -> f32 {
    let fixture = &harness.state().fixture;
    let row = fixture.view.row_of(&fixture.diff, None, Some(commented_line()));
    fixture.view.text_area(Pane::Inline, row).rect.top()
}

/// Top of the comment's text where a card shows it.
fn comment_top(harness: &Harness<'_, Scene>) -> Option<f32> {
    harness.query_all_by_label_contains("Prefer").map(|node| node.rect().top()).reduce(f32::min)
}

/// Runs `frames` frames and records (line below, comment top) after each.
fn record(harness: &mut Harness<'_, Scene>, frames: usize) -> Vec<(f32, Option<f32>)> {
    (0..frames)
        .map(|_| {
            harness.step();
            (below_y(harness), comment_top(harness))
        })
        .collect()
}

/// The line below moved from `from` to `to` without a jump: one way only, in at most the ease's
/// frames, no single frame taking most of the way, then resting.
fn assert_eased(case: &str, frames: &[(f32, Option<f32>)], from: f32, to: f32) {
    let ys: Vec<f32> = frames.iter().map(|(y, _)| *y).collect();
    let distance = (to - from).abs();
    assert!(distance > 20.0, "{case}: the hand-off changes the height ({from} → {to}): {ys:?}");
    let mut previous = from;
    let mut largest_step: f32 = 0.0;
    for &y in &ys {
        assert!((y - previous) * (to - from) >= -0.5, "{case}: the line below went back: {ys:?}");
        largest_step = largest_step.max((y - previous).abs());
        previous = y;
    }
    assert!(
        largest_step <= distance * 0.5,
        "{case}: one frame moved {largest_step} of {distance}, a jump: {ys:?}"
    );
    let landed = ys
        .iter()
        .position(|y| (y - to).abs() <= 1.0)
        .unwrap_or_else(|| panic!("{case}: never reached {to}: {ys:?}"));
    assert!(landed <= EASE_FRAMES, "{case}: took {landed} frames (at most {EASE_FRAMES}): {ys:?}");
    assert!(
        ys[landed..].iter().all(|y| (y - to).abs() <= 1.0),
        "{case}: moved after landing: {ys:?}"
    );
}

/// Nothing moved by more than a pixel: the line below, and the comment's text where shown.
fn assert_still(case: &str, frames: &[(f32, Option<f32>)], below: f32, comment: f32) {
    for (index, (y, top)) in frames.iter().enumerate() {
        assert!(
            (y - below).abs() <= 1.0,
            "{case}: frame {index} moved the line below from {below} to {y}: {frames:?}"
        );
        let top =
            top.unwrap_or_else(|| panic!("{case}: frame {index} lost the comment: {frames:?}"));
        assert!(
            (top - comment).abs() <= 1.0,
            "{case}: frame {index} moved the comment from {comment} to {top}: {frames:?}"
        );
    }
}

/// Opens a comment on the commented line with the "+", types `BODY` and lets it settle.
fn write_comment(harness: &mut Harness<'_, Scene>) {
    harness.run_steps(30);
    let row = harness.state().fixture.view.row_of(
        &harness.state().fixture.diff,
        None,
        Some(commented_line()),
    );
    let plus = harness.state().fixture.view.marker_point(Pane::Inline, row);
    harness.hover_at(plus);
    harness.step();
    harness.event(egui::Event::PointerButton {
        pos: plus,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    harness.step();
    harness.event(egui::Event::PointerButton {
        pos: plus,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(30);
    assert!(harness.state().fixture.view.composer().is_some(), "the + opens a comment");
    harness.event(egui::Event::Text(BODY.to_owned()));
    harness.run_steps(30);
}

/// The thread the platform stores for `new`, as it sends it back and the reload shows it.
fn stored(new: &NewThread) -> ReviewThread {
    ReviewThread {
        id: "t-created".into(),
        status: ThreadStatus::Active,
        anchor: new.anchor.clone(),
        is_outdated: false,
        origin: None,
        is_system: false,
        can_reply: true,
        can_resolve: true,
        comments: vec![ThreadComment {
            id: "c-created".into(),
            author: me(),
            body: new.body.clone(),
            created_at: time::OffsetDateTime::now_utc(),
            updated_at: None,
        }],
    }
}

/// Submit → pending → platform's answer → reloaded threads: the composer folds into the comment
/// with an ease, and from then on neither the line below nor the comment moves by a pixel; the
/// code above never moves.
#[test]
fn posting_a_new_thread_hands_the_composer_to_the_comment_in_place() {
    let mut harness = harness();
    write_comment(&mut harness);
    let (composer_below, above) = (below_y(&harness), line_y(&harness));

    harness.get_by_role_and_label(Role::Button, "Comment").click();
    let folding = record(&mut harness, 40);
    let (key, new) =
        harness.state().posted.first().cloned().expect("the view asked to post the thread");
    assert_eq!(key, new_thread_key(new.anchor.as_ref().expect("on a line")));
    let pending_below = below_y(&harness);
    assert!(pending_below < composer_below, "the comment is shorter than its composer");
    assert_eased("submit", &folding, composer_below, pending_below);
    assert!(harness.query_by_label("Posting…").is_some(), "the comment says it is on its way");
    let comment = comment_top(&harness).expect("the comment shows while it is posted");
    let pending = folding.iter().skip_while(|(_, top)| top.is_none()).copied().collect::<Vec<_>>();
    assert!(
        pending.iter().all(|(_, top)| top.is_some_and(|top| (top - comment).abs() <= 1.0)),
        "the comment stays where it appeared: {pending:?}"
    );

    // The platform answers: its copy (ids, time) takes over in place.
    let version = harness.state().version;
    harness.state_mut().fixture.threads_ui.deliver_for_tests(
        &key,
        Delivery { thread: Some(stored(&new)), ..Delivery::default() },
        version,
    );
    let answered = record(&mut harness, 30);
    assert_still("answered", &answered, pending_below, comment);
    assert!(harness.query_by_label("Posting…").is_none(), "the platform has it");

    // The reloaded threads show it: the loaded card replaces the pending one at the same height.
    let scene = harness.state_mut();
    scene.fixture.threads.push(stored(&new));
    scene.version += 1;
    let landed = record(&mut harness, 30);
    assert_still("reloaded", &landed, pending_below, comment);
    assert!(
        harness.state().fixture.view.composer().is_none(),
        "the loaded thread took the composer's place"
    );
    assert!((line_y(&harness) - above).abs() <= 1.0, "the code above the comment never moved");

    // A second reload (a refresh replacing every row) changes nothing either.
    harness.state_mut().version += 1;
    let refreshed = record(&mut harness, 20);
    assert_still("refreshed", &refreshed, pending_below, comment);
}

/// Submit → pending → failure: the composer comes back with the text, the reason and Retry, easing
/// open; Retry folds it into the comment again, and the post then lands without a move.
#[test]
fn a_failed_post_brings_the_composer_back_with_the_text_and_retry() {
    let mut harness = harness();
    write_comment(&mut harness);
    let composer_below = below_y(&harness);
    harness.get_by_role_and_label(Role::Button, "Comment").click();
    record(&mut harness, 40);
    let (key, new) = harness.state().posted.first().cloned().expect("posted");
    let pending_below = below_y(&harness);

    harness
        .state_mut()
        .fixture
        .threads_ui
        .fail_for_tests(&key, "GitHub could not be reached (timed out)");
    let reopening = record(&mut harness, 40);
    let failed_below = below_y(&harness);
    assert!(failed_below > composer_below, "the composer is back, with the failure row under it");
    assert_eased("failure", &reopening, pending_below, failed_below);
    assert!(harness.query_all_by_value(BODY).next().is_some(), "the text is back in the editor");
    assert!(
        harness
            .query_by_label_contains(
                "Not sent: GitHub could not be reached (timed out). Your text is kept."
            )
            .is_some(),
        "the reason shows"
    );
    assert!(harness.query_by_label("Posting…").is_none(), "nothing claims to be on its way");

    harness.get_by_role_and_label(Role::Button, "Retry").click();
    harness.step();
    harness.state_mut().fixture.threads_ui.retry_for_tests(&key);
    let refolding = record(&mut harness, 40);
    let retried_below = below_y(&harness);
    assert!(
        (retried_below - pending_below).abs() <= 1.0,
        "the comment on its way again is as tall as before"
    );
    assert_eased("retry", &refolding, failed_below, retried_below);
    let comment = comment_top(&harness).expect("the comment shows again");

    let version = harness.state().version;
    harness.state_mut().fixture.threads_ui.deliver_for_tests(
        &key,
        Delivery { thread: Some(stored(&new)), ..Delivery::default() },
        version,
    );
    let scene = harness.state_mut();
    scene.fixture.threads.push(stored(&new));
    scene.version += 1;
    let landed = record(&mut harness, 30);
    assert_still("retried and reloaded", &landed, retried_below, comment);
}

/// A reply folds into the comment it will be, and the reloaded thread shows it in place.
#[test]
fn a_reply_hands_its_composer_to_the_reply_in_place() {
    let mut harness = harness();
    let anchor = ThreadAnchor {
        path: PATH.into(),
        old_path: None,
        old_line: None,
        new_line: Some(commented_line()),
        range: None,
    };
    let mara = UserIdentity {
        id: "mara".into(),
        display_name: "Mara Schulz".into(),
        login: Some("mschulz".into()),
        avatar_url: None,
    };
    let thread = ReviewThread {
        id: "t-1".into(),
        status: ThreadStatus::Active,
        anchor: Some(anchor),
        is_outdated: false,
        origin: None,
        is_system: false,
        can_reply: true,
        can_resolve: true,
        comments: vec![ThreadComment {
            id: "c-1".into(),
            author: mara,
            body: "Why a loop here?".into(),
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: None,
        }],
    };
    harness.state_mut().fixture.threads.push(thread);
    harness.run_steps(30);
    harness.get_by_role_and_label(Role::Button, "Reply").click();
    harness.run_steps(30);
    harness.event(egui::Event::Text(BODY.to_owned()));
    harness.run_steps(30);
    let composer_below = below_y(&harness);

    harness.get_by_role_and_label(Role::Button, "Reply").click();
    let folding = record(&mut harness, 40);
    let pending_below = below_y(&harness);
    assert_eased("reply", &folding, composer_below, pending_below);
    assert!(harness.query_by_label("Posting…").is_some(), "the reply says it is on its way");
    let comment = comment_top(&harness).expect("the reply shows while it is posted");

    let reply = ThreadComment {
        id: "c-2".into(),
        author: me(),
        body: BODY.into(),
        created_at: time::OffsetDateTime::now_utc(),
        updated_at: None,
    };
    let version = harness.state().version;
    harness.state_mut().fixture.threads_ui.deliver_for_tests(
        "reply:t-1",
        Delivery { comment: Some(reply.clone()), ..Delivery::default() },
        version,
    );
    let answered = record(&mut harness, 30);
    assert_still("reply answered", &answered, pending_below, comment);

    let scene = harness.state_mut();
    scene.fixture.threads[0].comments.push(reply);
    scene.version += 1;
    let landed = record(&mut harness, 30);
    assert_still("reply reloaded", &landed, pending_below, comment);
    assert!(harness.query_by_label("Posting…").is_none());
}
