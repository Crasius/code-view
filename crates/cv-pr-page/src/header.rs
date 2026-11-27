use cv_base::theme::{self, icons};
use cv_pr_session::actions::ActionRunner;
use cv_pr_session::lifecycle::LifecycleAction;
use cv_pr_session::session::PullRequestSession;
use cv_pr_session::web_link::WebLink;
use cv_runtime::intents;
use cv_runtime::links::pull_request_link;
use cv_services::Services;
use cv_widgets::numbers::number_prefix;
use cv_widgets::refresh_status::{self, RefreshStatus};
use cv_widgets::{
    avatar, badges, buttons, copy_chip, elide, focus, glyph, provider_icon, row, skeleton, states,
};
use egui::{RichText, Ui};
use review_core::{MergeOptions, PullRequestDetail, PullRequestState, PullRequestSummary};

use crate::checkout::{self, CheckoutButton};
use crate::merge_button::MergeButton;
use crate::vote_comment::{self, VoteComposer};

/// The platform mark before the state pill, sized to the title line.
const PLATFORM_MARK_SIZE: f32 = 20.0;
/// The platform mark's menu: room for "Open pull request on Azure DevOps".
const PLATFORM_MENU_WIDTH: f32 = 260.0;
/// What a branch chip in the author line does, on hover and for screen readers.
const COPY_BRANCH: &str = "Copy branch name";
/// The share of the author line's room a long source branch may take before the target's chip
/// is cut; the source names the change, so it comes first.
const SOURCE_SHARE: f32 = 0.6;

pub(crate) enum HeaderAction {
    Run(LifecycleAction),
    /// The merge button was confirmed.
    Merge(MergeOptions),
    Refresh,
    /// The connection needs a token before the user can review.
    OpenSettings,
}

/// The platform mark before the state pill links to the pull request on its platform. Its menu
/// also offers the file shown in the Files tab (`file_link`) there, and copying the link.
fn platform_link(
    ui: &mut Ui,
    session: &PullRequestSession,
    url: &str,
    file_link: Option<&WebLink>,
) {
    // The mark's hover fill reaches into the margin, so the mark lines up with the avatar below.
    ui.add_space(-provider_icon::LINK_PADDING);
    let platform = session.kind.label();
    let action = format!("Open pull request on {platform}");
    let response = provider_icon::link(
        ui,
        session.kind,
        url,
        PLATFORM_MARK_SIZE,
        &action,
        "Right-click for more links",
    );
    if response.clicked() || response.middle_clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
    response.context_menu(|ui| {
        ui.set_width(PLATFORM_MENU_WIDTH);
        if buttons::menu_item(ui, provider_icon::glyph(session.kind), &action, None, None).clicked()
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
        if let Some(link) = file_link
            && buttons::menu_item(
                ui,
                link.icon,
                &format!("Open this file on {platform}"),
                None,
                None,
            )
            .clicked()
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(&link.url));
        }
        if buttons::menu_item(ui, icons::LINK, "Copy link", None, None).clicked() {
            ui.ctx().copy_text(url.to_owned());
        }
    });
}

