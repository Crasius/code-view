//! Which threads the panel lists under which filters, the order of their groups, and the counts of
//! its toggles.

use review_core::{ReviewThread, ThreadStatus};
use review_store::ThreadFilters;

use super::{
    PanelInputs, ToggleCounts, is_listed, read_file_ranks, toggle_counts, visible_entries,
};
use crate::ThreadsUi;
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

/// The groups the panel lists for `threads` with the files in `order`, given as of `version`.
fn groups(
    ui: &mut ThreadsUi,
    threads: &[ReviewThread],
    order: [&str; 2],
    version: u64,
) -> Vec<String> {
    let mut filters = NONE;
    let mut order = order.iter().copied();
    let mut inputs = PanelInputs {
        threads,
        file_order: &mut order,
        file_order_version: version,
        filters: &mut filters,
        width: 0.0,
    };
    read_file_ranks(&mut ui.panel, &mut inputs);
    let visible = visible_entries(ui, threads, NONE, &[]);
    visible.into_iter().map(|entry| entry.group.to_owned()).collect()
}

#[test]
fn the_groups_follow_the_tree_order_read_only_when_it_changed() {
    let mut ui = samples::page_ui();
    let on = |path: &str| {
        let comments = vec![comment(&format!("{path}-1"), &mara(), 900, "Why?")];
        thread(path, ThreadStatus::Active, Some(lines(path, 1, 1)), comments)
    };
    let general =
        thread("general", ThreadStatus::Active, None, vec![comment("g-1", &mara(), 900, "Hi")]);
    let threads = vec![on("b.rs"), general, on("a.rs")];
    assert_eq!(groups(&mut ui, &threads, ["a.rs", "b.rs"], 1), ["a.rs", "b.rs", "General"]);
    assert_eq!(
        groups(&mut ui, &threads, ["b.rs", "a.rs"], 1),
        ["a.rs", "b.rs", "General"],
        "the same version: the order is not read again"
    );
    assert_eq!(groups(&mut ui, &threads, ["b.rs", "a.rs"], 2), ["b.rs", "a.rs", "General"]);
}

#[test]
fn the_toggles_count_only_listed_threads() {
    let ui = samples::page_ui();
    let threads: Vec<ReviewThread> = variants().into_iter().map(|(_, thread)| thread).collect();
    let counts = toggle_counts(&threads, &ui);
    assert_eq!(counts, ToggleCounts { active: 4, resolved: 1, unread: 1, mine: Some(2) });
}
