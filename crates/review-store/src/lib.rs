//! Everything the app keeps on disk. All writes are atomic (temp file + rename).

mod assistant;
mod atomic_file;
pub mod checkouts;
mod cold_cache;
mod connections;
mod flat_layout;
mod log_file;
mod merge_preferences;
mod overview;
mod paths;
mod platform;
pub mod retention;
mod settings;
#[cfg(test)]
mod settings_tests;
#[cfg(test)]
mod test_support;
mod threads;
mod token_vault;
mod viewed;

pub use assistant::chats::{
    AssistantChats, Chat, ChatQuote, ChatStep, ChatStepKind, ChatTurn, ChatUsage, ChatsSnapshot,
    TurnEnd, TurnKind, agent_checkouts_dir, chats_dir,
};
pub(crate) use assistant::settings::{AssistantSettings, MiniChatCorner};
pub use atomic_file::write_bytes;
pub use cold_cache::{BUDGET_BYTES as COLD_CACHE_BUDGET_BYTES, ColdCache};
pub use connections::ConnectionStore;
pub use log_file::LogFile;
pub use merge_preferences::MergePreferences;
pub use overview::{Favorites, RepositorySections, SectionCard, SectionKey};
pub use paths::AppPaths;
pub use settings::{
    AppSettings, CheckFilter, CommentEditor, CommentEditors, DiffLayout, FileFilters, FilesStart,
    FrameRateLimit, ImageCompare, ListFilters, PaneSizes, PullRequestStart, RenderedWidth,
    ThemePreference, ThreadFilters, TreeSide, WindowPlacement,
};
pub use settings::{HeaderMode, HeaderModes};
pub(crate) use settings::{PipelineSettings, ThreadsPanel};
pub use threads::{Draft, DraftAnchor, DraftRange, ThreadStore};
pub use token_vault::{LINUX_HELP_URL, TokenVault, VaultProblem};
pub use viewed::{LastPlace, PullRequestTab, ViewedFiles};

/// Short, stable, filesystem-safe hash for cache file names (FNV-1a 64).
pub fn stable_hash(value: &str) -> String {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let hash =
        value.bytes().fold(OFFSET_BASIS, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(PRIME));
    format!("{hash:016x}")
}
