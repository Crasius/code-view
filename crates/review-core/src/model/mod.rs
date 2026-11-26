mod change_group;
mod checks;
mod file_category;
mod identity;
mod linked;
mod merge_policy;
mod pipeline;
mod pull_request;
mod repository;
mod review;
mod revision;
mod thread;

pub use change_group::{ChangeBreakdown, ChangeGroup, GroupChanges};
pub use checks::{Check, CheckState, RowStatus};
pub use file_category::FileCategory;
pub use identity::UserIdentity;
pub use linked::{ItemKind, ItemRef, LinkedItem, LinkedState};
pub use merge_policy::{MergePolicy, PolicySource, Unreported};
pub use pipeline::{
    AttemptSource, PipelineAttempt, PipelineJob, PipelineRun, RerunAction, RerunKind, RunStatus,
    RunningLogs, as_earlier_attempt, chain_sequentially, infer_dependencies_from_timing,
};
pub use pull_request::{
    AutoComplete, DiffStat, LineCounts, MergeOptions, MergeStatus, MergeStrategy,
    PullRequestDetail, PullRequestQuery, PullRequestScope, PullRequestState, PullRequestSummary,
    StateFilter,
};
pub use repository::{GitRemote, RepositoryInfo};
pub use review::{Label, ReviewVote, Reviewer, is_approved};
pub use revision::{Fingerprint, PullRequestRevision};
pub use thread::{
    DiffSide, FilePositions, LineRange, NewThread, RangeDetail, ReviewThread, ThreadAnchor,
    ThreadComment, ThreadOrigin, ThreadStatus,
};

/// Timestamps are UTC instants as the platform reports them.
pub type Timestamp = time::OffsetDateTime;
