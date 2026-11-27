//! Loading a preview in the background: the file's versions from the mirror, decoded (images
//! and SVG to pixels, Markdown to text), and for Markdown the blobs of the repository images each
//! version shows, so an image that changed under the same source counts as a change, and the
//! rendered diff's model: both versions parsed and lined up, which takes long for a large file.

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{ColorImage, vec2};
use review_core::{ProviderError, ProviderKind};
use review_git::{ChangedFile, GitMirror};

use super::{Loaded, PreviewKind, Side, rendered_key, text_of};
use crate::markdown_diff::{MarkdownModel, RenderedImages};
use cv_markdown::{self as markdown, ImageBase, file_base};
use cv_media::image_clipboard::Original;
use cv_media::images::{self as image_store, Decoded, Natural};
use cv_pr_session::images;
use cv_runtime::background::Background;
use cv_runtime::load::Load;

/// Starts loading the two versions of the Markdown file `file` and their images from `git` for a
/// rendered diff outside the file view (a card of the folder summary). Its images resolve in the
/// compared (base, head) `commits`.
pub fn load_markdown(
    ctx: &egui::Context,
    git: Arc<GitMirror>,
    commits: Option<(String, String)>,
    platform: ProviderKind,
    background: &Background,
    file: &ChangedFile,
) -> Load<Arc<Loaded>> {
    let job = LoadJob::new(ctx, git, commits, platform, file, PreviewKind::Markdown);
    let mut load = Load::Idle;
    load.start(background, job.run());
    load
}

impl Loaded {
    #[cfg(any(test, feature = "test-support"))]
    /// A Markdown file's versions without images, as a test's fake source hands them out: the
    /// model is built right here, where the load builds it in the background.
    pub fn markdown(
        file: &ChangedFile,
        old: Option<&str>,
        new: Option<&str>,
        platform: ProviderKind,
    ) -> Self {
        let text = |text: Option<&str>| text.map(|text| Side::Text(text.to_owned()));
        let (key, images) = (rendered_key(file, 0), RenderedImages::default());
        let model = MarkdownModel::new(key, old, new, platform, &images);
        Self { old: text(old), new: text(new), images_key: 0, model: Some(Arc::new(model)) }
    }
}

/// What loading a preview takes: the file's blobs, and for Markdown where its images resolve. Made
/// on the UI thread, run in the background.
pub(super) struct LoadJob {
    git: Arc<GitMirror>,
    /// The file loaded, whose blobs and paths key the rendered diff ([`rendered_key`]).
    file: ChangedFile,
    /// The file's old and new path.
    paths: [String; 2],
    /// Markdown: the compared (base, head) commits its images resolve in.
    commits: Option<(String, String)>,
    bases: [Option<ImageBase>; 2],
    platform: ProviderKind,
    kind: PreviewKind,
}

impl LoadJob {
    /// `commits` are the compared (base, head) commits Markdown images resolve in.
    pub(super) fn new(
        ctx: &egui::Context,
        git: Arc<GitMirror>,
        commits: Option<(String, String)>,
        platform: ProviderKind,
        file: &ChangedFile,
        kind: PreviewKind,
    ) -> Self {
        // Relative images of the old version point into the base commit, of the new into the head.
        let paths = [file.old_path.clone().unwrap_or_else(|| file.path.clone()), file.path.clone()];
        let bases = match &commits {
            Some((base, head)) => {
                [Some(file_base(ctx, base, &paths[0])), Some(file_base(ctx, head, &paths[1]))]
            }
            None => [None, None],
        };
        Self { git, file: file.clone(), paths, commits, bases, platform, kind }
    }

    /// The file's versions decoded; for Markdown also their images and the rendered diff's model,
    /// parsed and lined up off the UI thread.
    pub(super) async fn run(self) -> Result<Arc<Loaded>, ProviderError> {
        let Self { git, file, paths, commits, bases, platform, kind } = self;
        let old_oid = file.has_old_blob().then(|| file.old_oid.clone());
        let new_oid = file.has_new_blob().then(|| file.new_oid.clone());
        let old = load_side(&git, old_oid, kind).await?;
        let new = load_side(&git, new_oid, kind).await?;
        if kind != PreviewKind::Markdown {
            return Ok(Arc::new(Loaded { old, new, images_key: 0, model: None }));
        }
        let blobs = match &commits {
            Some((base, head)) => {
                image_blobs(
                    &git,
                    [text_of(old.as_ref()), text_of(new.as_ref())],
                    [base.as_str(), head.as_str()],
                    &paths,
                    Some(platform),
                )
                .await
            }
            None => Default::default(),
        };
        let images_key = egui::epaint::util::hash((&paths, &blobs));
        let images = RenderedImages { bases, content: changed_images(blobs) };
        let key = rendered_key(&file, images_key);
        let built = tokio::task::spawn_blocking(move || {
            let (old_text, new_text) = (text_of(old.as_ref()), text_of(new.as_ref()));
            let model = MarkdownModel::new(key, old_text, new_text, platform, &images);
            Loaded { old, new, images_key, model: Some(Arc::new(model)) }
        });
        let loaded = built
            .await
            .map_err(|error| ProviderError::Local(format!("lining up stopped: {error}")))?;
        Ok(Arc::new(loaded))
    }
}

