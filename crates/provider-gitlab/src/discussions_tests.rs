use super::*;
use review_core::{RangeDetail, UserIdentity};

const HEAD: &str = "4803c71e";

fn refs() -> DiffRefsDto {
    DiffRefsDto {
        base_sha: Some("b5d6".into()),
        start_sha: Some("7c9c".into()),
        head_sha: Some(HEAD.into()),
    }
}

fn anchor(old_line: Option<u32>, new_line: Option<u32>) -> ThreadAnchor {
    ThreadAnchor { path: "src/lib.rs".into(), old_path: None, old_line, new_line, range: None }
}

/// SHA-1 of "src/lib.rs" (`printf src/lib.rs | sha1sum`), which GitLab's line codes start with.
const PATH_HASH: &str = "b24749917179fb5e3e613ed2a703fcdcc6cdf9da";

fn range_anchor(
    start: (Option<u32>, Option<u32>),
    end: (Option<u32>, Option<u32>),
    detail: Option<RangeDetail>,
) -> ThreadAnchor {
    ThreadAnchor {
        range: Some(LineRange {
            start_old_line: start.0,
            start_new_line: start.1,
            side: DiffSide::New,
            detail,
        }),
        ..anchor(end.0, end.1)
    }
}

fn detail(start: (u32, u32), end: (u32, u32)) -> RangeDetail {
    RangeDetail {
        start: FilePositions { old: start.0, new: start.1 },
        end: FilePositions { old: end.0, new: end.1 },
        start_offset: 1,
        end_offset: 1,
    }
}

#[test]
fn positions_a_line_range_with_line_codes() {
    // Context line 9/10 down to the added line 14, which sits before base line 13.
    let anchor =
        range_anchor((Some(9), Some(10)), (None, Some(14)), Some(detail((9, 10), (13, 14))));
    let position = text_position(&anchor, &refs()).expect("position");
    assert_eq!(
        position["line_range"],
        json!({
            "start": { "line_code": format!("{PATH_HASH}_9_10"), "type": "old", "old_line": 9, "new_line": 10 },
            "end": { "line_code": format!("{PATH_HASH}_13_14"), "type": "new", "new_line": 14 },
        })
    );
    assert_eq!(position["new_line"], json!(14));
    assert!(position.get("old_line").is_none());
}

#[test]
fn refuses_a_range_without_diff_positions() {
    let anchor = range_anchor((None, Some(3)), (None, Some(5)), None);
    assert!(matches!(text_position(&anchor, &refs()), Err(ProviderError::Local(_))));
}

#[test]
fn maps_line_range_of_a_multi_line_note() {
    let discussion: DiscussionDto = serde_json::from_str(
        r#"{"id":"eee","notes":[{"id":5,"body":"range","author":{"id":1,"username":"ann"},
            "created_at":"2026-01-01T12:00:00Z","resolvable":true,"resolved":false,
            "position":{"head_sha":"4803c71e","old_path":"d.rs","new_path":"d.rs","position_type":"text",
                        "old_line":null,"new_line":11,
                        "line_range":{"start":{"line_code":"x_10_10","type":"new","old_line":null,"new_line":10},
                                      "end":{"line_code":"x_11_11","type":"new","old_line":null,"new_line":11}}}}]}"#,
    )
    .expect("fixture");
    let anchor =
        to_thread(discussion, Some(HEAD)).and_then(|thread| thread.anchor).expect("anchor");
    assert_eq!(anchor.new_line, Some(11));
    assert_eq!(
        anchor.range,
        Some(LineRange {
            start_old_line: None,
            start_new_line: Some(10),
            side: DiffSide::New,
            detail: None
        })
    );
}

#[test]
fn positions_added_line_with_new_line_only() {
    let position = text_position(&anchor(None, Some(12)), &refs()).expect("position");
    assert_eq!(
        position,
        json!({
            "position_type": "text", "base_sha": "b5d6", "start_sha": "7c9c", "head_sha": HEAD,
            "new_path": "src/lib.rs", "old_path": "src/lib.rs", "new_line": 12
        })
    );
}

