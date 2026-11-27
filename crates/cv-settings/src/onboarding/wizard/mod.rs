//! The connection wizard: platform → host → sign-in → access check → organizations.
//!
//! Every network call here is a read (sign-in, permission probes, discovery). The draft
//! connection only reaches the store when the user saves on the last step.

mod access_step;
mod scope_step;
mod start_steps;
mod token_step;

use std::time::{Duration, Instant};

use cv_base::theme::{self, icons};
use cv_runtime::background::{Task, TaskPoll};
use cv_runtime::load::Load;
use cv_services::Services;
use cv_widgets::buttons;
use egui::{Align, Layout, RichText, Ui};
use review_core::{
    AccessProblem, AccessReport, Connection, ProviderKind, ScopeCandidate, TokenGuide,
};
use review_store::VaultProblem;

use super::cli::{CliLogin, CliTools};
use super::guides;
use super::host::{self, Hosting, ResolvedHost};
use super::parts;

/// Pause after the last keystroke before a pasted or typed token is checked.
const CHECK_DELAY: Duration = Duration::from_millis(600);
const CONTENT_WIDTH: f32 = 760.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Step {
    Platform,
    Host,
    Token,
    Access,
    Scope,
}

impl Step {
    const ALL: [Step; 5] = [Step::Platform, Step::Host, Step::Token, Step::Access, Step::Scope];

    fn index(self) -> usize {
        self as usize
    }
}

/// Where an edit of an existing connection starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditStart {
    /// Change host or token.
    Token,
    /// Re-run the access check with the stored token.
    Recheck,
    /// A request was denied at runtime; show why next to the token options.
    Fix(Option<AccessProblem>),
}

pub enum WizardOutcome {
    Open,
    Cancelled,
    Saved { name: String },
}

struct Failure {
    message: String,
    problem: Option<AccessProblem>,
}

enum Check {
    Idle,
    Running { token: String, task: Task<Result<AccessReport, Failure>> },
    Done { token: String, report: AccessReport },
    Failed { token: String, failure: Failure },
}

impl Check {
    fn token(&self) -> Option<&str> {
        match self {
            Check::Idle => None,
            Check::Running { token, .. }
            | Check::Done { token, .. }
            | Check::Failed { token, .. } => Some(token),
        }
    }
}

/// Whether this system can keep a token now (on Linux: is a Secret Service running?). Asked once
/// when the wizard opens and again on "Check again"; a failed save brings its own answer.
enum VaultCheck {
    Idle,
    Running(Task<Result<(), VaultProblem>>),
    Ready,
    Problem(VaultProblem),
}

/// What the user chose in the keyring message.
#[derive(Default)]
struct VaultChoice {
    wants_recheck: bool,
    wants_tokenless_save: bool,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent flags of separate wizard steps (first run, token reveal, scope choice, custom name); none of them combine into a state"
)]
pub struct Wizard {
    existing: Option<Connection>,
    is_first_run: bool,
    problem: Option<AccessProblem>,
    step: Step,
    kind: ProviderKind,
    hosting: Hosting,
    host_input: String,
    token_input: String,
    reveals_token: bool,
    token_edited_at: Option<Instant>,
    cli_tools: CliTools,
    cli_token: Option<(CliLogin, String)>,
    cli_fetch: Option<(CliLogin, Task<Result<String, String>>)>,
    cli_error: Option<(CliLogin, String)>,
    wants_check: bool,
    check: Check,
    shows_alternative: bool,
    ticked: Vec<bool>,
    discovery: Load<Vec<ScopeCandidate>>,
    discovered_for: Option<String>,
    includes_everything: bool,
    selected: Vec<String>,
    extra_scopes: String,
    name: String,
    is_name_custom: bool,
    vault: VaultCheck,
    /// Why the last save failed, when the token store was not the reason.
    save_error: Option<String>,
}

