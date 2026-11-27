use egui::{RichText, TextEdit, TextStyle, TextWrapMode, Ui, WidgetText};
use review_core::{PullRequestDetail, PullRequestState, ReviewVote, Reviewer};

use cv_base::theme::{self, icons};
use cv_pr_session::actions::{ActionRunner, Refresh, Ticket};
use cv_pr_session::lifecycle::{LifecycleAction, NewReviewer, self_review_id};
use cv_pr_session::session::PullRequestSession;
use cv_services::Services;
use cv_services::self_review::{self, Identities, Me, Standing};
use cv_services::writes::Effect;
use cv_widgets::{avatar, badges, buttons, glyph, row, skeleton, states};

/// The card's way to a token where "Add me as reviewer" is off without one.
const ADD_TOKEN: &str = "Add a token";
/// The least width the add field keeps beside "Add me as reviewer"; narrower, the button moves
/// under it.
const FIELD_MIN_WIDTH: f32 = 96.0;
/// A reviewer's avatar in the list.
const AVATAR_SIZE: f32 = 28.0;

/// Reviewer list with votes, plus add/remove when a token is configured.
#[derive(Default)]
pub(crate) struct ReviewerEditor {
    new_reviewer: String,
    adding: Option<PendingAdd>,
}

/// An add in flight. Until the refreshed pull request lists the reviewer, a skeleton of their row
/// stands where it will appear: last among the optional reviewers, where an added one lands.
struct PendingAdd {
    ticket: Ticket,
    /// The name the row will show, which the skeleton's name bar is as wide as.
    name: String,
    /// Typed into the field, which empties once the add worked.
    is_typed: bool,
    /// The pull request's version when the add worked; a newer one lists the reviewer.
    succeeded_at: Option<u64>,
}

impl ReviewerEditor {
    /// A reviewer name is typed but not added yet.
    pub(crate) fn has_unsent_input(&self) -> bool {
        !self.new_reviewer.trim().is_empty()
    }