/// "Ana wants to merge [feature] → [main]" after the author's avatar, the branches as chips that
/// copy their name, then the slot of the spinner that turns while an action runs (`is_busy`).
/// `reserved` is the room the checkout chip after the line needs.
fn author_line(ui: &mut Ui, summary: &PullRequestSummary, is_busy: bool, reserved: f32) {
    let palette = theme::palette(ui.ctx());
    avatar::avatar(ui, &summary.author, 20.0);
    let prefix = format!("{} wants to merge", summary.author.display_name);
    let spacing = ui.spacing().item_spacing.x;
    let body = egui::TextStyle::Body.resolve(ui.style());
    // The spinner slot keeps its place after the line; the prefix, the arrow and the two chips
    // share the rest. A narrow header cuts the prefix only once both chips are at their least.
    // Five gaps: before the prefix, both chips, the arrow and the spinner slot.
    let room = ui.available_width()
        - icons::SIZE
        - elide::text_width(ui, icons::ARROW_RIGHT, &body)
        - 5.0 * spacing
        - reserved;
    let least_chip = copy_chip::min_width(ui);
    let prefix_width =
        elide::text_width(ui, &prefix, &body).min((room - 2.0 * least_chip).max(0.0));
    ui.scope(|ui| {
        ui.set_max_width(prefix_width);
        ui.add(egui::Label::new(RichText::new(prefix).color(palette.text_secondary)).truncate());
    });
    let (source_width, target_width) = branch_widths(
        room - prefix_width,
        copy_chip::width(ui, &summary.source_branch),
        copy_chip::width(ui, &summary.target_branch),
        least_chip,
    );
    copy_chip::show(ui, icons::GIT_BRANCH, &summary.source_branch, COPY_BRANCH, source_width);
    // "mona wants to merge feature into main": the arrow is the "into".
    glyph::named(ui, RichText::new(icons::ARROW_RIGHT).color(palette.text_secondary), "into");
    copy_chip::show(ui, icons::GIT_BRANCH, &summary.target_branch, COPY_BRANCH, target_width);
    // The slot is always there; the spinner only shows while an action runs.
    let (slot, _) =
        ui.allocate_exact_size(egui::vec2(icons::SIZE, icons::SIZE), egui::Sense::hover());
    if is_busy {
        states::spinner(ui, slot, palette.text_secondary);
    }
}

/// The widths of the source and target chips in `room`, from the widths they need. Both fit
/// whole where they can; otherwise the source takes what it needs up to the larger of
/// [`SOURCE_SHARE`] of the room and what the target leaves, and the target gets the rest. Neither
/// gets less than `least`.
fn branch_widths(room: f32, source: f32, target: f32, least: f32) -> (f32, f32) {
    if source + target <= room {
        return (source, target);
    }
    let source_cap = (room - target).max(room * SOURCE_SHARE);
    let source = source.min(source_cap).max(least);
    (source, target.min(room - source).max(least))
}

/// The header's buttons that keep state across frames: the merge button of an open pull request,
/// the vote composer and the checkout button.
pub(crate) struct Buttons<'a> {
    pub merge: Option<&'a mut MergeButton>,
    pub vote: &'a mut VoteComposer,
    pub checkout: &'a mut CheckoutButton,
}

/// `file_link` is the file the Files tab shows, if any, for the platform mark's menu.
pub(crate) fn show(
    ui: &mut Ui,
    session: &PullRequestSession,
    actions: &ActionRunner,
    file_link: Option<&WebLink>,
    buttons: Buttons<'_>,
    services: &mut Services,
) -> Option<HeaderAction> {
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    // F5 refreshes the pull request, as it refreshes the list; after a failed load it retries.
    let can_refresh = session.detail.ready().is_some() || session.detail.error().is_some();
    if ui.input(|input| input.key_pressed(egui::Key::F5)) && can_refresh {
        action = Some(HeaderAction::Refresh);
    }
    egui::Frame::new()
        .fill(palette.surface)
        .inner_margin(egui::Margin { left: 24, right: 24, top: 16, bottom: 4 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let detail = session.detail.ready();
            let failure = session.detail.error().filter(|_| !session.detail.is_loading());
            // Until the detail is in, the list row the page was opened from stands in for it.
            let listed = session.listed.as_ref().filter(|_| failure.is_none());
            let Some(summary) = detail.map(|detail| &detail.summary).or(listed) else {
                match failure {
                    Some(message) => {
                        action = action.take().or(load_error(ui, session, services, message));
                    }
                    None => skeleton::header(ui),
                }
                return;
            };
            ui.horizontal(|ui| {
                platform_link(ui, session, &summary.web_url, file_link);
                badges::state_pill(ui, summary.state, summary.is_draft);
                number_link(ui, number_prefix(session.kind), summary.number, &summary.web_url);
                // The command bar is measured first (right to left), the title takes what is left.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Votes, merge state and policies come with the detail only; until then the
                    // command bar keeps its skeleton.
                    match detail {
                        Some(detail) => {
                            action = action
                                .take()
                                .or(command_bar(ui, session, detail, actions, buttons, services));
                        }
                        None => skeleton::command_bar(ui),
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(&summary.title).heading()).truncate(),
                        );
                    });
                });
            });
            ui.add_space(theme::style::SPACE_S);
            ui.horizontal(|ui| {
                // A refresh behind the shown data (revisit, Refresh) says so here, in a fixed slot;
                // so does the detail still on its way behind the list row's summary. One that a
                // rate-limit pause holds back waits, as the banner says, with no spinner turning.
                let is_under_way = !crate::is_held(services, &session.key);
                let status = if detail.is_some() {
                    RefreshStatus::of(
                        session.is_refreshing() && is_under_way,
                        session.refresh_error(),
                    )
                } else {
                    RefreshStatus::of(is_under_way, None)
                };
                let (refresh, ()) = row::with_trailing(
                    ui,
                    egui::Align::Center,
                    |ui| refresh_status::show(ui, status),
                    |ui| {
                        let is_finished = summary.state != PullRequestState::Open;
                        let chip = checkout::chip_width(ui, &session.key, services, is_finished);
                        author_line(ui, summary, actions.is_busy(), chip);
                        checkout::chip(ui, &session.key, services, is_finished);
                    },
                );
                if refresh {
                    action = Some(HeaderAction::Refresh);
                }
            });
        });
    if matches!(action, Some(HeaderAction::Refresh)) {
        // Reloaded by hand: the page takes the host's word over the writes it may not show yet.
        services.writes.reloaded_by_hand(&session.key);
    }
    action
}

