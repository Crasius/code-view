//! Platform marks: which system a connection, pull request or link belongs to, at a glance.
//!
//! GitHub's Octocat and GitLab's tanuki are Phosphor glyphs (Fill weight); Azure DevOps has none,
//! so its mark is painted from its vector outline (simple-icons' 24-unit path, split into
//! triangles, edges anti-aliased by a hairline stroke). No images, no SVG rasterizer.
//!
//! Colors come from the palette: Azure DevOps blue and GitLab orange tuned for 3:1 or more on
//! both themes' surfaces, GitHub monochrome in the text color. A mark is never the only clue:
//! [`show`] and [`interact`] give it a tooltip and an accessible name with the platform and host
//! ("GitLab self-managed · gitlab.example.com").
//!
//! Every mark sits centred in a square slot of its size, so the layout never shifts between
//! platforms. Buttons, menu items and state actions take a mark through [`glyph`]: their icon
//! painters hand every glyph to [`paint_glyph`], which paints platform tokens as marks.

use cv_base::theme::{Palette, icons};
use egui::epaint::{Mesh, PathShape, Shape};
use egui::{
    Align2, Color32, FontId, Id, Painter, Pos2, Rect, Response, Sense, Stroke, Ui, pos2, vec2,
};
use review_core::ProviderKind;

use crate::surfaces;

/// The icon-slot token for Azure DevOps' mark: a private-use codepoint no bundled font draws, so
/// it only ever means "paint the mark" (never put it in running text).
const AZURE_DEVOPS: &str = "\u{F0A2D}";

/// Azure DevOps' outline fills this share of the slot; the Phosphor logos fill about as much of theirs.
const AZURE_SCALE: f32 = 0.92;
/// GitHub's disc fills this share of the slot, the Octocat inside it this share, nudged down by
/// this share so its ears clear the disc's top like in the official mark.
const GITHUB_DISC: f32 = 0.94;
const GITHUB_CAT: f32 = 0.66;
const GITHUB_CAT_DROP: f32 = 0.04;
/// Width of the anti-aliasing edge drawn around the painted outline, in points.
const EDGE: f32 = 0.6;

/// Azure DevOps' mark (simple-icons `azuredevops`, CC0) in a 24-unit square: two outlines.
const AZURE_UPPER: [[f32; 2]; 8] = [
    [0.0, 8.877],
    [2.247, 5.91],
    [10.652, 2.494],
    [10.652, 0.022],
    [18.022, 5.415],
    [2.966, 8.338],
    [2.966, 16.563],
    [0.0, 15.707],
];
const AZURE_LOWER: [[f32; 2]; 8] = [
    [24.0, 4.427],
    [24.0, 19.078],
    [18.247, 23.978],
    [8.944, 20.921],
    [8.944, 23.977],
    [2.966, 16.561],
    [18.023, 18.359],
    [18.023, 5.415],
];
/// Triangles covering each outline exactly (indices into its points).
const AZURE_UPPER_TRIANGLES: [[u32; 3]; 6] =
    [[0, 1, 5], [0, 5, 6], [0, 6, 7], [1, 2, 4], [1, 4, 5], [2, 3, 4]];
const AZURE_LOWER_TRIANGLES: [[u32; 3]; 6] =
    [[7, 0, 1], [7, 1, 2], [7, 2, 6], [5, 6, 2], [5, 2, 3], [3, 4, 5]];

/// The token buttons and menu items take in their icon slot to show `kind`'s mark.
pub fn glyph(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::AzureDevOps => AZURE_DEVOPS,
        ProviderKind::GitHub => icons::GITHUB_LOGO,
        ProviderKind::GitLab => icons::GITLAB_LOGO,
    }
}

fn kind_of(glyph: &str) -> Option<ProviderKind> {
    ProviderKind::ALL.into_iter().find(|kind| self::glyph(*kind) == glyph)
}

/// The mark's brand color in this theme.
fn color(palette: &Palette, kind: ProviderKind) -> Color32 {
    match kind {
        ProviderKind::AzureDevOps => palette.platform_azure,
        ProviderKind::GitHub => palette.text,
        ProviderKind::GitLab => palette.platform_gitlab,
    }
}

