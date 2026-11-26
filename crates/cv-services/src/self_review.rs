//! "Add me as reviewer": the signed-in user puts themself on a pull request's reviewers in one
//! click, from the Overview's Reviewers card (also Alt+R there) or a list row's context menu. No
//! vote is cast: the pull request then shows them as a reviewer whose feedback is pending
//! ([`ReviewVote::None`]).
//!
//! Who "me" is comes from [`Provider::current_user`], once per connection and run, in
//! [`Identities`]; the Reviewers card and the list rows share it through egui memory. The request itself
//! is [`add_me`]: it reads the user (the platform answers from its own cache where it has one),
//! reads the pull request, and asks [`Provider::add_reviewer`] with the user's login only when
//! they are not a reviewer yet, so a vote already cast is never reset (an Azure DevOps reviewer
//! entry written with vote 0 would reset it). The platforms resolve the login their usual way:
//! GitHub requests it as a user login, GitLab looks the username up, Azure DevOps uses the
//! signed-in identity of the organization directly.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use egui::{Id, Key, KeyboardShortcut, Modifiers};
use review_core::{Provider, ProviderError, PullRequestSummary, ReviewVote, UserIdentity};

use cv_base::platform::shortcut;
use cv_base::theme::{self, icons};
use cv_runtime::intents;
use cv_runtime::load::Load;
use cv_runtime::route::PullRequestKey;
use cv_runtime::toasts::{self, ProviderFailure, ToastAction, WaysOut};
use cv_widgets::{badges, buttons};

use crate::Services;

pub const LABEL: &str = "Add me as reviewer";
pub const PENDING_LABEL: &str = "Review pending";
/// The toast after a request: true both when it added the user and when they were a reviewer already.
pub const DONE: &str = "You are a reviewer now.";
pub const KEY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::ALT, Key::R);
pub const TOOLTIP: &str = concat!(
    "Adds you to the reviewers without a vote, so your feedback shows as pending (",
    shortcut!(alt "R"),
    ")"
);
const MENU_TOOLTIP: &str =
    "Adds you to the reviewers without a vote, so your feedback shows as pending.";
pub const PENDING_TOOLTIP: &str = "You are a reviewer of this pull request and have not voted yet.";
pub const NO_TOKEN_TOOLTIP: &str = "This connection has no token, so it cannot change reviewers. Add one with write access in \
     Settings, under the connection.";

/// What a list row's menu knows about its pull request's connection and request.
#[derive(Clone, Copy)]
pub struct RowAsk<'a> {
    pub connection_id: &'a str,
    pub has_token: bool,
    pub is_running: bool,
    /// Why the row's last request failed.
    pub failure: Option<&'a str>,
}

/// The row menu's entry: "Add me as reviewer", off with its reason where it cannot work, "Review
/// pending" once the user is a reviewer without a vote, nothing for the author or a voter. True
/// when it was clicked. Opening the menu asks who the user is; [`Identities::start_asked`] loads it.
pub fn menu_entry(ui: &mut egui::Ui, summary: &PullRequestSummary, row: &RowAsk<'_>) -> bool {
    let identities = Identities::of(ui.ctx());
    if row.has_token {
        identities.ask(row.connection_id);
    }
    let me = identities.known(row.connection_id);
    let is_checking = row.has_token
        && matches!(identities.state(row.connection_id), Some(Me::Asked | Me::Loading));
    match standing(summary, me.as_ref()) {
        Standing::Reviewer(ReviewVote::None) => {
            let palette = theme::palette(ui.ctx());
            let (icon, _) = badges::vote_icon(ReviewVote::None, &palette);
            ui.add_enabled_ui(false, |ui| buttons::menu_item(ui, icon, PENDING_LABEL, None, None))
                .response
                .on_disabled_hover_text(PENDING_TOOLTIP);
            return false;
        }
        Standing::Author | Standing::Reviewer(_) => return false,
        Standing::Unknown | Standing::Outside => {}
    }
    let label = if row.is_running { "Adding you as a reviewer…" } else { LABEL };
    let can_click = row.has_token && !row.is_running && !is_checking;
    let response = ui
        .add_enabled_ui(can_click, |ui| buttons::menu_item(ui, icons::USER_PLUS, label, None, None))
        .inner;
    let reason = if !row.has_token {
        NO_TOKEN_TOOLTIP
    } else if is_checking {
        "Checking which account this connection signs in with…"
    } else {
        "Your request is on its way."
    };
    let tooltip = match row.failure {
        Some(message) => format!("{message}\nClick to try again."),
        None => MENU_TOOLTIP.to_owned(),
    };
    let clicked = response.on_hover_text(tooltip).on_disabled_hover_text(reason).clicked();
    // The way out of the off state, as the Reviewers card offers it.
    if !row.has_token
        && buttons::menu_item(ui, icons::KEY, "Add a token to review", None, None).clicked()
    {
        intents::request_fix(ui.ctx(), row.connection_id);
        ui.close();
    }
    clicked
}

