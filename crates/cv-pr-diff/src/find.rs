//! The find widget of the open file's diff: the text diff ([`text`]) and the rendered Markdown
//! diff search the same way and share the widget ([`bar`]).
//!
//! The widget searches the pane that has the focus, as an editor's find does: side by side the
//! old version (the left pane) or the new one (the right pane), in one pane the whole diff. A
//! view hands the search its rows as [`Entry`]s in the order they show: an unchanged line (or
//! block) once, with its number in both versions; a changed pair side by side as one entry with
//! both sides; a removed or added line alone. One pane searches every entry once (an unchanged
//! line in its new text). A [`Hit`] keeps the side it was found on and the entry's numbers (line
//! numbers in the text diff, block indices in the rendered one), so a view finds where to paint
//! it and where to scroll without searching again.
//!
//! The search runs on a background thread ([`spawn`]) for every file size: typing pauses before
//! it starts (see [`bar`]) and a result for an older query or another version of the file is
//! dropped, so the UI thread never waits for it and scrolling never stops for it. The hits are
//! kept until the next search, indexed by line for the rows that paint.
//!
//! The same search finds the other occurrences of the text selected in the text diff
//! ([`occurrences`]).

pub mod bar;
mod matcher;
#[cfg(test)]
mod occurrence_check;
mod occurrences;
pub mod text;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::mpsc::{self, Receiver};

use cv_code::find_paint::HitSide;
use cv_syntax::pool::{Pool, Priority};
pub use review_store::AppSettings;

pub use self::matcher::{MatchOptions, find_all, is_word};

/// One thread searches, the newest search first; a search over a large file takes a few
/// milliseconds, so one is enough.
static SEARCHES: Pool = Pool::new("diff-find", 1);

/// What a diff's find takes from the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindInputs {
    /// Selected text highlights its other occurrences.
    pub highlight_occurrences: bool,
}

impl FindInputs {
    pub fn of(settings: &AppSettings) -> Self {
        Self { highlight_occurrences: settings.highlight_selection_occurrences }
    }
}

impl Default for FindInputs {
    /// The settings' defaults.
    fn default() -> Self {
        Self::of(&AppSettings::default())
    }
}

/// What the widget searches for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub options: MatchOptions,
    /// The version searched: Old for the left pane, New for the right one, Both for one pane
    /// showing the whole diff.
    pub side: HitSide,
}

/// One match: where its entry is (an unchanged one keeps both numbers) and its bytes in the text
/// searched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub side: HitSide,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub bytes: Range<usize>,
}

/// A row of a view as the search reads it: its number and text in each version it shows.
#[derive(Debug, Clone, Copy)]
pub struct Entry<'a> {
    pub old: Option<(u32, &'a str)>,
    pub new: Option<(u32, &'a str)>,
    /// The same line in both versions: searched once by one pane.
    pub is_unchanged: bool,
}

/// The hits of `query` in `entries`, in their order (old side before new side within one).
pub fn search<'a>(entries: impl IntoIterator<Item = Entry<'a>>, query: &Query) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut add = |side: HitSide, old: Option<u32>, new: Option<u32>, text: &str| {
        let found = find_all(text, &query.text, query.options);
        hits.extend(found.into_iter().map(|bytes| Hit { side, old, new, bytes }));
    };
    for entry in entries {
        if entry.is_unchanged
            && let (Some((old, old_text)), Some((new, new_text))) = (entry.old, entry.new)
        {
            let text = if query.side == HitSide::Old { old_text } else { new_text };
            add(query.side, Some(old), Some(new), text);
            continue;
        }
        if let Some((old, text)) = entry.old.filter(|_| query.side != HitSide::New) {
            add(HitSide::Old, Some(old), None, text);
        }
        if let Some((new, text)) = entry.new.filter(|_| query.side != HitSide::Old) {
            add(HitSide::New, None, Some(new), text);
        }
    }
    hits
}

/// The hits of a finished search, indexed by the numbers of their entries.
#[derive(Debug, Default)]
pub struct Hits {
    pub list: Vec<Hit>,
    by_old: HashMap<u32, Vec<usize>>,
    by_new: HashMap<u32, Vec<usize>>,
}

impl Hits {
    pub fn new(list: Vec<Hit>) -> Self {
        let (mut by_old, mut by_new) = (HashMap::new(), HashMap::new());
        for (index, hit) in list.iter().enumerate() {
            if let Some(old) = hit.old {
                by_old.entry(old).or_insert_with(Vec::new).push(index);
            }
            if let Some(new) = hit.new {
                by_new.entry(new).or_insert_with(Vec::new).push(index);
            }
        }
        Self { list, by_old, by_new }
    }

