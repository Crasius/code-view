use std::path::{Path, PathBuf};

use crate::{TokenVault, platform, stable_hash};

/// The data folder keeps the app's original name: renaming the product to Code View must not
/// strand existing connections, settings, mirrors or viewed state, and nobody browses this folder.
const APP_DIR_NAME: &str = "ReviewApp";

/// `config` roams with the user profile (connections, settings);
/// `cache` is machine-local and disposable (git mirrors, logs, viewed state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    config: PathBuf,
    cache: PathBuf,
    tokens: TokenVault,
}

impl AppPaths {
    /// The user's data folders; tokens go to the system's store.
    pub fn from_environment() -> Self {
        let roots = platform::data_roots();
        Self {
            config: roots.config.join(APP_DIR_NAME),
            cache: roots.cache.join(APP_DIR_NAME),
            tokens: TokenVault::System,
        }
    }

    /// Folders the caller picked (tests, fixtures): their tokens stay in their connections file
    /// ([`TokenVault::File`]) and never touch the system's store.
    pub fn new(config: PathBuf, cache: PathBuf) -> Self {
        Self { config, cache, tokens: TokenVault::File }
    }

    /// These folders with their tokens in `tokens` (a test's [`TokenVault::Refusing`] store).
    #[must_use]
    pub fn with_token_vault(self, tokens: TokenVault) -> Self {
        Self { tokens, ..self }
    }

    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    /// Where the connections' tokens rest.
    pub fn token_vault(&self) -> TokenVault {
        self.tokens
    }

    pub fn connections_file(&self) -> PathBuf {
        self.config.join("connections.json")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    /// The pull requests checked out on this computer (`checkouts::CheckoutRecords`).
    pub fn checkouts_file(&self) -> PathBuf {
        self.config.join("checkouts.json")
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }

    pub fn mirrors_dir(&self) -> PathBuf {
        self.cache.join("mirrors")
    }

    pub(crate) fn logs_dir(&self) -> PathBuf {
        self.cache.join("logs")
    }

    /// The app's memory readings over time (`memory_journal` in the app); the previous file sits
    /// beside it. Numbers only, no names, links or content.
    pub fn memory_journal_file(&self) -> PathBuf {
        self.logs_dir().join("memory.log")
    }

    /// Pipeline logs of one connection, so removing the connection removes them.
    pub fn connection_logs_dir(&self, connection_id: &str) -> PathBuf {
        self.logs_dir().join(stable_hash(connection_id))
    }

    /// One pipeline log of a pull request (`connection|repository|number`). The name starts with
    /// the pull request's hash, so a finished pull request's logs can go together.
    pub fn pull_request_log_file(
        &self,
        connection_id: &str,
        pull_request_key: &str,
        log_id: &str,
    ) -> PathBuf {
        let name = format!(
            "{}-{}.log",
            stable_hash(pull_request_key),
            stable_hash(&format!("{pull_request_key}|{log_id}"))
        );
        self.connection_logs_dir(connection_id).join(name)
    }

    /// Images that descriptions and comments embed, as downloaded.
    pub(crate) fn images_dir(&self) -> PathBuf {
        self.cache.join("images")
    }

    /// Embedded images fetched through one connection, so removing the connection removes them.
    pub fn connection_images_dir(&self, connection_id: &str) -> PathBuf {
        self.images_dir().join(stable_hash(connection_id))
    }

    pub fn viewed_dir(&self) -> PathBuf {
        self.cache.join("viewed")
    }

    /// Read marks and unsent drafts of comment threads, one file per pull request.
    pub fn threads_dir(&self) -> PathBuf {
        self.cache.join("threads")
    }

    /// Where deletions wait for the background thread; see `retention`.
    pub(crate) fn trash_dir(&self) -> PathBuf {
        self.cache.join("trash")
    }

    /// The disk tier of the memory caches; see `ColdCache`.
    pub fn cold_cache_dir(&self) -> PathBuf {
        self.cache.join("cold")
    }

    /// The browser extension's native host manifest (Windows) and the unpacked extension
    /// (`docs/browser-extension.md`); in the cache folder, since both name this machine's
    /// executable and must not roam.
    pub fn browser_dir(&self) -> PathBuf {
        self.cache.join("browser")
    }

    /// The Unix socket through which a later start hands its link to the running window (macOS
    /// and Linux; Windows uses a named pipe). Its folder is the owner's only.
    pub fn ipc_socket(&self) -> PathBuf {
        self.cache.join("ipc").join("codeview.sock")
    }
}
