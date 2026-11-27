//! Loads the images rendered Markdown embeds and keeps them as textures; videos load the same way
//! and show their first frame until they play ([`video`]).
//!
//! An image loads only when its widget is about to be seen ([`ImageStore::state`] with `want`),
//! while the window is active, and at most [`MAX_IN_FLIGHT`] at a time; one already loading is
//! never asked for twice. Remote images go through the connection's provider, which adds its
//! credentials for the connection's own hosts only (and signs private attachments right before
//! they load), or anonymously; repository files come from the local mirror. A private image the
//! connection cannot reach fails with the sign-in its connection needs. Bodies over
//! [`MAX_BYTES`] are refused, decoding runs off the UI thread ([`decode`]), and large images are
//! scaled down to [`MAX_TEXTURE_EDGE`]. A short animation keeps one texture per frame, a long one
//! plays in one texture a decoder streams into; either plays from its first frame once it is asked
//! for after a load. File previews decode and upload the same way ([`decode()`], [`upload`]). Textures live in memory up to
//! [`MEMORY_LIMIT`] (least recently drawn go first) and only while they are drawn: one nobody drew
//! for [`memory::UNSEEN_TEXTURE_SECONDS`] goes too ([`ImageStore::release_unseen`]). An image that
//! comes back loads again at the size it had, so nothing moves; remote bodies are also kept on
//! disk ([`disk`]), so that costs no download. A kept body past its host's lifetime is shown at
//! once all the same and asked about behind it with its validators: a `304` renews it, a changed
//! body replaces file and texture, and a failed question (offline, rate-limited) keeps it.
//!
//! Images may also be read ahead, before any widget asks ([`ahead`]): such a body waits in memory
//! for a while, and the first load that asks for it takes it instead of downloading.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use cv_base::platform::is_window_active;
use cv_base::theme::motion;
use cv_runtime::background::Spawner;
use cv_runtime::cache::memory::{self, Holder, TextureMemory};
use egui::{Context, TextureOptions, Vec2};
use review_core::http::image::{self as remote, Body, Validators};
use review_core::{Provider, ProviderError};
use review_git::GitMirror;

use crate::image_state::{ImageFailure, ImageState, Remedy, bump_image_generation};
use crate::picture::Picture;
use crate::video::Clip;

pub use decode::{Decoded, decode, full_pixels, svg_options};

use self::ahead::{AHEAD_SECONDS, Ahead};

/// Images larger than this are not downloaded or decoded.
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;
/// Longest texture edge; larger images are scaled down before upload (as file previews are).
pub const MAX_TEXTURE_EDGE: u32 = 2048;
/// Image loads running at once.
pub const MAX_IN_FLIGHT: usize = 4;
/// Texture memory kept for images drawn recently: our budget, about three of the largest textures
/// ([`MAX_TEXTURE_EDGE`] square, 21 MB with mipmaps) or a dozen full-window screenshots.
pub const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
/// Entries kept at most (sizes and failures of images not on screen).
const MAX_ENTRIES: usize = 4096;

/// The connection an image is loaded through: its provider and the repository whose texts embed it.
#[derive(Clone)]
pub struct Owner {
    pub connection_id: String,
    pub provider: Arc<dyn Provider>,
    pub repository: String,
}

/// Where one image comes from.
#[derive(Clone)]
pub enum Fetch {
    /// An absolute http(s) address as written: through `owner`'s provider (credentials for its own
    /// hosts only, signed addresses where the platform signs them), anonymously without one.
    /// `disk` keeps the body between runs ([`disk`]).
    Remote { url: String, owner: Option<Owner>, disk: Option<PathBuf> },
    /// `path` in `commit` of the local repository mirror.
    Git { git: Arc<GitMirror>, commit: String, path: String },
    /// Not loaded at all (a `file:` address, a path leaving the repository): this is why.
    Blocked(String),
}

/// One image to load: `key` identifies its content (equal keys share one entry and one load).
#[derive(Clone)]
pub struct Target {
    pub key: u64,
    pub fetch: Fetch,
    /// The host or repository path, for the failure card.
    pub place: String,
    /// For a video, the file its body is kept in for ffmpeg; `None` for an image.
    pub video: Option<PathBuf>,
}