#[test]
fn positions_deleted_line_with_old_line_only() {
    let mut renamed = anchor(Some(7), None);
    renamed.old_path = Some("src/old.rs".into());
    let position = text_position(&renamed, &refs()).expect("position");
    assert_eq!(position["old_line"], json!(7));
    assert!(position.get("new_line").is_none());
    assert_eq!(position["old_path"], json!("src/old.rs"));
    assert_eq!(position["new_path"], json!("src/lib.rs"));
}

#[test]
fn positions_context_line_with_both_lines() {
    let position = text_position(&anchor(Some(20), Some(23)), &refs()).expect("position");
    assert_eq!(position["old_line"], json!(20));
    assert_eq!(position["new_line"], json!(23));
}

#[test]
fn rejects_an_anchor_without_lines() {
    assert!(matches!(text_position(&anchor(None, None), &refs()), Err(ProviderError::Local(_))));
}

#[test]
fn rejects_incomplete_refs() {
    let incomplete = DiffRefsDto { base_sha: None, ..refs() };
    assert!(matches!(
        text_position(&anchor(None, Some(1)), &incomplete),
        Err(ProviderError::Decode(_))
    ));
}

const DISCUSSIONS: &str = r#"[
  {"id":"aaa","individual_note":false,"notes":[
    {"id":1,"type":"DiffNote","body":"inline","author":{"id":1,"username":"ann","name":"Ann"},
     "created_at":"2026-01-01T10:00:00.000Z","updated_at":"2026-01-01T10:00:00.000Z","system":false,
     "resolvable":true,"resolved":true,
     "position":{"base_sha":"b5d6","start_sha":"7c9c","head_sha":"4803c71e","old_path":"a.rs","new_path":"b.rs",
                 "position_type":"text","old_line":null,"new_line":5}},
    {"id":2,"type":"DiffNote","body":"reply","author":{"id":2,"username":"bob"},
     "created_at":"2026-01-01T11:00:00.000Z","updated_at":"2026-01-02T11:00:00+01:00","system":false,
     "resolvable":true,"resolved":true}]},
  {"id":"bbb","individual_note":true,"notes":[
    {"id":3,"type":null,"body":"added 1 commit","author":{"id":1,"username":"ann"},
     "created_at":"2026-01-01T12:00:00Z","system":true,"resolvable":false,"resolved":false}]},
  {"id":"ccc","individual_note":false,"notes":[
    {"id":4,"type":"DiffNote","body":"old","author":{"id":1,"username":"ann"},
     "created_at":"2026-01-01T12:00:00Z","system":false,"resolvable":true,"resolved":false,
     "position":{"head_sha":"0ld","old_path":"c.rs","new_path":"c.rs","position_type":"text","old_line":3,"new_line":3}}]},
  {"id":"ddd","individual_note":false,"notes":[]}
]"#;

fn parse_threads() -> Vec<ReviewThread> {
    let discussions: Vec<DiscussionDto> = serde_json::from_str(DISCUSSIONS).expect("fixture");
    discussions.into_iter().filter_map(|discussion| to_thread(discussion, Some(HEAD))).collect()
}

#[test]
fn maps_resolved_inline_thread_with_rename() {
    let threads = parse_threads();
    assert_eq!(threads.len(), 3, "discussions without notes are dropped");
    let inline = &threads[0];
    assert_eq!(inline.status, ThreadStatus::Resolved);
    assert!(!inline.is_outdated);
    assert!(inline.can_resolve && inline.can_reply && !inline.is_system);
    assert_eq!(
        inline.anchor,
        Some(ThreadAnchor {
            path: "b.rs".into(),
            old_path: Some("a.rs".into()),
            old_line: None,
            new_line: Some(5),
            range: None
        })
    );
    assert_eq!(inline.comments.len(), 2);
    assert_eq!(inline.comments[0].updated_at, None, "unchanged update time is dropped");
    assert_eq!(
        inline.comments[1].updated_at,
        Some(time::macros::datetime!(2026-01-02 10:00 UTC)),
        "the edit time, offset applied"
    );
}

