//! Finding the user's clone of a pull request's repository: the clone pinned in Settings, else the
//! first clone in the search folders whose remote names the repository. Then a look at it
//! (`review_git::checkout::survey`): its branch and changes, the local branch the pull request
//! gets, whether that is checked out already, whether the pull request is from a fork. Also the
//! default search folders and the system's folder dialog, which test builds (and the
//! `test-support` feature) answer from a script.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
#[cfg(any(test, feature = "test-support"))]
use std::sync::{Arc, Mutex, PoisonError};

use review_core::ProviderError;
use review_git::checkout::{self, FoundClone, Survey};

use cv_runtime::background::{Background, Task, TaskPoll};
use cv_runtime::load::Load;
use cv_runtime::route::PullRequestKey;

/// What the lookup found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    NoClone,
    Clone(Survey),
}

/// What a folder picked in the system's dialog is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderPurpose {
    /// A new clone of the pull request's repository goes into this folder.
    CloneInto(PullRequestKey),
    /// An existing clone of the pull request's repository.
    PickClone(PullRequestKey),
    /// The folder a new worktree of the pull request goes into.
    WorktreeFolder(PullRequestKey),
    /// Another folder to search for clones.
    SearchFolder,
    /// The clone of a repository (its settings key), pinned in Settings.
    PinClone(String),
}

/// A repository a lookup ran for in this app run: its name and clone URL, for Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownRepository {
    pub name: String,
    pub clone_url: String,
    /// The clone the last lookup found.
    pub found: Option<PathBuf>,
}

/// Where to look and what for.
pub struct Query {
    pub clone_url: String,
    pub source_branch: String,
    /// The search folders from Settings; `None` for the defaults.
    pub folders: Option<Vec<PathBuf>>,
    pub pinned: Option<PathBuf>,
}

#[derive(Default)]
pub(super) struct Clones {
    lookups: HashMap<PullRequestKey, Load<Found>>,
    repositories: BTreeMap<String, KnownRepository>,
    defaults: Load<Vec<PathBuf>>,
    dialog: Option<(FolderPurpose, Task<Option<PathBuf>>)>,
    picked: Option<(FolderPurpose, PathBuf)>,
    #[cfg(any(test, feature = "test-support"))]
    answers: Arc<Mutex<std::collections::VecDeque<Option<PathBuf>>>>,
}

impl Clones {
    pub(super) fn found(&self, key: &PullRequestKey) -> Option<&Load<Found>> {
        self.lookups.get(key)
    }

    pub(super) fn repositories(&self) -> &BTreeMap<String, KnownRepository> {
        &self.repositories
    }

    /// Looks for the clone of `key`'s repository and at it.
    pub(super) fn look_up(
        &mut self,
        key: &PullRequestKey,
        repository: (String, String),
        query: Query,
        background: &Background,
    ) {
        let (settings_key, name) = repository;
        let known = self.repositories.entry(settings_key).or_insert_with(|| KnownRepository {
            name,
            clone_url: query.clone_url.clone(),
            found: None,
        });
        known.clone_url.clone_from(&query.clone_url);
        let number = key.number;
        let mut load = Load::Idle;
        load.start(background, async move {
            let found = find(query.pinned, query.folders, query.clone_url.clone()).await;
            match found {
                None => Ok(Found::NoClone),
                Some(clone) => checkout::survey(&clone, &query.source_branch, number)
                    .await
                    .map(Found::Clone)
                    .map_err(|error| ProviderError::Local(error.to_string())),
            }
        });
        self.lookups.insert(key.clone(), load);
    }

    /// The default search folders, looked for once.
    pub(super) fn defaults(&mut self, background: &Background) -> Option<&Vec<PathBuf>> {
        if self.defaults.is_idle() {
            self.defaults.start(background, async {
                tokio::task::spawn_blocking(default_folders)
                    .await
                    .map_err(|error| ProviderError::Local(error.to_string()))
            });
        }
        self.defaults.ready()
    }

    /// Opens the system's folder dialog for `purpose`, unless one is open already.
    pub(super) fn ask_folder(
        &mut self,
        purpose: FolderPurpose,
        title: &'static str,
        background: &Background,
    ) {
        if self.dialog.is_some() {
            return;
        }
        let task = self.dialog_task(title, background);
        self.dialog = Some((purpose, task));
    }

