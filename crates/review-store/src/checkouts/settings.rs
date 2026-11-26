//! Settings › Local checkouts: where the app looks for the user's clones, where new worktrees go,
//! what opens after a checkout and in which terminal, and what the user chose to remember per
//! repository.

use std::collections::BTreeMap;
use std::path::PathBuf;

use review_core::AgentKind;
use serde::{Deserialize, Deserializer, Serialize};

use crate::settings::or_default;

/// The worktree folder pattern of new settings: one folder per pull request beside the clone.
pub const DEFAULT_WORKTREE_PATTERN: &str = "{clone}-wt/pr-{number}-{branch}";

/// Where a checkout goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CheckoutPlace {
    /// A new worktree beside the clone; the clone's own work stays untouched.
    #[default]
    Worktree,
    /// The clone itself switches to the branch.
    Clone,
}

/// What opens in the checkout's folder afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OpenWith {
    #[default]
    Nothing,
    Terminal,
    Agent(AgentKind),
}

/// What the popover preselects under "Then open".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OpenByDefault {
    /// The tool used last, so the user picks each time.
    #[default]
    Ask,
    Always(OpenWith),
}

/// The terminal "Terminal" opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TerminalChoice {
    /// Windows Terminal on Windows, Terminal on macOS, the desktop's terminal on Linux.
    #[default]
    System,
    /// Windows only.
    PowerShell,
    /// Windows only: Git for Windows' bash.
    GitBash,
}

/// What happens to a worktree once its pull request was merged or closed elsewhere: on the host,
/// not by a merge from Code View, which always asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WhenFinished {
    /// Removed when nothing of the user's would go with it; otherwise the page asks.
    #[default]
    RemoveAutomatically,
    /// The pull request's page offers the removal.
    Ask,
    /// The worktree stays and nothing asks.
    Keep,
}

/// "Remember for this repository": the place and the tool of the last checkout there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RememberedCheckout {
    pub place: CheckoutPlace,
    pub then: OpenWith,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckoutSettings {
    /// Folders searched for clones; `None` until the app filled in the defaults once (the usual
    /// development folders that exist on this computer), so a list the user emptied stays empty.
    #[serde(deserialize_with = "or_default")]
    pub search_folders: Option<Vec<PathBuf>>,
    /// New worktrees' folder, with `{clone}`, `{number}` and `{branch}`.
    pub worktree_pattern: String,
    /// What happens to a worktree once its pull request was merged or closed elsewhere. Settings
    /// from before it hold the switch `offer_removal` instead: on (or missing) removes, off keeps.
    #[serde(alias = "offer_removal", deserialize_with = "when_finished_or_switch")]
    pub when_finished: WhenFinished,
    #[serde(deserialize_with = "or_default")]
    pub open_by_default: OpenByDefault,
    #[serde(deserialize_with = "or_default")]
    pub terminal: TerminalChoice,
    /// The tool opened last, which "Ask each time" preselects.
    #[serde(deserialize_with = "or_default")]
    pub last_opened: OpenWith,
    /// Clones the user picked, by repository (`connection id|repository key`); they win over
    /// the search.
    pub pinned_clones: BTreeMap<String, PathBuf>,
    /// "Remember for this repository", by repository (`connection id|repository key`).
    #[serde(deserialize_with = "or_default")]
    pub remembered: BTreeMap<String, RememberedCheckout>,
}

/// The choice; from settings of before it the switch "Offer to remove a worktree": on removes
/// automatically, off keeps the worktree. Anything else this version cannot read gets the default.
fn when_finished_or_switch<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<WhenFinished, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::Bool(false) => WhenFinished::Keep,
        serde_json::Value::Bool(true) => WhenFinished::RemoveAutomatically,
        value => WhenFinished::deserialize(value).unwrap_or_default(),
    })
}

impl Default for CheckoutSettings {
    fn default() -> Self {
        Self {
            search_folders: None,
            worktree_pattern: DEFAULT_WORKTREE_PATTERN.to_owned(),
            when_finished: WhenFinished::RemoveAutomatically,
            open_by_default: OpenByDefault::Ask,
            terminal: TerminalChoice::System,
            last_opened: OpenWith::Nothing,
            pinned_clones: BTreeMap::new(),
            remembered: BTreeMap::new(),
        }
    }
}

impl CheckoutSettings {
    /// The key of a repository in [`Self::pinned_clones`] and [`Self::remembered`].
    pub fn repository_key(connection_id: &str, repository_key: &str) -> String {
        format!("{connection_id}|{repository_key}")
    }

    /// The tool "Then open" starts with for a repository: what was remembered there, else the
    /// default, else the tool used last.
    pub fn preselected(&self, repository: &str) -> (CheckoutPlace, OpenWith) {
        if let Some(remembered) = self.remembered.get(repository) {
            return (remembered.place, remembered.then);
        }
        let then = match self.open_by_default {
            OpenByDefault::Ask => self.last_opened,
            OpenByDefault::Always(then) => then,
        };
        (CheckoutPlace::Worktree, then)
    }

    /// Drops the per-repository entries of connections that no longer exist.
    pub fn retain_connections(&mut self, is_known: impl Fn(&str) -> bool) {
        let keeps = |key: &String| key.split_once('|').is_some_and(|(id, _)| is_known(id));
        self.pinned_clones.retain(|key, _| keeps(key));
        self.remembered.retain(|key, _| keeps(key));
    }
}