/// Paints `kind`'s mark in its brand color, centred on `center` in a `size` square.
pub fn paint(painter: &Painter, palette: &Palette, center: Pos2, size: f32, kind: ProviderKind) {
    if kind == ProviderKind::GitHub {
        // GitHub's mark is the Octocat cut out of a disc; the bare cat reads as "some animal" at 14 px.
        painter.circle_filled(center, size * GITHUB_DISC / 2.0, palette.text);
        let cat = center + vec2(0.0, size * GITHUB_CAT_DROP);
        painter.text(
            cat,
            Align2::CENTER_CENTER,
            glyph(kind),
            icons::font(size * GITHUB_CAT, icons::Weight::Fill),
            palette.surface,
        );
        return;
    }
    paint_tinted(painter, center, size, kind, color(palette, kind));
}

/// Paints `kind`'s mark in one `color` (on filled buttons, disabled controls).
fn paint_tinted(painter: &Painter, center: Pos2, size: f32, kind: ProviderKind, color: Color32) {
    match kind {
        ProviderKind::AzureDevOps => paint_azure(painter, center, size, color),
        ProviderKind::GitHub | ProviderKind::GitLab => {
            painter.text(
                center,
                Align2::CENTER_CENTER,
                glyph(kind),
                icons::font(size, icons::Weight::Fill),
                color,
            );
        }
    }
}

/// Paints an icon-slot glyph: a platform token from [`glyph`] as its mark (brand-colored when
/// `is_branded`, else in `color`), any other glyph as text in `font` and `color`.
pub fn paint_glyph(
    painter: &Painter,
    palette: &Palette,
    center: Pos2,
    glyph: &str,
    font: FontId,
    color: Color32,
    is_branded: bool,
) {
    match kind_of(glyph) {
        Some(kind) if is_branded => paint(painter, palette, center, font.size, kind),
        Some(kind) => paint_tinted(painter, center, font.size, kind, color),
        None => {
            painter.text(center, Align2::CENTER_CENTER, glyph, font, color);
        }
    }
}

/// `kind`'s mark in its own `size` square, with the platform and host of `url` (any URL on that
/// host: the connection's web root, a pull request's page) as tooltip and accessible name.
pub fn show(ui: &mut Ui, kind: ProviderKind, url: &str, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint(ui.painter(), &cv_base::theme::palette(ui.ctx()), rect.center(), size, kind);
    }
    describe_response(response, kind, url)
}

/// `kind`'s mark as a link to `url` (a page on that platform), in a square control with
/// [`LINK_PADDING`] around the mark: hover tint, pressed fill, pointer cursor, keyboard focus
/// ring. The tooltip and accessible name say "`action` · host", the tooltip adds `hint` below
/// (empty for none). The caller opens the page on [`Response::clicked`] (Enter or Space when
/// focused) and [`Response::middle_clicked`].
pub fn link(
    ui: &mut Ui,
    kind: ProviderKind,
    url: &str,
    size: f32,
    action: &str,
    hint: &str,
) -> Response {
    let side = size + LINK_PADDING * 2.0;
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    // Its middle click opens the page; autoscroll would take the press first.
    surfaces::mark_middle_click(ui, rect);
    let host = host(url);
    let name = || if host.is_empty() { action.to_owned() } else { format!("{action} · {host}") };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, ui.is_enabled(), name()));
    if ui.is_rect_visible(rect) {
        super::row::card_background(
            ui,
            &response,
            rect,
            Color32::TRANSPARENT,
            cv_base::theme::style::RADIUS_CONTROL,
        );
        paint(ui.painter(), &cv_base::theme::palette(ui.ctx()), rect.center(), size, kind);
        super::focus::ring(ui, &response, rect);
    }
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response.on_hover_ui(|ui| {
        ui.add(egui::Label::new(name()).wrap_mode(egui::TextWrapMode::Extend));
        if !hint.is_empty() {
            let palette = cv_base::theme::palette(ui.ctx());
            ui.add(
                egui::Label::new(egui::RichText::new(hint).color(palette.text_secondary))
                    .wrap_mode(egui::TextWrapMode::Extend),
            );
        }
    })
}

