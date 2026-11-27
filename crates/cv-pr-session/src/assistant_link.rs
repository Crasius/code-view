//! The pull request assistant's mailboxes: the frame-scoped data through which the diff, the
//! Files tab and the page reach the assistant without depending on it.
//!
//! The page publishes whether the assistant is on ([`publish_enabled`]); the diff reads it
//! ([`is_enabled`]) and asks for the assistant with the selected lines ([`request`]); the page
//! takes that ask once ([`take_request`]). The Files tab tells the mini chat where its content
//! is ([`publish_room`]) and which cited lines it painted ([`note_cited`]). They live in
//! `cv-pr-session`, below both the diff and the assistant, so neither crate names the other.

use cv_base::mailbox;
use cv_base::platform::shortcut;
use egui::{Context, Id, Rect};
use review_store::ChatQuote;

/// Opens or focuses the assistant.
pub const OPEN_SHORTCUT: &str = shortcut!(alt "A");

const REQUEST_ID: &str = "review.assistant.request";
const ENABLED_ID: &str = "review.assistant.enabled";
pub const ROOM_ID: &str = "review.assistant.mini.room";
pub const CITED_ID: &str = "review.assistant.mini.cited";

/// Tells the views drawn this frame whether the assistant is on (Settings → Assistant).
pub fn publish_enabled(ctx: &Context, is_enabled: bool) {
    ctx.data_mut(|data| data.insert_temp(Id::unique(ENABLED_ID), is_enabled));
}

/// Whether to offer the assistant ("Ask about these lines", Alt+A on selected lines); on until
/// the page says otherwise.
pub fn is_enabled(ctx: &Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(Id::unique(ENABLED_ID))).unwrap_or(true)
}

/// Opens the assistant from elsewhere on the page this frame: with the lines of the diff quoted
/// (Alt+A or the diff's context menu on a selection), or empty (Alt+A without one).
#[derive(Debug, Clone)]
pub struct Request {
    pub quote: Option<ChatQuote>,
}

/// Asks for the assistant; ignored while it is turned off.
pub fn request(ctx: &Context, quote: Option<ChatQuote>) {
    if !is_enabled(ctx) {
        return;
    }
    mailbox::post(ctx, Id::unique(REQUEST_ID), Request { quote });
}

/// The request of this frame, once.
pub fn take_request(ctx: &Context) -> Option<Request> {
    mailbox::take(ctx, Id::unique(REQUEST_ID))
}

/// Tells the mini chat where the Files tab's content is this frame: it floats inside.
pub fn publish_room(ctx: &Context, rect: Rect) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| data.insert_temp(Id::unique(ROOM_ID), (pass, rect)));
}

/// Adds `rect`, cited lines painted this frame, to what the mini chat should not cover.
pub fn note_cited(ctx: &Context, rect: Rect) {
    let pass = ctx.cumulative_pass_nr();
    let union = this_pass(ctx, CITED_ID).map_or(rect, |known| known.union(rect));
    ctx.data_mut(|data| data.insert_temp(Id::unique(CITED_ID), (pass, union)));
}

/// The rect published under `key` in this pass, if any.
pub fn this_pass(ctx: &Context, key: &str) -> Option<Rect> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|data| data.get_temp::<(u64, Rect)>(Id::unique(key)))
        .filter(|(noted, _)| *noted == pass)
        .map(|(_, rect)| rect)
}

/// The Files tab's content as the last pass published it, for tests.
#[cfg(any(test, feature = "test-support"))]
pub fn room(ctx: &Context) -> Option<Rect> {
    ctx.data(|data| data.get_temp::<(u64, Rect)>(Id::unique(ROOM_ID))).map(|(_, rect)| rect)
}
