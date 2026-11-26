//! The header's merge split button. The main part names exactly what a click runs ("Squash
//! merge"); a click arms it in place (green, "Click again to squash merge", a bar draining over
//! [`CONFIRM_WINDOW`]) and a second click, or Enter, completes the pull request. Esc or the
//! timeout disarm it. Nothing moves under the cursor.
//!
//! The chevron opens a dropdown right below the button (above it when the window ends too soon),
//! right-aligned with it: the strategies the target branch allows (number keys pick), why one is
//! preselected, "Always use for <repo>", what the branch forbids with the way to its policies, the
//! commit message (for strategies that write a commit) and the source branch choice. Ctrl+Enter there arms the button, so keyboard and
//! mouse confirm the same way. Edits stay for as long as the pull request page lives.
//!
//! While policies still block the pull request, the same button sets the platform's auto-complete
//! with the chosen strategy and options, and once that is set it cancels it ([`auto_complete`]);
//! a dropdown checkbox completes at once instead. Those writes go to the page's runner through
//! the context ([`actions::ask_auto_complete`]).
//!
//! A merge the platform refused leaves the button in a "Merge failed" state (error fill, the
//! reason on hover and for screen readers) until the next click, which arms it again: the failure
//! toast fades only when dismissed, but the button is where the user looks for the merge.

mod menu;
mod notes;
mod placement;
mod split;

use cv_base::platform::{self, shortcut};
use cv_base::theme::{self, style};
use cv_pr_session::actions::{self, AutoCompleteWrite};
use cv_pr_session::auto_complete::{self, Mode};
use cv_pr_session::merge_choice::{self, Reason, Scope};
use cv_pr_session::merge_paint::{
    CHEVRON_WIDTH, ICON_GAP, Look, PADDING_X, Row, divider, paint_in, strategy_row,
};
use cv_pr_session::session::PullRequestSession;
use cv_services::Services;
use cv_services::merge_policies::{PolicyKey, PolicyView};
use cv_widgets::{buttons, checkbox, focus};
use egui::{Key, Modifiers, PopupCloseBehavior, Rect, RichText, TextEdit, Ui, vec2};
use review_core::{
    MergeOptions, MergePolicy, MergeStatus, MergeStrategy, ProviderKind, PullRequestDetail,
};

use self::menu::{favourite, policies_link};
use self::notes::{blocked_warning, main_hover, tone};
use self::placement::MenuPlacement;
use self::split::{SplitParts, main_width};

/// How long the armed button waits for the second click.
pub const CONFIRM_WINDOW: f64 = 4.0;
const MENU_WIDTH: f32 = 400.0;
/// The main part's label after a refused merge.
const FAILED_LABEL: &str = "Merge failed";
const MESSAGE_MAX_HEIGHT: f32 = 160.0;
const NUMBER_KEYS: [Key; 9] = [
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
    Key::Num7,
    Key::Num8,
    Key::Num9,
];

/// The merge choices of one pull request page.
pub struct MergeButton {
    /// The strategy the user picked in the dropdown; `None` follows the preselection.
    picked: Option<MergeStrategy>,
    message: String,
    /// This page's source branch choice; `None` follows the repository's last one.
    delete_source_branch: Option<bool>,
    /// This page's choice of auto-complete while policies block; `None` sets it.
    wants_auto: Option<bool>,
    /// Input time of the first click.
    armed_at: Option<f64>,
    /// The dropdown was opened from the keyboard: its message field takes the focus once.
    focus_message: bool,
    /// The dropdown was open in the last frame.
    is_menu_open: bool,
    /// The target branch's policy as of this frame.
    policy: Option<MergePolicy>,
    key: PolicyKey,
    /// Why the last merge failed, until the next click on the main part.
    failure: Option<String>,
}

/// The overview card's merge type: what the button would run, and what else is allowed.
pub enum MergeTypeLine {
    Pending,
    Ready {
        strategy: MergeStrategy,
        name: &'static str,
        allowed: Vec<&'static str>,
        is_reported: bool,
    },
}