impl Wizard {
    /// A new connection, from the platform step.
    pub fn new(cli_tools: CliTools) -> Self {
        Self {
            existing: None,
            is_first_run: false,
            problem: None,
            step: Step::Platform,
            kind: ProviderKind::AzureDevOps,
            hosting: Hosting::Cloud,
            host_input: String::new(),
            token_input: String::new(),
            reveals_token: false,
            token_edited_at: None,
            cli_tools,
            cli_token: None,
            cli_fetch: None,
            cli_error: None,
            wants_check: false,
            check: Check::Idle,
            shows_alternative: false,
            ticked: Vec::new(),
            discovery: Load::Idle,
            discovered_for: None,
            includes_everything: true,
            selected: Vec::new(),
            extra_scopes: String::new(),
            name: String::new(),
            is_name_custom: false,
            vault: VaultCheck::Idle,
            save_error: None,
        }
    }

    /// The first connection of a profile without any: no way out but through the steps.
    pub fn first_run(cli_tools: CliTools) -> Self {
        Self { is_first_run: true, ..Self::new(cli_tools) }
    }

    pub fn edit(connection: &Connection, cli_tools: CliTools, start: EditStart) -> Self {
        let is_cloud =
            connection.base_url().eq_ignore_ascii_case(connection.kind.default_base_url());
        let hosting = if is_cloud { Hosting::Cloud } else { Hosting::SelfHosted };
        let first_organization = connection.organizations.first().cloned().unwrap_or_default();
        let host_input = match (connection.kind, hosting) {
            (ProviderKind::AzureDevOps, Hosting::Cloud) => first_organization,
            (ProviderKind::AzureDevOps, Hosting::SelfHosted) => {
                format!("{}/{first_organization}", connection.base_url())
            }
            (_, Hosting::Cloud) => String::new(),
            (_, Hosting::SelfHosted) => connection.base_url().to_owned(),
        };
        let (step, problem, wants_check) = match start {
            EditStart::Token => (Step::Token, None, false),
            EditStart::Recheck => (Step::Access, None, connection.token().is_some()),
            EditStart::Fix(problem) => (Step::Token, problem, false),
        };
        Self {
            existing: Some(connection.clone()),
            step,
            problem,
            wants_check,
            kind: connection.kind,
            hosting,
            host_input,
            includes_everything: connection.organizations.is_empty(),
            selected: connection.organizations.clone(),
            name: connection.name.clone(),
            is_name_custom: true,
            ..Self::new(cli_tools)
        }
    }

    /// A new connection to `kind`'s cloud host (and `organization` on Azure DevOps), starting at
    /// the sign-in step; platform and host stay a step back in the stepper.
    pub fn for_cloud(
        kind: ProviderKind,
        organization: Option<String>,
        cli_tools: CliTools,
    ) -> Self {
        let mut wizard = Self {
            kind,
            hosting: Hosting::Cloud,
            host_input: organization.unwrap_or_default(),
            ..Self::new(cli_tools)
        };
        wizard.step = if wizard.resolved().is_ok() { Step::Token } else { Step::Host };
        wizard
    }

    pub fn show(&mut self, ui: &mut Ui, services: &mut Services) -> WizardOutcome {
        self.poll();
        if matches!(self.vault, VaultCheck::Idle) {
            self.start_vault_check(services);
        }
        self.start_pending_check(ui.ctx(), services);
        let mut outcome = WizardOutcome::Open;
        ui.vertical_centered(|ui| {
            ui.set_max_width(CONTENT_WIDTH);
            ui.vertical(|ui| {
                ui.add_space(theme::style::SPACE_S);
                if self.header(ui) {
                    outcome = WizardOutcome::Cancelled;
                }
                ui.add_space(theme::style::SPACE_L);
                let labels = self.step_labels();
                if let Some(index) = parts::stepper(ui, &labels, self.step.index()) {
                    self.step = Step::ALL[index];
                }
                ui.add_space(theme::style::SPACE_L);
                parts::sheet(ui, |ui| {
                    let saved = match self.step {
                        Step::Platform => {
                            start_steps::platform(self, ui);
                            None
                        }
                        Step::Host => {
                            start_steps::host(self, ui);
                            None
                        }
                        Step::Token => {
                            token_step::show(self, ui, services);
                            None
                        }
                        Step::Access => {
                            access_step::show(self, ui, services);
                            None
                        }
                        Step::Scope => scope_step::show(self, ui, services),
                    };
                    if let Some(name) = saved {
                        outcome = WizardOutcome::Saved { name };
                    }
                });
            });
        });
        outcome
    }

