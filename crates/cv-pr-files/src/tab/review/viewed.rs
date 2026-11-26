//! The Files tab's viewed marks: which file versions the user marked viewed in this pull request,
//! saved on every change (a whole folder in one save), and a version the file tree rebuilds its
//! marks by. Where the user left the pull request (tab and open file) is saved with them, so it
//! opens there again after a restart and retention keeps or drops both together.

use review_git::ChangedFile;
use review_store::{LastPlace, LastTab, ViewedFiles};

use cv_runtime::route::PullRequestKey;
use cv_runtime::toasts::ToastAction;
use cv_services::Services;

/// A viewed mark the tree, a key or a header switched this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewedToggle {
    /// The file at this index.
    File(usize),
    /// Every changed file below the folder at this path.
    Folder(String),
}

pub struct ViewedMarks {
    /// `None` when the saved marks could not be read: nothing shows as viewed, nothing is saved.
    store: Option<ViewedFiles>,
    /// Bumped with every change.
    version: u64,
}

impl ViewedMarks {
    /// The pull request's saved marks; a failure to read them shows as a toast.
    pub fn load(key: &PullRequestKey, services: &mut Services) -> Self {
        let store = match ViewedFiles::load(&services.paths, &key.connection_id, &key.storage_key())
        {
            Ok(viewed) => Some(viewed),
            Err(error) => {
                let folder = ToastAction::open_folder(services.paths.viewed_dir());
                services.toasts.error(format!("Could not read viewed files: {error}"), folder);
                None
            }
        };
        Self { store, version: 0 }
    }

    /// This version of `file` was marked viewed.
    pub fn is_viewed(&self, file: &ChangedFile) -> bool {
        self.store.as_ref().is_some_and(|store| store.is_viewed(&file.path, &file.new_oid))
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Where the user left the pull request, kept with its marks.
    pub fn last_place(&self) -> Option<&LastPlace> {
        self.store.as_ref().and_then(ViewedFiles::last_place)
    }

    /// Saves `tab` and `file` as where the user is, when that changed; a failure to save shows as
    /// a toast, once (the place is kept in memory and not tried again until it changes).
    pub fn remember_place(&mut self, services: &mut Services, tab: LastTab, file: Option<&str>) {
        let Some(store) = self.store.as_mut() else {
            return;
        };
        let is_saved = store
            .last_place()
            .is_some_and(|saved| saved.tab == tab && saved.file.as_deref() == file);
        if is_saved {
            return;
        }
        let place = LastPlace { tab, file: file.map(str::to_owned) };
        if let Err(error) = store.set_last_place(place) {
            let folder = ToastAction::open_folder(services.paths.viewed_dir());
            let text = format!("Could not save where you left this pull request: {error}");
            services.toasts.error(text, folder);
        }
    }

    /// Switches `file`'s mark and saves it; a failure to save shows as a toast.
    pub fn toggle(&mut self, services: &mut Services, file: &ChangedFile) {
        let is_viewed = self.is_viewed(file);
        self.set(services, std::slice::from_ref(file).iter(), !is_viewed);
    }

    /// Switches what `toggle` names: a folder becomes viewed unless all its files already are,
    /// then it becomes unviewed.
    pub fn apply(&mut self, services: &mut Services, files: &[ChangedFile], toggle: &ViewedToggle) {
        match toggle {
            ViewedToggle::File(index) => self.toggle(services, &files[*index]),
            ViewedToggle::Folder(folder) => {
                let below = || files.iter().filter(|file| cv_pr_tree::is_below(&file.path, folder));
                let is_viewed = !below().all(|file| self.is_viewed(file));
                self.set(services, below(), is_viewed);
            }
        }
    }

    /// Marks or unmarks `files` with one save; a failure to save shows as a toast.
    fn set<'a>(
        &mut self,
        services: &mut Services,
        files: impl Iterator<Item = &'a ChangedFile>,
        is_viewed: bool,
    ) {
        let Some(store) = self.store.as_mut() else {
            return;
        };
        let marks = files.map(|file| (file.path.as_str(), file.new_oid.as_str()));
        if let Err(error) = store.set_viewed_many(marks, is_viewed) {
            let folder = ToastAction::open_folder(services.paths.viewed_dir());
            services.toasts.error(format!("Could not save the viewed mark: {error}"), folder);
        }
        self.version += 1;
    }
}

#[cfg(test)]
mod tests {
    use review_core::FileCategory;
    use review_git::{ChangeKind, ChangedFile};
    use review_store::AppPaths;

    use super::{ViewedMarks, ViewedToggle};
    use cv_runtime::route::PullRequestKey;
    use cv_services::Services;
    use cv_testkit::ScratchDir;

    fn file(path: &str) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            old_path: None,
            kind: ChangeKind::Modified,
            old_oid: "1".repeat(40),
            new_oid: "2".repeat(40),
            additions: Some(1),
            deletions: Some(1),
            is_binary: false,
            category: FileCategory::Code,
            whitespace_only: false,
        }
    }

    /// A folder's mark marks every file below it, deeper folders too, until all are viewed; then
    /// it unmarks them. Files of a folder whose name only starts the same stay as they were.
    #[test]
    fn a_folder_marks_all_its_files_then_unmarks_them() {
        let scratch = ScratchDir::new("viewed-folder");
        let paths = AppPaths::new(scratch.path().join("config"), scratch.path().join("cache"));
        let mut services =
            Services::new(egui::Context::default(), paths, cv_providers::create_provider);
        let key = PullRequestKey {
            connection_id: "ado".into(),
            repository_key: "Shop/web".into(),
            number: 4,
        };
        let files = [file("src/a.rs"), file("src/app/b.rs"), file("src-old/c.rs"), file("x.md")];
        let mut marks = ViewedMarks::load(&key, &mut services);
        let viewed = |marks: &ViewedMarks| -> Vec<bool> {
            files.iter().map(|file| marks.is_viewed(file)).collect()
        };
        marks.apply(&mut services, &files, &ViewedToggle::File(0));
        assert_eq!(viewed(&marks), [true, false, false, false]);

        let src = ViewedToggle::Folder("src".into());
        marks.apply(&mut services, &files, &src);
        assert_eq!(viewed(&marks), [true, true, false, false], "some viewed: all become viewed");
        assert_eq!(
            viewed(&ViewedMarks::load(&key, &mut services)),
            [true, true, false, false],
            "saved"
        );
        marks.apply(&mut services, &files, &src);
        assert_eq!(viewed(&marks), [false, false, false, false], "all viewed: all are unmarked");
        assert_eq!(marks.version(), 3, "one change per click");
    }
}
