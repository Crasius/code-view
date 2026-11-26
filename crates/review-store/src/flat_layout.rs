//! Files from before the per-connection layout of a category folder (viewed marks, thread read
//! marks and drafts): `<category>\<pull request>.json`. Their name is a hash of the pull request
//! key, so nothing on disk says which connection a file belongs to.
//!
//! The first time that matters (a load that finds such a file, or a removed connection), the ids
//! of the connections configured then go to `<category>\owners.json`. Only those connections adopt
//! a flat file, into their own folder, when they open its pull request. A connection removed later
//! leaves the list, so a new connection that reuses its id starts empty; once no owner is left,
//! the flat files that remain are deleted.

use std::collections::{BTreeSet, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::retention::connection_ids;
use crate::{AppPaths, atomic_file, stable_hash};

const OWNERS_FILE: &str = "owners.json";

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Owners {
    connections: BTreeSet<String>,
}

/// The flat files of one category folder and who may adopt them.
pub(crate) struct FlatLayout {
    dir: PathBuf,
    connections_file: PathBuf,
}

impl FlatLayout {
    /// The flat files directly in `dir`, owned by connections of `paths`.
    pub(crate) fn new(paths: &AppPaths, dir: PathBuf) -> Self {
        Self { dir, connections_file: paths.connections_file() }
    }

    /// Where the state of `pull_request_key` was stored before the layout.
    pub(crate) fn flat_file(&self, pull_request_key: &str) -> PathBuf {
        self.dir.join(format!("{}.json", stable_hash(pull_request_key)))
    }

    /// The list of connections that may adopt flat files; the age limit never takes it.
    pub(crate) fn is_owners_file(&self, path: &Path) -> bool {
        path == self.owners_file()
    }

    /// Moves the flat file of `pull_request_key` to `file`, its place in the folder of
    /// `connection_id`, when that connection owns flat files. A move that fails keeps the flat
    /// file and returns the error; the next load tries again.
    pub(crate) fn adopt(
        &self,
        connection_id: &str,
        pull_request_key: &str,
        file: &Path,
    ) -> io::Result<()> {
        let flat = self.flat_file(pull_request_key);
        if file.exists() || !flat.exists() {
            return Ok(());
        }
        let Some(owners) = self.owners()? else {
            return Ok(());
        };
        if !owners.connections.contains(connection_id) {
            return Ok(());
        }
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(flat, file)
    }

    /// Drops the connections missing from `known` from the owners. True when no owner is left,
    /// so the flat files are nobody's and can go. False while the owners cannot be read or
    /// written; the next removal or sweep tries again.
    pub(crate) fn release_removed(&self, known: &HashSet<String>) -> bool {
        let Ok(Some(mut owners)) = self.owners() else {
            return false;
        };
        let count = owners.connections.len();
        owners.connections.retain(|id| known.contains(id));
        if owners.connections.len() != count
            && atomic_file::write_json(&self.owners_file(), &owners).is_err()
        {
            return false;
        }
        owners.connections.is_empty()
    }

    /// The owners, written from the configured connections the first time flat files are found.
    /// `None` when there are no flat files, or the connection list cannot be read: unsure means
    /// no owner, and the flat files stay.
    fn owners(&self) -> io::Result<Option<Owners>> {
        let file = self.owners_file();
        if let Some(owners) = atomic_file::read_json(&file)? {
            return Ok(Some(owners));
        }
        if !self.has_flat_files() {
            return Ok(None);
        }
        let Some(ids) = connection_ids(&self.connections_file) else {
            return Ok(None);
        };
        let owners = Owners { connections: ids.into_iter().collect() };
        atomic_file::write_json(&file, &owners)?;
        Ok(Some(owners))
    }

    fn owners_file(&self) -> PathBuf {
        self.dir.join(OWNERS_FILE)
    }

    fn has_flat_files(&self) -> bool {
        let Ok(children) = std::fs::read_dir(&self.dir) else {
            return false;
        };
        children.flatten().any(|child| {
            let path = child.path();
            child.file_type().is_ok_and(|file_type| file_type.is_file())
                && path.extension().is_some_and(|extension| extension == "json")
                && !self.is_owners_file(&path)
        })
    }
}