/// The size an image showed at, kept after its texture was evicted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Natural {
    /// Raster images: one image pixel per screen pixel.
    Pixels(Vec2),
    /// SVG: its own size in CSS pixels, which are points.
    Points(Vec2),
}

impl Natural {
    fn points(self, pixels_per_point: f32) -> Vec2 {
        match self {
            Natural::Pixels(pixels) => pixels / pixels_per_point.max(0.1),
            Natural::Points(points) => points,
        }
    }
}

enum Phase {
    Idle,
    Loading,
    Ready(Picture),
    Failed(Arc<ImageFailure>),
}

struct Entry {
    phase: Phase,
    natural: Option<Natural>,
    /// The pass it was last asked for, for eviction.
    used: u64,
    /// Input time it was last asked for.
    seen: f64,
    /// Input time it was first asked for since it loaded: an animation plays from there.
    started: Option<f64>,
    /// A loaded video: what its player plays.
    clip: Option<Arc<Clip>>,
}

impl Entry {
    fn new(phase: Phase, pass: u64, now: f64) -> Self {
        Self { phase, natural: None, used: pass, seen: now, started: None, clip: None }
    }

    /// Lets go of the textures; the size stays, so the image comes back in the same place.
    fn release(&mut self, textures: &mut TextureMemory) {
        if let Phase::Ready(picture) = &self.phase {
            let_go(textures, picture);
            self.phase = Phase::Idle;
            self.started = None;
        }
    }
}

fn hold(textures: &mut TextureMemory, picture: &Picture) {
    picture.textures().iter().for_each(|texture| textures.add(texture));
}

fn let_go(textures: &mut TextureMemory, picture: &Picture) {
    picture.textures().iter().for_each(|texture| textures.remove(texture));
}

struct Inner {
    entries: HashMap<u64, Entry>,
    in_flight: usize,
    textures: TextureMemory,
    /// Images that failed as private, by key: the connection that did not reach them.
    denied: HashMap<u64, String>,
    /// Bodies read ahead that no load took yet, by key.
    ahead: HashMap<u64, Ahead>,
    /// Keys being read ahead right now.
    reading_ahead: HashSet<u64>,
}

pub struct ImageStore {
    inner: Arc<Mutex<Inner>>,
    spawner: Spawner,
}

impl Inner {
    fn new(memory_limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            in_flight: 0,
            textures: TextureMemory::new(Holder::Images, memory_limit),
            denied: HashMap::new(),
            ahead: HashMap::new(),
            reading_ahead: HashSet::new(),
        }
    }
}

impl ImageStore {
    pub fn new(spawner: Spawner) -> Self {
        Self { inner: Arc::new(Mutex::new(Inner::new(MEMORY_LIMIT))), spawner }
    }

    /// The image's state; with `want`, one not loaded yet starts loading when the window is
    /// active and fewer than [`MAX_IN_FLIGHT`] loads run.
    pub fn state(&self, ctx: &Context, target: &Target, want: bool) -> ImageState {
        let pass = ctx.cumulative_pass_nr();
        let now = ctx.input(|input| input.time);
        let pixels_per_point = ctx.pixels_per_point();
        let animates = !motion::is_reduced(ctx);
        let mut inner = self.lock();
        let is_new = !inner.entries.contains_key(&target.key);
        if is_new {
            if let Fetch::Blocked(reason) = &target.fetch {
                let failure = ImageFailure::new(reason.clone(), target.place.clone(), false);
                let entry = Entry::new(Phase::Failed(Arc::new(failure)), pass, now);
                inner.entries.insert(target.key, entry);
            } else {
                inner.entries.insert(target.key, Entry::new(Phase::Idle, pass, now));
            }
        }
        let may_start = want
            && inner.in_flight < MAX_IN_FLIGHT
            && ctx.input(|input| is_window_active(input.viewport()));
        let entry = inner.entries.get_mut(&target.key).expect("inserted above");
        entry.used = pass;
        entry.seen = now;
        let natural = entry.natural.map(|natural| natural.points(pixels_per_point));
        let state = match &entry.phase {
            Phase::Ready(picture) => {
                let started = *entry.started.get_or_insert(now);
                let (texture, next_frame) = picture.frame_at(animates.then_some(now - started));
                let size = natural.unwrap_or_else(|| picture.size_vec2() / pixels_per_point);
                ImageState::Ready { texture, size, next_frame, clip: entry.clip.clone() }
            }
            Phase::Failed(failure) => ImageState::Failed(failure.clone()),
            Phase::Idle if may_start => {
                entry.phase = Phase::Loading;
                inner.in_flight += 1;
                let ahead = inner.ahead.remove(&target.key).map(|ahead| ahead.image);
                self.start(ctx, target.clone(), pixels_per_point, ahead);
                ImageState::Waiting { size: natural }
            }
            Phase::Loading | Phase::Idle => ImageState::Waiting { size: natural },
        };
        if is_new && inner.entries.len() > MAX_ENTRIES {
            forget_oldest(&mut inner, pass);
        }
        if inner.textures.is_over_limit() {
            evict(&mut inner, pass);
        }
        state
    }