#[test]
fn maps_system_note_as_not_replyable() {
    let system = &parse_threads()[1];
    assert!(system.is_system && !system.can_reply && !system.can_resolve);
    assert_eq!(system.anchor, None);
    assert_eq!(system.status, ThreadStatus::Active);
}

#[test]
fn flags_positions_on_older_head_as_outdated() {
    let outdated = &parse_threads()[2];
    assert!(outdated.is_outdated);
    assert_eq!(outdated.status, ThreadStatus::Active);
    assert_eq!(outdated.anchor.as_ref().and_then(|anchor| anchor.old_line), Some(3));
}

/// A diff note in the shape of the discussions API documentation's example, left on an older head.
#[test]
fn outdated_notes_keep_the_diff_they_were_written_on() {
    let discussion: DiscussionDto = serde_json::from_str(
        r#"{"id":"6a9c1750b37d513a43987b574953fceb50b03ce7","individual_note":false,"notes":[{"id":1128,"type":"DiffNote",
            "body":"diff comment","author":{"id":1,"username":"root"},"created_at":"2018-03-04T09:17:22.520Z",
            "updated_at":"2018-03-04T09:17:22.520Z","system":false,"resolvable":true,"resolved":false,
            "position":{"base_sha":"b5d6e7b1613fca24d250fa8e5bc7bcc3dd6002ef","start_sha":"7c9c2ead8a320fb7ba0b4e234bd9529a2614e306",
              "head_sha":"4803c71e6b1833ca72b8b26ef2ecd5adc8a38031","old_path":"package.json","new_path":"package.json",
              "position_type":"text","old_line":27,"new_line":27}}]}"#,
    )
    .expect("discussion");
    let thread = to_thread(discussion.clone(), Some("ffffffff")).expect("thread");
    assert!(thread.is_outdated);
    let origin = thread.origin.expect("origin");
    assert_eq!(origin.base_sha.as_deref(), Some("b5d6e7b1613fca24d250fa8e5bc7bcc3dd6002ef"));
    assert_eq!(origin.head_sha, "4803c71e6b1833ca72b8b26ef2ecd5adc8a38031");
    assert_eq!(
        Some(&origin.anchor),
        thread.anchor.as_ref(),
        "the position stays on the diff the line was last in"
    );
    assert_eq!((origin.anchor.old_line, origin.anchor.new_line), (Some(27), Some(27)));

    let current =
        to_thread(discussion, Some("4803c71e6b1833ca72b8b26ef2ecd5adc8a38031")).expect("thread");
    assert!(
        !current.is_outdated && current.origin.is_none(),
        "a note on the current head is where it was written"
    );
}

fn discussion(note_ids: &[&str]) -> ReviewThread {
    let note = |note_id: &&str| ThreadComment {
        id: (*note_id).to_owned(),
        author: UserIdentity::default(),
        body: String::new(),
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: None,
    };
    ReviewThread {
        id: "6a9c1750b37d513a43987b574953fceb50b03ce7".to_owned(),
        status: ThreadStatus::Active,
        anchor: None,
        is_outdated: false,
        origin: None,
        is_system: false,
        can_reply: true,
        can_resolve: true,
        comments: note_ids.iter().map(note).collect(),
    }
}

#[test]
fn links_the_first_note() {
    let cases = [(&["301", "302"][..], Some("#note_301")), (&[][..], None)];
    for (note_ids, expected) in cases {
        assert_eq!(page_suffix(&discussion(note_ids)).as_deref(), expected, "{note_ids:?}");
    }
}

#[test]
fn system_notes_offer_no_edit_or_delete() {
    let mut thread = discussion(&[]);
    assert_eq!(comment_actions(&thread), CommentActions::EDIT_AND_DELETE);
    thread.is_system = true;
    assert_eq!(comment_actions(&thread), CommentActions::NONE);
}
