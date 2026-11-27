//! The other occurrences of the text selected in the text diff, highlighted as an editor does:
//! a selection within one line (a double-clicked word too) is searched in the whole file, both
//! sides, exactly as written and case-sensitive, and as a whole word when the selection is one.
//! The search runs on the find thread ([`super::spawn`]) as soon as the selection changes, and
//! a cleared selection clears the highlights at once. They show beside the find widget's hits,
//! each in its own colour (see [`super::text`]).

use std::sync::mpsc::{Receiver, TryRecvError};

use super::{Found, Hits, Query};

/// The search for the selection's other occurrences and its hits.
#[derive(Default)]
pub(crate) struct Occurrences {
    /// What is searched for, over which version of the view's text (see [`super::Found`]).
    wanted: Option<(Query, u64)>,
    running: Option<Receiver<Found>>,
    found: Option<Found>,
}

impl Occurrences {
    /// Follows `query` over the view's `source` text: a new one starts searching (`start` runs
    /// it), none clears the highlights. True when hits arrived this frame.
    pub(crate) fn follow(
        &mut self,
        query: Option<Query>,
        source: u64,
        start: impl FnOnce(Query) -> Receiver<Found>,
    ) -> bool {
        let wanted = query.map(|query| (query, source));
        if wanted != self.wanted {
            self.found = None;
            self.running = wanted.clone().map(|(query, _)| start(query));
            self.wanted = wanted;
        }
        let Some(receiver) = &self.running else {
            return false;
        };
        match receiver.try_recv() {
            Ok(found) => {
                self.running = None;
                self.found = Some(found);
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.running = None;
                false
            }
        }
    }

    pub(crate) fn hits(&self) -> Option<&Hits> {
        self.found.as_ref().map(|found| &found.hits)
    }
}