/// What the dropdown asked for, carried out after it closes its borrows.
#[derive(Default)]
struct MenuOutcome {
    arm: bool,
    retry: bool,
}

/// The pull request the button belongs to, and what its main part runs.
#[derive(Clone, Copy)]
struct Page<'a> {
    session: &'a PullRequestSession,
    detail: &'a PullRequestDetail,
    mode: Mode,
}

/// Where the dropdown hangs: the whole button, and whether the chevron toggled it this frame.
#[derive(Clone, Copy)]
struct MenuAnchor {
    id: egui::Id,
    rect: Rect,
    is_toggled: bool,
}

impl MergeButton {
    pub fn new(session: &PullRequestSession, detail: &PullRequestDetail) -> Self {
        Self {
            picked: None,
            message: format!("Merged PR {}: {}", detail.summary.number, detail.summary.title),
            delete_source_branch: None,
            wants_auto: None,
            armed_at: None,
            focus_message: false,
            is_menu_open: false,
            policy: None,
            key: PolicyKey {
                connection_id: session.key.connection_id.clone(),
                repository: session.key.repository_key.clone(),
                target_branch: detail.summary.target_branch.clone(),
            },
            failure: None,
        }
    }

    /// Asks for the target branch's policy (only while the window is active) once per frame.
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        session: &PullRequestSession,
        detail: &PullRequestDetail,
        services: &mut Services,
    ) {
        self.follow_target(&detail.summary.target_branch);
        let now = ctx.input(|input| input.time);
        let is_active = platform::is_window_active(&ctx.input(|input| input.viewport().clone()));
        self.policy = match services.merge_policies.want(
            &services.background,
            &session.provider,
            &self.key,
            now,
            is_active,
        ) {
            PolicyView::Ready(policy) => Some(policy.clone()),
            PolicyView::Pending => None,
        };
        if let Some(armed_at) = self.armed_at {
            if now - armed_at > CONFIRM_WINDOW {
                self.armed_at = None;
            } else {
                ctx.request_repaint();
            }
        }
    }

    /// The pull request was retargeted (a live update or a refresh): the new target branch's policy
    /// decides from now on.
    fn follow_target(&mut self, target_branch: &str) {
        if self.key.target_branch != target_branch {
            target_branch.clone_into(&mut self.key.target_branch);
            self.policy = None;
        }
    }

    /// The platform refused the merge: the button says so until its next click.
    pub fn fail(&mut self, reason: String) {
        self.failure = Some(reason);
        self.armed_at = None;
    }

    /// The dropdown is open or the button armed: live updates must not pull the page away.
    pub fn is_engaged(&self) -> bool {
        self.armed_at.is_some() || self.is_menu_open
    }

    fn choice(
        &self,
        session: &PullRequestSession,
        detail: &PullRequestDetail,
        services: &Services,
    ) -> Option<(MergeStrategy, Option<Reason>)> {
        let policy = self.policy.as_ref()?;
        if let Some(picked) = self.picked.filter(|picked| policy.allowed.contains(picked)) {
            return Some((picked, None));
        }
        merge_choice::preselect(
            policy,
            detail.auto_complete.as_ref().and_then(|auto_complete| auto_complete.strategy),
            &services.settings.merge,
            scope(session),
        )
        .map(|(strategy, reason)| (strategy, Some(reason)))
    }

    fn deletes_source_branch(&self, session: &PullRequestSession, services: &Services) -> bool {
        self.delete_source_branch
            .or_else(|| {
                services
                    .settings
                    .merge
                    .delete_source_branch(&session.key.connection_id, &session.key.repository_key)
            })
            .unwrap_or(true)
    }

    /// The overview card's merge type: the button's strategy once the policy is known.
    pub fn line(
        &self,
        session: &PullRequestSession,
        detail: &PullRequestDetail,
        services: &Services,
    ) -> MergeTypeLine {
        match (&self.policy, self.choice(session, detail, services)) {
            (Some(policy), Some((strategy, _))) => MergeTypeLine::Ready {
                strategy,
                name: merge_choice::button_label(strategy, session.kind),
                allowed: policy
                    .allowed
                    .iter()
                    .map(|strategy| merge_choice::button_label(*strategy, session.kind))
                    .collect(),
                is_reported: policy.is_reported(),
            },
            _ => MergeTypeLine::Pending,
        }
    }

    /// The split button, laid out right to left. Returns the merge to run once confirmed.
    /// `can_write`: a token and no other write running; `is_busy`: a write runs.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        session: &PullRequestSession,
        detail: &PullRequestDetail,
        services: &mut Services,
        can_write: bool,
        is_busy: bool,
    ) -> Option<MergeOptions> {
        let id = ui.make_persistent_id("merge_button");
        let now = ui.input(|input| input.time);
        let kind = session.kind;
        let runs = session.runs.ready().map(Vec::as_slice);
        let mode = auto_complete::mode(detail, runs, kind, self.wants_auto);
        let has_conflicts = detail.merge_status == MergeStatus::Conflicts;
        let choice = self.choice(session, detail, services);
        let can_merge = match mode {
            Mode::CancelAutoComplete => can_write,
            Mode::Now | Mode::SetAutoComplete => can_write && !has_conflicts && choice.is_some(),
        };
        if !can_merge {
            self.armed_at = None;
        }
        // Keys of the armed button come before the widgets, so Enter is not also a click.
        let mut confirmed = self.armed_keys(ui);

        let (label, name) = self.label(choice, (mode, kind), is_busy);
        let is_actionable = can_merge && !is_busy;
        // A set auto-complete has nothing to choose: the button only cancels it.
        let has_menu = mode != Mode::CancelAutoComplete;
        let menu_id = menu_id(id);
        let is_open = egui::Popup::is_id_open(ui.ctx(), menu_id);
        let parts = SplitParts {
            id,
            main_width: main_width(ui, session, &label),
            is_main_enabled: is_actionable,
            is_chevron_enabled: can_write && !is_busy && self.policy.is_some() && has_menu,
            is_open,
        };
        let (rect, main, chevron) = parts.interact(ui, name.as_deref().unwrap_or(&label));
        let main_rect = main.rect;

        if main.clicked() {
            self.failure = None;
            if self.armed_at.is_some() {
                confirmed = true;
            } else {
                self.armed_at = Some(now);
                egui::Popup::close_id(ui.ctx(), menu_id);
            }
        }
        if chevron.clicked() {
            self.armed_at = None;
            self.focus_message = !is_open && focus::is_visible(ui.ctx());
        }

        let remaining = self
            .armed_at
            .map(|armed_at| (1.0 - (now - armed_at) / CONFIRM_WINDOW).clamp(0.0, 1.0) as f32);
        let look = Look {
            strategy: choice.map(|(strategy, _)| strategy),
            label: &label,
            remaining,
            is_enabled: can_merge,
            is_busy,
            is_open,
        };
        paint_in(ui, &main, &chevron, rect, &look, tone(mode, name.is_some()));
        let page = Page { session, detail, mode };
        let main = main_hover(main, page, name.as_deref(), choice, can_write);

        let anchor = MenuAnchor { id: menu_id, rect, is_toggled: chevron.clicked() };
        let outcome = self.menu_popup(ui, page, services, anchor, choice);
        chevron.on_hover_text("Merge options");
        if outcome.arm && can_merge {
            egui::Popup::close_id(ui.ctx(), menu_id);
            self.armed_at = Some(now);
        }
        self.is_menu_open = egui::Popup::is_id_open(ui.ctx(), menu_id);
        if outcome.retry {
            services.merge_policies.forget(&self.key);
        }
        let is_forced = mode == Mode::Now && detail.merge_status == MergeStatus::Blocked;
        if self.armed_at.is_some() && is_forced {
            blocked_warning(ui, id, rect, kind, &detail.summary.web_url);
        }
        focus::ring(ui, &main, main_rect);

        if !(confirmed && is_actionable) {
            return None;
        }
        self.armed_at = None;
        self.run(ui.ctx(), page, services, choice.map(|(strategy, _)| strategy))
    }

    /// Carries out the confirmed click in the page's mode: the merge to run now, or `None` with the
    /// auto-complete write left for the page's runner.
    fn run(
        &self,
        ctx: &egui::Context,
        page: Page<'_>,
        services: &Services,
        strategy: Option<MergeStrategy>,
    ) -> Option<MergeOptions> {
        let Page { session, detail, mode } = page;
        if mode == Mode::CancelAutoComplete {
            actions::ask_auto_complete(ctx, session, AutoCompleteWrite::Cancel);
            return None;
        }
        let deletes_source_branch = self.deletes_source_branch(session, services)
            && auto_complete::offers_branch_deletion(mode, session.kind);
        let options = self.options(strategy?, &detail.head_sha, deletes_source_branch);
        if mode == Mode::Now {
            return Some(options);
        }
        let settings_url = self.policy.as_ref().and_then(|policy| policy.settings_url.clone());
        actions::ask_auto_complete(ctx, session, AutoCompleteWrite::Set { options, settings_url });
        None
    }

    /// What the main part shows, and after a refused merge its name with the reason, for screen
    /// readers and on hover.
    fn label(
        &self,
        choice: Option<(MergeStrategy, Option<Reason>)>,
        (mode, kind): (Mode, ProviderKind),
        is_busy: bool,
    ) -> (String, Option<String>) {
        if let Some(reason) = self.failure.as_ref().filter(|_| !is_busy) {
            return (FAILED_LABEL.to_owned(), Some(format!("{FAILED_LABEL}: {reason}")));
        }
        if let Some(label) = auto_complete::label(mode, kind, self.armed_at.is_some()) {
            return (label, None);
        }
        let label = match (self.armed_at, choice) {
            (Some(_), Some((strategy, _))) => merge_choice::confirm_label(strategy, kind),
            (None, Some((strategy, _))) => merge_choice::button_label(strategy, kind).to_owned(),
            (_, None) => complete_label(kind).to_owned(),
        };
        (label, None)
    }

    /// The merge to run with `strategy`, its message and the source branch choice, guarded by
    /// `head_sha`: the head of the detail the page shows, which is what the user reviewed.
    fn options(
        &self,
        strategy: MergeStrategy,
        head_sha: &str,
        delete_source_branch: bool,
    ) -> MergeOptions {
        let message = self.message.trim().to_owned();
        MergeOptions {
            strategy,
            delete_source_branch,
            commit_message: (strategy.takes_message() && !message.is_empty()).then_some(message),
            head_sha: head_sha.to_owned(),
        }
    }

    /// Esc disarms the armed button and Enter confirms it, unless a text field takes the keys;
    /// true when confirmed.
    fn armed_keys(&mut self, ui: &Ui) -> bool {
        if self.armed_at.is_none() || ui.ctx().text_edit_focused() {
            return false;
        }
        if ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
            self.armed_at = None;
            return false;
        }
        ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter))
    }

    /// The dropdown, once the policy is known and a strategy chosen.
    fn menu_popup(
        &mut self,
        ui: &Ui,
        page: Page<'_>,
        services: &mut Services,
        anchor: MenuAnchor,
        choice: Option<(MergeStrategy, Option<Reason>)>,
    ) -> MenuOutcome {
        let mut outcome = MenuOutcome::default();
        let Some(choice) = choice else {
            return outcome;
        };
        let Some(policy) = self.policy.clone() else {
            return outcome;
        };
        // The gap, the menu frame's margins and some room to the window's edge.
        let chrome = vec2(
            ui.spacing().menu_margin.sum().x + 2.0 * style::SPACE_S,
            style::SPACE_XS + ui.spacing().menu_margin.sum().y + style::SPACE_S,
        );
        let placement = MenuPlacement::new(ui.ctx().content_rect(), anchor.rect, chrome);
        egui::Popup::new(anchor.id, ui.ctx().clone(), anchor.rect, ui.layer_id())
            .open_memory(anchor.is_toggled.then_some(egui::SetOpenCommand::Toggle))
            .align(placement.align)
            // The side is picked here, for the height the menu gets there; egui's own pick
            // goes by last frame's size and could put a menu sized for below above the button.
            .align_alternatives(&[])
            .gap(style::SPACE_XS)
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .width(placement.width)
            .accessible_name("Merge options")
            .show(|ui| {
                // The popup's room starts at its size from the last frame; without this the
                // scroll area never grows when the menu does (a strategy with a commit message).
                ui.set_max_height(placement.max_height);
                egui::ScrollArea::vertical()
                    .id_salt("merge_menu")
                    .max_height(placement.max_height)
                    .show(ui, |ui| {
                        ui.set_width(placement.width);
                        outcome = self.menu(ui, page, services, &policy, choice);
                    });
            });
        outcome
    }

    fn menu(
        &mut self,
        ui: &mut Ui,
        page: Page<'_>,
        services: &mut Services,
        policy: &MergePolicy,
        (strategy, reason): (MergeStrategy, Option<Reason>),
    ) -> MenuOutcome {
        let palette = theme::palette(ui.ctx());
        let Page { session, detail, mode } = page;
        let kind = session.kind;
        let target = &detail.summary.target_branch;
        let mut outcome = MenuOutcome {
            arm: ui.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::Enter)),
            retry: false,
        };
        ui.spacing_mut().item_spacing.y = style::SPACE_XS;
        self.strategy_rows(ui, policy, strategy, kind, target);
        // A pick this frame shows next frame; the lines below follow the current one.
        let current =
            self.picked.filter(|picked| policy.allowed.contains(picked)).unwrap_or(strategy);
        let reason = if current == strategy { reason } else { None };
        divider(ui);
        favourite(ui, page, services, policy, (current, reason));
        if let Some(text) =
            merge_choice::disallowed(policy, &session.capabilities.merge_strategies, kind, target)
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(text).size(style::SMALL_SIZE).color(palette.text_secondary));
                policies_link(ui, policy);
            });
        } else if !policy.is_reported() {
            ui.horizontal_wrapped(|ui| {
                let text =
                    format!("{} didn't report which merge types {target} allows.", kind.label());
                ui.label(RichText::new(text).size(style::SMALL_SIZE).color(palette.text_secondary));
                outcome.retry = buttons::text_link(ui, "Retry", "Ask again").clicked();
                policies_link(ui, policy);
            });
        }
        if let Some(label) = merge_choice::message_label(current) {
            divider(ui);
            self.message_field(ui, label);
        }
        divider(ui);
        if auto_complete::offers_branch_deletion(mode, kind) {
            self.source_branch_choice(ui, page, services);
        }
        if auto_complete::is_waiting(detail, session.runs.ready().map(Vec::as_slice), kind) {
            let mut wants_auto = self.wants_auto.unwrap_or(true);
            if checkbox::checkbox(ui, &mut wants_auto, auto_complete::choice_label(kind)).changed()
            {
                self.wants_auto = Some(wants_auto);
            }
        }
        let action = match mode {
            Mode::Now => merge_choice::button_label(current, kind),
            Mode::SetAutoComplete | Mode::CancelAutoComplete => auto_complete::set_label(kind),
        };
        let hint = format!(
            concat!(shortcut!(command Enter), ", then ", shortcut!(Enter), ": {}"),
            action.to_lowercase()
        );
        ui.label(RichText::new(hint).size(style::SMALL_SIZE).color(palette.text_secondary));
        outcome
    }

    /// The strategies the branch allows, as menu radios; the number keys pick them too.
    fn strategy_rows(
        &mut self,
        ui: &mut Ui,
        policy: &MergePolicy,
        strategy: MergeStrategy,
        kind: ProviderKind,
        target: &str,
    ) {
        if !ui.ctx().text_edit_focused() {
            for (index, key) in NUMBER_KEYS.iter().enumerate().take(policy.allowed.len()) {
                if ui.input_mut(|input| input.consume_key(Modifiers::NONE, *key)) {
                    self.picked = Some(policy.allowed[index]);
                }
            }
        }
        let is_choice = merge_choice::is_choice(policy);
        let selected =
            self.picked.filter(|picked| policy.allowed.contains(picked)).unwrap_or(strategy);
        for (index, allowed) in policy.allowed.iter().enumerate() {
            let shown = Row {
                strategy: *allowed,
                kind,
                is_selected: *allowed == selected,
                is_default: policy.default == Some(*allowed),
                number: is_choice.then_some(index + 1),
                explanation: merge_choice::explain(*allowed, kind, policy, target),
            };
            if strategy_row(ui, &shown).clicked() {
                self.picked = Some(*allowed);
            }
        }
    }

    /// The commit message under `label`; it takes the focus once when the menu was opened from
    /// the keyboard.
    fn message_field(&mut self, ui: &mut Ui, label: &str) {
        let palette = theme::palette(ui.ctx());
        let caption =
            ui.label(RichText::new(label).size(style::SMALL_SIZE).color(palette.text_secondary));
        let edit = egui::ScrollArea::vertical()
            .id_salt("merge_message")
            .max_height(MESSAGE_MAX_HEIGHT)
            .show(ui, |ui| {
                ui.add(
                    TextEdit::multiline(&mut self.message)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                )
            })
            .inner
            .labelled_by(caption.id);
        if std::mem::take(&mut self.focus_message) {
            edit.request_focus();
        }
    }

    /// "Delete <branch> after merging", remembered for the repository.
    fn source_branch_choice(&mut self, ui: &mut Ui, page: Page<'_>, services: &mut Services) {
        let Page { session, detail, .. } = page;
        let scope = scope(session);
        let mut deletes = self.deletes_source_branch(session, services);
        if checkbox::checkbox(
            ui,
            &mut deletes,
            format!("Delete {} after merging", detail.summary.source_branch),
        )
        .changed()
        {
            self.delete_source_branch = Some(deletes);
            services.settings.merge.remember_delete_source_branch(
                scope.connection_id,
                scope.repository,
                deletes,
            );
            services.save_settings();
        }
    }
}

