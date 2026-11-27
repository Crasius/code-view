//! Mermaid diagrams for rendered Markdown, drawn off the UI thread and outside the app's process.
//!
//! A diagram is asked for by its source and theme ([`DiagramStore::state`]); the first ask queues
//! it for the render thread, which hands it to the renderer process (`worker`) and turns the SVG
//! into a texture with resvg. Nothing is sent anywhere else: the renderer is pure Rust, in a
//! process of this app. Each step is cached: the SVG by source and theme, the texture by its
//! pixel scale, so a theme switch draws again, a zoom or a move to another screen only
//! rasterizes again, and scrolling costs nothing. The size a source drew at is remembered on its
//! own, so a diagram drawn again (another theme) holds its place meanwhile.
//!
//! Textures live in memory up to `MEMORY_LIMIT` (least recently drawn go first, their SVG kept)
//! and only while they are drawn: one nobody drew for [`memory::UNSEEN_TEXTURE_SECONDS`] goes too
//! ([`DiagramStore::release_unseen`]) and is rasterized again from its SVG when it is seen again.
//! Entries beyond `MAX_ENTRIES` are forgotten oldest first. The render thread, and with it the
//! renderer process, ends after `IDLE_EXIT` without work and starts again on the next diagram.

mod diagram;
mod worker;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use cv_runtime::cache::memory::{self, Holder, TextureMemory};
use egui::{ColorImage, Context, Id, TextureHandle, TextureOptions, Vec2};

use self::diagram::Svg;
pub use self::diagram::{DiagramFailure, DiagramTheme};
use self::worker::Worker;
pub use self::worker::serve_if_asked;
use crate::image_state::bump_image_generation;

/// How long one diagram may take in the renderer before it is stopped.
pub(crate) const RENDER_TIMEOUT: Duration = Duration::from_secs(5);
/// Texture memory kept for diagrams drawn recently: our budget, a page of ordinary diagrams (a
/// flowchart at 2x is a few MB). The one diagram drawn right now stays even when it is larger.
pub(crate) const MEMORY_LIMIT: usize = 48 * 1024 * 1024;
/// Diagrams kept at most (their SVG, size or failure).
const MAX_ENTRIES: usize = 512;
/// A diagram's texture is never larger than this, edge and area.
const MAX_TEXTURE_EDGE: f32 = 4096.0;
const MAX_TEXTURE_PIXELS: f32 = 16.0 * 1024.0 * 1024.0;
/// The render thread ends after this long without work.
const IDLE_EXIT: Duration = Duration::from_secs(30);

const STORE_ID: &str = "review.mermaid.store";

/// What a diagram is right now.
#[derive(Clone)]
pub enum DiagramState {
    /// Being drawn; `size` (points at zoom 1) when it is known already.
    Drawing {
        size: Option<Vec2>,
    },
    /// `size` is its own size in points at zoom 1. The texture may be of an earlier pixel scale
    /// while the current one is rasterized.
    Ready {
        texture: TextureHandle,
        size: Vec2,
    },
    Failed(Arc<DiagramFailure>),
}

/// One diagram as a document shows it.
#[derive(Clone, Copy)]
pub struct DiagramRequest<'a> {
    pub source: &'a str,
    pub theme: &'a DiagramTheme,
}

impl DiagramRequest<'_> {
    fn key(&self) -> u64 {
        egui::epaint::util::hash((self.source, self.theme))
    }

    fn source_key(&self) -> u64 {
        egui::epaint::util::hash(self.source)
    }
}

enum Phase {
    /// Waiting for the render thread.
    Queued,
    Drawn(Arc<Svg>),
    Failed(Arc<DiagramFailure>),
}

struct Entry {
    source: Arc<str>,
    theme: DiagramTheme,
    phase: Phase,
    /// The texture and the pixel scale (hundredths) it was rasterized at.
    texture: Option<(u32, TextureHandle)>,
    /// The pixel scale a raster job is queued or running for.
    rastering: Option<u32>,
    /// The pass it was last asked for, for eviction.
    used: u64,
    /// Input time it was last asked for.
    seen: f64,
}

enum Job {
    Draw(u64),
    Raster(u64, u32),
}

struct Inner {
    entries: HashMap<u64, Entry>,
    /// The size each source drew at, kept when its entry goes.
    sizes: HashMap<u64, Vec2>,
    jobs: VecDeque<Job>,
    is_running: bool,
    textures: TextureMemory,
}

