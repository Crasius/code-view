//! Download of a binary file: its head version (base for a deletion) goes into the user's
//! Downloads folder, under a name no file there has yet. A download that failed offers Retry (the
//! same file once more) and the Downloads folder.

use std::path::PathBuf;
use std::sync::Arc;

use review_core::ProviderError;
use review_git::{ChangedFile, GitMirror};

use cv_base::platform;
use cv_pr_session::session::PullRequestSession;
use cv_runtime::load::Load;
use cv_runtime::toasts::{self, ToastAction, WaysOut};
use cv_services::Services;

/// Where a failure toast's Retry asks for the failed download once more.
const RETRY_ID: &str = "files.download.retry";

/// The download running or just finished, one at a time.
#[derive(Default)]
pub struct Download {
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
    pub fn is_loading(&self) -> bool {
        self.load.is_loading()
    }

    /// Saves `file` from the mirror, unless a download is running already.
    pub fn start(&mut self, session: &PullRequestSession, services: &Services, file: &ChangedFile) {
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
            let target = download_target(&name)?;
            tokio::fs::write(&target, bytes).await?;
            Ok(target)
        });
    }

    /// Starts the download a failure toast's Retry asked for, then says where a finished
    /// download went, or why it failed.
    pub fn poll(&mut self, services: &mut Services) {
        let ctx = services.background.ctx();
        if toasts::take_retry(&ctx, egui::Id::unique(RETRY_ID))
            && let Some(blob) = self.blob.clone()
        {
            self.save(services, blob);
        }
        if !self.load.poll() {
            return;
        }
        match std::mem::take(&mut self.load) {
            Load::Ready(path) => services.toasts.success(format!("Saved to {}", path.display())),
            Load::Failed(message) => {
                let retry = ToastAction::retry(egui::Id::unique(RETRY_ID));
                let folder = platform::downloads_dir().map(ToastAction::open_folder);
                let ways_out = WaysOut::new(retry).and_some(folder);
                services.toasts.error(format!("Download failed: {message}"), ways_out);
            }
            Load::Idle | Load::Loading(_) => {}
        }
    }
}

/// `Downloads/name`, or `name (2).ext` and so on when that file exists.
fn download_target(name: &str) -> Result<PathBuf, ProviderError> {
    let folder = platform::downloads_dir()
        .ok_or_else(|| ProviderError::Local("the user profile folder is unknown".into()))?;
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    let mut candidate = folder.join(name);
    let mut counter = 2;
    while candidate.exists() {
        candidate = folder.join(format!("{stem} ({counter}){extension}"));
        counter += 1;
    }
    Ok(candidate)
}
