//! The editing mode a composer opens in, per platform: `AppSettings::comment_editors`. The app
//! publishes the saved choice into egui's temp data before the page draws; a composer reads it
//! when it opens and after a post or cancel, and its mode switch writes a new choice back and
//! marks it. After the page the app takes a marked choice into the settings and saves it, so a
//! widget never touches the settings file. Preview is never a choice: it is a look, not an editor.

use egui::{Context, Id};
use review_core::ProviderKind;
use review_store::{CommentEditor, CommentEditors};

use super::toolbar::Mode;

const STORE_ID: &str = "comment_editors";

#[derive(Clone, Copy, Default)]
struct Shared {
    editors: CommentEditors,
    /// A composer's switch chose an editor since the app last took the choice.
    is_chosen: bool,
}

fn id() -> Id {
    Id::unique(STORE_ID)
}

/// Hands the saved choice to this frame's composers; call before they draw.
pub fn publish(ctx: &Context, editors: CommentEditors) {
    ctx.data_mut(|data| data.insert_temp(id(), Shared { editors, is_chosen: false }));
}

/// The choice a composer's switch made since the last call, to be saved; call after they drew.
pub fn take_chosen(ctx: &Context) -> Option<CommentEditors> {
    ctx.data_mut(|data| {
        let shared = data.get_temp_mut_or_default::<Shared>(id());
        let chosen = shared.is_chosen.then_some(shared.editors);
        shared.is_chosen = false;
        chosen
    })
}

/// The editing mode a composer on `platform` opens in. Without a known platform it is Markdown:
/// the rich editor needs the platform's HTML tags and mention syntax.
pub(super) fn mode(ctx: &Context, platform: Option<ProviderKind>) -> Mode {
    let Some(platform) = platform else {
        return Mode::Markdown;
    };
    let editors = ctx.data(|data| data.get_temp::<Shared>(id())).unwrap_or_default().editors;
    match editors.get(platform) {
        CommentEditor::Rich => Mode::Rich,
        CommentEditor::Markdown => Mode::Markdown,
    }
}

/// Remembers `mode` as the editor for `platform`; Preview changes nothing.
pub(super) fn choose(ctx: &Context, platform: ProviderKind, mode: Mode) {
    let editor = match mode {
        Mode::Rich => CommentEditor::Rich,
        Mode::Markdown => CommentEditor::Markdown,
        Mode::Preview => return,
    };
    ctx.data_mut(|data| {
        let shared = data.get_temp_mut_or_default::<Shared>(id());
        shared.editors.set(platform, editor);
        shared.is_chosen = true;
    });
}