/// Room between a [`link`] mark and its hover fill.
pub const LINK_PADDING: f32 = 4.0;

/// Tooltip and accessible name for a mark painted at `rect` by a custom-painted row. The area
/// only senses hover, so a click still reaches the row below it.
pub fn interact(ui: &Ui, rect: Rect, id: Id, kind: ProviderKind, url: &str) -> Response {
    describe_response(ui.interact(rect, id, Sense::hover()), kind, url)
}

fn describe_response(response: Response, kind: ProviderKind, url: &str) -> Response {
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, describe(kind, url)));
    // Built only while hovered, not every frame.
    response.on_hover_ui(|ui| {
        tooltip_line(ui, kind, url);
    })
}

/// The platform and host on one unwrapped line, for tooltips that name the platform.
pub fn tooltip_line(ui: &mut Ui, kind: ProviderKind, url: &str) {
    ui.add(egui::Label::new(describe(kind, url)).wrap_mode(egui::TextWrapMode::Extend));
}

/// "GitHub · github.com", "GitHub Enterprise Server · github.example.com", "Azure DevOps · dev.azure.com".
fn describe(kind: ProviderKind, url: &str) -> String {
    let host = host(url);
    if host.is_empty() {
        return kind.label().to_owned();
    }
    format!("{} · {host}", edition(kind, host))
}

/// The host of `url`, without scheme, credentials, port or path.
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    host.split(':').next().unwrap_or(host)
}

/// Which product of the platform answers on `host`: its cloud service or a self-hosted server.
fn edition(kind: ProviderKind, host: &str) -> &'static str {
    let host = host.trim_start_matches("www.");
    let is = |name: &str| host.eq_ignore_ascii_case(name);
    // `get`: a host typed with non-ASCII letters may have no char boundary where the suffix starts.
    let ends = |suffix: &str| {
        host.len() > suffix.len()
            && host
                .get(host.len() - suffix.len()..)
                .is_some_and(|end| end.eq_ignore_ascii_case(suffix))
    };
    match kind {
        ProviderKind::GitHub if is("github.com") => "GitHub",
        ProviderKind::GitHub if ends(".ghe.com") => "GitHub Enterprise Cloud",
        ProviderKind::GitHub => "GitHub Enterprise Server",
        ProviderKind::GitLab if is("gitlab.com") => "GitLab",
        ProviderKind::GitLab => "GitLab self-managed",
        ProviderKind::AzureDevOps if is("dev.azure.com") || ends(".visualstudio.com") => {
            "Azure DevOps"
        }
        ProviderKind::AzureDevOps => "Azure DevOps Server",
    }
}

