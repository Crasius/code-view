//! Turns an image body into texture images, off the UI thread: raster formats through `image`,
//! animations (GIF, animated WebP and PNG) frame by frame, SVG through resvg.
//!
//! An animation whose frames together take at most [`ANIMATION_BYTES`] keeps them all. A longer
//! one keeps only its first frame and is decoded again, one frame after another, while it plays
//! ([`spawn_frames`]): it shows at its full size however long it is.
//!
//! Every decoded body keeps its [`Original`], which "Copy image" decodes again at its full
//! resolution ([`full_pixels`]).

use std::io::Cursor;
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};

use egui::{ColorImage, vec2};
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::imageops::FilterType;
use image::{AnimationDecoder as _, Frames, ImageDecoder as _, ImageError, ImageFormat, RgbaImage};

use super::{MAX_TEXTURE_EDGE, Natural};
use crate::image_clipboard::{Original, Pixels};
use crate::picture::Frames as FrameFeed;

/// Pixel limits for decoding: larger images are refused before memory is allocated for them.
const MAX_DIMENSION: u32 = 16_384;
const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;
/// RGBA bytes all frames of one animation take at most when they are kept: our budget, half of
/// [`super::MEMORY_LIMIT`] once the mipmaps are added, so a few other images stay beside it.
pub(super) const ANIMATION_BYTES: usize = 24 * 1024 * 1024;
/// Decoded frames waiting for a streamed animation: a little slack, little memory.
const QUEUED_FRAMES: usize = 2;

/// A decoded body: one frame for a still image, each frame with its delay in seconds for an
/// animation, and the size it shows at. A long animation has its first frame only, and `stream`
/// to play it. `original` is the body, for "Copy image".
#[derive(Clone)]
pub struct Decoded {
    pub frames: Vec<(ColorImage, f64)>,
    pub natural: Natural,
    pub stream: Option<Stream>,
    pub original: Option<Original>,
}

/// A long animation's body, decoded again while it plays.
#[derive(Clone)]
pub struct Stream {
    bytes: Arc<[u8]>,
    format: ImageFormat,
}

/// Decodes a body into texture images no larger than [`MAX_TEXTURE_EDGE`] and the size it shows at.
pub fn decode(
    bytes: &[u8],
    content_type: Option<&str>,
    pixels_per_point: f32,
) -> Result<Decoded, String> {
    let original = Original::new(bytes.into(), content_type);
    let head = head(bytes);
    let mut decoded = if is_svg(&head, content_type) {
        decode_svg(bytes, pixels_per_point)?
    } else if head.starts_with("<!doctype html") || head.starts_with("<html") {
        return Err("The host sent a web page, not an image".to_owned());
    } else {
        decode_raster(&original.bytes, ANIMATION_BYTES)?
    };
    decoded.original = Some(original);
    Ok(decoded)
}

/// The full-resolution pixels of `original`, never scaled down to a texture: a raster image's own
/// pixels (an animation's first frame), an SVG drawn at `pixels_per_point`.
pub fn full_pixels(original: &Original, pixels_per_point: f32) -> Result<Pixels, String> {
    let bytes = &original.bytes;
    if is_svg(&head(bytes), original.content_type.as_deref()) {
        let (tree, natural) = svg_tree(bytes)?;
        let longest = natural.x.max(natural.y).max(1.0);
        let scale = pixels_per_point.max(0.5).min(MAX_DIMENSION as f32 / longest);
        let pixmap = draw_svg(&tree, natural, scale)?;
        let rgba = pixmap
            .pixels()
            .iter()
            .flat_map(|pixel| {
                let color = pixel.demultiply();
                [color.red(), color.green(), color.blue(), color.alpha()]
            })
            .collect();
        let (width, height) = (pixmap.width() as usize, pixmap.height() as usize);
        return Ok(Pixels { width, height, rgba });
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "Unsupported image format".to_owned())?;
    reader.limits(limits());
    let image = reader.decode().map_err(|error| reason(&error))?.into_rgba8();
    let (width, height) = (image.width() as usize, image.height() as usize);
    Ok(Pixels { width, height, rgba: image.into_raw() })
}

