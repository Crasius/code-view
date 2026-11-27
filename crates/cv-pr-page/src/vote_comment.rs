//! The header's vote split button and the composer some votes open under it.
//!
//! The main part casts "Approve" in one click until the platform lists a vote of the signed-in
//! user. Then it shows that vote ("Approved" with its icon in the vote's colour) and opens the
//! vote menu instead: a second click never posts another approval (GitHub creates a new review
//! per call). Before the platform said who the user is, it stays "Approve".
//!
//! On GitHub, whose reviews carry a body, "Approve with comment…" and "Request changes…" open a
//! small composer under the button (the shared Markdown editor: Ctrl+Enter submits, Esc cancels).
//! GitHub refuses a change request without text, so its Submit stays disabled and the footer says
//! why. Azure DevOps and GitLab keep their one-click votes. The open composer and its text live
//! with the page ([`VoteComposer`]), so a reload underneath or another tab keeps them; a page the
//! app closes takes them along, as it does its thread drafts.
//!
//! GitHub refuses every review vote by the pull request's author (422 "Can not approve your own
//! pull request"), so for the author there both votes stay in place but off, with that reason
//! ([`author_refusal`], in `cv-pr-session` because the Overview's and the Files tab's Approve ask
//! it too). GitLab lets authors approve only where the project allows it, which the app is not
//! told, so it keeps the vote and shows GitLab's reason when refused; Azure DevOps lets authors
//! vote.

use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, style};
use cv_editor::markdown_editor::{self, Outcome};
use cv_pr_session::lifecycle::{LifecycleAction, author_refusal};
use cv_pr_session::session::PullRequestSession;
use cv_runtime::route::PullRequestKey;
use cv_services::Services;
use cv_services::self_review::{self, Identities, Standing};
use cv_widgets::{badges, buttons};
use egui::text::{LayoutJob, TextFormat};
use egui::{Align, FontId, RectAlign, Response, Ui};
use review_core::{ProviderKind, PullRequestDetail, ReviewVote};

use crate::header::HeaderAction;

/// The composer's width under the vote button.
const COMPOSER_WIDTH: f32 = 420.0;
/// Why Submit is disabled in GitHub's empty change request.
const NEEDS_COMMENT: &str = "GitHub needs a comment to request changes";

/// Whether a vote entry takes a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Comment {
    /// One click casts it.
    None,
    /// The composer opens; the comment is the point of the entry.
    Optional,
    /// The composer opens and the platform refuses the vote without text.
    Required,
}

/// One entry of the vote menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    vote: ReviewVote,
    comment: Comment,
}

/// The vote menu's entries for the votes `kind` can cast, in their order. GitHub reviews carry a
/// body: approving offers a second entry with a comment, requesting changes needs one.
fn entries(kind: ProviderKind, votes: &[ReviewVote]) -> Vec<Entry> {
    let is_github = kind == ProviderKind::GitHub;
    let mut entries = Vec::with_capacity(votes.len() + 1);
    for &vote in votes {
        match vote {
            ReviewVote::ChangesRequested if is_github => {
                entries.push(Entry { vote, comment: Comment::Required });
            }
            ReviewVote::Approved if is_github => {
                entries.push(Entry { vote, comment: Comment::None });
                entries.push(Entry { vote, comment: Comment::Optional });
            }
            _ => entries.push(Entry { vote, comment: Comment::None }),
        }
    }
    entries
}

/// The vote shown on the main part: the signed-in user's, once the platform lists one.
fn own_vote(standing: Standing) -> Option<ReviewVote> {
    match standing {
        Standing::Reviewer(vote) if vote != ReviewVote::None => Some(vote),
        Standing::Reviewer(_) | Standing::Unknown | Standing::Author | Standing::Outside => None,
    }
}

/// The composer's state while it is open.
struct Draft {
    entry: Entry,
    text: String,
    /// The editor takes the focus once, in the frame after the menu closed.
    grab_focus: bool,
}

/// The vote composer of one pull request page: its draft while it is open.
#[derive(Default)]
pub(super) struct VoteComposer {
    draft: Option<Draft>,
}

impl VoteComposer {
    /// The composer is open with text in it: closing the pull request would lose it.
    pub(super) fn has_unsent_text(&self) -> bool {
        self.draft.as_ref().is_some_and(|draft| !draft.text.trim().is_empty())
    }
}

