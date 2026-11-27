use review_core::Timestamp;
use serde::Deserialize;

use super::UserDto;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DiscussionDto {
    pub id: String,
    #[serde(default)]
    pub notes: Vec<NoteDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct NoteDto {
    pub id: u64,
    #[serde(default)]
    pub body: String,
    pub author: UserDto,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub updated_at: Option<Timestamp>,
    #[serde(default)]
    pub system: bool,
    #[serde(default)]
    pub resolvable: bool,
    #[serde(default)]
    pub resolved: bool,
    pub position: Option<PositionDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PositionDto {
    /// Merge base of the diff the note sits in.
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    /// `text`, `image` or `file`.
    pub position_type: Option<String>,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// First and last line of a multi-line note.
    pub line_range: Option<LineRangeDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LineRangeDto {
    pub start: RangeLineDto,
    pub end: RangeLineDto,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RangeLineDto {
    /// `new` for an added line, `old` otherwise.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
}
