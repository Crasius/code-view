//! Registers Code View as the extension's native messaging host for the current user, in Chrome
//! and Edge, and tells whether it is registered for the running executable.
//!
//! Windows looks the host up in the registry: each browser's key names one manifest file, which
//! lives in the app's cache folder ([`Registry::Keys`]). macOS and Linux look for the manifest in a
//! folder of each browser's profile ([`Registry::Folders`]). Both are reached through what the
//! [`Registry`] holds, so tests pass a scratch folder and a [`KeyStore`] of their own and never
//! touch the real registry or the user's browser folders.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use super::{HOST_NAME, extension_origin};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Browser {
    Chrome,
    Edge,
}

impl Browser {
    pub const ALL: [Self; 2] = [Self::Chrome, Self::Edge];

    /// The key under `HKEY_CURRENT_USER` whose default value names the manifest (Windows).
    fn key(self) -> String {
        match self {
            Self::Chrome => format!(r"Software\Google\Chrome\NativeMessagingHosts\{HOST_NAME}"),
            Self::Edge => format!(r"Software\Microsoft\Edge\NativeMessagingHosts\{HOST_NAME}"),
        }
    }

    /// The folder below the home folder that holds the browser's per-user host manifests.
    fn folder(self) -> &'static str {
        if cfg!(target_os = "macos") {
            match self {
                Self::Chrome => "Library/Application Support/Google/Chrome/NativeMessagingHosts",
                Self::Edge => "Library/Application Support/Microsoft Edge/NativeMessagingHosts",
            }
        } else {
            match self {
                Self::Chrome => ".config/google-chrome/NativeMessagingHosts",
                Self::Edge => ".config/microsoft-edge/NativeMessagingHosts",
            }
        }
    }
}

/// Where the registration stands for the running executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Every browser starts this executable for the extension.
    Registered,
    /// No browser knows the host.
    NotRegistered,
    /// Some browsers know it, or it names another copy of Code View (moved or replaced).
    Outdated,
}

/// The registry's default values under `HKEY_CURRENT_USER`, by key path.
pub trait KeyStore: Send + Sync {
    fn value(&self, key: &str) -> io::Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> io::Result<()>;
    /// Removes the key; a key that is not there is no error.
    fn delete(&self, key: &str) -> io::Result<()>;
}

pub enum Registry {
    /// Windows: one manifest file, named by each browser's key.
    Keys { manifest: PathBuf, keys: Arc<dyn KeyStore> },
    /// macOS and Linux: a manifest in each browser's folder below `home`.
    Folders { home: PathBuf },
}

impl Registry {
    /// The current user's registration on this system.
    #[cfg_attr(
        windows,
        expect(
            clippy::unnecessary_wraps,
            reason = "only the HOME lookup of macOS and Linux fails"
        )
    )]
    #[cfg_attr(
        not(windows),
        expect(unused_variables, reason = "the browsers' folders lie below HOME, not the app's")
    )]
    /// Tests use it only in the live check run by hand; the others keep the registry in memory
    /// ([`MemoryKeys`]) and their files in scratch folders.
    pub fn system(paths: &review_store::AppPaths) -> io::Result<Self> {
        #[cfg(windows)]
        {
            let manifest = paths.browser_dir().join(format!("{HOST_NAME}.json"));
            Ok(Self::Keys { manifest, keys: Arc::new(super::platform::UserKeys) })
        }
        #[cfg(not(windows))]
        {
            let home =
                std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?;
            Ok(Self::Folders { home: PathBuf::from(home) })
        }
    }

    /// The manifest `browser` finds, if any.
    fn manifest_of(&self, browser: Browser) -> io::Result<Option<String>> {
        let path = match self {
            Self::Keys { keys, .. } => match keys.value(&browser.key())? {
                Some(path) => PathBuf::from(path),
                None => return Ok(None),
            },
            Self::Folders { home } => folder_manifest(home, browser),
        };
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn status(&self, exe: &Path) -> io::Result<Status> {
        let mut ours = 0;
        let mut found = 0;
        for browser in Browser::ALL {
            let Some(manifest) = self.manifest_of(browser)? else {
                continue;
            };
            found += 1;
            if is_ours(&manifest, exe) {
                ours += 1;
            }
        }
        Ok(match (found, ours) {
            (0, _) => Status::NotRegistered,
            (_, ours) if ours == Browser::ALL.len() => Status::Registered,
            _ => Status::Outdated,
        })
    }

    /// Points every browser at `exe` for the extension.
    pub fn register(&self, exe: &Path) -> io::Result<()> {
        let text = manifest(exe);
        match self {
            Self::Keys { manifest, keys } => {
                write_file(manifest, &text)?;
                let value = manifest.to_string_lossy();
                Browser::ALL.into_iter().try_for_each(|browser| keys.set(&browser.key(), &value))
            }
            Self::Folders { home } => Browser::ALL
                .into_iter()
                .try_for_each(|browser| write_file(&folder_manifest(home, browser), &text)),
        }
    }

    /// Takes the host out of every browser; what is not there is no error.
    pub fn unregister(&self) -> io::Result<()> {
        match self {
            Self::Keys { manifest, keys } => {
                Browser::ALL.into_iter().try_for_each(|browser| keys.delete(&browser.key()))?;
                remove_file(manifest)
            }
            Self::Folders { home } => Browser::ALL
                .into_iter()
                .try_for_each(|browser| remove_file(&folder_manifest(home, browser))),
        }
    }
}