    /// `add_clicked`: the card's add button (in its title line) was clicked this frame.
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        session: &PullRequestSession,
        services: &mut Services,
        actions: &mut ActionRunner,
        add_clicked: bool,
    ) {
        let palette = theme::palette(ui.ctx());
        self.settle(session, actions);
        let Some(detail) = session.detail.ready() else {
            for _ in 0..3 {
                ui.horizontal(|ui| {
                    skeleton::circle(ui, AVATAR_SIZE);
                    skeleton::bar(ui, 140.0, 12.0);
                });
                ui.add_space(6.0);
            }
            return;
        };
        let add_me = (detail.summary.state == PullRequestState::Open)
            .then(|| add_me_offer(ui, session, detail, services, actions))
            .flatten();
        let reviewers = &detail.summary.reviewers;
        let pending = self.adding.as_ref().map(|adding| adding.name.as_str());
        if reviewers.is_empty() && pending.is_none() {
            ui.label(RichText::new("No reviewers.").color(palette.text_secondary));
        }
        let (required, optional): (Vec<&Reviewer>, Vec<&Reviewer>) =
            reviewers.iter().partition(|reviewer| reviewer.is_required);
        let mut removed = None;
        for (title, group, pending) in
            [("Required", required, None), ("Optional", optional, pending)]
        {
            if group.is_empty() && pending.is_none() {
                continue;
            }
            ui.label(
                RichText::new(title).size(theme::style::SMALL_SIZE).color(palette.text_secondary),
            );
            for reviewer in group {
                if reviewer_row(ui, reviewer, session.has_token) {
                    removed = Some(reviewer.identity.id.clone());
                }
            }
            if let Some(name) = pending {
                skeleton_row(ui, name, session.has_token);
            }
            ui.add_space(theme::style::SPACE_S);
        }
        if let Some(reviewer_id) = removed {
            let provider = session.provider.clone();
            let repository = session.key.repository_key.clone();
            let number = session.key.number;
            let effect = (&session.key, Effect::ReviewerRemoved(reviewer_id.clone()));
            actions.run(services, "Reviewer removed.", Refresh::PullRequest, effect, async move {
                provider.remove_reviewer(&repository, number, &reviewer_id).await
            });
        }
        self.add_row(ui, session, services, actions, add_clicked, add_me.as_ref());
    }

    /// Follows the add in flight: a refused one drops its skeleton (the reason shows where the
    /// add was asked), a finished one keeps it until the refreshed pull request arrived.
    fn settle(&mut self, session: &PullRequestSession, actions: &ActionRunner) {
        let Some(adding) = &mut self.adding else {
            return;
        };
        let is_pending = match actions.outcome(adding.ticket) {
            Some(true) => {
                if adding.succeeded_at.is_none() && adding.is_typed {
                    self.new_reviewer.clear();
                }
                let version = session.detail.version();
                *adding.succeeded_at.get_or_insert(version) == version
                    && session.detail.refresh_error().is_none()
            }
            Some(false) => false,
            None => actions.is_running(adding.ticket),
        };
        if !is_pending {
            self.adding = None;
        }
    }

    /// The card's last row: the field that adds a reviewer by name and, while it is offered,
    /// "Add me as reviewer" at its right end, both one control tall. A column too narrow for both
    /// puts the button under the field. Without a token only the button shows, off, with the way
    /// to a token beside it. Alt+R presses the button while the card shows.
    fn add_row(
        &mut self,
        ui: &mut Ui,
        session: &PullRequestSession,
        services: &mut Services,
        actions: &mut ActionRunner,
        add_clicked: bool,
        add_me: Option<&AddMe>,
    ) {
        let Some(add_me) = add_me else {
            if session.has_token {
                self.add_box(ui, session, services, actions, add_clicked);
            }
            return;
        };
        // The gap between field and button, item spacing included.
        let gap = theme::style::SPACE_M;
        let field = ui.available_width() - buttons::icon_button_width(ui, self_review::LABEL) - gap;
        let mut clicked = false;
        if !session.has_token {
            clicked = ui.horizontal(|ui| add_me_button(ui, session, add_me)).inner;
        } else if field >= FIELD_MIN_WIDTH {
            let row = egui::vec2(ui.available_width(), theme::style::CONTROL_HEIGHT);
            let layout = egui::Layout::right_to_left(egui::Align::Center);
            ui.allocate_ui_with_layout(row, layout, |ui| {
                clicked = add_me_button(ui, session, add_me);
                ui.add_space(gap - ui.spacing().item_spacing.x);
                self.add_box(ui, session, services, actions, add_clicked);
            });
        } else {
            self.add_box(ui, session, services, actions, add_clicked);
            ui.add_space(theme::style::SPACE_S);
            clicked = ui.horizontal(|ui| add_me_button(ui, session, add_me)).inner;
        }
        let pressed = add_me.can_click
            && !ui.ctx().text_edit_focused()
            && ui.input_mut(|input| input.consume_shortcut(&self_review::KEY));
        if let Some(message) = &add_me.failure {
            ui.add_space(theme::style::SPACE_XS);
            states::error(ui, &format!("Could not add you as a reviewer: {message}"));
        }
        if (clicked || pressed) && self.adding.is_none() {
            let add = LifecycleAction::AddReviewer(NewReviewer::Me);
            self.adding = add.start(session, services, actions).map(|ticket| PendingAdd {
                ticket,
                name: add_me.name.clone(),
                is_typed: false,
                succeeded_at: None,
            });
        }
    }

    /// Enter in the field or the card's add button adds the typed login; the button on an empty
    /// field moves the focus into it. Nothing is sent until then.
    fn add_box(
        &mut self,
        ui: &mut Ui,
        session: &PullRequestSession,
        services: &Services,
        actions: &mut ActionRunner,
        add_clicked: bool,
    ) {
        let field = ui
            .add(
                TextEdit::singleline(&mut self.new_reviewer)
                    .hint_text("Add reviewer")
                    .min_size(egui::vec2(0.0, theme::style::CONTROL_HEIGHT))
                    .align(egui::Align2::LEFT_CENTER)
                    .desired_width(f32::INFINITY),
            )
            .on_hover_text("A login or an e-mail address; Enter adds the reviewer.");
        // Esc leaves the field and drops what was typed.
        if field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.new_reviewer.clear();
        }
        let wants_add = add_clicked
            || (field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)));
        let user = self.new_reviewer.trim().to_owned();
        if add_clicked && user.is_empty() {
            field.request_focus();
        }
        if !wants_add || user.is_empty() || self.adding.is_some() {
            return;
        }
        let add = LifecycleAction::AddReviewer(NewReviewer::Named(user.clone()));
        self.adding = add.start(session, services, actions).map(|ticket| PendingAdd {
            ticket,
            name: user,
            is_typed: true,
            succeeded_at: None,
        });
    }
}

