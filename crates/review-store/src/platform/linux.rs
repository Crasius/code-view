//! Linux: tokens in the Secret Service (GNOME Keyring, `KWallet`, `KeePassXC`) over D-Bus, one item
//! per connection like the macOS Keychain items: the attributes `service` ("Code View") and
//! `account` (the connection id), so `secret-tool` and Passwords and Keys show and remove them. The
//! connections file keeps the marker `secret-service`; removing a connection deletes its item.
//! Housekeeping keeps the normal priority, child processes open no window, data follows the XDG
//! base directories.
//!
//! The D-Bus calls sit behind [`SecretStore`], so the marker and error logic is tested against an
//! in-memory store. Tests elsewhere never get here: their folders keep tokens in the connections
//! file ([`crate::TokenVault::File`]).

use std::io;
use std::path::PathBuf;
use std::process::Command;

use super::DataRoots;
use crate::token_vault::{LINUX_HELP_URL, VaultProblem};

pub(crate) const VAULT_SUMMARY: &str = "Tokens are stored in your keyring through the Secret Service (GNOME Keyring, KWallet or KeePassXC). Code View never shows or logs them.";

/// What the connections file keeps for a token that lives in the Secret Service.
const IN_SECRET_SERVICE: &str = "secret-service";
/// Starts the keyring's secret store on a session that has GNOME Keyring installed but not running.
const START_KEYRING: &str = "gnome-keyring-daemon --start --components=secrets";

/// Stores `token` as the Secret Service item of the connection `account`; the file keeps a marker.
pub(crate) fn protect_token(account: &str, token: &str) -> io::Result<String> {
    protect_with(system(), account, token)
}

/// The token of `account` from the Secret Service. A value that is no marker came from another
/// system's store (a connections file copied from Windows) and cannot be read here.
pub(crate) fn unprotect_token(account: &str, stored: &str) -> io::Result<String> {
    unprotect_with(system(), account, stored)
}

/// Deletes the Secret Service item of `account`; one that is not there is fine.
pub(crate) fn forget_token(account: &str) -> io::Result<()> {
    forget_with(system(), account)
}

pub(crate) fn check_vault() -> Result<(), VaultProblem> {
    system().check().map_err(|failure| problem(&failure, &os_release()))
}

/// The normal priority: a niced thread would also need `ioprio_set` to stay out of git's way.
pub(crate) fn enter_background_priority() {}

/// Child processes open no window here: nothing to hide.
pub(crate) fn hide_console(_command: &mut Command) {}

/// `$XDG_CONFIG_HOME` (`~/.config`) for the config, `$XDG_CACHE_HOME` (`~/.cache`) for the cache.
pub(crate) fn data_roots() -> DataRoots {
    let home = std::env::var_os("HOME").map_or_else(std::env::temp_dir, PathBuf::from);
    let absolute =
        |name: &str| std::env::var_os(name).map(PathBuf::from).filter(|path| path.is_absolute());
    DataRoots {
        config: absolute("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")),
        cache: absolute("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache")),
    }
}

/// What went wrong talking to the secret store.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    /// Nothing answers for the Secret Service on the session bus (no keyring daemon, no session bus).
    Unavailable(String),
    /// The keyring stayed locked: the unlock prompt was dismissed or went away.
    Locked,
    /// The service has no collection to keep items in.
    NoCollection,
    Other(String),
}

/// The four calls the token logic needs from a secret store.
trait SecretStore: Sync {
    /// Creates or replaces the item of `account`.
    fn write(&self, account: &str, secret: &str) -> Result<(), Failure>;
    fn read(&self, account: &str) -> Result<Option<String>, Failure>;
    /// Deletes every item of `account`; none is fine.
    fn delete(&self, account: &str) -> Result<(), Failure>;
    /// Whether an item could be written now.
    fn check(&self) -> Result<(), Failure>;
}

fn protect_with(store: &dyn SecretStore, account: &str, token: &str) -> io::Result<String> {
    store
        .write(account, token)
        .map_err(|failure| io::Error::from(problem(&failure, &os_release())))?;
    Ok(IN_SECRET_SERVICE.to_owned())
}