    /// Lets go of the textures nobody drew for [`memory::UNSEEN_TEXTURE_SECONDS`] up to input time
    /// `now` (the pages that showed them were left), and of the bodies read ahead longer than
    /// [`AHEAD_SECONDS`] ago. Call once per frame; it looks once a second.
    pub fn release_unseen(&self, now: f64) {
        let mut inner = self.lock();
        let Inner { entries, textures, ahead, .. } = &mut *inner;
        // Whether or not a texture is held: a list read ahead with nothing on screen.
        ahead.retain(|_, body| now - body.at < AHEAD_SECONDS);
        if !textures.is_sweep_due(now) {
            return;
        }
        for entry in entries.values_mut() {
            if now - entry.seen >= memory::UNSEEN_TEXTURE_SECONDS {
                entry.release(textures);
            }
        }
    }

    /// Forgets a failure, so the next wanted [`Self::state`] loads again.
    pub fn retry(&self, key: u64) {
        let mut inner = self.lock();
        if let Some(entry) = inner.entries.get_mut(&key)
            && matches!(entry.phase, Phase::Failed(_))
        {
            entry.phase = Phase::Idle;
        }
    }

    /// Forgets the failures of private images `connection_id` did not reach, after its settings
    /// changed: each loads once more the next time it is seen.
    pub fn retry_private(&self, connection_id: &str) {
        let mut inner = self.lock();
        let keys: Vec<u64> = inner
            .denied
            .iter()
            .filter(|(_, connection)| *connection == connection_id)
            .map(|(key, _)| *key)
            .collect();
        for key in keys {
            inner.denied.remove(&key);
            if let Some(entry) = inner.entries.get_mut(&key)
                && matches!(entry.phase, Phase::Failed(_))
            {
                entry.phase = Phase::Idle;
            }
        }
    }

    /// Reads the bodies of the remote `targets` ahead into memory, where they wait for a while for
    /// a load to take them ([`ahead`]); `now` is the input time.
    pub fn fetch_ahead(&self, now: f64, targets: Vec<Target>) {
        ahead::fetch(self, now, targets);
    }

    pub fn picture(&self, key: u64) -> Option<Picture> {
        match &self.lock().entries.get(&key)?.phase {
            Phase::Ready(picture) => Some(picture.clone()),
            _ => None,
        }
    }