/// The start of a body, lowercased, for telling SVG and web pages from raster images.
fn head(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(512)])
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_lowercase()
}

fn is_svg(head: &str, content_type: Option<&str>) -> bool {
    content_type.is_some_and(|kind| kind.contains("svg"))
        || head.starts_with("<svg")
        || (head.starts_with("<?xml") && head.contains("<svg"))
}

fn limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    limits
}

fn reason(error: &ImageError) -> String {
    match error {
        ImageError::Limits(_) => format!("Larger than {MAX_DIMENSION} × {MAX_DIMENSION} pixels"),
        ImageError::Unsupported(_) => "Unsupported image format".to_owned(),
        _ => "The image is damaged".to_owned(),
    }
}

/// A raster body; an animation of two or more frames keeps them all within `budget` bytes, else it
/// is streamed.
pub(super) fn decode_raster(bytes: &Arc<[u8]>, budget: usize) -> Result<Decoded, String> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "Unsupported image format".to_owned())?;
    let Some(format) = reader.format() else {
        return Err("Unsupported image format".to_owned());
    };
    if let Some(animation) = decode_animation(bytes, format, budget)? {
        return Ok(animation);
    }
    let mut reader = reader;
    reader.limits(limits());
    let image = reader.decode().map_err(|error| reason(&error))?;
    let natural = Natural::Pixels(vec2(image.width() as f32, image.height() as f32));
    let image = if image.width().max(image.height()) > MAX_TEXTURE_EDGE {
        image.thumbnail(MAX_TEXTURE_EDGE, MAX_TEXTURE_EDGE)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(Decoded {
        frames: vec![(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()), 0.0)],
        natural,
        stream: None,
        original: None,
    })
}

/// The frames of an animated body, or `None` when the format has none.
fn frames_of(bytes: &[u8], format: ImageFormat) -> Result<Option<Frames<'_>>, ImageError> {
    let cursor = Cursor::new(bytes);
    let frames = match format {
        ImageFormat::Gif => {
            let mut decoder = GifDecoder::new(cursor)?;
            decoder.set_limits(limits())?;
            decoder.into_frames()
        }
        ImageFormat::WebP => {
            let mut decoder = WebPDecoder::new(cursor)?;
            if !decoder.has_animation() {
                return Ok(None);
            }
            decoder.set_limits(limits())?;
            decoder.into_frames()
        }
        ImageFormat::Png => {
            let mut decoder = PngDecoder::new(cursor)?;
            if !decoder.is_apng()? {
                return Ok(None);
            }
            decoder.set_limits(limits())?;
            decoder.apng()?.into_frames()
        }
        _ => return Ok(None),
    };
    Ok(Some(frames))
}

/// All frames of an animation, or its first frame and a [`Stream`] when they would take more than
/// `budget` bytes; `None` for a single frame, which decodes as a still image.
fn decode_animation(
    bytes: &Arc<[u8]>,
    format: ImageFormat,
    budget: usize,
) -> Result<Option<Decoded>, String> {
    let Some(frames) = frames_of(bytes, format).map_err(|error| reason(&error))? else {
        return Ok(None);
    };
    let (mut kept, mut held, mut is_long) = (Vec::new(), 0, false);
    for frame in frames {
        let frame = frame.map_err(|error| reason(&error))?;
        held += frame.buffer().as_raw().len();
        kept.push(frame);
        if held > budget {
            // Long: what plays it is the stream; the first frame shows until it starts.
            is_long = true;
            kept.truncate(1);
            break;
        }
    }
    if kept.len() < 2 && !is_long {
        return Ok(None);
    }
    let canvas = kept.first().map(|frame| frame.buffer().dimensions()).unwrap_or_default();
    let natural = Natural::Pixels(vec2(canvas.0 as f32, canvas.1 as f32));
    let frames = kept.into_iter().map(texture_frame).collect();
    let stream = is_long.then(|| Stream { bytes: bytes.clone(), format });
    Ok(Some(Decoded { frames, natural, stream, original: None }))
}