fn unprotect_with(store: &dyn SecretStore, account: &str, stored: &str) -> io::Result<String> {
    if stored != IN_SECRET_SERVICE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the token was protected on another system; add it again",
        ));
    }
    match store.read(account) {
        Ok(Some(token)) => Ok(token),
        Ok(None) => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the keyring has no token for this connection; add it again",
        )),
        Err(failure) => Err(io::Error::from(problem(&failure, &os_release()))),
    }
}

/// Without a Secret Service there is nothing to delete (and nothing this process could have
/// written), so a connection without a token still saves on such a system.
fn forget_with(store: &dyn SecretStore, account: &str) -> io::Result<()> {
    match store.delete(account) {
        Ok(()) | Err(Failure::Unavailable(_)) => Ok(()),
        Err(failure) => Err(io::Error::from(problem(&failure, &os_release()))),
    }
}

/// The fix for `failure`, with the install command of the distribution `os_release` describes.
fn problem(failure: &Failure, os_release: &str) -> VaultProblem {
    let (title, fix, commands) = match failure {
        Failure::Unavailable(_) => (
            "No keyring answers on this desktop session",
            "Code View keeps tokens in your keyring through the Secret Service, and nothing provides it here. \
             Install GNOME Keyring (or turn on the Secret Service in KeePassXC or KWallet), start it or sign out \
             and back in, then check again. Connections without a token work meanwhile."
                .to_owned(),
            install_command(os_release).into_iter().chain([START_KEYRING.to_owned()]).collect(),
        ),
        Failure::Locked => (
            "The keyring stayed locked",
            "Unlock it when your desktop asks (usually with your login password), then try again.".to_owned(),
            Vec::new(),
        ),
        Failure::NoCollection => (
            "The keyring has no place for passwords",
            "Open Passwords and Keys (Seahorse) or your keyring app, create a keyring and make it the default, \
             then check again."
                .to_owned(),
            Vec::new(),
        ),
        Failure::Other(detail) => (
            "The keyring did not take the token",
            format!("{detail}. Make sure the keyring is unlocked, then try again."),
            Vec::new(),
        ),
    };
    VaultProblem { title: title.to_owned(), fix, commands, help_url: LINUX_HELP_URL }
}

/// The package manager line that installs GNOME Keyring on the distribution `os_release`
/// (the text of `/etc/os-release`) describes.
fn install_command(os_release: &str) -> Option<String> {
    let field = |key: &str| {
        os_release
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(|value| value.trim().trim_matches('"').to_ascii_lowercase())
            .unwrap_or_default()
    };
    let family = format!("{} {}", field("ID"), field("ID_LIKE"));
    let is = |names: &[&str]| family.split_whitespace().any(|id| names.contains(&id));
    let command = if is(&["debian", "ubuntu"]) {
        "sudo apt install gnome-keyring"
    } else if is(&["fedora", "rhel", "centos"]) {
        "sudo dnf install gnome-keyring"
    } else if is(&["arch", "manjaro"]) {
        "sudo pacman -S gnome-keyring"
    } else if is(&["opensuse", "suse"]) {
        "sudo zypper install gnome-keyring"
    } else {
        return None;
    };
    Some(command.to_owned())
}

fn os_release() -> String {
    std::fs::read_to_string("/etc/os-release").unwrap_or_default()
}

fn system() -> &'static dyn SecretStore {
    &dbus::SecretServiceStore
}

mod dbus {
    use std::collections::HashMap;

    use secret_service::EncryptionType;
    use secret_service::blocking::SecretService;

    use super::{Failure, SecretStore};

    /// The items' `service` attribute; tests keep to their own items.
    const SERVICE: &str = if cfg!(test) { "Code View (tests)" } else { "Code View" };
    const CONTENT_TYPE: &str = "text/plain";

    /// The Secret Service on the session bus, with a Diffie-Hellman session so the token crosses
    /// D-Bus encrypted. Each call connects afresh: saves are rare, and a keyring started or
    /// unlocked meanwhile is found.
    pub(super) struct SecretServiceStore;