    /// Loads `target`, from the body read `ahead` when there is one.
    fn start(
        &self,
        ctx: &Context,
        target: Target,
        pixels_per_point: f32,
        ahead: Option<remote::Image>,
    ) {
        let inner = self.inner.clone();
        let ctx = ctx.clone();
        self.spawner.spawn(async move {
            let mut result = load(&ctx, &target, pixels_per_point, ahead).await;
            let stale = result.as_mut().ok().and_then(|loaded| loaded.stale.take());
            settle(&inner, &target, result);
            bump_image_generation(&ctx);
            if let Some(kept) = stale
                && let Some(changed) = revalidate(&ctx, &target, kept, pixels_per_point).await
            {
                replace(&inner, target.key, changed);
                bump_image_generation(&ctx);
            }
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Ends a load: the texture or the failure goes into the entry, and its slot frees up.
fn settle(inner: &Mutex<Inner>, target: &Target, result: Result<Loaded, ImageFailure>) {
    let key = target.key;
    let mut inner = inner.lock().unwrap_or_else(PoisonError::into_inner);
    inner.in_flight = inner.in_flight.saturating_sub(1);
    let mut denied = None;
    let Inner { entries, textures, .. } = &mut *inner;
    if let Some(entry) = entries.get_mut(&key) {
        match result {
            Ok(Loaded { picture, natural, clip, .. }) => {
                hold(textures, &picture);
                entry.natural = Some(natural);
                entry.started = None;
                entry.clip = clip;
                entry.phase = Phase::Ready(picture);
            }
            Err(failure) => {
                if failure.remedy == Remedy::SignIn
                    && let Fetch::Remote { owner: Some(owner), .. } = &target.fetch
                {
                    denied = Some(owner.connection_id.clone());
                }
                entry.phase = Phase::Failed(Arc::new(failure));
            }
        }
    }
    match denied {
        Some(connection_id) => inner.denied.insert(key, connection_id),
        None => inner.denied.remove(&key),
    };
}

/// Shows the changed body a revalidation brought in place of the stale one, when the stale one is
/// still shown; one released meanwhile loads the new file the next time it is seen.
fn replace(inner: &Mutex<Inner>, key: u64, (picture, natural): (Picture, Natural)) {
    let mut inner = inner.lock().unwrap_or_else(PoisonError::into_inner);
    let Inner { entries, textures, .. } = &mut *inner;
    if let Some(entry) = entries.get_mut(&key)
        && let Phase::Ready(stale) = &entry.phase
    {
        let_go(textures, stale);
        hold(textures, &picture);
        entry.natural = Some(natural);
        entry.started = None;
        entry.phase = Phase::Ready(picture);
    }
}

/// Drops the textures drawn longest ago (none drawn this pass) until the memory fits again.
fn evict(inner: &mut Inner, pass: u64) {
    let mut ready: Vec<(u64, u64)> = inner
        .entries
        .iter()
        .filter(|(_, entry)| matches!(entry.phase, Phase::Ready(_)) && entry.used < pass)
        .map(|(key, entry)| (entry.used, *key))
        .collect();
    ready.sort_unstable();
    for (_, key) in ready {
        if !inner.textures.is_over_limit() {
            break;
        }
        if let Some(entry) = inner.entries.get_mut(&key) {
            entry.release(&mut inner.textures);
        }
    }
}

/// Forgets the entries asked for longest ago that hold no texture and no running load.
fn forget_oldest(inner: &mut Inner, pass: u64) {
    let mut idle: Vec<(u64, u64)> = inner
        .entries
        .iter()
        .filter(|(_, entry)| {
            matches!(entry.phase, Phase::Idle | Phase::Failed(_)) && entry.used < pass
        })
        .map(|(key, entry)| (entry.used, *key))
        .collect();
    idle.sort_unstable();
    let excess = inner.entries.len().saturating_sub(MAX_ENTRIES * 3 / 4);
    for (_, key) in idle.into_iter().take(excess) {
        inner.entries.remove(&key);
        inner.denied.remove(&key);
    }
}

/// A loaded image; `stale` is the kept body it came from when that is to be asked about. A
/// video's picture is its first frame, and `clip` what its player plays.
struct Loaded {
    picture: Picture,
    natural: Natural,
    stale: Option<disk::Kept>,
    clip: Option<Arc<Clip>>,
}

async fn load(
    ctx: &Context,
    target: &Target,
    pixels_per_point: f32,
    ahead: Option<remote::Image>,
) -> Result<Loaded, ImageFailure> {
    if let Some(file) = &target.video {
        return video::load(ctx, target, file).await;
    }
    let fail = |reason, can_retry| ImageFailure::new(reason, target.place.clone(), can_retry);
    let (bytes, content_type, kept_at, stale) = match (&target.fetch, ahead) {
        (Fetch::Blocked(reason), _) => return Err(fail(reason.clone(), false)),
        // Read ahead and now shown: kept on disk from here on, like a body just downloaded.
        (Fetch::Remote { disk, .. }, Some(image)) => {
            if let Some(path) = disk.as_deref() {
                let content_type = image.content_type.as_deref();
                disk::write(path, &image.bytes, content_type, &image.validators, image.max_age);
            }
            (image.bytes.to_vec(), image.content_type, disk.as_deref(), None)
        }
        (Fetch::Remote { url, owner, disk }, None) => match disk.as_deref().and_then(disk::read) {
            Some(kept) if kept.is_fresh => (kept.bytes, kept.content_type, disk.as_deref(), None),
            Some(kept) => {
                (kept.bytes.clone(), kept.content_type.clone(), disk.as_deref(), Some(kept))
            }
            None => {
                let known = &Validators::NONE;
                let fetched = fetch_remote(url, owner.as_ref(), disk.as_deref(), known, MAX_BYTES)
                    .await
                    .map_err(|failure| failure.card(&target.place))?;
                match fetched {
                    Fetched::Changed { bytes, content_type } => (bytes, content_type, None, None),
                    Fetched::Unchanged => {
                        return Err(fail("The host's answer could not be read".to_owned(), true));
                    }
                }
            }
        },
        (Fetch::Git { git, commit, path }, _) => {
            let bytes = git_body(git, commit, path, MAX_BYTES)
                .await
                .map_err(|(reason, can_retry)| fail(reason, can_retry))?;
            (bytes, None, None, None)
        }
    };
    match picture_of(ctx, target.key, bytes, content_type, pixels_per_point).await {
        Ok((picture, natural)) => Ok(Loaded { picture, natural, stale, clip: None }),
        Err(reason) => {
            if let Some(path) = kept_at {
                disk::remove(path);
            }
            Err(fail(reason, false))
        }
    }
}

/// The file `path` at `commit` of the mirror, up to `limit` bytes; else the failure card's reason
/// and whether a retry may help.
async fn git_body(
    git: &GitMirror,
    commit: &str,
    path: &str,
    limit: u64,
) -> Result<Vec<u8>, (String, bool)> {
    let short = review_core::short_sha(commit);
    let entry = match git.file_entry(commit, path).await {
        Ok(Some(entry)) => entry,
        Ok(None) => return Err((format!("Not in the repository at {short}"), false)),
        Err(error) => return Err((reason_of(&error), can_retry(&error))),
    };
    // The blob-less mirror knows a blob's size only once it is downloaded.
    match git.blob(&entry.oid).await {
        Ok(bytes) if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit => {
            Err((too_large(limit), false))
        }
        Ok(bytes) => Ok(bytes),
        Err(error) => Err((reason_of(&error), can_retry(&error))),
    }
}

/// Asks the host whether the stale body `kept` of `target` changed. A `304` renews the file; a
/// changed body replaces it and comes back decoded; the same body or a failed question changes
/// nothing on screen.
async fn revalidate(
    ctx: &Context,
    target: &Target,
    kept: disk::Kept,
    pixels_per_point: f32,
) -> Option<(Picture, Natural)> {
    let Fetch::Remote { url, owner, disk: Some(path) } = &target.fetch else {
        return None;
    };
    match fetch_remote(url, owner.as_ref(), Some(path), &kept.validators, MAX_BYTES).await {
        Ok(Fetched::Changed { bytes, content_type }) if bytes != kept.bytes => {
            picture_of(ctx, target.key, bytes, content_type, pixels_per_point).await.ok()
        }
        // Offline, rate-limited or refused: the copy shown stays, and the next load asks again.
        Ok(Fetched::Changed { .. } | Fetched::Unchanged) | Err(_) => None,
    }
}

/// Decodes a body off the UI thread and uploads its frames as the textures of image `key`.
async fn picture_of(
    ctx: &Context,
    key: u64,
    bytes: Vec<u8>,
    content_type: Option<String>,
    pixels_per_point: f32,
) -> Result<(Picture, Natural), String> {
    let decoded = tokio::task::spawn_blocking(move || {
        decode::decode(&bytes, content_type.as_deref(), pixels_per_point)
    })
    .await
    .map_err(|_| "Decoding stopped".to_owned())??;
    Ok(upload(ctx, &format!("markdown-image-{key:016x}"), decoded))
}

/// Uploads decoded frames as textures named `name` and the frame's index; a long animation gets
/// the one texture its stream plays in. The picture keeps the body for "Copy image".
pub fn upload(ctx: &Context, name: &str, decoded: Decoded) -> (Picture, Natural) {
    let options =
        TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..TextureOptions::LINEAR };
    let Decoded { frames, natural, stream, original } = decoded;
    let mut frames: Vec<_> = frames
        .into_iter()
        .enumerate()
        .map(|(index, (image, delay))| {
            (ctx.load_texture(format!("{name}-{index}"), image, options), delay)
        })
        .collect();
    let picture = match stream {
        Some(stream) => {
            Picture::streamed(frames.remove(0).0, decode::spawn_frames(stream), options)
        }
        None if frames.len() == 1 => Picture::still(frames.remove(0).0),
        None => Picture::animation(frames),
    };
    (picture.with_original(original), natural)
}

/// What a download brought.
enum Fetched {
    Changed {
        bytes: Vec<u8>,
        content_type: Option<String>,
    },
    /// The copy the validators named is current.
    Unchanged,
}

/// Why a download failed, before it becomes a failure card.
struct FetchFailure {
    reason: String,
    can_retry: bool,
    remedy: Remedy,
}

impl FetchFailure {
    fn new(reason: String, can_retry: bool) -> Self {
        Self { reason, can_retry, remedy: Remedy::None }
    }

    /// The card of an image asked from `place`.
    fn card(self, place: &str) -> ImageFailure {
        let Self { reason, can_retry, remedy } = self;
        ImageFailure { reason, place: place.to_owned(), can_retry, remedy }
    }
}

/// Downloads a remote image (up to `limit` bytes) through `owner`'s provider (anonymously without
/// one), or asks whether the copy `known` names changed, and keeps the answer in `disk` ([`disk`]).
async fn fetch_remote(
    url: &str,
    owner: Option<&Owner>,
    disk: Option<&Path>,
    known: &Validators,
    limit: u64,
) -> Result<Fetched, FetchFailure> {
    let body = match owner {
        Some(owner) => owner.provider.fetch_image(&owner.repository, url, limit, known).await,
        None => remote::fetch(url, remote::IMAGE_ACCEPT, limit, known, &remote::Anonymous).await,
    };
    match body {
        Ok(Body::Image(image)) => {
            if let Some(path) = disk {
                let content_type = image.content_type.as_deref();
                disk::write(path, &image.bytes, content_type, &image.validators, image.max_age);
            }
            Ok(Fetched::Changed { bytes: image.bytes.to_vec(), content_type: image.content_type })
        }
        Ok(Body::NotModified { max_age }) => {
            if let Some(path) = disk {
                disk::renew(path, max_age);
            }
            Ok(Fetched::Unchanged)
        }
        Ok(Body::TooLarge(_)) => Err(FetchFailure::new(too_large(limit), false)),
        Ok(Body::Private) => Err(FetchFailure {
            remedy: Remedy::SignIn,
            ..FetchFailure::new(PRIVATE.to_owned(), true)
        }),
        Err(error) => Err(FetchFailure::new(reason_of(&error), can_retry(&error))),
    }
}

/// The reason of an image the platform keeps behind a sign-in the connection does not have.
const PRIVATE: &str = "Sign-in needed: this image is private";

fn too_large(limit: u64) -> String {
    format!("Larger than {} MB", limit / (1024 * 1024))
}

/// The failure card's reason for an error; never the address or anything the request carried.
fn reason_of(error: &ProviderError) -> String {
    match error {
        ProviderError::Status { status: 404, .. } => "Not found (404)".to_owned(),
        ProviderError::Status { status: 401, .. } => "Sign-in required (401)".to_owned(),
        ProviderError::Status { status: 403, .. } => "Access denied (403)".to_owned(),
        ProviderError::Status { status, .. } => format!("The host answered {status}"),
        ProviderError::RateLimited { .. } => "Rate-limited; try again in a moment".to_owned(),
        ProviderError::HeadMoved => error.to_string(),
        ProviderError::Network(_) => "Could not reach the host".to_owned(),
        ProviderError::Decode(_) => "The host's answer could not be read".to_owned(),
        ProviderError::Unsupported(message) | ProviderError::Local(message) => message.clone(),
    }
}

fn can_retry(error: &ProviderError) -> bool {
    !matches!(error, ProviderError::Unsupported(_) | ProviderError::Status { status: 404, .. })
}

mod ahead;
mod decode;
mod disk;
mod video;

#[cfg(test)]
mod tests;
