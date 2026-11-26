use super::{Timestamp, UserIdentity};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThreadStatus {
    Active,
    Resolved,
}

/// Where an inline thread sits. Exactly the line numbers the diff shows:
/// added line → only `new_line`; deleted line → only `old_line`; unchanged context line → both.
/// A multi-line thread also has a `range`; the line numbers here are then its last line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadAnchor {
    /// Path in the head commit.
    pub path: String,
    /// Path in the base commit when the file was renamed.
    pub old_path: Option<String>,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// `None` for a single line.
    pub range: Option<LineRange>,
}

impl ThreadAnchor {
    /// The anchor's (last) line counted in `side`; a context line counts in either file.
    pub fn line_in(&self, side: DiffSide) -> Option<u32> {
        match side {
            DiffSide::Old => self.old_line,
            DiffSide::New => self.new_line,
        }
    }
}

/// Which file of the diff a line counts in: the base (`Old`, the left pane) or the head (`New`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiffSide {
    Old,
    New,
}

/// The first line of a multi-line anchor, numbered like the anchor's own line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    pub start_old_line: Option<u32>,
    pub start_new_line: Option<u32>,
    /// The file the range was picked in. Context lines, which exist in both files, count there.
    pub side: DiffSide,
    /// Set for new threads, `None` for ranges read back from a provider.
    pub detail: Option<RangeDetail>,
}

impl LineRange {
    pub fn start_line_in(&self, side: DiffSide) -> Option<u32> {
        match side {
            DiffSide::Old => self.start_old_line,
            DiffSide::New => self.start_new_line,
        }
    }
}

/// What only the diff knows about a new range. Providers that need it read it from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeDetail {
    /// Where the first and the last line sit in both files.
    pub start: FilePositions,
    pub end: FilePositions,
    /// 1-based character offset of the first selected character in the first line.
    pub start_offset: u32,
    /// 1-based character offset just past the last selected character in the last line.
    pub end_offset: u32,
}

/// A diff line's position in both files. A line missing from one file counts the line it comes
/// before there: a line added after base line 4 sits at `old` 5. GitLab line codes count this way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FilePositions {
    pub old: u32,
    pub new: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadComment {
    pub id: String,
    pub author: UserIdentity,
    /// Markdown.
    pub body: String,
    pub created_at: Timestamp,
    pub updated_at: Option<Timestamp>,
}

/// Where an inline thread was written: the two commits of the diff its author looked at and its
/// lines in that diff. Platforms keep this when the code moves on, so the version the comment was
/// made on can be shown again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadOrigin {
    /// The diff's base commit. `None` where the platform does not say (GitHub): the merge base of
    /// the target branch and `head_sha` stands in for it.
    pub base_sha: Option<String>,
    /// The head commit the comment was written on.
    pub head_sha: String,
    /// Path and lines in that diff, numbered like [`ReviewThread::anchor`].
    pub anchor: ThreadAnchor,
    /// The lines around the comment as the platform keeps them (a unified diff hunk, GitHub
    /// `diff_hunk`), for when the commits are gone.
    pub diff_hunk: Option<String>,
    /// The comment on the platform's web page, which shows it with its original code.
    pub web_url: Option<String>,
}

/// A conversation: an inline review thread, a general discussion, or a single PR comment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent facts each platform reports per thread (outdated, system note, may \
              reply, may resolve); no two of them form one state"
)]
pub struct ReviewThread {
    /// Provider id used for replies and status changes (GitHub: GraphQL thread node id or issue comment id).
    pub id: String,
    pub status: ThreadStatus,
    pub anchor: Option<ThreadAnchor>,
    /// The anchored code changed after the thread was opened.
    pub is_outdated: bool,
    /// Where the thread was written. Set for outdated inline threads whose platform reports it;
    /// providers may leave it `None` for current ones.
    pub origin: Option<ThreadOrigin>,
    /// Generated by the platform (Azure DevOps vote/update notices, GitLab system notes).
    pub is_system: bool,
    pub can_reply: bool,
    pub can_resolve: bool,
    pub comments: Vec<ThreadComment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewThread {
    /// Markdown.
    pub body: String,
    /// `None` for a general pull request comment.
    pub anchor: Option<ThreadAnchor>,
}