fn paint_azure(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let scale = size * AZURE_SCALE / 24.0;
    let origin = center - vec2(12.0, 12.0) * scale;
    let at = |[x, y]: [f32; 2]| pos2(origin.x + x * scale, origin.y + y * scale);
    // One unfeathered mesh for both outlines (no seams between triangles), then a hairline along
    // each outline for the anti-aliased edge.
    let mut mesh = Mesh::default();
    mesh.reserve_vertices(AZURE_UPPER.len() + AZURE_LOWER.len());
    mesh.reserve_triangles(AZURE_UPPER_TRIANGLES.len() + AZURE_LOWER_TRIANGLES.len());
    for (points, triangles) in
        [(&AZURE_UPPER, &AZURE_UPPER_TRIANGLES), (&AZURE_LOWER, &AZURE_LOWER_TRIANGLES)]
    {
        let first = mesh.vertices.len() as u32;
        for point in points {
            mesh.colored_vertex(at(*point), color);
        }
        for [a, b, c] in triangles {
            mesh.add_triangle(first + a, first + b, first + c);
        }
    }
    painter.add(Shape::mesh(mesh));
    for points in [&AZURE_UPPER, &AZURE_LOWER] {
        painter.add(PathShape::closed_line(
            points.iter().map(|point| at(*point)).collect(),
            Stroke::new(EDGE, color),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_platform_edition_and_host() {
        assert_eq!(describe(ProviderKind::GitHub, "https://github.com"), "GitHub · github.com");
        assert_eq!(
            describe(ProviderKind::GitHub, "https://github.com/cli/cli/pull/7"),
            "GitHub · github.com"
        );
        assert_eq!(
            describe(ProviderKind::GitHub, "https://octo.ghe.com"),
            "GitHub Enterprise Cloud · octo.ghe.com"
        );
        assert_eq!(
            describe(ProviderKind::GitHub, "https://github.example.com:8443/x"),
            "GitHub Enterprise Server · github.example.com"
        );
        assert_eq!(
            describe(ProviderKind::GitLab, "https://gitlab.com/group/project/-/merge_requests/1"),
            "GitLab · gitlab.com"
        );
        assert_eq!(
            describe(ProviderKind::GitLab, "https://gitlab.example.com"),
            "GitLab self-managed · gitlab.example.com"
        );
        assert_eq!(
            describe(ProviderKind::AzureDevOps, "https://dev.azure.com/contoso"),
            "Azure DevOps · dev.azure.com"
        );
        assert_eq!(
            describe(ProviderKind::AzureDevOps, "https://contoso.visualstudio.com"),
            "Azure DevOps · contoso.visualstudio.com"
        );
        assert_eq!(
            describe(ProviderKind::AzureDevOps, "https://tfs.example.com/tfs/Default"),
            "Azure DevOps Server · tfs.example.com"
        );
        assert_eq!(describe(ProviderKind::GitLab, ""), "GitLab");
    }

    #[test]
    fn a_host_drops_scheme_credentials_port_and_path() {
        assert_eq!(
            host("https://user:secret@jira.example.com:8443/browse/PROJ-1"),
            "jira.example.com"
        );
        assert_eq!(host("jira.example.com/browse/PROJ-1?focus#top"), "jira.example.com");
        assert_eq!(host("https://jira.example.com#PROJ-1"), "jira.example.com");
    }

    /// A host typed with non-ASCII letters, where the suffix would start inside a letter.
    #[test]
    fn hosts_with_non_ascii_letters_are_named() {
        assert_eq!(
            describe(ProviderKind::GitHub, "https://aé1234567"),
            "GitHub Enterprise Server · aé1234567"
        );
        assert_eq!(
            describe(ProviderKind::AzureDevOps, "https://tfs.bäckerei-müller.de"),
            "Azure DevOps Server · tfs.bäckerei-müller.de"
        );
    }

    #[test]
    fn tokens_round_trip_and_ordinary_glyphs_stay_glyphs() {
        for kind in ProviderKind::ALL {
            assert_eq!(kind_of(glyph(kind)), Some(kind));
        }
        assert_eq!(kind_of(icons::ARROW_SQUARE_OUT), None);
    }

    /// The hand-kept triangle indices cover each outline exactly once (their areas add up to the
    /// outline's), so the single unfeathered mesh draws the mark without holes or overlaps.
    #[test]
    fn azure_triangles_cover_the_outline() {
        fn area(points: &[[f32; 2]]) -> f32 {
            let mut sum = 0.0;
            for index in 0..points.len() {
                let [x1, y1] = points[index];
                let [x2, y2] = points[(index + 1) % points.len()];
                sum += x1 * y2 - x2 * y1;
            }
            (sum / 2.0).abs()
        }
        for (points, triangles) in
            [(&AZURE_UPPER, &AZURE_UPPER_TRIANGLES), (&AZURE_LOWER, &AZURE_LOWER_TRIANGLES)]
        {
            let covered: f32 = triangles
                .iter()
                .map(|[a, b, c]| {
                    area(&[points[*a as usize], points[*b as usize], points[*c as usize]])
                })
                .sum();
            assert!(
                (covered - area(points)).abs() < 0.01,
                "covered {covered}, outline {}",
                area(points)
            );
        }
    }
}