/// "Add me as reviewer" as the card shows it this frame.
struct AddMe {
    /// The signed-in user's name as their row will show it, once known.
    name: String,
    can_click: bool,
    is_running: bool,
    /// Why the last request was refused; the button then offers to try again.
    failure: Option<String>,
}

/// "Add me as reviewer" is offered while the signed-in user is neither the author nor a reviewer.
/// Off with its reason and the way to a token where the connection has none; its icon turns while
/// the request runs; a refused request keeps it, with the reason right under its row and a click
/// to try again. Once added, the refreshed list shows the user.
fn add_me_offer(
    ui: &Ui,
    session: &PullRequestSession,
    detail: &PullRequestDetail,
    services: &mut Services,
    actions: &ActionRunner,
) -> Option<AddMe> {
    let connection_id = &session.key.connection_id;
    if session.has_token && services.identities.state(connection_id).is_none() {
        services.identities.ask(connection_id);
        Identities::start_asked(services);
    }
    let identities = &services.identities;
    let ticket = ui.ctx().data(|data| data.get_temp::<Ticket>(self_review_id(session)));
    let is_running = ticket.is_some_and(|ticket| actions.is_running(ticket));
    let has_added = ticket.and_then(|ticket| actions.outcome(ticket)) == Some(true);
    let failure = ticket.and_then(|ticket| actions.failure(ticket)).map(str::to_owned);
    let is_me_loading = matches!(identities.state(connection_id), Some(Me::Asked | Me::Loading));
    let me = identities.known(connection_id);
    let is_offered = match self_review::standing(&detail.summary, me.as_ref()) {
        _ if is_running => true,
        Standing::Author | Standing::Reviewer(_) => false,
        // Added; the refreshed pull request lists the user in a moment.
        Standing::Outside => !has_added,
        // Without a token nobody is asked: the disabled button says why.
        Standing::Unknown => !session.has_token || !is_me_loading,
    };
    let can_click = session.has_token && !actions.is_busy();
    // Before the platform said who the user is, the skeleton's name bar takes a typical width.
    let name = me.map_or_else(|| self_review::LABEL.to_owned(), |me| me.display_name);
    is_offered.then_some(AddMe { name, can_click, is_running, failure })
}

/// The button, and without a token the way to one right beside it (the header's votes offer it
/// too). Returns whether it was clicked.
fn add_me_button(ui: &mut Ui, session: &PullRequestSession, add_me: &AddMe) -> bool {
    let is_failed = add_me.failure.is_some();
    let icon = if is_failed { icons::ARROW_CLOCKWISE } else { icons::USER_PLUS };
    let tooltip = if is_failed { "Click to try again." } else { self_review::TOOLTIP };
    let reason = if session.has_token {
        "Another change to this pull request is still being saved."
    } else {
        self_review::NO_TOKEN_TOOLTIP
    };
    let response = ui
        .add_enabled_ui(add_me.can_click || add_me.is_running, |ui| {
            buttons::secondary_busy(ui, icon, self_review::LABEL, add_me.is_running)
        })
        .inner;
    if !session.has_token && buttons::subtle(ui, icons::KEY, ADD_TOKEN).clicked() {
        cv_runtime::intents::request_fix(ui.ctx(), &session.key.connection_id);
    }
    response.on_hover_text(tooltip).on_disabled_hover_text(reason).clicked()
}

