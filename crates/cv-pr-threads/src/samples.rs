//! Threads shaped like a real review, for the tests of the cards, the panel and the diff, and a
//! long review for the frame benchmarks. Times are relative to now, so the cards show "12 min ago"
//! and the like.

use review_core::{
    DiffSide, LineRange, ProviderKind, ReviewThread, ThreadAnchor, ThreadComment, ThreadStatus,
    UserIdentity,
};
use time::OffsetDateTime;

use super::ThreadsUi;

pub(crate) const PULL_REQUEST_URL: &str = "https://github.com/acme/shop/pull/42";

pub fn person(id: &str, name: &str) -> UserIdentity {
    UserIdentity {
        id: id.into(),
        display_name: name.into(),
        login: Some(id.into()),
        avatar_url: None,
    }
}

/// The pull request's author.
pub fn jonas() -> UserIdentity {
    person("jweber", "Jonas Weber")
}

pub fn mara() -> UserIdentity {
    person("mlind", "Mara Lind")
}

/// The signed-in user.
pub fn erika() -> UserIdentity {
    person("ebrandt", "Erika Brandt")
}

pub(crate) fn minutes_ago(minutes: i64) -> OffsetDateTime {
    OffsetDateTime::now_utc() - time::Duration::minutes(minutes)
}

pub fn comment(id: &str, author: &UserIdentity, minutes: i64, body: &str) -> ThreadComment {
    comment_written(id, author, minutes_ago(minutes), body)
}

/// A comment written at `unix` seconds, for tests that set the visit to a fixed time.
pub fn comment_at(id: &str, author: &UserIdentity, unix: i64, body: &str) -> ThreadComment {
    let created_at = OffsetDateTime::from_unix_timestamp(unix).expect("a time in range");
    comment_written(id, author, created_at, body)
}

fn comment_written(
    id: &str,
    author: &UserIdentity,
    created_at: OffsetDateTime,
    body: &str,
) -> ThreadComment {
    ThreadComment {
        id: id.into(),
        author: author.clone(),
        body: body.into(),
        created_at,
        updated_at: None,
    }
}

/// Head lines `start..=end` of `path` (one line when they are equal).
pub fn lines(path: &str, start: u32, end: u32) -> ThreadAnchor {
    let range = (start != end).then_some(LineRange {
        start_old_line: None,
        start_new_line: Some(start),
        side: DiffSide::New,
        detail: None,
    });
    ThreadAnchor { path: path.into(), old_path: None, old_line: None, new_line: Some(end), range }
}

pub fn thread(
    id: &str,
    status: ThreadStatus,
    anchor: Option<ThreadAnchor>,
    comments: Vec<ThreadComment>,
) -> ReviewThread {
    ReviewThread {
        id: id.into(),
        status,
        anchor,
        is_outdated: false,
        origin: None,
        is_system: false,
        can_reply: true,
        can_resolve: true,
        comments,
    }
}

/// The conversation of the mockups: a question, an answer with a suggested change, a reply.
pub fn discussion(id: &str, anchor: ThreadAnchor) -> ReviewThread {
    thread(
        id,
        ThreadStatus::Active,
        Some(anchor),
        vec![
            comment(
                &format!("{id}-1"),
                &mara(),
                180,
                "This loop runs `item.quantity as u64` for every item. Could `total` be **one expression** instead?",
            ),
            comment(
                &format!("{id}-2"),
                &jonas(),
                95,
                "Right, something like this?\n```suggestion\nself.items.iter().map(|item| item.price_cents * u64::from(item.quantity)).sum()\n```",
            ),
            comment(
                &format!("{id}-3"),
                &mara(),
                40,
                "Yes, and `u64::from` keeps clippy quiet. Ship it.",
            ),
        ],
    )
}

/// Files the threads of [`review`] are spread over, and the lines of each.
pub(crate) const REVIEW_FILES: usize = 20;
const REVIEW_FILE_LINES: u32 = 400;

/// The path of the changed file `index` of [`review`].
pub(crate) fn review_path(index: usize) -> String {
    format!("src/module_{}/file_{index}.rs", index / 5)
}

/// A changed file of [`review`]: a diff that replaced line 20 by two lines, and the head text.
struct ReviewFile {
    path: String,
    diff: String,
    text: String,
}

fn review_file(index: usize) -> ReviewFile {
    let path = review_path(index);
    let diff =
        "@@ -20 +20,2 @@\n-    let total = 0;\n+    let mut total = 0;\n+    total += offset;\n"
            .to_owned();
    let text = (1..=REVIEW_FILE_LINES)
        .map(|line| match line {
            20 => "    let mut total = 0;".to_owned(),
            21 => "    total += offset;".to_owned(),
            _ if line % 12 == 0 => format!("pub fn step_{line}(items: &[Item], offset: u64) -> u64 {{"),
            _ if line % 12 == 11 => "}".to_owned(),
            _ => format!(
                "    let value_{line} = items.iter().map(|item| item.price_cents * {line}).sum::<u64>();"
            ),
        })
        .map(|line| line + "\n")
        .collect();
    ReviewFile { path, diff, text }
}

