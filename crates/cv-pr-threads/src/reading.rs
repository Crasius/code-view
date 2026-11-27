//! Read marks from reading: an open card in view for [`READ_DWELL_SECONDS`] counts as read up to
//! the newest comment it showed once it leaves the view (a collapsed one shows no replies). Cards
//! report themselves each frame they are in view ([`Reading::note`]); [`Reading::track`] runs once
//! per frame before any card. What is still news after the read marks: [`news`].

use std::collections::HashMap;

use review_core::{ReviewThread, ThreadComment};
use review_store::ThreadStore;

/// Which comments of `thread` are news to the user `me`: other people's, newer than what the user
/// has seen. `None` when none can be (a platform notice, or no visit yet).
pub fn news<'a>(
    store: &ThreadStore,
    me: Option<&'a str>,
    thread: &ReviewThread,
) -> Option<impl Fn(&ThreadComment) -> bool + 'a> {
    if thread.is_system {
        return None;
    }
    let seen = store.seen_until(&thread.id)?;
    Some(move |comment: &ThreadComment| {
        comment.created_at.unix_timestamp() > seen && Some(comment.author.id.as_str()) != me
    })
}

/// How long a card has to stay on screen before its comments count as read when it leaves.
const READ_DWELL_SECONDS: f64 = 1.0;

/// A card that was on screen, since when, and the newest comment it showed.
#[derive(Clone, Copy)]
struct OnScreen {
    since: f64,
    newest: i64,
}

#[derive(Default)]
pub struct Reading {
    on_screen: HashMap<String, OnScreen>,
    /// The cards in view this frame, with the newest comment each showed.
    painted: HashMap<String, i64>,
}

impl Reading {
    /// The card of `thread` is in view this frame.
    pub fn note(&mut self, thread: &ReviewThread) {
        if let Some(newest) = super::newest(thread) {
            self.painted.insert(thread.id.clone(), newest);
        }
    }

    /// Cards that left the view after the user had time to read them are read up to what they
    /// showed.
    pub fn track(&mut self, now: f64, store: &mut ThreadStore) {
        let painted = std::mem::take(&mut self.painted);
        let left: Vec<(String, OnScreen)> = self
            .on_screen
            .iter()
            .filter(|(id, _)| !painted.contains_key(*id))
            .map(|(id, seen)| (id.clone(), *seen))
            .collect();
        for (id, seen) in left {
            self.on_screen.remove(&id);
            if now - seen.since >= READ_DWELL_SECONDS {
                store.mark_seen(&id, seen.newest);
            }
        }
        for (id, newest) in painted {
            self.on_screen.entry(id).or_insert(OnScreen { since: now, newest }).newest = newest;
        }
    }
}
