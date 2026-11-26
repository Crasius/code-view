//! Serde shapes of the GitLab REST API v4 responses, reduced to the fields the app uses.

// RS-DONT-04: parses provider data, so indexing and narrowing casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

mod discussion;
mod merge_request;
mod pipeline;
mod project;
mod user;

pub(crate) use discussion::{DiscussionDto, NoteDto, PositionDto};
pub(crate) use merge_request::{
    ApprovalsDto, DiffRefsDto, LabelDto, LabelRef, MergeRequestDto, ReviewerDto,
};
pub(crate) use pipeline::{CommitStatusDto, JobDto, PipelineDto};
pub(crate) use project::ProjectDto;
pub(crate) use user::UserDto;