/// Decodes `stream` over and over on a thread of its own, handing over each frame with its delay;
/// the thread ends when nobody takes them any more, or at a damaged frame.
pub fn spawn_frames(stream: Stream) -> FrameFeed {
    let (sender, frames) = mpsc::sync_channel(QUEUED_FRAMES);
    let decoder = move || {
        loop {
            let Ok(Some(frames)) = frames_of(&stream.bytes, stream.format) else { return };
            let mut delivered = false;
            for frame in frames {
                let Ok(frame) = frame else { return };
                if sender.send(texture_frame(frame)).is_err() {
                    return;
                }
                delivered = true;
            }
            if !delivered {
                return;
            }
        }
    };
    // A thread that does not start leaves the animation on its first frame.
    let _still = std::thread::Builder::new().name("animation-frames".to_owned()).spawn(decoder);
    frames
}

/// `canvas` within [`MAX_TEXTURE_EDGE`], keeping its aspect.
fn fitted((width, height): (u32, u32)) -> (u32, u32) {
    let scale = (f64::from(MAX_TEXTURE_EDGE) / f64::from(width.max(height).max(1))).min(1.0);
    let scaled = |side: u32| ((f64::from(side) * scale).floor() as u32).max(1);
    (scaled(width), scaled(height))
}

/// A frame as a texture image within [`MAX_TEXTURE_EDGE`], and its delay in seconds.
fn texture_frame(frame: image::Frame) -> (ColorImage, f64) {
    let (numerator, denominator) = frame.delay().numer_denom_ms();
    let delay = f64::from(numerator) / f64::from(denominator.max(1)) / 1000.0;
    let buffer = frame.into_buffer();
    let (width, height) = fitted(buffer.dimensions());
    let buffer: RgbaImage = if buffer.dimensions() == (width, height) {
        buffer
    } else {
        image::imageops::resize(&buffer, width, height, FilterType::Triangle)
    };
    let size = [width as usize, height as usize];
    (ColorImage::from_rgba_unmultiplied(size, buffer.as_raw()), delay)
}

fn decode_svg(bytes: &[u8], pixels_per_point: f32) -> Result<Decoded, String> {
    let (tree, natural) = svg_tree(bytes)?;
    let longest = natural.x.max(natural.y).max(1.0);
    let scale = pixels_per_point.max(0.5).min(MAX_TEXTURE_EDGE as f32 / longest);
    let pixmap = draw_svg(&tree, natural, scale)?;
    let size = [pixmap.width() as usize, pixmap.height() as usize];
    let image = ColorImage::from_rgba_premultiplied(size, pixmap.data());
    let natural = Natural::Points(natural);
    Ok(Decoded { frames: vec![(image, 0.0)], natural, stream: None, original: None })
}

/// The parsed SVG and its own size in CSS pixels.
fn svg_tree(bytes: &[u8]) -> Result<(resvg::usvg::Tree, egui::Vec2), String> {
    let tree = resvg::usvg::Tree::from_data(bytes, &svg_options())
        .map_err(|_| "The SVG is damaged".to_owned())?;
    let size = tree.size();
    Ok((tree, vec2(size.width(), size.height())))
}

/// The SVG of `natural` size drawn at `scale` pixels per CSS pixel.
fn draw_svg(
    tree: &resvg::usvg::Tree,
    natural: egui::Vec2,
    scale: f32,
) -> Result<resvg::tiny_skia::Pixmap, String> {
    let width = ((natural.x * scale).ceil() as u32).max(1);
    let height = ((natural.y * scale).ceil() as u32).max(1);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| "The SVG has no drawable size".to_owned())?;
    resvg::render(
        tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

/// SVG options that never reach outside the document: `<image href>` to a file or an address is
/// not loaded (only embedded `data:` images are drawn), and text is set in the system's fonts,
/// loaded once.
pub fn svg_options() -> resvg::usvg::Options<'static> {
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    let fonts = FONTS.get_or_init(|| {
        let mut database = resvg::usvg::fontdb::Database::new();
        database.load_system_fonts();
        Arc::new(database)
    });
    let mut options = resvg::usvg::Options::default();
    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
    options.fontdb = fonts.clone();
    "Arial".clone_into(&mut options.font_family);
    options
}
