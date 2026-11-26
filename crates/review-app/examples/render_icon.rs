//! Rasterizes the Code View app icon from its SVG masters into the committed `.ico`, `.icns` and
//! `.png` files, plus a light/dark preview sheet. A dev tool: the app itself only embeds the
//! results (the `.ico` in `CodeView.exe`, the `.icns` in the macOS app bundle).
//!
//! ```text
//! cargo run -p review-app --example render_icon
//! cargo run -p review-app --example render_icon -- --compare a.svg dir-b   # preview only
//! ```
//!
//! Each size comes from the smallest artwork in [`ARTWORKS`] drawn for it: optional pixel-fitted
//! variants for 16 to 32 px, the master `codeview.svg` (required) for the rest. Artworks may have
//! a transparent background. `cv_base::platform::windows::app_mark` picks the same way.
//! `--compare` takes master files (every size from that one SVG) or folders laid out like
//! `assets/icon`. The preview shows each size on the Windows light and dark backgrounds in
//! `target/live/app-identity/preview.png`.
//!
//! The `.icns` holds PNG images on Apple's icon grid: the artwork takes the middle 824 of 1024
//! points, so the icon lines up with the system's icons in the Dock and the Finder.
//! A solid rounded tile sits behind the mark on macOS. The runtime PNG uses the same renderer
//! because eframe sets the Dock and Command-Tab icon when the app starts.

use std::path::{Path, PathBuf};

use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage, imageops};

const ICO_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];
/// The `.icns` entries: (type, pixel size). `ic11` to `ic14` and `ic10` are the Retina (@2x)
/// images of the size half theirs.
const ICNS_ENTRIES: [(&[u8; 4], u32); 11] = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"ic11", 32),
    (b"icp6", 64),
    (b"ic12", 64),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic13", 256),
    (b"ic09", 512),
    (b"ic14", 512),
    (b"ic10", 1024),
];
/// The share of a macOS icon's canvas the artwork takes (Apple's grid: 824 of 1024).
const MAC_ART_SHARE: f32 = 824.0 / 1024.0;
const PNG_SIZES: [u32; 4] = [32, 64, 256, 512];
/// Sizes shown 1:1 in the preview, then the small ones magnified to check single pixels.
const PREVIEW_SIZES: [u32; 6] = [16, 20, 24, 32, 48, 256];
const MAGNIFIED: [(u32, u32); 3] = [(16, 8), (24, 6), (32, 4)];
/// Windows 11 taskbar and Explorer backgrounds in light and dark mode.
const BACKGROUNDS: [[u8; 3]; 2] = [[0xF3, 0xF3, 0xF3], [0x20, 0x20, 0x20]];
const GAP: u32 = 24;
/// The largest pixel size each artwork is drawn for, smallest first.
const ARTWORKS: [(u32, &str); 4] = [
    (20, "codeview-16.svg"),
    (24, "codeview-24.svg"),
    (32, "codeview-32.svg"),
    (u32::MAX, "codeview.svg"),
];

#[expect(clippy::print_stdout, reason = "a dev tool that says what it wrote")]
fn main() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = crate_dir
        .parent()
        .and_then(Path::parent)
        .expect("crate lives in <workspace>/crates/<name>");
    let preview_path = workspace.join("target/live/app-identity/preview.png");
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--compare") {
        let marks: Vec<Mark> = args[1..]
            .iter()
            .map(|path| {
                let path = Path::new(path);
                if path.is_dir() {
                    Mark::load(path)
                } else {
                    Mark { artworks: vec![(u32::MAX, parse(path))] }
                }
            })
            .collect();
        write_preview(&marks, &preview_path);
        return;
    }

    let icon_dir = crate_dir.join("assets/icon");
    let mark = Mark::load(&icon_dir);
    let frames: Vec<IcoFrame<'static>> = ICO_SIZES
        .iter()
        .map(|&size| {
            let image = mark.render(size);
            IcoFrame::as_png(image.as_raw(), size, size, ExtendedColorType::Rgba8)
                .expect("encode an icon frame")
        })
        .collect();
    let ico = std::fs::File::create(icon_dir.join("codeview.ico")).expect("create codeview.ico");
    IcoEncoder::new(ico).encode_images(&frames).expect("write codeview.ico");
    for size in PNG_SIZES {
        mark.render(size).save(icon_dir.join(format!("codeview-{size}.png"))).expect("write a PNG");
    }
    write_icns(&mark, &icon_dir.join("codeview.icns"));
    std::fs::write(icon_dir.join("codeview-macos-512.png"), mac_png(&mark, 512))
        .expect("write macOS runtime icon");
    write_preview(&[mark], &preview_path);
    println!("wrote {} and {}", icon_dir.display(), preview_path.display());
}

/// The icon artworks of one folder, paired with the largest size each is drawn for.
struct Mark {
    artworks: Vec<(u32, resvg::usvg::Tree)>,
}

impl Mark {
    /// The artworks present in `dir`; only the master must exist.
    fn load(dir: &Path) -> Self {
        let artworks = ARTWORKS
            .iter()
            .map(|&(max, file)| (max, dir.join(file)))
            .filter(|(max, path)| *max == u32::MAX || path.exists())
            .map(|(max, path)| (max, parse(&path)))
            .collect();
        Self { artworks }
    }