/// Blob ids of the repository images each version of a Markdown file shows, by source as
/// written, each version's at its own commit (`commits` are base and head, `paths` the file's old
/// and new path).
async fn image_blobs(
    git: &GitMirror,
    texts: [Option<&str>; 2],
    commits: [&str; 2],
    paths: &[String; 2],
    platform: Option<ProviderKind>,
) -> [BTreeMap<String, String>; 2] {
    let mut blobs: [BTreeMap<String, String>; 2] = Default::default();
    for side in 0..2 {
        let Some(text) = texts[side] else {
            continue;
        };
        for source in markdown::image_sources(text, platform) {
            let Some(path) = images::repository_file(&paths[side], &source) else {
                continue;
            };
            if let Some(entry) = git.file_entry(commits[side], &path).await.ok().flatten() {
                blobs[side].insert(source, entry.oid);
            }
        }
    }
    blobs
}

/// The images of `blobs` both versions show by the same source with other blobs: where they
/// differ the image changed although the text did not.
pub(super) fn changed_images(
    blobs: [BTreeMap<String, String>; 2],
) -> [BTreeMap<String, String>; 2] {
    let [old, new] = blobs;
    let mut content: [BTreeMap<String, String>; 2] = Default::default();
    for (source, old_blob) in old {
        if let Some(new_blob) = new.get(&source)
            && *new_blob != old_blob
        {
            content[1].insert(source.clone(), new_blob.clone());
            content[0].insert(source, old_blob);
        }
    }
    content
}

async fn load_side(
    git: &Arc<GitMirror>,
    oid: Option<String>,
    kind: PreviewKind,
) -> Result<Option<Side>, ProviderError> {
    let Some(oid) = oid else {
        return Ok(None);
    };
    let bytes = git.blob(&oid).await?;
    let side = tokio::task::spawn_blocking(move || decode(&bytes, kind))
        .await
        .map_err(|error| ProviderError::Local(format!("decoding stopped: {error}")))??;
    Ok(Some(side))
}

pub(super) fn decode(bytes: &[u8], kind: PreviewKind) -> Result<Side, ProviderError> {
    match kind {
        PreviewKind::Markdown => Ok(Side::Text(String::from_utf8_lossy(bytes).into_owned())),
        PreviewKind::Raster => decode_raster(bytes),
        PreviewKind::Svg => decode_svg(bytes),
    }
}

fn decode_raster(bytes: &[u8]) -> Result<Side, ProviderError> {
    let decoded = image_store::decode(bytes, None, 1.0)
        .map_err(|reason| ProviderError::Local(format!("cannot decode image: {reason}")))?;
    Ok(Side::Image(decoded))
}

/// SVG previews rasterize with their longest edge at this many pixels (within the texture cap).
const SVG_EDGE: f32 = 1024.0;

fn decode_svg(bytes: &[u8]) -> Result<Side, ProviderError> {
    // Never reads files or addresses an SVG refers to (`<image href>`).
    let tree = resvg::usvg::Tree::from_data(bytes, &image_store::svg_options())
        .map_err(|error| ProviderError::Local(format!("cannot parse SVG: {error}")))?;
    let size = tree.size();
    let longest = size.width().max(size.height()).max(1.0);
    let scale = SVG_EDGE.min(image_store::MAX_TEXTURE_EDGE as f32) / longest;
    let width = ((size.width() * scale).ceil() as u32).max(1);
    let height = ((size.height() * scale).ceil() as u32).max(1);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| ProviderError::Local("SVG has no drawable size".into()))?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let image =
        ColorImage::from_rgba_premultiplied([width as usize, height as usize], pixmap.data());
    let natural = Natural::Pixels(vec2(width as f32, height as f32));
    let original = Some(Original::new(bytes.into(), Some("image/svg+xml")));
    Ok(Side::Image(Decoded { frames: vec![(image, 0.0)], natural, stream: None, original }))
}