    fn attributes(account: &str) -> HashMap<&str, &str> {
        HashMap::from([("service", SERVICE), ("account", account)])
    }

    fn connect<'a>() -> Result<SecretService<'a>, Failure> {
        Ok(SecretService::connect(EncryptionType::Dh)?)
    }

    impl SecretStore for SecretServiceStore {
        fn write(&self, account: &str, secret: &str) -> Result<(), Failure> {
            let service = connect()?;
            let collection = service.get_any_collection()?;
            if collection.is_locked()? {
                collection.unlock()?;
            }
            let label = format!("{SERVICE}: {account}");
            collection.create_item(
                &label,
                attributes(account),
                secret.as_bytes(),
                true,
                CONTENT_TYPE,
            )?;
            Ok(())
        }

        fn read(&self, account: &str) -> Result<Option<String>, Failure> {
            let service = connect()?;
            let found = service.search_items(attributes(account))?;
            let Some(item) = found.unlocked.first().or(found.locked.first()) else {
                return Ok(None);
            };
            if item.is_locked()? {
                item.unlock()?;
            }
            let secret = item.get_secret()?;
            String::from_utf8(secret)
                .map(Some)
                .map_err(|_| Failure::Other("the stored token is not text".to_owned()))
        }

        fn delete(&self, account: &str) -> Result<(), Failure> {
            let service = connect()?;
            let found = service.search_items(attributes(account))?;
            for item in found.unlocked.iter().chain(&found.locked) {
                if item.is_locked()? {
                    item.unlock()?;
                }
                item.delete()?;
            }
            Ok(())
        }

        fn check(&self) -> Result<(), Failure> {
            let service = connect()?;
            service.get_any_collection()?;
            Ok(())
        }
    }
}

impl From<secret_service::Error> for Failure {
    fn from(error: secret_service::Error) -> Self {
        use secret_service::Error;
        match error {
            Error::Unavailable => Failure::Unavailable(error.to_string()),
            Error::Locked | Error::Prompt | Error::PromptDisconnected => Failure::Locked,
            Error::NoResult => Failure::NoCollection,
            other => classify(other.to_string()),
        }
    }
}

/// A D-Bus error that says nobody owns the Secret Service name means no keyring runs; anything
/// else is the keyring's own answer.
fn classify(detail: String) -> Failure {
    const MISSING: [&str; 4] =
        ["ServiceUnknown", "NameHasNoOwner", "No such file or directory", "Connection refused"];
    if MISSING.iter().any(|marker| detail.contains(marker)) {
        Failure::Unavailable(detail)
    } else {
        Failure::Other(detail)
    }
}

/// A store in memory: the stand-in for the Secret Service in this module's tests.
#[cfg(test)]
struct MemoryStore {
    items: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    failure: Option<Failure>,
}

#[cfg(test)]
impl MemoryStore {
    const fn new() -> Self {
        Self { items: std::sync::Mutex::new(std::collections::BTreeMap::new()), failure: None }
    }