    #[expect(clippy::cast_precision_loss, reason = "icon sizes stay at 1024 px and below")]
    fn render(&self, size: u32) -> RgbaImage {
        let (_, tree) = self
            .artworks
            .iter()
            .find(|(max, _)| size <= *max)
            .expect("the master covers every size");
        // Fit the longer side and center the other, so a non-square master stays undistorted.
        let (width, height) = (tree.size().width(), tree.size().height());
        let scale = size as f32 / width.max(height);
        let offset = |side: f32| (size as f32 - side * scale) / 2.0;
        let transform = resvg::tiny_skia::Transform::from_scale(scale, scale)
            .post_translate(offset(width), offset(height));
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).expect("icon size is positive");
        resvg::render(tree, transform, &mut pixmap.as_mut());
        let mut image = RgbaImage::new(size, size);
        for (pixel, source) in image.pixels_mut().zip(pixmap.pixels()) {
            let color = source.demultiply();
            *pixel = Rgba([color.red(), color.green(), color.blue(), color.alpha()]);
        }
        image
    }
}

/// The macOS icon file: the `icns` header, then one PNG entry per type, lengths big-endian and
/// including each 8-byte header.
#[expect(clippy::panic, reason = "a dev tool: a failed write ends the run naming the file")]
fn write_icns(mark: &Mark, path: &Path) {
    let mut entries = Vec::new();
    for (kind, size) in ICNS_ENTRIES {
        let png = mac_png(mark, size);
        entries.extend_from_slice(kind);
        entries.extend_from_slice(&icns_length(png.len()).to_be_bytes());
        entries.extend_from_slice(&png);
    }
    let mut file = b"icns".to_vec();
    file.extend_from_slice(&icns_length(entries.len()).to_be_bytes());
    file.extend_from_slice(&entries);
    std::fs::write(path, file).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn icns_length(content: usize) -> u32 {
    u32::try_from(content + 8).expect("an icon file stays far below 4 GB")
}

/// The mark on a solid rounded tile, on Apple's grid in a `size` × `size` canvas.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "icon sizes stay at 1024 px and below; the shares are positive and rounded"
)]
fn mac_png(mark: &Mark, size: u32) -> Vec<u8> {
    let art = ((size as f32 * MAC_ART_SHARE).round() as u32).max(1);
    let mut canvas = RgbaImage::new(size, size);
    let offset = i64::from((size - art) / 2);
    let tile_svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
        <rect width="64" height="64" rx="14" fill="#202632"/>
    </svg>"##;
    let tile = Mark {
        artworks: vec![(
            u32::MAX,
            resvg::usvg::Tree::from_data(tile_svg, &resvg::usvg::Options::default())
                .expect("valid tile SVG"),
        )],
    };
    imageops::overlay(&mut canvas, &tile.render(art), offset, offset);
    let foreground = ((art as f32 * 0.82).round() as u32).max(1);
    let inset = i64::from((size - foreground) / 2);
    imageops::overlay(&mut canvas, &mark.render(foreground), inset, inset);
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(canvas.as_raw(), size, size, ExtendedColorType::Rgba8)
        .expect("encode an icns image");
    png
}

#[expect(clippy::panic, reason = "a dev tool: a missing or broken artwork ends the run naming it")]
fn parse(path: &Path) -> resvg::usvg::Tree {
    let data =
        std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default())
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

/// One band per mark and background: every size 1:1, then the small sizes magnified.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a handful of bands, far below u32::MAX pixels"
)]
fn write_preview(marks: &[Mark], path: &Path) {
    let band_height = 256 + 2 * GAP;
    let width = GAP
        + PREVIEW_SIZES.iter().map(|size| size + GAP).sum::<u32>()
        + MAGNIFIED.iter().map(|(size, factor)| size * factor + GAP).sum::<u32>();
    let bands = (marks.len() * BACKGROUNDS.len()) as u32;
    let mut sheet = RgbaImage::new(width, band_height * bands);
    for (index, (mark, background)) in marks
        .iter()
        .flat_map(|mark| BACKGROUNDS.iter().map(move |background| (mark, background)))
        .enumerate()
    {
        let top = index as u32 * band_height;
        for y in top..top + band_height {
            for x in 0..width {
                sheet.put_pixel(x, y, Rgba([background[0], background[1], background[2], 255]));
            }
        }
        let mut x = GAP;
        for size in PREVIEW_SIZES {
            imageops::overlay(
                &mut sheet,
                &mark.render(size),
                i64::from(x),
                i64::from(top + GAP + (256 - size) / 2),
            );
            x += size + GAP;
        }
        for (size, factor) in MAGNIFIED {
            let big = imageops::resize(
                &mark.render(size),
                size * factor,
                size * factor,
                imageops::FilterType::Nearest,
            );
            imageops::overlay(
                &mut sheet,
                &big,
                i64::from(x),
                i64::from(top + GAP + (256 - size * factor) / 2),
            );
            x += size * factor + GAP;
        }
    }
    std::fs::create_dir_all(path.parent().expect("preview path has a folder"))
        .expect("create the preview folder");
    sheet.save(path).expect("write the preview");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_icon_has_a_solid_tile_and_transparent_outer_margin() {
        let mark = Mark::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/icon"));
        for (_, size) in ICNS_ENTRIES {
            let icon = image::load_from_memory(&mac_png(&mark, size)).unwrap().to_rgba8();
            assert_eq!(icon.dimensions(), (size, size));
            assert_eq!(icon.get_pixel(0, 0)[3], 0, "keep the outer margin transparent");
            for y in size / 4..3 * size / 4 {
                for x in size / 4..3 * size / 4 {
                    // Allow one alpha level of rasterizer rounding at the smallest sizes.
                    assert!(
                        icon.get_pixel(x, y)[3] >= 254,
                        "missing solid tile at {x},{y} in {size}px icon"
                    );
                }
            }
        }
    }
}