/// Why the pull request did not load, with Retry (F5) and the pull request on its platform.
fn load_error(
    ui: &mut Ui,
    session: &PullRequestSession,
    services: &Services,
    message: &str,
) -> Option<HeaderAction> {
    let open_label = states::open_on_platform(session.kind);
    let actions = [
        states::Action::primary(icons::ARROW_CLOCKWISE, "Retry"),
        states::Action::secondary(provider_icon::glyph(session.kind), &open_label),
        states::Action::secondary(icons::GEAR, "Check connection"),
    ];
    match states::error_with_actions(ui, message, &actions)? {
        0 => return Some(HeaderAction::Refresh),
        1 => {
            let connection = services.connection(&session.key.connection_id)?;
            let key = &session.key;
            let url = pull_request_link::web_url(connection, &key.repository_key, key.number);
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
        _ => intents::request_fix(ui.ctx(), &session.key.connection_id),
    }
    None
}

/// The pull request's number in the title line; a click (or Enter and Space) copies its web
/// address. At rest it reads as plain secondary text; the prefix turns into a check mark for a
/// moment after the copy, and the element keeps its size so the title beside it never moves.
fn number_link(ui: &mut Ui, prefix: &str, number: impl std::fmt::Display, web_url: &str) {
    let palette = theme::palette(ui.ctx());
    let font = egui::FontId::proportional(theme::style::TITLE_SIZE);
    let (prefix_galley, digits) = ui.fonts_mut(|fonts| {
        (
            fonts.layout_no_wrap(prefix.to_owned(), font.clone(), palette.text_secondary),
            fonts.layout_no_wrap(number.to_string(), font, palette.text_secondary),
        )
    });
    let prefix_width = prefix_galley.size().x.max(icons::SIZE);
    let pad = theme::style::SPACE_S;
    let size = egui::vec2(
        pad * 2.0 + prefix_width + digits.size().x,
        digits.size().y + theme::style::SPACE_XS * 2.0,
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let name = format!("{prefix}{}", digits.text());
    let is_copied = copy_chip::copy_on_click(ui, &response, web_url);
    let label =
        if is_copied { format!("Copied link to {name}") } else { format!("Copy link to {name}") };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));

    if ui.is_rect_visible(rect) {
        let fill = row::state_fill(ui, &response, egui::Color32::TRANSPARENT);
        let painter = ui.painter();
        painter.rect_filled(rect, theme::style::RADIUS_CONTROL, fill);
        let slot_center = egui::pos2(rect.left() + pad + prefix_width / 2.0, rect.center().y);
        if is_copied {
            painter.text(
                slot_center,
                egui::Align2::CENTER_CENTER,
                icons::CHECK,
                icons::rest_font(&palette, icons::SIZE),
                palette.success,
            );
        } else {
            let prefix_size = prefix_galley.size();
            painter.galley(slot_center - prefix_size / 2.0, prefix_galley, palette.text_secondary);
        }
        let digits_pos =
            egui::pos2(rect.left() + pad + prefix_width, rect.center().y - digits.size().y / 2.0);
        painter.galley(digits_pos, digits, palette.text_secondary);
    }
    focus::ring(ui, &response, rect);
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Copy link to pull request");
}

