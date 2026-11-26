//! The pull request's repository as the session reaches it: its git remote and its mirror.

use std::sync::Arc;

use cv_services::mirrors::MirrorRegistry;
use review_core::{GitRemote, Provider, ProviderError};
use review_git::GitMirror;
use tokio::sync::OnceCell;

/// The repository's remote and mirror, each worked out once by whoever asks first; later askers
/// wait for that answer. A failure stays until [`super::PullRequestSession::open_repository`] starts over
/// (Retry).
#[derive(Clone, Default)]
pub(super) struct Repository {
    remote: Arc<OnceCell<Result<GitRemote, ProviderError>>>,
    mirror: Arc<OnceCell<Result<Arc<GitMirror>, ProviderError>>>,
}

impl Repository {
    /// The clone URL, once the platform named it (no credentials: those travel in the
    /// authorization header).
    pub(super) fn clone_url(&self) -> Option<String> {
        let remote = self.remote.get()?.as_ref().ok()?;
        Some(remote.clone_url.clone())
    }

    pub(super) async fn remote(
        &self,
        provider: &dyn Provider,
        repository: &str,
    ) -> Result<GitRemote, ProviderError> {
        self.remote.get_or_init(|| provider.git_remote(repository)).await.clone()
    }

    pub(super) async fn mirror(
        &self,
        mirrors: &MirrorRegistry,
        remote: &GitRemote,
    ) -> Result<Arc<GitMirror>, ProviderError> {
        self.mirror.get_or_init(|| mirrors.open_unstamped(remote)).await.clone()
    }
}