/// Where the signed-in user stands on one pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Who "me" is is not known yet.
    Unknown,
    Author,
    Reviewer(ReviewVote),
    /// Neither: "Add me as reviewer" is offered.
    Outside,
}

/// `me`'s part in `summary`; reviewer groups (teams) never count as the user.
pub fn standing(summary: &PullRequestSummary, me: Option<&UserIdentity>) -> Standing {
    let Some(me) = me else {
        return Standing::Unknown;
    };
    if is_same_person(&summary.author, me) {
        return Standing::Author;
    }
    summary
        .reviewers
        .iter()
        .find(|reviewer| !reviewer.is_group && is_same_person(&reviewer.identity, me))
        .map_or(Standing::Outside, |reviewer| Standing::Reviewer(reviewer.vote))
}

/// Same id, or the same login ignoring case: Azure DevOps identity ids differ per organization,
/// while the account name stays.
pub fn is_same_person(identity: &UserIdentity, me: &UserIdentity) -> bool {
    let same_id = !me.id.is_empty() && identity.id == me.id;
    let same_login = match (identity.login.as_deref(), me.login.as_deref()) {
        (Some(login), Some(mine)) => !mine.is_empty() && login.eq_ignore_ascii_case(mine),
        _ => false,
    };
    same_id || same_login
}

/// What the platform should resolve to the user: the login, else the id.
fn handle(me: &UserIdentity) -> &str {
    me.login.as_deref().filter(|login| !login.is_empty()).unwrap_or(&me.id)
}

/// Adds the signed-in user of `connection_id` to the reviewers of `number` in `repository`, unless
/// they are one already or its author, and returns who they are. Remembers the user in
/// `identities`.
pub async fn add_me(
    provider: Arc<dyn Provider>,
    identities: Identities,
    connection_id: String,
    repository: String,
    number: u64,
) -> Result<UserIdentity, ProviderError> {
    let me = provider.current_user().await?;
    identities.learned(&connection_id, me.clone());
    let detail = provider.pull_request(&repository, number).await?;
    match standing(&detail.summary, Some(&me)) {
        Standing::Reviewer(_) => Ok(me),
        Standing::Author => Err(ProviderError::Local(
            "You are the author of this pull request, so you cannot be its reviewer.".to_owned(),
        )),
        Standing::Unknown | Standing::Outside => {
            provider.add_reviewer(&repository, number, handle(&me)).await?;
            Ok(me)
        }
    }
}

/// What is known about the signed-in user of one connection.
#[derive(Clone)]
pub enum Me {
    /// Asked for, not started yet: the next [`Identities::start_asked`] loads it.
    Asked,
    Loading,
    Known(UserIdentity),
    /// The platform did not say (no token, a denial); "Add me as reviewer" asks again.
    Failed,
}

/// The signed-in user per connection id, shared by every view through egui memory and filled by
/// background tasks. Kept for the run: a token saved for another account shows after a restart
/// or after the next "Add me as reviewer", which reads the user again.
#[derive(Clone, Default)]
pub struct Identities {
    by_connection: Arc<Mutex<HashMap<String, Me>>>,
}

