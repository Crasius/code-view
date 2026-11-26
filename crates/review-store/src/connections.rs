use std::io;
use std::path::PathBuf;

use review_core::{Connection, ProviderKind};
use serde::{Deserialize, Serialize};

use crate::{AppPaths, TokenVault, atomic_file};

#[derive(Debug, Serialize, Deserialize)]
struct StoredConnection {
    id: String,
    name: String,
    kind: ProviderKind,
    base_url: String,
    #[serde(default)]
    organizations: Vec<String>,
    /// The token as the store's [`TokenVault`] protects it. Absent for anonymous connections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    protected_token: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoredConnections {
    connections: Vec<StoredConnection>,
}

/// The list of configured connections, persisted with their tokens in a [`TokenVault`].
pub struct ConnectionStore {
    file: PathBuf,
    vault: TokenVault,
    connections: Vec<Connection>,
}

impl ConnectionStore {
    /// The connections file of `paths`, with the vault those paths pick.
    pub fn open(paths: &AppPaths) -> io::Result<Self> {
        Self::load(paths.connections_file(), paths.token_vault())
    }

    pub fn load(file: PathBuf, vault: TokenVault) -> io::Result<Self> {
        let stored: StoredConnections = atomic_file::read_json(&file)?.unwrap_or_default();
        let connections = stored
            .connections
            .into_iter()
            .map(|stored| into_connection(stored, vault))
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Self { file, vault, connections })
    }

    pub fn all(&self) -> &[Connection] {
        &self.connections
    }

    pub fn find(&self, id: &str) -> Option<&Connection> {
        self.connections.iter().find(|connection| connection.id == id)
    }

    /// Inserts or replaces by id and saves. A save that fails leaves the list as it was, so a retry
    /// (without the token the vault refused) replaces nothing that never reached the file.
    pub fn upsert(&mut self, connection: Connection) -> io::Result<()> {
        let before = self.connections.clone();
        match self.connections.iter_mut().find(|existing| existing.id == connection.id) {
            Some(existing) => *existing = connection,
            None => self.connections.push(connection),
        }
        self.save().inspect_err(|_| self.connections = before)
    }

    pub fn remove(&mut self, id: &str) -> io::Result<()> {
        let before = self.connections.clone();
        self.connections.retain(|connection| connection.id != id);
        self.save().inspect_err(|_| self.connections = before)?;
        self.vault.forget(id)
    }

    fn save(&self) -> io::Result<()> {
        let connections = self
            .connections
            .iter()
            .map(|connection| to_stored(connection, self.vault))
            .collect::<io::Result<Vec<_>>>()?;
        atomic_file::write_json(&self.file, &StoredConnections { connections })
    }

    /// A fresh id that is not used yet.
    pub fn next_id(&self, kind: ProviderKind) -> String {
        let prefix = match kind {
            ProviderKind::GitHub => "gh",
            ProviderKind::GitLab => "gl",
            ProviderKind::AzureDevOps => "ado",
        };
        // One more candidate than there are connections: at least one of them is free.
        (1..=self.connections.len() + 1)
            .map(|index| format!("{prefix}-{index}"))
            .find(|candidate| self.find(candidate).is_none())
            .unwrap_or_else(|| prefix.to_owned())
    }
}

fn into_connection(stored: StoredConnection, vault: TokenVault) -> io::Result<Connection> {
    let token = stored
        .protected_token
        .as_deref()
        .map(|protected| vault.unprotect(&stored.id, protected))
        .transpose()?;
    Ok(Connection {
        id: stored.id,
        name: stored.name,
        kind: stored.kind,
        base_url: stored.base_url,
        organizations: stored.organizations,
        token,
    })
}

fn to_stored(connection: &Connection, vault: TokenVault) -> io::Result<StoredConnection> {
    let protected_token = if let Some(token) = connection.token() {
        Some(vault.protect(&connection.id, token)?)
    } else {
        // A connection that lost its token leaves nothing behind in the system's store either.
        vault.forget(&connection.id)?;
        None
    };
    Ok(StoredConnection {
        id: connection.id.clone(),
        name: connection.name.clone(),
        kind: connection.kind,
        base_url: connection.base_url.clone(),
        organizations: connection.organizations.clone(),
        protected_token,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VaultProblem;
    use crate::test_support::{TestDir, no_keyring};

    fn work(id: &str) -> Connection {
        Connection {
            id: id.to_owned(),
            name: "Work".into(),
            kind: ProviderKind::GitHub,
            base_url: "https://github.com".into(),
            organizations: vec!["acme".into()],
            token: Some("sample-secret".into()),
        }
    }

    /// DPAPI keeps nothing outside the file; elsewhere the system's store is the user's own (the
    /// login Keychain on macOS), which `cargo test` leaves alone: `-- --ignored` checks it there.
    #[test]
    #[cfg_attr(not(windows), ignore = "writes to the system's token store; run with --ignored")]
    fn persists_connections_without_plain_tokens() {
        let dir = TestDir::new("connections");
        let file = dir.path().join("connections.json");
        let mut store = ConnectionStore::load(file.clone(), TokenVault::System).unwrap();
        let id = store.next_id(ProviderKind::GitHub);
        store.upsert(work(&id)).unwrap();

        let raw = std::fs::read_to_string(&file).unwrap();
        assert!(!raw.contains("sample-secret"));

        let mut reloaded = ConnectionStore::load(file.clone(), TokenVault::System).unwrap();
        assert_eq!(reloaded.find(&id).unwrap().token(), Some("sample-secret"));
        assert_eq!(reloaded.next_id(ProviderKind::GitHub), "gh-2");
        // Removing the connection drops its token wherever the platform keeps it.
        reloaded.remove(&id).unwrap();
        assert_eq!(ConnectionStore::load(file, TokenVault::System).unwrap().all(), []);
    }

    #[test]
    fn folders_a_caller_picked_keep_their_tokens_in_the_file() {
        let root = TestDir::new("file-vault");
        let paths = AppPaths::new(root.path().join("config"), root.path().join("cache"));
        assert_eq!(paths.token_vault(), TokenVault::File);
        assert_eq!(
            AppPaths::from_environment().token_vault(),
            TokenVault::System,
            "the user's folders use the system's store"
        );

        let mut store = ConnectionStore::open(&paths).unwrap();
        store.upsert(work("gh-1")).unwrap();
        let mut reloaded = ConnectionStore::open(&paths).unwrap();
        assert_eq!(reloaded.find("gh-1").unwrap().token(), Some("sample-secret"));
        reloaded.remove("gh-1").unwrap();
        assert_eq!(ConnectionStore::open(&paths).unwrap().all(), []);
    }

    #[test]
    fn a_refused_token_saves_nothing_and_the_connection_saves_without_it() {
        let root = TestDir::new("refusing-vault");
        let problem = no_keyring();
        let paths = AppPaths::new(root.path().join("config"), root.path().join("cache"))
            .with_token_vault(TokenVault::Refusing(problem));
        let mut store = ConnectionStore::open(&paths).unwrap();

        let error = store.upsert(work("gh-1")).unwrap_err();
        assert_eq!(VaultProblem::of(&error), Some(problem));
        assert!(store.all().is_empty(), "the refused connection is not kept");
        assert_eq!(store.next_id(ProviderKind::GitHub), "gh-1");

        store.upsert(Connection { token: None, ..work("gh-1") }).unwrap();
        let reloaded = ConnectionStore::open(&paths).unwrap();
        assert_eq!(reloaded.all().len(), 1);
        assert_eq!(reloaded.find("gh-1").unwrap().token(), None);
    }
}
