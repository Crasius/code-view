//! The checkouts the app made, one record per pull request, in `checkouts.json` of the config
//! folder: where the branch is checked out, which head of the pull request it has, and a short log
//! (the checkout itself, every update) the "Show log" link shows again after a restart.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::settings::OpenWith;
use crate::atomic_file;

/// Log lines a record keeps; older ones fall out.
pub const MAX_LOG_LINES: usize = 200;

/// What kind of folder a checkout went to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckoutKind {
    /// A worktree the checkout made; only this kind can be removed from the app.
    Worktree,
    /// The user's own clone, switched to the branch.
    Clone,
    /// A new clone the checkout made from the mirror.
    NewClone,
    /// A working tree that had the branch already ("Open it"); the app never removes it.
    Adopted,
}

/// One pull request checked out on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutRecord {
    pub connection_id: String,
    pub repository_key: String,
    pub number: u64,
    /// The clone the branch lives in.
    pub clone: PathBuf,
    /// The working tree the branch is checked out in.
    pub path: PathBuf,
    pub kind: CheckoutKind,
    pub branch: String,
    /// The checkout created the branch; a removal may delete it while nothing was added to it.
    pub is_own_branch: bool,
    /// The clone's ref for the pull request's head.
    pub pull_ref: String,
    /// The pull request's head the checkout last brought in.
    pub head: String,
    /// The tool opened last from this checkout.
    #[serde(default)]
    pub last_opened: OpenWith,
    /// How long the checkout took, in milliseconds.
    #[serde(default)]
    pub took_ms: u64,
    /// The pull request was merged from Code View: once it shows merged, the page asks before
    /// removing the worktree instead of removing it by itself.
    #[serde(default)]
    pub merged_here: bool,
    /// The checkout's log and its updates, newest last, at most [`MAX_LOG_LINES`].
    #[serde(default)]
    pub log: Vec<String>,
}

impl CheckoutRecord {
    pub fn is_for(&self, connection_id: &str, repository_key: &str, number: u64) -> bool {
        self.connection_id == connection_id
            && self.repository_key == repository_key
            && self.number == number
    }

    /// Appends `lines` to the log, dropping the oldest beyond [`MAX_LOG_LINES`].
    pub fn append_log(&mut self, lines: impl IntoIterator<Item = String>) {
        self.log.extend(lines);
        let excess = self.log.len().saturating_sub(MAX_LOG_LINES);
        self.log.drain(..excess);
    }
}

/// Every checkout record, loaded once and written on each change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRecords {
    file: PathBuf,
    records: Vec<CheckoutRecord>,
}

impl CheckoutRecords {
    /// The records in `file`; none when it does not exist yet.
    pub fn load(file: PathBuf) -> io::Result<Self> {
        let records = atomic_file::read_json(&file)?.unwrap_or_default();
        Ok(Self { file, records })
    }

    /// No records, kept in `file` once one is saved; for a file that could not be read.
    pub fn empty(file: PathBuf) -> Self {
        Self { file, records: Vec::new() }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    pub fn all(&self) -> &[CheckoutRecord] {
        &self.records
    }

    pub fn find(
        &self,
        connection_id: &str,
        repository_key: &str,
        number: u64,
    ) -> Option<&CheckoutRecord> {
        self.records.iter().find(|record| record.is_for(connection_id, repository_key, number))
    }

    /// Changes the record of a pull request in place and saves; false when there is none.
    pub fn change(
        &mut self,
        connection_id: &str,
        repository_key: &str,
        number: u64,
        change: impl FnOnce(&mut CheckoutRecord),
    ) -> io::Result<bool> {
        let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.is_for(connection_id, repository_key, number))
        else {
            return Ok(false);
        };
        change(record);
        self.save()?;
        Ok(true)
    }

    /// Adds `record`, replacing the pull request's earlier one, and saves.
    pub fn put(&mut self, record: CheckoutRecord) -> io::Result<()> {
        self.records.retain(|old| {
            !old.is_for(&record.connection_id, &record.repository_key, record.number)
        });
        self.records.push(record);
        self.save()
    }

    /// Forgets the pull request's record and saves.
    pub fn remove(
        &mut self,
        connection_id: &str,
        repository_key: &str,
        number: u64,
    ) -> io::Result<()> {
        self.records.retain(|record| !record.is_for(connection_id, repository_key, number));
        self.save()
    }

    /// How many records name a worktree of the clone in `clone`.
    pub fn worktrees_of(&self, clone: &Path) -> usize {
        self.records
            .iter()
            .filter(|record| record.kind == CheckoutKind::Worktree && record.clone == clone)
            .count()
    }

    fn save(&self) -> io::Result<()> {
        atomic_file::write_json(&self.file, &self.records)
    }
}