impl Identities {
    pub fn of(ctx: &egui::Context) -> Self {
        ctx.data_mut(|data| {
            data.get_temp_mut_or_default::<Self>(Id::unique("self_review_me")).clone()
        })
    }

    /// The user of `connection_id` once known.
    pub fn known(&self, connection_id: &str) -> Option<UserIdentity> {
        match self.lock().get(connection_id) {
            Some(Me::Known(me)) => Some(me.clone()),
            _ => None,
        }
    }

    pub fn state(&self, connection_id: &str) -> Option<Me> {
        self.lock().get(connection_id).cloned()
    }

    /// Marks the user of `connection_id` as wanted; widgets without the services call this.
    pub fn ask(&self, connection_id: &str) {
        self.lock().entry(connection_id.to_owned()).or_insert(Me::Asked);
    }

    /// Loads every asked user in the background; each wakes the UI once when it arrives.
    pub fn start_asked(&self, services: &mut Services) {
        let asked: Vec<String> = self
            .lock()
            .iter()
            .filter(|(_, me)| matches!(me, Me::Asked))
            .map(|(id, _)| id.clone())
            .collect();
        for connection_id in asked {
            let Some(provider) = services.provider(&connection_id) else {
                continue;
            };
            self.lock().insert(connection_id.clone(), Me::Loading);
            let identities = self.clone();
            services.background.spawner().spawn(async move {
                let me = provider.current_user().await.map_or(Me::Failed, Me::Known);
                identities.lock().insert(connection_id, me);
            });
        }
    }

