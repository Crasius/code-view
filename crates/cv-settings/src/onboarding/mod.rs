//! Connection setup: the guided wizard (platform, host, token, access check, organizations), the
//! access banner for token problems at runtime, and the GitHub/Azure CLI sign-in options.
//!
//! Other views link token problems here without knowing the wizard:
//! * [`intents::request_fix`] opens the wizard for a connection at its token step ("Fix token").
//! * [`fix_token_button`] is the ready-made button for a 401/403 message.
//! * [`intents::request_add`] opens the wizard for a new connection to a cloud host, prefilled.
//! * `services.providers.issues().get(connection_id)` tells whether a connection has a recorded
//!   token problem, explained in the platform's permission names.
//!
//! Settings takes both requests ([`intents::take_fix_request`], [`intents::take_add_request`]);
//! the mailboxes live in `cv_runtime` so that views below this crate can ask too.

mod banner;
mod cli;
mod guides;
mod host;
pub mod parts;
mod renewal;
mod wizard;

use cv_base::theme::icons;
use cv_runtime::intents;
use cv_widgets::buttons;
use egui::{Response, Ui};

pub use banner::show as show_banner;
pub use cli::CliTools;
pub use renewal::{CliRenewal, uses_azure_cli};
pub use wizard::{EditStart, Wizard, WizardOutcome};

/// "Fix token" for a 401/403 message about `connection_id`; clicking it calls
/// [`intents::request_fix`].
pub fn fix_token_button(ui: &mut Ui, connection_id: &str) -> Response {
    let response = buttons::primary_icon(ui, icons::KEY, "Fix token")
        .on_hover_text("Open the connection setup with what the token is missing");
    if response.clicked() {
        intents::request_fix(ui.ctx(), connection_id);
    }
    response
}
