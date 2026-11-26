//! Mailboxes for asking another page to open something: the connection setup at a token step or
//! for a new cloud connection ([`request_fix`], [`request_add`]), Settings at a section or at the
//! keyboard shortcuts ([`request_settings_section`], [`request_shortcuts`]), and the title bar's
//! search ([`open_search`]).
//!
//! Each request is one temporary value in the egui context, left by whoever asks and taken at most
//! once by the page that answers, after the current frame. They live here, low in the crate graph,
//! so that toasts, the window chrome, lists and pull request pages can ask without depending on
//! the onboarding wizard, Settings or the title bar that answer.

use egui::Id;
use review_core::ProviderKind;

const FIX_REQUEST_ID: &str = "review.onboarding.fix_request";
const ADD_REQUEST_ID: &str = "review.onboarding.add_request";
const SHORTCUTS_REQUEST_ID: &str = "settings_shortcuts_request";
const SECTION_REQUEST_ID: &str = "settings_section_request";

/// Asks the app to open the setup wizard for `connection_id` at the token step. Takes effect
/// after the current frame; the app switches to Settings.
pub fn request_fix(ctx: &egui::Context, connection_id: &str) {
    ctx.data_mut(|data| data.insert_temp(Id::unique(FIX_REQUEST_ID), connection_id.to_owned()));
    ctx.request_repaint();
}

/// The pending [`request_fix`], at most once.
pub fn take_fix_request(ctx: &egui::Context) -> Option<String> {
    ctx.data_mut(|data| data.remove_temp::<String>(Id::unique(FIX_REQUEST_ID)))
}

/// A connection setup to start on a platform's cloud host: the platform and, for Azure DevOps,
/// the organization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddRequest {
    pub kind: ProviderKind,
    pub organization: Option<String>,
}

/// Asks the app to open the setup wizard for a new connection to `request`'s cloud host, at the
/// sign-in step. Takes effect after the current frame; the app switches to Settings.
pub fn request_add(ctx: &egui::Context, request: AddRequest) {
    ctx.data_mut(|data| data.insert_temp(Id::unique(ADD_REQUEST_ID), request));
    ctx.request_repaint();
}

/// The pending [`request_add`], at most once.
pub fn take_add_request(ctx: &egui::Context) -> Option<AddRequest> {
    ctx.data_mut(|data| {
        let id = Id::unique(ADD_REQUEST_ID);
        let request = data.get_temp::<AddRequest>(id);
        data.remove::<AddRequest>(id);
        request
    })
}

/// Asks Settings to open at Keyboard and mouse, at the shortcuts of the page shown: what `?` does,
/// for the rail's keyboard button.
pub fn request_shortcuts(ctx: &egui::Context) {
    ctx.data_mut(|data| data.insert_temp(Id::unique(SHORTCUTS_REQUEST_ID), true));
    ctx.request_repaint();
}

/// Whether [`request_shortcuts`] asked since the last call; true once per request.
pub fn take_shortcuts_request(ctx: &egui::Context) -> bool {
    ctx.data_mut(|data| data.remove_temp::<bool>(Id::unique(SHORTCUTS_REQUEST_ID))).unwrap_or(false)
}

/// A section of Settings other pages lead to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    /// Assistant, where the coding agents are set up.
    Assistant,
    /// Local checkouts.
    LocalCheckouts,
}

/// Asks Settings to open at `section`. Takes effect after the current frame.
pub fn request_settings_section(ctx: &egui::Context, section: SettingsSection) {
    ctx.data_mut(|data| data.insert_temp(Id::unique(SECTION_REQUEST_ID), section));
    ctx.request_repaint();
}

/// The pending [`request_settings_section`], at most once.
pub fn take_settings_section(ctx: &egui::Context) -> Option<SettingsSection> {
    ctx.data_mut(|data| {
        let id = Id::unique(SECTION_REQUEST_ID);
        let section = data.get_temp::<SettingsSection>(id);
        data.remove::<SettingsSection>(id);
        section
    })
}

/// Opens the title bar's search from elsewhere (a page's search button); it takes focus on the
/// next frame, also where the bar had no room to show the box until then.
pub fn open_search(ctx: &egui::Context) {
    ctx.data_mut(|data| data.insert_temp(open_request_id(), true));
}

/// Whether [`open_search`] asked since the last call; true once per request.
pub fn take_search_request(ctx: &egui::Context) -> bool {
    ctx.data_mut(|data| data.remove_temp::<bool>(open_request_id())).unwrap_or(false)
}

fn open_request_id() -> Id {
    Id::unique("title_bar_jump_open_request")
}