/// The split button, laid out right to left: the caret with the vote menu, then the main part;
/// the composer hangs below both. `can_write`: a token and no write running.
pub(super) fn show(
    ui: &mut Ui,
    session: &PullRequestSession,
    detail: &PullRequestDetail,
    can_write: bool,
    composer: &mut VoteComposer,
    services: &mut Services,
) -> Option<HeaderAction> {
    let mut action = None;
    let votes = &session.capabilities.votes;
    let me = known_user(session, services);
    let standing = self_review::standing(&detail.summary, me.as_ref());
    let own = own_vote(standing);
    let refusal = author_refusal(session.kind, standing);
    let draft = &mut composer.draft;
    // A composer opened before the platform said who the user is can never submit for the author.
    if refusal.is_some() {
        *draft = None;
    }
    let rect = ui
        .add_enabled_ui(can_write && !votes.is_empty(), |ui| {
            // Laid out right to left: the caret sits right of the main part, like a split button.
            ui.spacing_mut().item_spacing.x = 2.0;
            let caret = buttons::secondary_square(ui, icons::CARET_DOWN, "More votes");
            let menu = egui::Popup::menu(&caret);
            let menu_id = menu.get_id();
            menu.show(|ui| {
                ui.set_min_width(220.0);
                if let Some(entry) = vote_menu(ui, session, refusal) {
                    match entry.comment {
                        Comment::None => action = Some(vote(entry.vote, None)),
                        Comment::Optional | Comment::Required => {
                            let text = draft.take().map(|draft| draft.text).unwrap_or_default();
                            *draft = Some(Draft { entry, text, grab_focus: true });
                        }
                    }
                }
            });
            let main = match refusal {
                Some(reason) => ui
                    .add_enabled_ui(false, |ui| main_part(ui, own))
                    .inner
                    .on_disabled_hover_text(reason),
                None => main_part(ui, own),
            };
            if main.clicked() {
                match own {
                    // A vote is in: the menu changes it, a second approval never goes out.
                    Some(_) => {
                        egui::Popup::open_id(ui.ctx(), menu_id);
                        ui.ctx().request_repaint();
                    }
                    None if votes.contains(&ReviewVote::Approved) => {
                        action = Some(vote(ReviewVote::Approved, None));
                    }
                    None => {}
                }
            }
            main.rect.union(caret.rect)
        })
        .inner;
    action = action.or(show_draft(ui, &session.key, session.kind, rect, draft, can_write));
    if !session.has_token && add_token_button(ui).clicked() {
        action = Some(HeaderAction::OpenSettings);
    }
    action
}

/// Below this room the button keeps only its key and tooltip, so that the checkout button and a
/// readable title still fit beside it. Our own bound: the label takes about 160 points, the checkout
/// button about 120, and the title keeps the other 200.
const TOKEN_LABEL_ROOM: f32 = 480.0;

fn add_token_button(ui: &mut Ui) -> egui::Response {
    if ui.available_width() < TOKEN_LABEL_ROOM {
        buttons::icon(ui, icons::KEY, "Add a token to review")
    } else {
        buttons::subtle(ui, icons::KEY, "Add a token to review")
    }
}

/// The open composer under `anchor`, if any; returns the vote it submitted. A submit, Cancel or a
/// dismiss closes it. While another write runs it stays open with Submit disabled, so the text
/// typed so far is kept.
fn show_draft(
    ui: &Ui,
    key: &PullRequestKey,
    kind: ProviderKind,
    anchor: egui::Rect,
    draft: &mut Option<Draft>,
    can_write: bool,
) -> Option<HeaderAction> {
    let open = draft.as_mut()?;
    let outcome = composer(ui, key, kind, anchor, open, !can_write)?;
    let submitted = match outcome {
        Outcome::Submitted { .. } => {
            Some(vote(open.entry.vote, Some(std::mem::take(&mut open.text))))
        }
        Outcome::Cancelled | Outcome::Dismissed => None,
    };
    *draft = None;
    submitted
}

fn vote(vote: ReviewVote, comment: Option<String>) -> HeaderAction {
    HeaderAction::Run(LifecycleAction::Vote { vote, comment })
}

/// The signed-in user once the platform said; asks for them once per connection with a token.
fn known_user(
    session: &PullRequestSession,
    services: &mut Services,
) -> Option<review_core::UserIdentity> {
    let connection_id = &session.key.connection_id;
    if session.has_token && services.identities.state(connection_id).is_none() {
        services.identities.ask(connection_id);
        Identities::start_asked(services);
    }
    services.identities.known(connection_id)
}