pub struct DiagramStore {
    shared: Arc<(Mutex<Inner>, Condvar)>,
}

impl DiagramStore {
    /// The store of this app (one per egui context).
    pub fn of(ctx: &Context) -> Arc<Self> {
        ctx.data_mut(|data| {
            data.get_temp_mut_or_insert_with(Id::unique(STORE_ID), || {
                Arc::new(Self::new(MEMORY_LIMIT))
            })
            .clone()
        })
    }

    fn new(memory_limit: usize) -> Self {
        let inner = Inner {
            entries: HashMap::new(),
            sizes: HashMap::new(),
            jobs: VecDeque::new(),
            is_running: false,
            textures: TextureMemory::new(Holder::Diagrams, memory_limit),
        };
        Self { shared: Arc::new((Mutex::new(inner), Condvar::new())) }
    }

    /// Lets go of the textures nobody drew for [`memory::UNSEEN_TEXTURE_SECONDS`] (the pages that
    /// showed them were left), in the store of `ctx` if it has one. Call once per frame; it looks
    /// once a second.
    pub fn release_unseen(ctx: &Context) {
        let now = ctx.input(|input| input.time);
        let Some(store) = ctx.data(|data| data.get_temp::<Arc<Self>>(Id::unique(STORE_ID))) else {
            return;
        };
        let mut inner = store.lock();
        let Inner { entries, textures, .. } = &mut *inner;
        if !textures.is_sweep_due(now) {
            return;
        }
        let unseen =
            entries.values_mut().filter(|entry| now - entry.seen >= memory::UNSEEN_TEXTURE_SECONDS);
        for (_, texture) in unseen.filter_map(|entry| entry.texture.take()) {
            textures.remove(&texture);
        }
    }

    /// The diagram's state. A diagram not asked for before is queued for drawing. With
    /// `pixel_scale` (texture pixels per point at zoom 1: zoom × pixels per point) it is also
    /// rasterized at that scale; without it (a block measured out of view) only drawn, so its
    /// size is known.
    pub fn state(
        &self,
        ctx: &Context,
        request: &DiagramRequest<'_>,
        pixel_scale: Option<f32>,
    ) -> DiagramState {
        let pass = ctx.cumulative_pass_nr();
        let now = ctx.input(|input| input.time);
        let key = request.key();
        let mut inner = self.lock();
        let is_new = !inner.entries.contains_key(&key);
        if is_new {
            let failure = diagram::check_source(request.source).err();
            let phase = if let Some(failure) = failure {
                Phase::Failed(Arc::new(failure))
            } else {
                inner.jobs.push_back(Job::Draw(key));
                Phase::Queued
            };
            let entry = Entry {
                source: request.source.into(),
                theme: *request.theme,
                phase,
                texture: None,
                rastering: None,
                used: pass,
                seen: now,
            };
            inner.entries.insert(key, entry);
        }
        let known_size = inner.sizes.get(&request.source_key()).copied();
        let entry = inner.entries.get_mut(&key).expect("inserted above");
        entry.used = pass;
        entry.seen = now;
        let mut raster = None;
        let state = match &entry.phase {
            Phase::Queued => DiagramState::Drawing { size: known_size },
            Phase::Failed(failure) => DiagramState::Failed(failure.clone()),
            Phase::Drawn(svg) => {
                let wanted = pixel_scale.map(|scale| scale_key(texture_scale(svg.size, scale)));
                let has_wanted = entry.texture.as_ref().is_some_and(|(at, _)| Some(*at) == wanted);
                if let Some(wanted) =
                    wanted.filter(|wanted| !has_wanted && entry.rastering != Some(*wanted))
                {
                    entry.rastering = Some(wanted);
                    raster = Some(Job::Raster(key, wanted));
                }
                match &entry.texture {
                    Some((_, texture)) => {
                        DiagramState::Ready { texture: texture.clone(), size: svg.size }
                    }
                    None => DiagramState::Drawing { size: Some(svg.size) },
                }
            }
        };
        let has_work = is_new && matches!(state, DiagramState::Drawing { .. }) || raster.is_some();
        inner.jobs.extend(raster);
        if is_new && inner.entries.len() > MAX_ENTRIES {
            forget_oldest(&mut inner, pass);
        }
        if inner.textures.is_over_limit() {
            evict(&mut inner, pass);
        }
        if has_work {
            self.wake(&mut inner, ctx);
        }
        state
    }

