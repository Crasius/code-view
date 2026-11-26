//! Editing and deleting one's own comments end to end: the real app with the real GitHub provider
//! against the mocked host ([`super::github_mock`]), driven through the accessibility tree like a
//! user. Only the signed-in user's comments offer the menu; an edit opens the composer on the
//! comment's text (in GitHub's rich editor; Esc on the unchanged text closes it, the switch to
//! Markdown shows the same text and is saved as GitHub's editor), a refused save keeps the text
//! with Retry, and the saved comment shows "edited";
//! a deletion asks inside the card, and the thread of its last comment leaves the overview. Needs
//! `git` on PATH, no network.

use egui::Role;
use egui::accesskit::Toggled;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_store::{AppSettings, CommentEditor};
use serde_json::{Value, json};

use super::ReviewApp;
use super::action_e2e::{app, open_pull_request, settle_background, wait_until};
use super::github_mock::{GitHubHost, OwnComment, REPOSITORY, THREAD_TEXT};
use cv_testkit::ScratchDir;

const MENU: &str = "Comment actions";
const KEPT: &str = "Should the greeting stay a const?";
const GONE: &str = "Drop the farewell?";
const ADDED: &str = " Or a static.";

#[test]
fn own_comments_can_be_edited_and_deleted() {
    let scratch = ScratchDir::new("comment-edit");
    let host = GitHubHost::with_pull_request(scratch.path());
    host.sign_in_as_me();
    host.add_own_comments(
        &[
            OwnComment { thread_id: "PRRT_kept", comment_id: 701, line: 2, text: KEPT },
            OwnComment { thread_id: "PRRT_gone", comment_id: 702, line: 6, text: GONE },
        ],
        true,
    );
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_pull_request(&mut harness);
    wait_until(&mut harness, "the threads", |harness| {
        harness.query_by_label(THREAD_TEXT).is_some() && harness.query_by_label("All 3").is_some()
    });
    // Three comments, two of them mine: cy's comment on line 3 offers no menu, mine do.
    assert!(menu_above(&harness, THREAD_TEXT).is_none(), "cy's comment has no menu");
    reveal(&mut harness, GONE);
    assert!(menu_above(&harness, GONE).is_some(), "my comment has its menu");

    // Edit: the composer opens on the comment's text; the first save is refused and keeps it.
    open_menu(&mut harness, KEPT);
    // The menu is as wide as its entries, not as egui's 600-point default area of the first frame.
    let menu_width = harness.get_by_role_and_label(Role::MenuItem, "Delete").rect().width();
    assert!(menu_width < 200.0, "the menu fits its entries, got {menu_width}");
    harness.get_by_role_and_label(Role::MenuItem, "Edit").click();
    wait_until(&mut harness, "the edit composer", |harness| {
        harness.query_by_role_and_label(Role::Button, "Save").is_some()
    });
    assert!(menu_above(&harness, KEPT).is_none(), "the edited comment hides its menu");
    editor_modes(&mut harness);
    editor(&harness, KEPT).focus();
    harness.step();
    editor(&harness, KEPT).type_text(ADDED);
    harness.step();
    harness.get_by_role_and_label(Role::Button, "Save").click();
    let edited = format!("{KEPT}{ADDED}");
    wait_until(&mut harness, "the refused save with Retry", |harness| {
        harness.query_by_label_contains("Not sent:").is_some()
            && harness.query_by_role_and_label(Role::Button, "Retry").is_some()
    });
    assert!(has_editor(&harness, &edited), "the composer keeps the text");
    harness.get_by_role_and_label(Role::Button, "Retry").click();
    wait_until(&mut harness, "the saved comment, marked edited", |harness| {
        harness.query_by_label(&edited).is_some()
            && harness.query_by_label("edited").is_some()
            && harness.query_by_role_and_label(Role::Button, "Save").is_none()
    });
    // The card shows the text while the save is on its way; the toast follows the answer.
    wait_until(&mut harness, "the update toast", |harness| {
        harness.query_by_label_contains("Comment updated.").is_some()
    });
    let patches = requests(&host, "PATCH", 701);
    assert_eq!(patches, vec![json!({ "body": edited }), json!({ "body": edited })]);

    // Delete: asked inside the card, then the thread of the last comment leaves the overview.
    open_menu(&mut harness, GONE);
    harness.get_by_role_and_label(Role::MenuItem, "Delete").click();
    wait_until(&mut harness, "the delete question", |harness| {
        harness.query_by_label("Delete this comment? This cannot be undone.").is_some()
    });
    assert!(requests(&host, "DELETE", 702).is_empty(), "nothing is deleted before the answer");
    harness.get_by_role_and_label(Role::Button, "Delete").click();
    wait_until(&mut harness, "the deleted thread gone", |harness| {
        harness.query_by_label("All 2").is_some()
            && harness.query_by_label(GONE).is_none()
            && harness.query_by_label_contains("Comment deleted.").is_some()
    });
    assert_eq!(requests(&host, "DELETE", 702).len(), 1);
    reveal(&mut harness, &edited);
    assert!(menu_above(&harness, &edited).is_some(), "the edited comment keeps its menu");
}