/// "Approve", or the user's own vote with its icon in the vote's colour.
fn main_part(ui: &mut Ui, own: Option<ReviewVote>) -> Response {
    let Some(own) = own else {
        return buttons::secondary_icon(ui, icons::CHECK, "Approve");
    };
    let palette = theme::palette(ui.ctx());
    let (icon, color) = badges::vote_icon(own, &palette);
    let label = badges::vote_label(own);
    let centered = |font: FontId, color| TextFormat {
        valign: Align::Center,
        ..TextFormat::simple(font, color)
    };
    let mut job = LayoutJob::default();
    job.append(icon, 0.0, centered(icons::rest_font(&palette, icons::SIZE), color));
    job.append(label, 6.0, centered(FontId::proportional(style::BODY_SIZE), palette.text));
    let response = buttons::secondary(ui, job);
    // The icon is a font glyph; screen readers hear the vote and what a click does.
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            response.enabled(),
            format!("Your vote: {label}"),
        )
    });
    response.on_hover_text("Your vote. Click to change it.")
}

/// The vote menu's entries; returns the one clicked. With a `refusal` they all stay off with it.
fn vote_menu(ui: &mut Ui, session: &PullRequestSession, refusal: Option<&str>) -> Option<Entry> {
    let palette = theme::palette(ui.ctx());
    let mut clicked = None;
    for entry in entries(session.kind, &session.capabilities.votes) {
        let (icon, color) = match entry.comment {
            Comment::Optional => (icons::CHAT_CIRCLE_TEXT, palette.success),
            Comment::None | Comment::Required => badges::vote_icon(entry.vote, &palette),
        };
        let tint = (entry.vote != ReviewVote::None).then_some(color);
        let label = entry_label(entry, session.kind);
        let item = ui
            .add_enabled_ui(refusal.is_none(), |ui| buttons::menu_item(ui, icon, label, None, tint))
            .inner;
        if let Some(reason) = refusal {
            item.on_disabled_hover_text(reason);
        } else if item.clicked() {
            clicked = Some(entry);
        }
    }
    clicked
}

fn entry_label(entry: Entry, kind: ProviderKind) -> &'static str {
    let is_azure = kind == ProviderKind::AzureDevOps;
    match (entry.vote, entry.comment) {
        (ReviewVote::Approved, Comment::Optional | Comment::Required) => "Approve with comment…",
        (ReviewVote::ChangesRequested, Comment::Optional | Comment::Required) => "Request changes…",
        (ReviewVote::Approved, _) => "Approve",
        (ReviewVote::ApprovedWithSuggestions, _) => "Approve with suggestions",
        (ReviewVote::WaitingForAuthor, _) => "Wait for author",
        (ReviewVote::ChangesRequested, _) if is_azure => "Reject",
        (ReviewVote::ChangesRequested, _) => "Request changes",
        (ReviewVote::None, _) if is_azure => "Reset feedback",
        (ReviewVote::None, _) => "Revoke approval",
    }
}

/// The composer under the vote button, right-aligned with it; returns how this frame ended it.
/// `is_busy`: a write runs, Submit waits for it.
fn composer(
    ui: &Ui,
    key: &PullRequestKey,
    kind: ProviderKind,
    anchor: egui::Rect,
    draft: &mut Draft,
    is_busy: bool,
) -> Option<Outcome> {
    let (title, submit, placeholder) = match draft.entry.vote {
        ReviewVote::ChangesRequested => {
            ("Request changes", "Request changes", "Say what should change")
        }
        _ => ("Approve with comment", "Approve", "Add a comment to your approval"),
    };
    let is_blocked = draft.entry.comment == Comment::Required && draft.text.trim().is_empty();
    let hint = if is_blocked {
        NEEDS_COMMENT
    } else {
        concat!(shortcut!(command Enter), " to submit, Esc to cancel")
    };
    let options = markdown_editor::Options {
        hint: placeholder,
        submit_label: submit,
        grab_focus: std::mem::take(&mut draft.grab_focus),
        min_rows: 3,
        is_busy,
        platform: Some(kind),
        footer_hint: Some(hint),
        ..markdown_editor::Options::default()
    };
    let palette = theme::palette(ui.ctx());
    let id = egui::Id::unique(("vote_comment", key)).with("composer");
    egui::Popup::new(id, ui.ctx().clone(), anchor, ui.layer_id())
        .open(true)
        .align(RectAlign::BOTTOM_END)
        .gap(style::SPACE_XS)
        .width(COMPOSER_WIDTH)
        .accessible_name(title)
        .show(|ui| {
            ui.set_width(COMPOSER_WIDTH - 2.0 * style::SPACE_S);
            ui.label(
                egui::RichText::new(title)
                    .font(theme::fonts::semibold_font(style::BODY_SIZE))
                    .color(palette.text),
            );
            ui.add_space(style::SPACE_XS);
            markdown_editor::show(ui, id.with("editor"), &mut draft.text, &options)
        })
        .and_then(|shown| shown.inner)
}