fn folder_manifest(home: &Path, browser: Browser) -> PathBuf {
    home.join(browser.folder()).join(format!("{HOST_NAME}.json"))
}

/// The host manifest for `exe` (the fields of Chrome's native messaging docs).
pub fn manifest(exe: &Path) -> String {
    let manifest = json!({
        "name": HOST_NAME,
        "description": "Code View opens pull request links from the browser.",
        "path": exe.to_string_lossy(),
        "type": "stdio",
        "allowed_origins": [extension_origin()],
    });
    serde_json::to_string_pretty(&manifest).unwrap_or_default()
}

/// The manifest names this host, starts `exe` and lets the extension in.
fn is_ours(text: &str, exe: &Path) -> bool {
    let Ok(manifest) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    let origin = extension_origin();
    let is_named = manifest.get("name").and_then(Value::as_str) == Some(HOST_NAME);
    let starts_exe =
        manifest.get("path").and_then(Value::as_str).is_some_and(|path| Path::new(path) == exe);
    let lets_in =
        manifest.get("allowed_origins").and_then(Value::as_array).is_some_and(|origins| {
            origins.iter().any(|allowed| allowed.as_str() == Some(origin.as_str()))
        });
    is_named && starts_exe && lets_in
}

/// Writes through a temp file next to `path`, so a browser never reads half a manifest.
fn write_file(path: &Path, text: &str) -> io::Result<()> {
    let folder =
        path.parent().ok_or_else(|| io::Error::other("the manifest path has no folder"))?;
    std::fs::create_dir_all(folder)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path)
}