    fn items(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, std::collections::BTreeMap<String, String>>, Failure>
    {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        Ok(self.items.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }
}

#[cfg(test)]
impl SecretStore for MemoryStore {
    fn write(&self, account: &str, secret: &str) -> Result<(), Failure> {
        self.items()?.insert(account.to_owned(), secret.to_owned());
        Ok(())
    }

    fn read(&self, account: &str) -> Result<Option<String>, Failure> {
        Ok(self.items()?.get(account).cloned())
    }

    fn delete(&self, account: &str) -> Result<(), Failure> {
        self.items()?.remove(account);
        Ok(())
    }

    fn check(&self) -> Result<(), Failure> {
        self.items().map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failing(failure: Failure) -> MemoryStore {
        MemoryStore { failure: Some(failure), ..MemoryStore::new() }
    }

    #[test]
    fn keeps_a_marker_in_the_file_and_the_token_in_the_store() {
        let store = MemoryStore::new();
        let stored = protect_with(&store, "gh-1", "example-value").unwrap();
        assert_eq!(stored, "secret-service");
        assert_eq!(unprotect_with(&store, "gh-1", &stored).unwrap(), "example-value");
        forget_with(&store, "gh-1").unwrap();
        let gone = unprotect_with(&store, "gh-1", &stored).unwrap_err();
        assert_eq!(gone.kind(), io::ErrorKind::NotFound);
        forget_with(&store, "gh-1").unwrap();
    }

    #[test]
    fn a_value_from_another_system_asks_for_the_token_again() {
        let error =
            unprotect_with(&MemoryStore::new(), "gh-1", "AQAAANCMnd8BFdERjHoAwE").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_missing_keyring_fails_the_save_with_the_fix() {
        let store =
            failing(Failure::Unavailable("org.freedesktop.DBus.Error.ServiceUnknown".to_owned()));
        let error = protect_with(&store, "gh-1", "example-value").unwrap_err();
        let problem = VaultProblem::of(&error).expect("the error carries the fix");
        assert_eq!(problem.help_url, LINUX_HELP_URL);
        assert!(problem.commands.iter().any(|command| command == START_KEYRING), "{problem:?}");
        assert!(!error.to_string().contains("example-value"), "the token never shows in an error");
    }

    #[test]
    fn forgetting_without_a_keyring_lets_tokenless_connections_save() {
        forget_with(&failing(Failure::Unavailable("no session bus".to_owned())), "gh-1").unwrap();
        let locked = forget_with(&failing(Failure::Locked), "gh-1").unwrap_err();
        assert_eq!(
            VaultProblem::of(&locked).map(|problem| problem.title.as_str()),
            Some("The keyring stayed locked")
        );
    }

    #[test]
    fn reading_from_a_locked_keyring_says_how_to_unlock() {
        let error =
            unprotect_with(&failing(Failure::Locked), "gh-1", "secret-service").unwrap_err();
        assert!(VaultProblem::of(&error).is_some_and(|problem| problem.fix.contains("Unlock")));
    }

    #[test]
    fn install_command_follows_the_distribution() {
        let ubuntu = "NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\n";
        assert_eq!(install_command(ubuntu).as_deref(), Some("sudo apt install gnome-keyring"));
        let mint = "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n";
        assert_eq!(install_command(mint).as_deref(), Some("sudo apt install gnome-keyring"));
        assert_eq!(
            install_command("ID=fedora\n").as_deref(),
            Some("sudo dnf install gnome-keyring")
        );
        assert_eq!(
            install_command("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n").as_deref(),
            Some("sudo zypper install gnome-keyring")
        );
        assert_eq!(install_command("ID=nixos\n"), None);
        let problem = problem(&Failure::Unavailable(String::new()), "ID=nixos\n");
        assert_eq!(problem.commands, vec![START_KEYRING.to_owned()]);
    }

    #[test]
    fn dbus_errors_without_an_owner_mean_no_keyring() {
        let unknown = "org.freedesktop.DBus.Error.ServiceUnknown: The name org.freedesktop.secrets was not provided by any .service files";
        assert!(matches!(classify(unknown.to_owned()), Failure::Unavailable(_)));
        assert!(matches!(
            classify("org.freedesktop.Secret.Error.IsLocked".to_owned()),
            Failure::Other(_)
        ));
    }

    /// Against the session's real Secret Service: `cargo test -p review-store -- --ignored` on a
    /// Linux desktop with an unlocked keyring.
    #[test]
    #[ignore = "needs a Secret Service on the session bus"]
    fn round_trips_through_the_secret_service() {
        let store = dbus::SecretServiceStore;
        let account = format!("round-trip-{}", std::process::id());
        let stored = protect_with(&store, &account, "example-value").unwrap();
        assert_eq!(unprotect_with(&store, &account, &stored).unwrap(), "example-value");
        forget_with(&store, &account).unwrap();
        assert!(unprotect_with(&store, &account, &stored).is_err(), "the item is gone");
        forget_with(&store, &account).unwrap();
    }
}