fn menu_id(id: egui::Id) -> egui::Id {
    id.with("menu")
}

fn scope(session: &PullRequestSession) -> Scope<'_> {
    Scope {
        kind: session.kind,
        connection_id: &session.key.connection_id,
        repository: &session.key.repository_key,
    }
}

/// The label while the branch's merge types load.
fn complete_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::AzureDevOps => "Complete",
        ProviderKind::GitHub | ProviderKind::GitLab => "Merge",
    }
}

#[cfg(test)]
mod tests {
    use review_core::MergeStrategy;

    use super::MergeButton;
    use cv_services::merge_policies::PolicyKey;

    fn button(message: &str) -> MergeButton {
        MergeButton {
            picked: None,
            message: message.to_owned(),
            delete_source_branch: None,
            wants_auto: None,
            armed_at: None,
            focus_message: false,
            is_menu_open: false,
            policy: None,
            key: PolicyKey {
                connection_id: "gh".into(),
                repository: "acme/app".into(),
                target_branch: "feature/base".into(),
            },
            failure: None,
        }
    }

    #[test]
    fn a_retargeted_pull_request_asks_the_new_target_branch_policy() {
        let mut button = button("");
        button.follow_target("main");
        assert_eq!(button.key.target_branch, "main");
    }

    /// The merge is guarded by the head the page showed, so a push after it is refused.
    #[test]
    fn merge_options_carry_the_shown_head() {
        let options = button(" Merged PR 7 ").options(MergeStrategy::Squash, "shown-head", true);
        assert_eq!(options.head_sha, "shown-head");
        assert_eq!(options.commit_message.as_deref(), Some("Merged PR 7"));
        assert!(options.delete_source_branch);
        let rebase = button("Merged PR 7").options(MergeStrategy::Rebase, "shown-head", false);
        assert_eq!((rebase.head_sha.as_str(), rebase.commit_message), ("shown-head", None));
    }
}