    /// Title row; returns true when the user cancels.
    fn header(&self, ui: &mut Ui) -> bool {
        let palette = theme::palette(ui.ctx());
        let mut is_cancelled = false;
        ui.horizontal(|ui| {
            let title = match &self.existing {
                Some(connection) if self.problem.is_some() => {
                    format!("Fix the token of {}", connection.name)
                }
                Some(connection) => format!("Edit {}", connection.name),
                None if self.is_first_run => "Welcome to Code View".to_owned(),
                None => "Add a connection".to_owned(),
            };
            ui.label(
                RichText::new(title).font(theme::fonts::semibold_font(theme::style::TITLE_SIZE)),
            );
            if !self.is_first_run {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if buttons::subtle(ui, icons::X, "Cancel").clicked() {
                        is_cancelled = true;
                    }
                });
            }
        });
        if self.is_first_run {
            ui.label(
                RichText::new(
                    "Connect the place where your pull requests live. It takes about a minute.",
                )
                .color(palette.text_secondary),
            );
        }
        is_cancelled
    }

    fn step_labels(&self) -> [&'static str; 5] {
        let scope = match self.kind {
            ProviderKind::GitLab => "Groups",
            ProviderKind::AzureDevOps if self.hosting == Hosting::SelfHosted => "Collections",
            _ => "Organizations",
        };
        ["Platform", "Host", "Sign in", "Access", scope]
    }

    fn poll(&mut self) {
        if let VaultCheck::Running(task) = &mut self.vault {
            match task.poll() {
                TaskPoll::Pending => {}
                // Lost is unknown: the save itself reports a store that cannot take the token.
                TaskPoll::Done(Ok(())) | TaskPoll::Lost => self.vault = VaultCheck::Ready,
                TaskPoll::Done(Err(problem)) => self.vault = VaultCheck::Problem(problem),
            }
        }
        if let Some((login, task)) = &mut self.cli_fetch {
            match task.poll() {
                TaskPoll::Pending => {}
                TaskPoll::Done(Ok(token)) => {
                    self.cli_token = Some((login.clone(), token));
                    self.cli_error = None;
                    self.cli_fetch = None;
                    self.token_input.clear();
                    self.wants_check = true;
                }
                TaskPoll::Done(Err(message)) => {
                    self.cli_error = Some((login.clone(), message));
                    self.cli_fetch = None;
                }
                TaskPoll::Lost => self.cli_fetch = None,
            }
        }
        if let Check::Running { token, task } = &mut self.check {
            let next = match task.poll() {
                TaskPoll::Pending => None,
                TaskPoll::Done(Ok(report)) => Some(Check::Done { token: token.clone(), report }),
                TaskPoll::Done(Err(failure)) => {
                    Some(Check::Failed { token: token.clone(), failure })
                }
                TaskPoll::Lost => Some(Check::Failed {
                    token: token.clone(),
                    failure: Failure {
                        message: "The check stopped unexpectedly.".to_owned(),
                        problem: None,
                    },
                }),
            };
            if let Some(next) = next {
                let is_done = matches!(next, Check::Done { .. });
                self.check = next;
                // A check of a token replaced while it ran shows nothing: the new one is checked next.
                if is_done && self.report().is_some() && self.step == Step::Token {
                    self.step = Step::Access;
                }
                self.suggest_name();
            }
        }
        // A new Azure DevOps connection starts with the organization its host names.
        if self.discovery.poll()
            && self.discovery.ready().is_some()
            && self.existing.is_none()
            && self.selected.is_empty()
            && self.kind == ProviderKind::AzureDevOps
        {
            self.selected =
                self.resolved().ok().and_then(|host| host.organization).into_iter().collect();
        }
    }

    fn start_pending_check(&mut self, ctx: &egui::Context, services: &mut Services) {
        if let Some(edited_at) = self.token_edited_at {
            if let Some(left) = CHECK_DELAY.checked_sub(edited_at.elapsed())
                && !left.is_zero()
            {
                ctx.request_repaint_after(left);
                return;
            }
            self.token_edited_at = None;
            if !self.token_input.trim().is_empty() {
                self.wants_check = true;
            }
        }
        if std::mem::take(&mut self.wants_check) {
            self.start_check(services);
        }
    }

    // --- derived state ------------------------------------------------------------------

    fn resolved(&self) -> Result<ResolvedHost, String> {
        host::resolve(self.kind, self.hosting, &self.host_input)
    }

    fn guide(&self) -> Option<TokenGuide> {
        self.resolved().ok().map(|host| self.guide_at(&host))
    }

    fn guide_at(&self, host: &ResolvedHost) -> TokenGuide {
        let owner = self.selected.first().map(String::as_str);
        let organization = host.organization.as_deref().or(owner);
        guides::token_guide(self.kind, &host.base_url, organization)
    }

    /// Where a new token with the recommended settings is created on `host`.
    fn fix_url(&self, host: &ResolvedHost) -> String {
        self.guide_at(host).recommended.create_url
    }

    /// The token the next check uses: CLI sign-in, then pasted text, then the stored one.
    fn token(&self) -> Option<String> {
        if let Some((_, token)) = &self.cli_token {
            return Some(token.clone());
        }
        let pasted = self.token_input.trim();
        if !pasted.is_empty() {
            return Some(pasted.to_owned());
        }
        self.existing.as_ref().and_then(|connection| connection.token().map(str::to_owned))
    }

    fn report(&self) -> Option<&AccessReport> {
        match &self.check {
            Check::Done { token, report } if Some(token.as_str()) == self.token().as_deref() => {
                Some(report)
            }
            _ => None,
        }
    }

    fn cli_login(&self) -> Option<CliLogin> {
        let host = self.resolved().ok()?;
        self.cli_tools.login_for(self.kind, host.host_name(), self.hosting)
    }

    /// The connection as it would be saved; organizations as currently chosen.
    fn draft(&self, services: &Services) -> Option<Connection> {
        let host = self.resolved().ok()?;
        let id = self.existing.as_ref().map_or_else(
            || services.connections.next_id(self.kind),
            |connection| connection.id.clone(),
        );
        Some(Connection {
            id,
            name: self.name.trim().to_owned(),
            kind: self.kind,
            base_url: host.base_url.clone(),
            organizations: self.organizations(&host),
            token: self.token(),
        })
    }

    fn organizations(&self, host: &ResolvedHost) -> Vec<String> {
        let typed = self
            .extra_scopes
            .split([',', ';', '\n'])
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_owned);
        let mut organizations: Vec<String> = Vec::new();
        let wants_all = self.includes_everything && self.kind != ProviderKind::AzureDevOps;
        if !wants_all {
            for organization in self.selected.iter().cloned().chain(typed) {
                if !organizations.iter().any(|known| known.eq_ignore_ascii_case(&organization)) {
                    organizations.push(organization);
                }
            }
        }
        if organizations.is_empty()
            && let Some(organization) =
                host.organization.clone().filter(|_| self.kind == ProviderKind::AzureDevOps)
        {
            organizations.push(organization);
        }
        organizations
    }

    fn suggest_name(&mut self) {
        if self.is_name_custom {
            return;
        }
        let Some(report) = self.report() else {
            return;
        };
        let Ok(host) = self.resolved() else {
            return;
        };
        let who = report.user.login.clone().unwrap_or_else(|| report.user.display_name.clone());
        self.name = format!("{who} on {}", host.display());
    }

    // --- actions ------------------------------------------------------------------------

    fn select_kind(&mut self, kind: ProviderKind) {
        if kind != self.kind {
            self.kind = kind;
            self.host_input.clear();
            self.ticked.clear();
            self.reset_token();
        }
        self.step = Step::Host;
    }

    fn reset_token(&mut self) {
        self.token_input.clear();
        self.cli_token = None;
        self.cli_error = None;
        self.check = Check::Idle;
        self.discovery = Load::Idle;
        self.discovered_for = None;
    }

    fn start_check(&mut self, services: &mut Services) {
        let Some(draft) = self.draft(services) else {
            return;
        };
        let Some(token) = draft.token.clone() else {
            return;
        };
        if self.check.token() == Some(token.as_str()) && !matches!(self.check, Check::Failed { .. })
        {
            return;
        }
        let provider = services.providers.build_uncached(&draft);
        let task = services.background.spawn(async move {
            provider.inspect_access().await.map_err(|error| Failure {
                problem: provider.explain_denial(&error),
                message: error.to_string(),
            })
        });
        self.check = Check::Running { token, task };
    }

    /// Forces a fresh check of the current token.
    fn recheck(&mut self, services: &mut Services) {
        self.check = Check::Idle;
        self.start_check(services);
    }

    fn start_cli(&mut self, services: &Services, login: CliLogin) {
        let task = services.background.spawn(login.clone().fetch_token());
        self.cli_error = None;
        self.cli_fetch = Some((login, task));
    }

    fn start_discovery(&mut self, services: &mut Services) {
        let Some(draft) = self.draft(services) else {
            return;
        };
        if self.discovered_for == draft.token {
            return;
        }
        self.discovered_for.clone_from(&draft.token);
        let provider = services.providers.build_uncached(&draft);
        self.discovery.start(&services.background, async move { provider.discover_scopes().await });
    }

    /// Asks off the UI thread whether the profile's token vault could store a token (for the
    /// system's store a D-Bus round trip on Linux).
    fn start_vault_check(&mut self, services: &Services) {
        let vault = services.paths.token_vault();
        let task = services.background.spawn(async move {
            tokio::task::spawn_blocking(move || vault.check()).await.unwrap_or(Ok(()))
        });
        self.vault = VaultCheck::Running(task);
    }

    /// Stores the connection, without its token when `drops_token`; returns its name. A failure
    /// stays on the step with its fix: the store's own ([`VaultProblem`]) or the error.
    fn save(&mut self, services: &mut Services, drops_token: bool) -> Option<String> {
        let mut connection = self.draft(services)?;
        if drops_token {
            connection.token = None;
        }
        let connection_id = connection.id.clone();
        let name = connection.name.clone();
        self.save_error = None;
        match services.connections.upsert(connection) {
            Ok(()) => {
                services.connection_saved(&connection_id);
                Some(name)
            }
            Err(error) => {
                match VaultProblem::of(&error) {
                    Some(problem) => self.vault = VaultCheck::Problem(problem.clone()),
                    None => {
                        self.save_error = Some(format!(
                            "{error} ({})",
                            services.paths.connections_file().display()
                        ));
                    }
                }
                None
            }
        }
    }

    /// The token store's problem with its fix: the commands to copy, the setup guide, a new check
    /// and, where a connection is about to be saved, saving it without the token.
    fn vault_message(&self, ui: &mut Ui, offers_tokenless_save: bool) -> VaultChoice {
        let mut choice = VaultChoice::default();
        let VaultCheck::Problem(problem) = &self.vault else {
            return choice;
        };
        let body = if problem.commands.is_empty() {
            problem.fix.clone()
        } else {
            format!("{}\n\n{}", problem.fix, problem.commands.join("\n"))
        };
        parts::callout(ui, parts::Tone::Warning, &problem.title, &body, |ui| {
            parts::open_button(ui, false, "Open the setup guide", problem.help_url);
            if !problem.commands.is_empty()
                && buttons::secondary_icon(ui, icons::COPY, "Copy commands").clicked()
            {
                ui.ctx().copy_text(problem.commands.join("\n"));
            }
            if buttons::secondary_icon(ui, icons::ARROW_CLOCKWISE, "Check again").clicked() {
                choice.wants_recheck = true;
            }
            if offers_tokenless_save
                && buttons::secondary_icon(ui, icons::FLOPPY_DISK, "Save without the token")
                    .clicked()
            {
                choice.wants_tokenless_save = true;
            }
        });
        choice
    }
}