/// The edit opens in GitHub's rich editor; Esc on the unchanged text closes it without asking.
/// Opened again, the switch to Markdown shows the same text and saves Markdown as GitHub's
/// editor; then it goes back to rich text.
fn editor_modes(harness: &mut Harness<'_, ReviewApp>) {
    assert_eq!(mode_tab(harness, "Rich text").accesskit_node().toggled(), Some(Toggled::True));
    editor(harness, KEPT).focus();
    harness.step();
    harness.key_press(egui::Key::Escape);
    wait_until(harness, "the closed composer", |harness| {
        harness.query_by_role_and_label(Role::Button, "Save").is_none()
    });
    assert!(harness.query_by_label("Discard draft?").is_none(), "nothing changed to discard");
    open_menu(harness, KEPT);
    harness.get_by_role_and_label(Role::MenuItem, "Edit").click();
    wait_until(harness, "the edit composer again", |harness| {
        harness.query_by_role_and_label(Role::Button, "Save").is_some()
    });
    mode_tab(harness, "Markdown").click();
    wait_until(harness, "the Markdown source", |harness| {
        mode_tab(harness, "Markdown").accesskit_node().toggled() == Some(Toggled::True)
            && has_editor(harness, KEPT)
    });
    let saved = AppSettings::load(&harness.state().services.paths.settings_file()).expect("saved");
    assert_eq!(saved.comment_editors.github, CommentEditor::Markdown, "the switch is saved");
    mode_tab(harness, "Rich text").click();
    wait_until(harness, "rich text again", |harness| {
        mode_tab(harness, "Rich text").accesskit_node().toggled() == Some(Toggled::True)
    });
}

/// Scrolls the overview down until the comment reading `text` shows inside the window (the
/// overview lays out only the cards near the view).
fn reveal(harness: &mut Harness<'_, ReviewApp>, text: &str) {
    let window = harness.ctx.content_rect();
    for _ in 0..40 {
        let shown = harness.query_by_label(text).map(|node| node.rect());
        if shown.is_some_and(|body| window.contains_rect(body.expand2(egui::vec2(0.0, 40.0)))) {
            return;
        }
        // Over the overview's column, left of the side column.
        harness.hover_at(egui::pos2(500.0, window.center().y));
        harness.step();
        let delta =
            if shown.is_some_and(|body| body.top() < window.top() + 40.0) { 120.0 } else { -120.0 };
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, delta),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Wheel,
        });
        for _ in 0..3 {
            harness.step();
        }
    }
    let labels: Vec<(String, egui::Rect)> = harness
        .query_all_by(|_| true)
        .filter_map(|node| Some((node.accesskit_node().label()?, node.rect())))
        .collect();
    panic!("the comment {text:?} never came into view; {window:?} {labels:?}");
}

/// The menu button heading the comment reading `text`, if it has one.
fn menu_above(harness: &Harness<'_, ReviewApp>, text: &str) -> Option<egui::Rect> {
    let body = harness.query_by_label(text)?.rect();
    harness
        .query_all_by_label(MENU)
        .map(|node| node.rect())
        .filter(|menu| menu.top() <= body.top() && body.top() - menu.top() < 60.0)
        .max_by(|a, b| a.top().total_cmp(&b.top()))
}

/// Opens the menu of the comment reading `text`. The click goes in once the page is at rest: an
/// answer that lands late can grow what lies above the card, and a button read before that is no
/// longer under the pointer when the click's frames come.
fn open_menu(harness: &mut Harness<'_, ReviewApp>, text: &str) {
    reveal(harness, text);
    settle_background(harness);
    reveal(harness, text);
    let menu = menu_above(harness, text).expect("the comment's menu");
    harness.query_all_by_label(MENU).find(|node| node.rect() == menu).expect("menu").click();
    wait_until(harness, "the comment menu", |harness| {
        harness.query_all_by_role(Role::MenuItem).next().is_some()
    });
}

/// The mode switch's tab `mode` of the composer on the comment being edited (the overview's own
/// composer has a switch too).
fn mode_tab<'h>(harness: &'h Harness<'_, ReviewApp>, mode: &'h str) -> egui_kittest::Node<'h> {
    let area = harness
        .query_by(|node| {
            node.role() == Role::MultilineTextInput
                && node.value().is_some_and(|value| value.starts_with(KEPT))
        })
        .expect("the edit composer")
        .rect();
    harness
        .query_all_by(|node| node.role() == Role::Tab && node.label().as_deref() == Some(mode))
        .find(|tab| tab.rect().bottom() <= area.top() && area.top() - tab.rect().bottom() < 40.0)
        .expect("the edit composer's mode tab")
}

/// The edit composer's text field while it holds `text`.
fn editor<'h>(harness: &'h Harness<'_, ReviewApp>, text: &'static str) -> egui_kittest::Node<'h> {
    harness.get_by(move |node| {
        node.role() == Role::MultilineTextInput && node.value().is_some_and(|value| value == text)
    })
}

fn has_editor(harness: &Harness<'_, ReviewApp>, text: &str) -> bool {
    harness
        .query_by(|node| {
            node.role() == Role::MultilineTextInput
                && node.value().is_some_and(|value| value == text)
        })
        .is_some()
}

/// The JSON bodies of the `verb` requests to review comment `id`, in order.
fn requests(host: &GitHubHost, verb: &str, id: u64) -> Vec<Value> {
    let comment_path = format!("/api/v3/repos/{REPOSITORY}/pulls/comments/{id}");
    host.requests()
        .into_iter()
        .filter(|request| request.method.as_str() == verb && request.url.path() == comment_path)
        .map(|request| request.body_json::<Value>().unwrap_or(Value::Null))
        .collect()
}