    /// The hits (index and hit) whose entry has `number` in the old version.
    pub fn on_old(&self, number: u32) -> impl Iterator<Item = (usize, &Hit)> {
        self.indexed(self.by_old.get(&number))
    }

    /// The hits whose entry has `number` in the new version.
    pub fn on_new(&self, number: u32) -> impl Iterator<Item = (usize, &Hit)> {
        self.indexed(self.by_new.get(&number))
    }

    fn indexed<'s>(
        &'s self,
        indices: Option<&'s Vec<usize>>,
    ) -> impl Iterator<Item = (usize, &'s Hit)> {
        indices
            .into_iter()
            .flatten()
            .filter_map(|index| self.list.get(*index).map(|hit| (*index, hit)))
    }
}

/// A finished search: what it looked for, in which version of the view's text, and what it found.
#[derive(Debug)]
pub struct Found {
    pub query: Query,
    /// The view's text the search ran over (see [`bar::FindBar::refresh`]).
    pub source: u64,
    pub hits: Hits,
}

impl Found {
    /// The search of `query` over the `source` text that found `hits`.
    pub fn new(query: Query, source: u64, hits: Vec<Hit>) -> Self {
        Self { query, source, hits: Hits::new(hits) }
    }
}

/// Runs `search` on the find thread; the result arrives on the receiver with one repaint.
pub fn spawn(
    ctx: &egui::Context,
    search: impl FnOnce() -> Found + Send + 'static,
) -> Receiver<Found> {
    let (sender, receiver) = mpsc::channel();
    let ctx = ctx.clone();
    SEARCHES.submit(Priority::Urgent, move || {
        // The view dropped the receiver: a newer search replaced this one.
        let _replaced = sender.send(search());
        ctx.request_repaint();
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(text: &str, side: HitSide) -> Query {
        Query { text: text.to_owned(), options: MatchOptions::default(), side }
    }

    fn found(query: &Query) -> Vec<Hit> {
        search(entries(), query)
    }

    /// An unchanged line, a replaced pair side by side, a removed line and an added one.
    fn entries() -> Vec<Entry<'static>> {
        vec![
            Entry {
                old: Some((1, "let word = 1;")),
                new: Some((1, "let word = 1;")),
                is_unchanged: true,
            },
            Entry {
                old: Some((2, "word one")),
                new: Some((2, "word two word")),
                is_unchanged: false,
            },
            Entry { old: Some((3, "only old word")), new: None, is_unchanged: false },
            Entry { old: None, new: Some((3, "only new")), is_unchanged: false },
        ]
    }

    fn sides(hits: &[Hit]) -> Vec<(HitSide, Option<u32>, Option<u32>)> {
        hits.iter().map(|hit| (hit.side, hit.old, hit.new)).collect()
    }

    #[test]
    fn one_pane_counts_an_unchanged_line_once() {
        let hits = found(&query("word", HitSide::Both));
        assert_eq!(
            sides(&hits),
            [
                (HitSide::Both, Some(1), Some(1)),
                (HitSide::Old, Some(2), None),
                (HitSide::New, None, Some(2)),
                (HitSide::New, None, Some(2)),
                (HitSide::Old, Some(3), None),
            ]
        );
    }

    #[test]
    fn each_pane_searches_its_version_only() {
        let left = found(&query("word", HitSide::Old));
        assert_eq!(
            sides(&left),
            [
                (HitSide::Old, Some(1), Some(1)),
                (HitSide::Old, Some(2), None),
                (HitSide::Old, Some(3), None)
            ]
        );
        let right = found(&query("word", HitSide::New));
        assert_eq!(right.len(), 3, "the unchanged line and the two in the new pair");
        assert!(right.iter().all(|hit| hit.side == HitSide::New));
        assert_eq!(found(&query("only new", HitSide::Old)), []);
    }

    #[test]
    fn the_index_finds_hits_by_their_numbers() {
        let hits = Hits::new(found(&query("word", HitSide::Both)));
        assert_eq!(hits.on_old(1).count(), 1, "the unchanged line under its old number");
        assert_eq!(hits.on_new(1).count(), 1, "and its new one");
        assert_eq!(hits.on_new(2).map(|(index, _)| index).collect::<Vec<_>>(), [2, 3]);
        assert_eq!(hits.on_old(9).count(), 0);
    }
}
