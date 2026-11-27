//! Azure CLI sign-ins are Microsoft Entra tokens that last at most an hour. Connections the user
//! set up with "Use my Azure CLI sign-in" (their stored token is a JWT) are renewed from the Azure
//! CLI when the app starts and then periodically, so they keep working without another click.
//!
//! The one timer that keeps running while the window is inactive: it only starts the local Azure
//! CLI (no request to a review host), and a token that lapsed while the app sat in the background
//! would make the catch-up refreshes on focus fail with 401 and a misleading "Fix token" banner.
//! While a renewal runs, the other automatic refreshes wait for it.

use std::time::Duration;

use cv_runtime::background::{Task, TaskPoll};
use cv_runtime::refresh::WhenInactive;
use cv_runtime::toasts::ToastAction;
use cv_services::Services;
use review_core::{AccessProblem, Connection, ProviderKind};

use super::cli::{CliLogin, CliTools};

/// Entra tokens are valid for at least five and at most sixty minutes after they are issued.
const RENEW_EVERY: Duration = Duration::from_mins(40);
const REFRESH_KEY: &str = "azure-cli-renewal";

#[derive(Default)]
pub struct CliRenewal {
    has_started: bool,
    running: Vec<(String, Task<Result<String, String>>)>,
}

impl CliRenewal {
    pub fn update(&mut self, ctx: &egui::Context, services: &mut Services, tools: CliTools) {
        self.collect(services);
        if !tools.azure {
            return;
        }
        if !self.running.is_empty() {
            services.refresh.hold(ctx);
            return;
        }
        let renewable: Vec<String> = services
            .connections
            .all()
            .iter()
            .filter(|connection| uses_azure_cli(connection))
            .map(|connection| connection.id.clone())
            .collect();
        if renewable.is_empty() {
            return;
        }
        let is_due = services.refresh.due(ctx, REFRESH_KEY, RENEW_EVERY, WhenInactive::KeepRunning);
        if self.has_started && !is_due {
            return;
        }
        self.has_started = true;
        services.refresh.restart(ctx, REFRESH_KEY);
        for connection_id in renewable {
            let task = services.background.spawn(CliLogin::Azure.fetch_token());
            self.running.push((connection_id, task));
        }
        services.refresh.hold(ctx);
    }

    fn collect(&mut self, services: &mut Services) {
        let mut finished = Vec::new();
        self.running.retain_mut(|(connection_id, task)| match task.poll() {
            TaskPoll::Pending => true,
            TaskPoll::Done(result) => {
                finished.push((connection_id.clone(), result));
                false
            }
            TaskPoll::Lost => false,
        });
        for (connection_id, result) in finished {
            apply(services, &connection_id, result);
        }
    }
}

/// Only the Azure CLI option stores JWTs; personal access tokens never look like one.
pub fn uses_azure_cli(connection: &Connection) -> bool {
    connection.kind == ProviderKind::AzureDevOps
        && connection.base_url().eq_ignore_ascii_case(ProviderKind::AzureDevOps.default_base_url())
        && connection
            .token()
            .is_some_and(|token| token.starts_with("eyJ") && token.split('.').count() == 3)
}

/// Stores a renewed sign-in, or records why it failed; nothing when the connection was removed or
/// no longer signs in with the Azure CLI (the user saved another token while the renewal ran).
fn apply(services: &mut Services, connection_id: &str, result: Result<String, String>) {
    let Some(mut connection) =
        services.connection(connection_id).filter(|connection| uses_azure_cli(connection)).cloned()
    else {
        return;
    };
    match result {
        Ok(token) if connection.token() == Some(token.as_str()) => {}
        Ok(token) => {
            connection.token = Some(token);
            match services.connections.upsert(connection) {
                Ok(()) => services.connection_saved(connection_id),
                // The wizard saves the token again: "Use my Azure CLI sign-in" at its token step.
                Err(error) => services.toasts.error(
                    format!("Could not save the renewed Azure CLI sign-in: {error}"),
                    ToastAction::fix_token(connection_id),
                ),
            }
        }
        Err(message) => services.providers.issues().record(
            connection_id,
            AccessProblem {
                status: 401,
                title: format!("The Azure CLI sign-in could not be renewed. {message}"),
                fix: "Run az login in a terminal, then choose Fix token and Use my Azure CLI sign-in.".to_owned(),
                actions: Vec::new(),
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use review_core::{Connection, ProviderKind};
    use review_store::AppPaths;

    use super::apply;
    use cv_services::Services;
    use cv_testkit::ScratchDir;

    const SIGN_IN: &str = "eyJhead.payload.signature";

    fn services_with(dir: &ScratchDir, token: &str) -> Services {
        let paths = AppPaths::new(dir.path().join("config"), dir.path().join("cache"));
        let mut services =
            Services::new(egui::Context::default(), paths, cv_providers::create_provider);
        let connection = Connection {
            id: "ado".into(),
            name: "Contoso".into(),
            kind: ProviderKind::AzureDevOps,
            base_url: ProviderKind::AzureDevOps.default_base_url().into(),
            organizations: vec!["contoso".into()],
            token: Some(token.into()),
        };
        services.connections.upsert(connection).expect("the test profile keeps tokens");
        services
    }

    #[test]
    fn a_renewal_keeps_a_token_saved_while_it_ran() {
        let dir = ScratchDir::new("renewal-replaced");
        // The user switched to a personal access token while the Azure CLI was asked.
        let mut services = services_with(&dir, "personal-access-token");
        apply(&mut services, "ado", Ok("eyJnew.payload.signature".into()));
        let token = services.connection("ado").and_then(Connection::token);
        assert_eq!(token, Some("personal-access-token"), "the saved token stays");
        apply(&mut services, "ado", Err("az: not signed in".into()));
        assert!(
            services.providers.issues().get("ado").is_none(),
            "no problem about a sign-in it no longer uses"
        );
    }

    #[test]
    fn a_renewal_replaces_the_azure_cli_sign_in() {
        let dir = ScratchDir::new("renewal-renewed");
        let mut services = services_with(&dir, SIGN_IN);
        apply(&mut services, "ado", Ok("eyJnew.payload.signature".into()));
        let token = services.connection("ado").and_then(Connection::token);
        assert_eq!(token, Some("eyJnew.payload.signature"));
        apply(&mut services, "ado", Err("az: not signed in".into()));
        let problem = services.providers.issues().get("ado").expect("a failed renewal says so");
        assert_eq!(problem.status, 401, "the banner offers Fix token");
        assert!(problem.title.contains("az: not signed in"), "{}", problem.title);
    }
}