fn remove_file(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The registry in memory, for tests and for test builds of the Settings row.
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct MemoryKeys(std::sync::Mutex<std::collections::HashMap<String, String>>);

#[cfg(any(test, feature = "test-support"))]
impl MemoryKeys {
    fn keys(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, String>> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl KeyStore for MemoryKeys {
    fn value(&self, key: &str) -> io::Result<Option<String>> {
        Ok(self.keys().get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> io::Result<()> {
        self.keys().insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, key: &str) -> io::Result<()> {
        self.keys().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use serde_json::Value;

    use review_store::AppPaths;

    use cv_testkit::ScratchDir;

    use super::{Browser, KeyStore, MemoryKeys, Registry, Status, manifest};
    use crate::browser::{HOST_NAME, extension_dir, extension_files, extension_origin};

    fn exe(scratch: &ScratchDir) -> std::path::PathBuf {
        scratch.path().join("dist").join("CodeView.exe")
    }

    #[test]
    fn the_manifest_names_the_host_the_exe_and_the_extension() {
        let manifest: Value =
            serde_json::from_str(&manifest(Path::new("/opt/codeview/CodeView"))).expect("JSON");
        assert_eq!(manifest["name"], HOST_NAME);
        assert_eq!(manifest["path"], "/opt/codeview/CodeView");
        assert_eq!(manifest["type"], "stdio");
        assert_eq!(manifest["allowed_origins"], serde_json::json!([extension_origin()]));
    }

    #[test]
    fn keys_name_one_manifest_for_both_browsers() {
        let scratch = ScratchDir::new("registration-keys");
        let keys = Arc::new(MemoryKeys::default());
        let manifest = scratch.path().join("browser").join(format!("{HOST_NAME}.json"));
        let registry = Registry::Keys { manifest: manifest.clone(), keys: keys.clone() };
        let exe = exe(&scratch);
        assert_eq!(registry.status(&exe).expect("status"), Status::NotRegistered);

        registry.register(&exe).expect("registers");
        assert_eq!(registry.status(&exe).expect("status"), Status::Registered);
        for browser in Browser::ALL {
            assert_eq!(
                keys.value(&browser.key()).expect("key"),
                Some(manifest.to_string_lossy().into_owned())
            );
        }
        assert!(
            keys.value(r"Software\Microsoft\Edge\NativeMessagingHosts\de.crasius.codeview")
                .expect("key")
                .is_some(),
            "Edge's own key, which it reads before Chrome's"
        );

        // A moved executable: the registration names the old one.
        assert_eq!(
            registry.status(&scratch.path().join("CodeView.exe")).expect("status"),
            Status::Outdated
        );
        // One browser's key gone: only the other starts the host.
        keys.delete(&Browser::Edge.key()).expect("delete");
        assert_eq!(registry.status(&exe).expect("status"), Status::Outdated);

        registry.unregister().expect("unregisters");
        assert_eq!(registry.status(&exe).expect("status"), Status::NotRegistered);
        assert!(!manifest.exists(), "the manifest goes too");
        registry.unregister().expect("unregistering twice is fine");
    }

    #[test]
    fn folders_hold_a_manifest_per_browser() {
        let scratch = ScratchDir::new("registration-folders");
        let home = scratch.path().join("home");
        let registry = Registry::Folders { home: home.clone() };
        let exe = exe(&scratch);
        registry.register(&exe).expect("registers");
        assert_eq!(registry.status(&exe).expect("status"), Status::Registered);
        for browser in Browser::ALL {
            let file = home.join(browser.folder()).join("de.crasius.codeview.json");
            assert!(file.is_file(), "{}", file.display());
        }
        let chrome = home.join(Browser::Chrome.folder()).join("de.crasius.codeview.json");
        std::fs::write(&chrome, "{ broken").expect("corrupt");
        assert_eq!(registry.status(&exe).expect("status"), Status::Outdated);

        registry.unregister().expect("unregisters");
        assert_eq!(registry.status(&exe).expect("status"), Status::NotRegistered);
    }

    /// The live step: registers the build `CODEVIEW_EXE` names for the owner, in the real
    /// registration (HKCU keys and the manifest in the real cache folder on Windows), and unpacks
    /// the extension. `CODEVIEW_EXE=<exe> cargo test -p cv-shell registers_the_build -- --ignored`
    #[test]
    #[ignore = "writes the owner's browser registration; run by hand for the live step"]
    fn registers_the_build_for_the_owner() {
        let exe = std::env::var_os("CODEVIEW_EXE").expect("CODEVIEW_EXE names the build");
        let exe = std::path::PathBuf::from(exe);
        let paths = AppPaths::from_environment();
        let registry = Registry::system(&paths).expect("the registration");
        extension_files::unpack(&extension_dir(&paths)).expect("unpacks the extension");
        registry.register(&exe).expect("registers");
        assert_eq!(registry.status(&exe).expect("status"), Status::Registered);
    }

    /// Takes the live step's registration out again.
    /// `cargo test -p cv-shell unregisters_the_owners -- --ignored`
    #[test]
    #[ignore = "writes the owner's browser registration; run by hand after the live step"]
    fn unregisters_the_owners_registration() {
        let registry = Registry::system(&AppPaths::from_environment()).expect("the registration");
        registry.unregister().expect("unregisters");
        let exe = std::env::current_exe().expect("test binary");
        assert_eq!(registry.status(&exe).expect("status"), Status::NotRegistered);
    }
}
