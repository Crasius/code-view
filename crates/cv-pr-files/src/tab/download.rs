//! Download of a binary file: its head version (base for a deletion) goes into the user's
//! Downloads folder, under a name no file there has yet. A download that failed offers Retry (the
//! same file once more) and the Downloads folder.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use review_core::ProviderError;
use review_git::{ChangedFile, GitMirror};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt as _;

use cv_base::platform;
use cv_pr_session::session::PullRequestSession;
use cv_runtime::load::Load;
use cv_runtime::toasts::{self, ToastAction, WaysOut};
use cv_services::Services;

/// Where a failure toast's Retry asks for the failed download once more.
const RETRY_ID: &str = "files.download.retry";

/// The download running or just finished, one at a time.
#[derive(Default)]
pub(crate) struct Download {
    load: Load<PathBuf>,
    /// What runs or ran last, for Retry.
    blob: Option<Blob>,
}

/// A file version to save: the mirror holding it, its blob id and its file name.
#[derive(Clone)]
struct Blob {
    git: Arc<GitMirror>,
    oid: String,
    name: String,
}

impl Download {
    pub(crate) fn is_loading(&self) -> bool {
        self.load.is_loading()
    }

    /// Saves `file` from the mirror, unless a download is running already.
    pub(crate) fn start(
        &mut self,
        session: &PullRequestSession,
        services: &Services,
        file: &ChangedFile,
    ) {
        let Some(mirror) = session.mirror.ready() else {
            return;
        };
        let oid = if file.has_new_blob() { file.new_oid.clone() } else { file.old_oid.clone() };
        let name = file.file_name().to_owned();
        self.save(services, Blob { git: mirror.git.clone(), oid, name });
    }

    fn save(&mut self, services: &Services, blob: Blob) {
        if self.load.is_loading() {
            return;
        }
        self.blob = Some(blob.clone());
        let Blob { git, oid, name } = blob;
        self.load.start(&services.background, async move {
            let bytes = git.blob(&oid).await?;
            let folder = platform::downloads_dir()
                .ok_or_else(|| ProviderError::Local("the user profile folder is unknown".into()))?;
            Ok(write_new(&folder, &name, &bytes).await?)
        });
    }

    /// Starts the download a failure toast's Retry asked for, then says where a finished
    /// download went, or why it failed.
    pub(crate) fn poll(&mut self, services: &mut Services) {
        let ctx = services.background.ctx();
        if toasts::take_retry(&ctx, egui::Id::unique(RETRY_ID))
            && let Some(blob) = self.blob.clone()
        {
            self.save(services, blob);
        }
        match self.load.take_finished() {
            Some(Ok(path)) => services.toasts.success(format!("Saved to {}", path.display())),
            Some(Err(message)) => {
                let retry = ToastAction::retry(egui::Id::unique(RETRY_ID));
                let folder = platform::downloads_dir().map(ToastAction::open_folder);
                let ways_out = WaysOut::new(retry).and_some(folder);
                services.toasts.error(format!("Download failed: {message}"), ways_out);
            }
            None => {}
        }
    }
}

/// Writes `bytes` to `folder/name`, or to `name (2).ext` and so on when that file exists. Each name
/// is taken by creating its file, so a download of the same name beside this one never overwrites
/// it.
async fn write_new(folder: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    let mut candidate = folder.join(name);
    let mut counter = 2;
    let mut file = loop {
        let created = OpenOptions::new().write(true).create_new(true).open(&candidate).await;
        match created {
            Ok(file) => break file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                candidate = folder.join(format!("{stem} ({counter}){extension}"));
                counter += 1;
            }
            Err(error) => return Err(error),
        }
    };
    file.write_all(bytes).await?;
    file.flush().await?;
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::write_new;
    use cv_testkit::ScratchDir;

    #[test]
    fn a_download_takes_the_next_free_name_and_keeps_the_files_there() {
        let scratch = ScratchDir::new("download-names");
        let folder = scratch.path();
        std::fs::write(folder.join("logo.png"), b"there before").expect("a file in the folder");
        let runtime = tokio::runtime::Builder::new_current_thread().build().expect("a runtime");
        let save = |bytes: &'static [u8]| {
            runtime.block_on(write_new(folder, "logo.png", bytes)).expect("the download is saved")
        };
        assert_eq!(save(b"first"), folder.join("logo (2).png"));
        assert_eq!(save(b"second"), folder.join("logo (3).png"));
        let read = |name: &str| std::fs::read(folder.join(name)).expect("a saved file");
        assert_eq!(read("logo.png"), b"there before", "the file there before is kept");
        assert_eq!(read("logo (2).png"), b"first", "a later download keeps the earlier one");
        assert_eq!(read("logo (3).png"), b"second");
    }
}