/// What the user asked for to fix the token.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fix {
    /// Check the same token again.
    Recheck,
    /// Back to the token step with an empty field.
    NewToken,
}

/// Why the check failed: the problem's title and fix over the error, else the error alone, with a
/// new token, a paste where `offers_paste` (the token step has its field right there) and a retry.
fn failed_callout(
    ui: &mut Ui,
    failure: &Failure,
    fix_url: &str,
    offers_paste: bool,
) -> Option<Fix> {
    let (title, body) = match &failure.problem {
        Some(problem) => (problem.title.as_str(), format!("{}\n{}", problem.fix, failure.message)),
        None => ("The token could not be checked", failure.message.clone()),
    };
    let mut fix = None;
    parts::callout(ui, parts::Tone::Error, title, &body, |ui| {
        parts::open_button(ui, true, "Create a new token", fix_url);
        if offers_paste
            && buttons::secondary_icon(ui, icons::CLIPBOARD_TEXT, "Paste a new token").clicked()
        {
            fix = Some(Fix::NewToken);
        }
        if buttons::secondary_icon(ui, icons::ARROW_CLOCKWISE, "Try again").clicked() {
            fix = Some(Fix::Recheck);
        }
    });
    fix
}

/// Back on the left, the step's actions on the right.
fn footer(ui: &mut Ui, back: Option<&mut bool>, add_actions: impl FnOnce(&mut Ui)) {
    ui.add_space(theme::style::SPACE_XL);
    parts::divider(ui);
    ui.add_space(theme::style::SPACE_M);
    ui.horizontal(|ui| {
        if let Some(back) = back
            && buttons::secondary_icon(ui, icons::ARROW_LEFT, "Back").clicked()
        {
            *back = true;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), add_actions);
    });
}