    /// Draws a failed diagram again (a stopped renderer may manage it now).
    pub fn retry(&self, ctx: &Context, request: &DiagramRequest<'_>) {
        let key = request.key();
        let mut inner = self.lock();
        let Some(entry) = inner.entries.get_mut(&key) else {
            return;
        };
        if matches!(entry.phase, Phase::Failed(_)) {
            entry.phase = Phase::Queued;
            inner.jobs.push_back(Job::Draw(key));
            self.wake(&mut inner, ctx);
        }
    }

    /// Starts the render thread if it is not running, or tells it about new work.
    fn wake(&self, inner: &mut Inner, ctx: &Context) {
        if inner.is_running {
            self.shared.1.notify_one();
            return;
        }
        let shared = self.shared.clone();
        let ctx = ctx.clone();
        let spawned =
            std::thread::Builder::new().name("mermaid".into()).spawn(move || run(&shared, &ctx));
        match spawned {
            Ok(_) => inner.is_running = true,
            Err(error) => {
                // No thread, no drawing: every queued diagram fails with the reason.
                let failure = Arc::new(DiagramFailure::retryable(format!(
                    "Could not start drawing: {error}"
                )));
                for job in inner.jobs.drain(..) {
                    if let Job::Draw(key) = job
                        && let Some(entry) = inner.entries.get_mut(&key)
                    {
                        entry.phase = Phase::Failed(failure.clone());
                    }
                }
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.shared.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The texture scale for a diagram of `size` points at `pixel_scale`: as asked, but never more
/// than the texture limits allow.
fn texture_scale(size: Vec2, pixel_scale: f32) -> f32 {
    let longest = size.x.max(size.y).max(1.0);
    let area = (size.x * size.y).max(1.0);
    pixel_scale
        .clamp(0.25, 8.0)
        .min(MAX_TEXTURE_EDGE / longest)
        .min((MAX_TEXTURE_PIXELS / area).sqrt())
}

/// A pixel scale in hundredths, so tiny float differences do not rasterize again.
fn scale_key(scale: f32) -> u32 {
    (scale * 100.0).round().max(1.0) as u32
}

/// The render thread: takes jobs until none came for [`IDLE_EXIT`], keeping one renderer
/// process for all of them (a new one after a crash or a timeout).
fn run(shared: &(Mutex<Inner>, Condvar), ctx: &Context) {
    let mut worker: Option<Worker> = None;
    loop {
        let job = {
            let mut inner = shared.0.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if let Some(job) = inner.jobs.pop_front() {
                    break job;
                }
                let (next, timeout) =
                    shared.1.wait_timeout(inner, IDLE_EXIT).unwrap_or_else(PoisonError::into_inner);
                inner = next;
                if timeout.timed_out() && inner.jobs.is_empty() {
                    inner.is_running = false;
                    return;
                }
            }
        };
        match job {
            Job::Draw(key) => draw_job(shared, ctx, &mut worker, key),
            Job::Raster(key, scale) => raster_job(shared, ctx, key, scale),
        }
        ctx.request_repaint();
    }
}

fn draw_job(
    shared: &(Mutex<Inner>, Condvar),
    ctx: &Context,
    worker: &mut Option<Worker>,
    key: u64,
) {
    let Some((source, theme)) = shared
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entries
        .get(&key)
        .map(|entry| (entry.source.clone(), entry.theme))
    else {
        return;
    };
    let started = match worker.take() {
        Some(running) => Ok(running),
        None => Worker::start(),
    };
    let result =
        match started.map(|running| worker.insert(running).draw(&source, &theme, RENDER_TIMEOUT)) {
            Err(error) => Err(DiagramFailure::retryable(format!(
                "The diagram renderer did not start: {error}"
            ))),
            Ok(Ok(answer)) => answer,
            // Gone or stopped: the next diagram gets a new renderer.
            Ok(Err(failure)) => {
                *worker = None;
                Err(failure)
            }
        };
    let mut inner = shared.0.lock().unwrap_or_else(PoisonError::into_inner);
    let source_key = egui::epaint::util::hash(&*source);
    let mut size_changed = true;
    if let Ok(svg) = &result {
        size_changed = inner.sizes.insert(source_key, svg.size) != Some(svg.size);
    }
    if let Some(entry) = inner.entries.get_mut(&key) {
        entry.phase = match result {
            Ok(svg) => Phase::Drawn(Arc::new(svg)),
            Err(failure) => Phase::Failed(Arc::new(failure)),
        };
    }
    drop(inner);
    if size_changed {
        // Views that keep laid-out heights (the rendered diff) measure again.
        bump_image_generation(ctx);
    }
}

fn raster_job(shared: &(Mutex<Inner>, Condvar), ctx: &Context, key: u64, scale: u32) {
    let svg = match shared
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entries
        .get(&key)
        .map(|entry| &entry.phase)
    {
        Some(Phase::Drawn(svg)) => svg.clone(),
        _ => return,
    };
    let image = rasterize(&svg, scale as f32 / 100.0);
    let mut inner = shared.0.lock().unwrap_or_else(PoisonError::into_inner);
    let Inner { entries, textures, .. } = &mut *inner;
    let Some(entry) = entries.get_mut(&key) else {
        return;
    };
    if entry.rastering == Some(scale) {
        entry.rastering = None;
    }
    let replaced = match image {
        Ok(image) => {
            let options = TextureOptions {
                mipmap_mode: Some(egui::TextureFilter::Linear),
                ..TextureOptions::LINEAR
            };
            let texture = ctx.load_texture(format!("mermaid-{key:016x}"), image, options);
            textures.add(&texture);
            entry.texture.replace((scale, texture))
        }
        Err(failure) => {
            entry.phase = Phase::Failed(Arc::new(failure));
            entry.texture.take()
        }
    };
    if let Some((_, old)) = replaced {
        textures.remove(&old);
    }
}

/// The SVG at `scale` texture pixels per point.
fn rasterize(svg: &Svg, scale: f32) -> Result<ColorImage, DiagramFailure> {
    let tree =
        resvg::usvg::Tree::from_str(&svg.text, &crate::images::svg_options()).map_err(|error| {
            DiagramFailure::new(format!("The drawing could not be read: {error}"), None)
        })?;
    let width = ((svg.size.x * scale).ceil() as u32).max(1);
    let height = ((svg.size.y * scale).ceil() as u32).max(1);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| DiagramFailure::new("The drawing has no size", None))?;
    let tree_size = tree.size();
    let transform = resvg::tiny_skia::Transform::from_scale(
        width as f32 / tree_size.width(),
        height as f32 / tree_size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(ColorImage::from_rgba_premultiplied([width as usize, height as usize], pixmap.data()))
}

/// Drops the textures drawn longest ago (none drawn this pass) until the memory fits again.
fn evict(inner: &mut Inner, pass: u64) {
    let mut drawn: Vec<(u64, u64)> = inner
        .entries
        .iter()
        .filter(|(_, entry)| entry.texture.is_some() && entry.used < pass)
        .map(|(key, entry)| (entry.used, *key))
        .collect();
    drawn.sort_unstable();
    for (_, key) in drawn {
        if !inner.textures.is_over_limit() {
            break;
        }
        if let Some((_, texture)) =
            inner.entries.get_mut(&key).and_then(|entry| entry.texture.take())
        {
            inner.textures.remove(&texture);
        }
    }
}

/// Forgets the entries asked for longest ago that no job waits for.
fn forget_oldest(inner: &mut Inner, pass: u64) {
    let mut idle: Vec<(u64, u64)> = inner
        .entries
        .iter()
        .filter(|(_, entry)| {
            !matches!(entry.phase, Phase::Queued) && entry.rastering.is_none() && entry.used < pass
        })
        .map(|(key, entry)| (entry.used, *key))
        .collect();
    idle.sort_unstable();
    let excess = inner.entries.len().saturating_sub(MAX_ENTRIES * 3 / 4);
    for (_, key) in idle.into_iter().take(excess) {
        if let Some(entry) = inner.entries.remove(&key)
            && let Some((_, texture)) = entry.texture
        {
            inner.textures.remove(&texture);
        }
    }
    // The sizes outlive their entries, up to a point: then they start over.
    if inner.sizes.len() > MAX_ENTRIES * 4 {
        inner.sizes.clear();
    }
}

#[cfg(test)]
mod tests;