/// Returns true when the remove button was clicked.
fn reviewer_row(ui: &mut Ui, reviewer: &Reviewer, can_edit: bool) -> bool {
    let mut is_removed = false;
    let row = row_frame(ui, can_edit, |ui, text_width| {
        avatar::voted(ui, &reviewer.identity, AVATAR_SIZE, reviewer.vote);
        row::centered_lines(ui, |ui| {
            ui.set_max_width(text_width);
            ui.spacing_mut().item_spacing.y = theme::style::SPACE_LINE;
            let name = RichText::new(&reviewer.identity.display_name);
            if reviewer.is_group {
                let label = glyph::truncated(ui, RichText::new(icons::USERS), name);
                glyph::speak_as(&label, &format!("{} (group)", reviewer.identity.display_name));
            } else {
                ui.add(egui::Label::new(name).truncate());
            }
            ui.label(vote_text(ui, reviewer.vote));
        });
        if can_edit {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                is_removed = buttons::icon(ui, icons::X, "Remove reviewer").clicked();
            });
        }
    });
    let name = &reviewer.identity.display_name;
    ui.interact(
        row,
        ui.scope_id().with(("reviewer_row", &reviewer.identity.id)),
        egui::Sense::hover(),
    )
    .widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::ListItem, true, format!("Reviewer {name}"))
    });
    is_removed
}

/// The row of a reviewer being added, as [`reviewer_row`] will draw them without a vote: the
/// avatar's circle and a bar as wide as each text line, in the same places.
fn skeleton_row(ui: &mut Ui, name: &str, can_edit: bool) {
    let row = row_frame(ui, can_edit, |ui, text_width| {
        skeleton::circle(ui, AVATAR_SIZE);
        row::centered_lines(ui, |ui| {
            ui.set_max_width(text_width);
            ui.spacing_mut().item_spacing.y = theme::style::SPACE_LINE;
            text_bar(ui, RichText::new(name).into(), text_width);
            text_bar(ui, vote_text(ui, ReviewVote::None), text_width);
        });
    });
    let id = ui.scope_id().with("reviewer_row_skeleton");
    skeleton::announce(&ui.interact(row, id, egui::Sense::hover()), "Adding reviewer");
}

/// A reviewer row's frame, as tall as the name with its vote below it so the avatar centers on
/// both, across the card's width; `add` gets the width the text may take beside the remove
/// button's slot. Returns the row's rect.
fn row_frame(ui: &mut Ui, can_edit: bool, add: impl FnOnce(&mut Ui, f32)) -> egui::Rect {
    ui.horizontal(|ui| {
        ui.set_min_width(ui.available_width());
        ui.set_min_height(theme::style::ROW_COMPACT);
        // The remove button keeps its slot; a long name is cut with an ellipsis (full on hover).
        let button_slot =
            if can_edit { buttons::SUBTLE_HEIGHT + ui.spacing().item_spacing.x } else { 0.0 };
        let avatar_slot = AVATAR_SIZE + ui.spacing().item_spacing.x;
        add(ui, (ui.available_width() - avatar_slot - button_slot).max(0.0));
    })
    .response
    .rect
}

/// The vote in words and its color; its icon sits on the avatar's badge, so the text repeats none.
fn vote_text(ui: &Ui, vote: ReviewVote) -> WidgetText {
    let palette = theme::palette(ui.ctx());
    let (_, color) = badges::vote_icon(vote, &palette);
    RichText::new(badges::vote_label(vote)).size(theme::style::SMALL_SIZE).color(color).into()
}

/// A skeleton bar in the place `text` takes as a label at most `max_width` wide.
fn text_bar(ui: &mut Ui, text: WidgetText, max_width: f32) {
    let galley = text.into_galley(ui, Some(TextWrapMode::Truncate), max_width, TextStyle::Body);
    let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    skeleton::paint_bar(ui, rect.shrink2(egui::vec2(0.0, rect.height() * 0.2)));
}
