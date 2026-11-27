//! Which threads the panel lists under which filters, and the counts of its toggles.

use review_core::{ReviewThread, ThreadStatus};
use review_store::ThreadFilters;

use super::{ToggleCounts, is_listed, toggle_counts};
use crate::samples::{self, comment, erika, lines, mara, thread};

const NONE: ThreadFilters = ThreadFilters::ALL;
const ACTIVE: ThreadFilters = ThreadFilters { active: true, ..ThreadFilters::ALL };
const RESOLVED: ThreadFilters = ThreadFilters { resolved: true, ..ThreadFilters::ALL };
const UNREAD: ThreadFilters = ThreadFilters { unread: true, ..ThreadFilters::ALL };
const MINE: ThreadFilters = ThreadFilters { mine: true, ..ThreadFilters::ALL };

/// An active thread of one comment by Mara, read since `page_ui`'s visit.
fn old() -> ReviewThread {
    let anchor = Some(lines("src/lib.rs", 12, 12));
    thread("old", ThreadStatus::Active, anchor, vec![comment("old-1", &mara(), 900, "Why?")])
}

fn variants() -> Vec<(&'static str, ReviewThread)> {
    let resolved = samples::resolved("resolved", None);
    let system = ReviewThread { is_system: true, ..old() };
    let empty = ReviewThread { comments: Vec::new(), ..old() };
    let outdated = ReviewThread { is_outdated: true, ..old() };
    let mut unread = old();
    unread.comments.push(comment("old-2", &mara(), 5, "Any news?"));
    let mine = ReviewThread { comments: vec![comment("old-1", &erika(), 900, "Why?")], ..old() };
    vec![
        ("old", old()),
        ("resolved", resolved),
        ("system", system),
        ("empty", empty),
        ("outdated", outdated),
        ("unread", unread),
        ("mine", mine),
    ]
}

#[test]
fn the_filters_decide_which_threads_are_listed() {
    let ui = samples::page_ui();
    let cases = [
        ("old", NONE, true),
        ("old", ACTIVE, true),
        ("old", RESOLVED, false),
        ("old", UNREAD, false),
        ("old", MINE, false),
        ("resolved", NONE, true),
        ("resolved", ACTIVE, false),
        ("resolved", RESOLVED, true),
        ("system", NONE, false),
        ("system", ACTIVE, false),
        ("empty", NONE, false),
        ("outdated", ACTIVE, true),
        ("outdated", RESOLVED, false),
        ("unread", UNREAD, true),
        ("mine", MINE, true),
    ];
    let variants = variants();
    for (name, filters, expected) in cases {
        let (_, thread) = variants.iter().find(|(variant, _)| *variant == name).unwrap();
        assert_eq!(is_listed(thread, filters, &ui), expected, "{name} under {filters:?}");
    }
}

#[test]
fn the_toggles_count_only_listed_threads() {
    let ui = samples::page_ui();
    let threads: Vec<ReviewThread> = variants().into_iter().map(|(_, thread)| thread).collect();
    let counts = toggle_counts(&threads, &ui);
    assert_eq!(counts, ToggleCounts { active: 4, resolved: 1, unread: 1, mine: Some(2) });
}