    pub fn learned(&self, connection_id: &str, me: UserIdentity) {
        self.lock().insert(connection_id.to_owned(), Me::Known(me));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Me>> {
        self.by_connection.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// "Add me as reviewer" requests started from list rows, by pull request, and the last failure
/// of each. The list has no per-page write queue, so rows run their own, one per pull request.
#[derive(Default)]
pub struct RowRequests {
    running: HashMap<PullRequestKey, Load<Result<UserIdentity, ProviderFailure>>>,
    failed: HashMap<PullRequestKey, String>,
}

impl RowRequests {
    pub fn start(&mut self, services: &mut Services, key: PullRequestKey) {
        if self.is_running(&key) {
            return;
        }
        let Some(provider) = services.provider(&key.connection_id) else {
            return;
        };
        self.failed.remove(&key);
        let identities = Identities::of(&services.background.ctx());
        let future = add_me(
            provider,
            identities,
            key.connection_id.clone(),
            key.repository_key.clone(),
            key.number,
        );
        let mut load = Load::Idle;
        load.start(
            &services.background,
            async move { Ok(future.await.map_err(ProviderFailure::from)) },
        );
        self.running.insert(key, load);
    }

    fn is_running(&self, key: &PullRequestKey) -> bool {
        self.running.get(key).is_some_and(Load::is_loading)
    }

    /// Whether a row's request runs, and why its last one failed; a scan of the few requests, so
    /// painting a row builds no key.
    pub fn state_of(
        &self,
        connection_id: &str,
        repository_key: &str,
        number: u64,
    ) -> (bool, Option<&str>) {
        let is_row = |key: &PullRequestKey| {
            key.number == number
                && key.connection_id == connection_id
                && key.repository_key == repository_key
        };
        let is_running = self.running.iter().any(|(key, load)| is_row(key) && load.is_loading());
        let failure =
            self.failed.iter().find(|(key, _)| is_row(key)).map(|(_, message)| message.as_str());
        (is_running, failure)
    }

    /// Starts the requests a failure toast's Retry asked for, moves finished requests out, toasts
    /// their outcome and returns the ones that worked with the user they added, so the page can
    /// show them as pending reviewers at once.
    pub fn poll(&mut self, services: &mut Services) -> Vec<(PullRequestKey, UserIdentity)> {
        let ctx = services.background.ctx();
        let retried: Vec<PullRequestKey> = self
            .failed
            .keys()
            .filter(|key| toasts::take_retry(&ctx, retry_id(key)))
            .cloned()
            .collect();
        for key in retried {
            self.start(services, key);
        }
        let finished: Vec<PullRequestKey> = self
            .running
            .iter_mut()
            .filter_map(|(key, load)| load.poll().then(|| key.clone()))
            .collect();
        let mut added = Vec::new();
        for key in finished {
            match self.running.remove(&key) {
                Some(Load::Ready(Ok(me))) => {
                    services.toasts.success(DONE);
                    added.push((key, me));
                }
                Some(Load::Ready(Err(failure))) => self.failed_with(services, key, failure),
                Some(Load::Failed(message)) => {
                    self.failed_with(services, key, ProviderFailure::local(message));
                }
                Some(Load::Idle | Load::Loading(_)) | None => {}
            }
        }
        added
    }

    /// Keeps why the request for `key` failed and toasts it with Retry (this request once more),
    /// "Fix token" when the platform refused the token and the pull request on its platform.
    fn failed_with(
        &mut self,
        services: &mut Services,
        key: PullRequestKey,
        failure: ProviderFailure,
    ) {
        let page =
            ToastAction::open_pull_request(services.connection(&key.connection_id), &key, None);
        let retry = ToastAction::retry(retry_id(&key));
        let ways_out =
            WaysOut::new(retry).and_some(failure.fix_token(&key.connection_id)).and_some(page);
        let text = format!("Could not add you as a reviewer: {}", failure.message);
        services.toasts.error(text, ways_out);
        self.failed.insert(key, failure.message);
    }
}

/// Where a failure toast's Retry asks for the request about `key` once more.
fn retry_id(key: &PullRequestKey) -> Id {
    Id::unique(("self_review.retry", key))
}

#[cfg(test)]
mod tests {
    use review_core::{PullRequestState, PullRequestSummary, ReviewVote, Reviewer, UserIdentity};

    use super::{Standing, standing};

    fn person(id: &str, login: &str) -> UserIdentity {
        UserIdentity {
            id: id.into(),
            display_name: login.into(),
            login: Some(login.into()),
            avatar_url: None,
        }
    }

    fn summary(author: UserIdentity, reviewers: Vec<Reviewer>) -> PullRequestSummary {
        PullRequestSummary {
            repository_key: "cli/cli".into(),
            repository_name: "cli".into(),
            number: 7,
            title: "Change".into(),
            author,
            state: PullRequestState::Open,
            is_draft: false,
            source_branch: "feature".into(),
            target_branch: "main".into(),
            created_at: time::macros::datetime!(2026-10-16 09:30 UTC),
            updated_at: None,
            web_url: String::new(),
            labels: Vec::new(),
            reviewers,
            checks: None,
            diff_stat: review_core::DiffStat::default(),
            head_sha: None,
            target_sha: None,
        }
    }

    fn reviewer(identity: UserIdentity, vote: ReviewVote, is_group: bool) -> Reviewer {
        Reviewer { identity, vote, is_required: false, is_group }
    }

    #[test]
    fn tells_author_reviewer_and_outsider_apart() {
        let me = person("1", "Ada@Example.com");
        let ana = person("2", "ana");
        assert_eq!(standing(&summary(ana.clone(), Vec::new()), None), Standing::Unknown);
        assert_eq!(standing(&summary(me.clone(), Vec::new()), Some(&me)), Standing::Author);
        assert_eq!(standing(&summary(ana.clone(), Vec::new()), Some(&me)), Standing::Outside);
        // Azure DevOps: another organization's id, the same account name in other case.
        let elsewhere = person("other-org-id", "ada@example.com");
        let voted = summary(ana.clone(), vec![reviewer(elsewhere, ReviewVote::Approved, false)]);
        assert_eq!(standing(&voted, Some(&me)), Standing::Reviewer(ReviewVote::Approved));
        // A team named like the user is not the user.
        let team = summary(ana, vec![reviewer(person("1", "x"), ReviewVote::None, true)]);
        assert_eq!(standing(&team, Some(&me)), Standing::Outside);
    }
}