fn command_bar(
    ui: &mut Ui,
    session: &PullRequestSession,
    detail: &PullRequestDetail,
    actions: &ActionRunner,
    buttons: Buttons<'_>,
    services: &mut Services,
) -> Option<HeaderAction> {
    let Buttons { merge, vote, checkout } = buttons;
    let mut action = None;
    let is_open = detail.summary.state == PullRequestState::Open;
    let can_write = session.has_token && !actions.is_busy();

    let more = buttons::icon(ui, icons::DOTS_THREE, "More actions");
    egui::Popup::menu(&more).show(|ui| {
        ui.set_min_width(220.0);
        let open_icon = provider_icon::glyph(session.kind);
        if buttons::menu_item(ui, open_icon, &states::open_on_platform(session.kind), None, None)
            .clicked()
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(&detail.summary.web_url));
        }
        if buttons::menu_item(ui, icons::LINK, "Copy link", None, None).clicked() {
            ui.ctx().copy_text(detail.summary.web_url.clone());
        }
        if buttons::menu_item(ui, icons::ARROW_CLOCKWISE, "Refresh", Some("F5"), None).clicked() {
            action = Some(HeaderAction::Refresh);
        }
        ui.separator();
        ui.add_enabled_ui(can_write, |ui| {
            if is_open
                && !detail.summary.is_draft
                && buttons::menu_item(ui, icons::FILE_DASHED, "Mark as draft", None, None).clicked()
            {
                action = Some(HeaderAction::Run(LifecycleAction::SetDraft(true)));
            }
            if is_open
                && detail.summary.is_draft
                && buttons::menu_item(ui, icons::PAPER_PLANE_TILT, "Publish", None, None).clicked()
            {
                action = Some(HeaderAction::Run(LifecycleAction::SetDraft(false)));
            }
            if is_open
                && buttons::menu_item(ui, icons::PROHIBIT, abandon_label(session), None, None)
                    .clicked()
            {
                action = Some(HeaderAction::Run(LifecycleAction::Close));
            }
            if detail.summary.state == PullRequestState::Closed
                && buttons::menu_item(ui, icons::ARROW_COUNTER_CLOCKWISE, "Reactivate", None, None)
                    .clicked()
            {
                action = Some(HeaderAction::Run(LifecycleAction::Reopen));
            }
        });
    });

    if is_open {
        if let Some(merge) = merge
            && let Some(options) =
                merge.show(ui, session, detail, services, session.has_token, actions.is_busy())
        {
            action = Some(HeaderAction::Merge(options));
        }
        let vote_action = vote_comment::show(ui, session, detail, can_write, vote, services);
        action = action.or(vote_action);
    }
    checkout.show(ui, session, detail, services);
    action
}

fn abandon_label(session: &PullRequestSession) -> &'static str {
    match session.kind {
        review_core::ProviderKind::AzureDevOps => "Abandon",
        review_core::ProviderKind::GitHub | review_core::ProviderKind::GitLab => "Close",
    }
}

#[cfg(test)]
mod tests {
    use super::branch_widths;

    #[test]
    fn branches_that_fit_stay_whole() {
        assert_eq!(branch_widths(300.0, 120.0, 80.0, 40.0), (120.0, 80.0));
    }

    #[test]
    fn a_long_source_takes_what_a_short_target_leaves() {
        assert_eq!(branch_widths(200.0, 400.0, 60.0, 40.0), (140.0, 60.0));
    }

    #[test]
    fn two_long_branches_split_the_room_source_first() {
        assert_eq!(branch_widths(500.0, 1000.0, 1000.0, 40.0), (300.0, 200.0));
    }

    #[test]
    fn a_short_source_leaves_the_rest_to_the_target() {
        assert_eq!(branch_widths(200.0, 70.0, 400.0, 40.0), (70.0, 130.0));
    }

    #[test]
    fn no_chip_gets_less_than_its_least() {
        assert_eq!(branch_widths(60.0, 400.0, 400.0, 40.0), (40.0, 40.0));
    }
}