/// Comment `index` of thread `thread`: one-liners, prose with inline marks and links, fenced
/// code, lists and suggested changes, in turn.
fn review_body(thread: usize, index: usize) -> String {
    match (thread + index) % 6 {
        0 => "Done, thanks.".to_owned(),
        1 => format!(
            "Does `step_{thread}` need to **allocate** here? The caller already holds the slice, so a borrow would do; see [the guide](https://example.test/guide/{index})."
        ),
        2 => format!(
            "This reads every item twice:\n```rust\nlet sum: u64 = items.iter().map(|item| item.price_cents).sum();\nlet count = items.len() as u64;\nif count == 0 {{\n    return 0;\n}}\nsum / count + {thread}\n```\nOne pass would keep it cheap."
        ),
        3 => "A few things before this merges:\n- the name of `total` hides the outer one\n- `offset` is never negative, use `u64`\n- [ ] a test for the empty cart\n- [x] the changelog entry".to_owned(),
        4 => format!(
            "Something like this?\n```suggestion\n    let total = items.iter().map(|item| item.price_cents * {thread}).sum::<u64>();\n```"
        ),
        _ => format!(
            "I went through the whole module once more. The loop around line {thread} reads the price of every item, multiplies it by the quantity and adds the offset on every step, which is fine for the carts we see today but grows with the number of discounts as well. *Moving the offset out of the loop* keeps the result the same and makes the intent clearer to the next reader; the benchmark in `benches/cart.rs` should show it, too."
        ),
    }
}

/// A long review for the frame benchmarks: `count` threads over [`REVIEW_FILES`] files, one to
/// eight comments each by four people, every fourth resolved, every eighth a general comment,
/// every tenth outdated, every fifth with news since the visit 30 minutes ago.
#[expect(clippy::cast_possible_wrap, reason = "comment ages count back by index")]
pub fn review(count: usize) -> Vec<ReviewThread> {
    let people = [mara(), jonas(), erika(), person("kpatel", "Kiran Patel")];
    (0..count)
        .map(|index| {
            let id = format!("r-{index}");
            let comments = 1 + index * 7 % 8;
            let is_news = index % 5 == 0;
            let comments = (0..comments)
                .map(|position| {
                    let age = if is_news && position + 1 == comments { 5 } else { 60 };
                    let minutes = age + (comments - position) as i64 * 40;
                    let author = &people[(index + position) % people.len()];
                    comment(
                        &format!("{id}-{position}"),
                        author,
                        minutes,
                        &review_body(index, position),
                    )
                })
                .collect();
            let anchor = (index % 8 != 7).then(|| {
                let line = 1 + (index as u32 * 37) % (REVIEW_FILE_LINES - 1);
                lines(&review_path(index % REVIEW_FILES), line, line + (index as u32 % 3))
            });
            let status = if index % 4 == 3 { ThreadStatus::Resolved } else { ThreadStatus::Active };
            let mut thread = thread(&id, status, anchor, comments);
            thread.is_outdated = index % 10 == 4;
            thread
        })
        .collect()
}

/// The threads' state as the pull request page sets it: Erika signed in, Jonas the author, the
/// page's link, and the visit 30 minutes ago, so comments of the last half hour are new.
pub fn page_ui() -> ThreadsUi {
    let mut ui = ThreadsUi::for_tests(ProviderKind::GitHub);
    ui.context.author_id = Some(jonas().id);
    ui.context.web_url = Some(PULL_REQUEST_URL.into());
    ui.set_me(erika());
    ui.visited_at(minutes_ago(30).unix_timestamp());
    ui
}

/// [`page_ui`] for [`review`], with the code of every file loaded for the excerpts.
pub fn review_ui(threads: &[ReviewThread]) -> ThreadsUi {
    let mut ui = page_ui();
    for index in 0..REVIEW_FILES {
        let file = review_file(index);
        let first = threads
            .iter()
            .find(|thread| thread.anchor.as_ref().is_some_and(|anchor| anchor.path == file.path));
        if let Some(thread) = first {
            ui.insert_code_for_tests(thread, &file.diff, &file.text);
        }
    }
    ui
}

/// A resolved thread with two replies.
pub fn resolved(id: &str, anchor: Option<ThreadAnchor>) -> ReviewThread {
    thread(
        id,
        ThreadStatus::Resolved,
        anchor,
        vec![
            comment(&format!("{id}-1"), &erika(), 300, "Rename to `is_empty` to match `Vec`?"),
            comment(&format!("{id}-2"), &jonas(), 280, "Done."),
            comment(&format!("{id}-3"), &erika(), 270, "Thanks!"),
        ],
    )
}
