//! Draft keys and the storable shape of a new thread's lines.

use review_core::{DiffSide, FilePositions, LineRange, RangeDetail, ThreadAnchor};
use review_store::{DraftAnchor, DraftRange};

/// The general (pull request level) comment, shared by the overview and the threads panel.
pub const GENERAL_KEY: &str = "general";

/// What the draft keys of replies start with.
pub const REPLY_PREFIX: &str = "reply:";

pub fn reply_key(thread_id: &str) -> String {
    format!("{REPLY_PREFIX}{thread_id}")
}

/// One key per file and lines, so reopening a comment on the same lines finds its draft.
pub fn new_thread_key(anchor: &ThreadAnchor) -> String {
    let number = |line: Option<u32>| line.map_or_else(|| "-".to_owned(), |line| line.to_string());
    let range = anchor.range.as_ref().map_or_else(String::new, |range| {
        let side = if range.side == DiffSide::New { "n" } else { "o" };
        format!(":{side}{}/{}", number(range.start_old_line), number(range.start_new_line))
    });
    format!("new:{}:{}/{}{range}", anchor.path, number(anchor.old_line), number(anchor.new_line))
}

pub fn to_stored(anchor: &ThreadAnchor) -> DraftAnchor {
    DraftAnchor {
        path: anchor.path.clone(),
        old_path: anchor.old_path.clone(),
        old_line: anchor.old_line,
        new_line: anchor.new_line,
        range: anchor.range.as_ref().map(|range| DraftRange {
            start_old_line: range.start_old_line,
            start_new_line: range.start_new_line,
            is_new_side: range.side == DiffSide::New,
            detail: range.detail.map(|detail| {
                [
                    detail.start.old,
                    detail.start.new,
                    detail.end.old,
                    detail.end.new,
                    detail.start_offset,
                    detail.end_offset,
                ]
            }),
        }),
    }
}

pub fn from_stored(anchor: &DraftAnchor) -> ThreadAnchor {
    ThreadAnchor {
        path: anchor.path.clone(),
        old_path: anchor.old_path.clone(),
        old_line: anchor.old_line,
        new_line: anchor.new_line,
        range: anchor.range.as_ref().map(|range| LineRange {
            start_old_line: range.start_old_line,
            start_new_line: range.start_new_line,
            side: if range.is_new_side { DiffSide::New } else { DiffSide::Old },
            detail: range.detail.map(
                |[start_old, start_new, end_old, end_new, start_offset, end_offset]| RangeDetail {
                    start: FilePositions { old: start_old, new: start_new },
                    end: FilePositions { old: end_old, new: end_new },
                    start_offset,
                    end_offset,
                },
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_anchor_survives_the_round_trip_and_keys_by_its_lines() {
        let anchor = ThreadAnchor {
            path: "src/a.rs".into(),
            old_path: Some("src/old.rs".into()),
            old_line: None,
            new_line: Some(12),
            range: Some(LineRange {
                start_old_line: Some(7),
                start_new_line: Some(9),
                side: DiffSide::New,
                detail: Some(RangeDetail {
                    start: FilePositions { old: 7, new: 9 },
                    end: FilePositions { old: 9, new: 12 },
                    start_offset: 1,
                    end_offset: 30,
                }),
            }),
        };
        assert_eq!(from_stored(&to_stored(&anchor)), anchor);
        assert_eq!(new_thread_key(&anchor), "new:src/a.rs:-/12:n7/9");
        let single = ThreadAnchor { range: None, ..anchor };
        assert_eq!(new_thread_key(&single), "new:src/a.rs:-/12");
    }
}