#[cfg(test)]
mod tests {
    use cv_pr_session::lifecycle::LifecycleAction;
    use cv_runtime::route::PullRequestKey;
    use egui::{Key, Modifiers};
    use review_core::{ProviderKind, ReviewVote};

    use super::{Comment, Draft, Entry, VoteComposer, entries, show_draft};
    use crate::header::HeaderAction;

    #[test]
    fn github_change_requests_need_a_comment_and_approvals_offer_one() {
        let votes = [ReviewVote::Approved, ReviewVote::ChangesRequested];
        assert_eq!(
            entries(ProviderKind::GitHub, &votes),
            [
                Entry { vote: ReviewVote::Approved, comment: Comment::None },
                Entry { vote: ReviewVote::Approved, comment: Comment::Optional },
                Entry { vote: ReviewVote::ChangesRequested, comment: Comment::Required },
            ]
        );
    }

    #[test]
    fn azure_and_gitlab_votes_stay_one_click() {
        let votes = [ReviewVote::Approved, ReviewVote::ChangesRequested, ReviewVote::None];
        for kind in [ProviderKind::AzureDevOps, ProviderKind::GitLab] {
            assert!(entries(kind, &votes).iter().all(|entry| entry.comment == Comment::None));
        }
    }

    #[test]
    fn text_in_the_vote_composer_is_unsent_input() {
        let mut composer = VoteComposer::default();
        assert!(!composer.has_unsent_text(), "no composer open");
        let entry = Entry { vote: ReviewVote::ChangesRequested, comment: Comment::Required };
        let open = |text: &str| Some(Draft { entry, text: text.into(), grab_focus: false });
        composer.draft = open(" \n");
        assert!(!composer.has_unsent_text(), "an empty composer loses nothing");
        composer.draft = open("Please split this");
        assert!(composer.has_unsent_text());
    }

    struct Scene {
        composer: VoteComposer,
        can_write: bool,
        submitted: Option<HeaderAction>,
    }

    fn submit(harness: &mut egui_kittest::Harness<'_, Scene>) {
        harness.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
        harness.run_steps(2);
    }

    #[test]
    fn the_vote_composer_keeps_its_text_while_another_write_runs() {
        let key = PullRequestKey {
            connection_id: "github".into(),
            repository_key: "owner/repo".into(),
            number: 7,
        };
        let entry = Entry { vote: ReviewVote::ChangesRequested, comment: Comment::Required };
        let draft = Draft { entry, text: "Please split this".into(), grab_focus: true };
        let mut harness = egui_kittest::Harness::new_ui_state(
            move |ui, scene: &mut Scene| {
                cv_base::theme::fonts::install(ui.ctx());
                let anchor =
                    egui::Rect::from_min_size(egui::pos2(500.0, 10.0), egui::vec2(80.0, 24.0));
                let draft = &mut scene.composer.draft;
                let action =
                    show_draft(ui, &key, ProviderKind::GitHub, anchor, draft, scene.can_write);
                if action.is_some() {
                    scene.submitted = action;
                }
            },
            Scene {
                composer: VoteComposer { draft: Some(draft) },
                can_write: false,
                submitted: None,
            },
        );
        harness.run_steps(3);
        submit(&mut harness);
        assert!(harness.state().submitted.is_none(), "nothing is sent while the other write runs");
        assert!(
            harness.state().composer.has_unsent_text(),
            "the text stays while the other write runs"
        );

        // The busy editor is disabled and lost the focus; the user puts it back once the write ends.
        let scene = harness.state_mut();
        scene.can_write = true;
        scene.composer.draft.as_mut().expect("the composer stays open").grab_focus = true;
        harness.run_steps(3);
        submit(&mut harness);
        assert!(
            matches!(
                harness.state().submitted,
                Some(HeaderAction::Run(LifecycleAction::Vote {
                    vote: ReviewVote::ChangesRequested,
                    comment: Some(ref text),
                })) if text == "Please split this"
            ),
            "the vote goes out with its comment once writing is allowed"
        );
        assert!(!harness.state().composer.has_unsent_text(), "the sent text leaves the composer");
    }
}