    /// The system's folder dialog, off the UI thread. Test builds have no way to open a dialog:
    /// they answer from the queue below.
    #[cfg(not(any(test, feature = "test-support")))]
    #[expect(
        clippy::unused_self,
        reason = "the test build's variant answers from the queue in self"
    )]
    fn dialog_task(&self, title: &'static str, background: &Background) -> Task<Option<PathBuf>> {
        // Taken on the UI thread, where the clicked window is the active one.
        let owner = cv_base::platform::dialog_owner();
        background.spawn(async move {
            tokio::task::spawn_blocking(move || cv_base::platform::ask_folder(owner, title))
                .await
                .ok()
                .flatten()
        })
    }

    /// Test builds answer with the next folder the test queued.
    #[cfg(any(test, feature = "test-support"))]
    fn dialog_task(&self, _title: &'static str, background: &Background) -> Task<Option<PathBuf>> {
        let answer =
            self.answers.lock().unwrap_or_else(PoisonError::into_inner).pop_front().flatten();
        background.spawn(async move { answer })
    }

    pub(super) fn is_asking(&self) -> bool {
        self.dialog.is_some()
    }

    /// The folder picked for `purpose`, once.
    pub(super) fn take_picked(&mut self, purpose: &FolderPurpose) -> Option<PathBuf> {
        if self.picked.as_ref().is_some_and(|(picked, _)| picked == purpose) {
            return self.picked.take().map(|(_, folder)| folder);
        }
        None
    }

    /// The next folder dialog answers `answer` (`None`: cancelled).
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn answer_folder_dialog(&self, answer: Option<PathBuf>) {
        self.answers.lock().unwrap_or_else(PoisonError::into_inner).push_back(answer);
    }

    pub(super) fn poll(&mut self) {
        for load in self.lookups.values_mut() {
            load.poll();
        }
        for (key, load) in &self.lookups {
            let settings_key = super::settings_key(key);
            if let (Some(found), Some(known)) =
                (load.ready(), self.repositories.get_mut(&settings_key))
            {
                known.found = match found {
                    Found::Clone(survey) => Some(survey.clone.clone()),
                    Found::NoClone => None,
                };
            }
        }
        self.defaults.poll();
        if let Some((_, task)) = &mut self.dialog {
            let picked = match task.poll() {
                TaskPoll::Pending => return,
                TaskPoll::Done(picked) => picked,
                TaskPoll::Lost => None,
            };
            let (purpose, _) = self.dialog.take().expect("polled just now");
            self.picked = picked.map(|folder| (purpose, folder));
        }
    }
}

/// The pinned clone when its remote still names the repository, else the first clone found in
/// the search folders.
async fn find(
    pinned: Option<PathBuf>,
    folders: Option<Vec<PathBuf>>,
    clone_url: String,
) -> Option<FoundClone> {
    tokio::task::spawn_blocking(move || {
        if let Some(dir) = pinned
            && let Some(remote) = checkout::matching_remote(&dir, &clone_url)
        {
            return Some(FoundClone { dir, remote });
        }
        let folders = folders.unwrap_or_else(default_folders);
        checkout::find_clones(&folders, &clone_url).into_iter().next()
    })
    .await
    .ok()
    .flatten()
}

/// The usual places for clones that exist on this computer: `source`, `src`, `dev`, `code`,
/// `projects`, `repos` and the like in the home folder and, on Windows, at the root of each drive.
#[cfg(not(any(test, feature = "test-support")))]
fn default_folders() -> Vec<PathBuf> {
    {
        const NAMES: [&str; 12] = [
            "source",
            "src",
            "dev",
            "Development",
            "code",
            "Code",
            "projects",
            "Projects",
            "repos",
            "git",
            "GitHub",
            "workspace",
        ];
        let mut roots: Vec<PathBuf> =
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(PathBuf::from)
                .into_iter()
                .collect();
        if cfg!(windows) {
            roots.extend((b'C'..=b'Z').map(|drive| PathBuf::from(format!("{}:\\", drive as char))));
        }
        let mut folders: Vec<PathBuf> = roots
            .iter()
            .flat_map(|root| NAMES.iter().map(move |name| root.join(name)))
            .filter(|folder| folder.is_dir())
            .collect();
        // Case-insensitive file systems list `Code` and `code` twice.
        folders.dedup_by(|first, second| checkout::same_folder(first, second));
        folders
    }
}

/// Test builds have no default search folders: they never search the real disk.
#[cfg(any(test, feature = "test-support"))]
fn default_folders() -> Vec<PathBuf> {
    Vec::new()
}
