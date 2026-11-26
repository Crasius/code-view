//! Image bodies read ahead, before any widget asks: the web images of the descriptions of the pull
//! requests the pointer is near in the list. Such a body stays in memory only, up to
//! [`AHEAD_BYTES`] and for [`AHEAD_SECONDS`]: most of them are never shown, so they never reach
//! the disk. The first load that asks for one takes it instead of downloading
//! ([`ImageStore::state`]), and keeps it on disk from then on like any image shown.

use std::collections::HashMap;
use std::sync::PoisonError;

use review_core::http::image::{self as remote, Body, Validators};

use super::{Fetch, ImageStore, MAX_BYTES, Phase, Target};

/// How long a body read ahead waits for a load to take it: our own number, the minutes to short
/// hours a list stays the way into the pull requests it shows.
pub const AHEAD_SECONDS: f64 = 30.0 * 60.0;
/// Memory for bodies read ahead: our own budget, a few dozen screenshots; the oldest go first.
pub const AHEAD_BYTES: usize = 32 * 1024 * 1024;

/// A body read ahead, and the input time it was asked for.
pub(super) struct Ahead {
    pub(super) image: remote::Image,
    pub(super) at: f64,
}

/// [`ImageStore::fetch_ahead`]: one body after the other. Skipped: images shown or loading
/// already, read ahead already, kept on disk, and videos. Failures are dropped: the load that
/// shows the image meets them itself.
pub(super) fn fetch(store: &ImageStore, now: f64, targets: Vec<Target>) {
    let wanted: Vec<Target> = {
        let mut inner = store.lock();
        let wanted: Vec<Target> = targets
            .into_iter()
            .filter(|target| target.video.is_none())
            .filter(|target| matches!(target.fetch, Fetch::Remote { disk: Some(_), .. }))
            .filter(|target| {
                inner.entries.get(&target.key).is_none_or(|entry| {
                    matches!(entry.phase, Phase::Idle) && entry.natural.is_none()
                })
            })
            .filter(|target| {
                !inner.ahead.contains_key(&target.key) && !inner.reading_ahead.contains(&target.key)
            })
            .collect();
        inner.reading_ahead.extend(wanted.iter().map(|target| target.key));
        wanted
    };
    if wanted.is_empty() {
        return;
    }
    let inner = store.inner.clone();
    store.spawner.spawn(async move {
        for target in wanted {
            let image = read_ahead(&target).await;
            let mut inner = inner.lock().unwrap_or_else(PoisonError::into_inner);
            inner.reading_ahead.remove(&target.key);
            if let Some(image) = image {
                inner.ahead.insert(target.key, Ahead { image, at: now });
                keep_ahead_within_budget(&mut inner.ahead);
            }
        }
    });
}

/// Bodies read ahead that wait for a load.
#[cfg(test)]
pub(super) fn waiting(store: &ImageStore) -> usize {
    store.lock().ahead.len()
}

/// The body of a remote image without a copy on disk, read ahead; `None` when it is kept on disk
/// already or did not come.
async fn read_ahead(target: &Target) -> Option<remote::Image> {
    let Fetch::Remote { url, owner, disk: Some(path) } = &target.fetch else {
        return None;
    };
    if path.exists() {
        return None;
    }
    let none = &Validators::NONE;
    let body = match owner {
        Some(owner) => owner.provider.fetch_image(&owner.repository, url, MAX_BYTES, none).await,
        None => remote::fetch(url, remote::IMAGE_ACCEPT, MAX_BYTES, none, &remote::Anonymous).await,
    };
    match body {
        Ok(Body::Image(image)) => Some(image),
        Ok(Body::NotModified { .. } | Body::TooLarge(_) | Body::Private) | Err(_) => None,
    }
}

/// Lets go of the bodies read ahead longest ago until they fit [`AHEAD_BYTES`].
fn keep_ahead_within_budget(ahead: &mut HashMap<u64, Ahead>) {
    let mut held: usize = ahead.values().map(|body| body.image.bytes.len()).sum();
    while held > AHEAD_BYTES {
        let oldest = ahead.iter().min_by(|a, b| a.1.at.total_cmp(&b.1.at)).map(|(key, _)| *key);
        let Some(body) = oldest.and_then(|key| ahead.remove(&key)) else {
            return;
        };
        held -= body.image.bytes.len();
    }
}