#[cfg(test)]
mod tests {
    use review_core::{AccessReport, ProviderKind, UserIdentity};

    use super::{Check, CliTools, Step, Wizard};
    use cv_runtime::background::Task;

    /// The wizard at the sign-in step with `typed` in the token field, while the token `checked`
    /// was checked and found working.
    fn checked(typed: &str, checked: &str) -> Wizard {
        let mut wizard = Wizard::for_cloud(ProviderKind::GitHub, None, CliTools::default());
        assert_eq!(wizard.step, Step::Token);
        wizard.token_input = typed.to_owned();
        let report = AccessReport::unverified(UserIdentity::default(), "Token");
        let task = Task::ready(Ok(report));
        wizard.check = Check::Running { token: checked.to_owned(), task };
        wizard.poll();
        assert!(
            !matches!(wizard.check, Check::Running { .. }),
            "a finished check is taken at once"
        );
        wizard
    }

    #[test]
    fn a_working_token_moves_on_to_its_access() {
        assert_eq!(checked("ghp_new", "ghp_new").step, Step::Access);
    }

    #[test]
    fn a_check_of_a_replaced_token_keeps_the_sign_in_step() {
        let wizard = checked("ghp_new", "ghp_old");
        assert_eq!(wizard.step, Step::Token, "the user is still pasting the new token");
        assert!(wizard.report().is_none(), "the old token's report is not the new one's");
    }
}
