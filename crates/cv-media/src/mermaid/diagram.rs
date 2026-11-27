//! One diagram from Mermaid source to SVG, in the app's colors: what the renderer process runs
//! (see [`super::worker`]). The source never leaves the machine.

use std::ops::Range;

use cv_base::theme::{FileHue, Palette};
use egui::{Color32, Vec2, vec2};
use mermaid_rs_renderer::{ParseError, RenderOptions, Theme};

/// Sources longer than this are not drawn: their source shows instead.
const MAX_SOURCE_BYTES: usize = 64 * 1024;
const MAX_SOURCE_LINES: usize = 2_000;
/// A diagram laid out wider or taller than this (points) is not drawn.
const MAX_EXTENT: f32 = 20_000.0;
/// The diagrams' type size at zoom 1, in points.
const FONT_SIZE: f32 = 14.0;
/// The system UI fonts of each platform, then the common fallbacks; the renderer measures text in
/// the same system fonts that later draw it.
const FONT_FAMILY: &str = "\"Segoe UI\", -apple-system, \"SF Pro Text\", \"Helvetica Neue\", Cantarell, Ubuntu, \"Noto Sans\", \"DejaVu Sans\", Arial, sans-serif";

/// A drawn diagram: the SVG and its own size in points at zoom 1.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Svg {
    pub(crate) text: String,
    pub(crate) size: Vec2,
}

/// Why a diagram is not drawn, for the note above its source.
#[derive(Debug, Clone, PartialEq)]
pub struct DiagramFailure {
    pub reason: String,
    /// The source line (1-based) the renderer named, when it named one.
    pub line: Option<u32>,
    /// Drawing again may work (the renderer was stopped or could not start).
    pub can_retry: bool,
}

impl DiagramFailure {
    pub(crate) fn new(reason: impl Into<String>, line: Option<u32>) -> Self {
        Self { reason: reason.into(), line, can_retry: false }
    }

    pub(crate) fn retryable(reason: impl Into<String>) -> Self {
        Self { reason: reason.into(), line: None, can_retry: true }
    }
}

/// The colors a diagram is drawn in, taken from the app's palette. Equal themes draw equal
/// diagrams; a theme switch draws them again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagramTheme {
    pub is_dark: bool,
    /// Behind the whole diagram.
    pub background: Color32,
    /// Nodes, actors, classes.
    pub node: Color32,
    pub node_border: Color32,
    pub text: Color32,
    /// Edges, arrows and lifelines.
    pub line: Color32,
    /// Subgraphs and alternate rows.
    pub cluster: Color32,
    pub cluster_border: Color32,
    pub note: Color32,
    pub note_border: Color32,
    /// Activation bars and secondary fills.
    pub muted: Color32,
    /// Pie slices and other series, in order.
    pub series: [Color32; 12],
}

impl DiagramTheme {
    /// How many fields [`Self::fields`] writes: the mode, 10 colors, 12 series colors.
    const FIELDS: usize = 1 + 10 + 12;

    pub fn from_palette(palette: &Palette) -> Self {
        // Series fills carry the text color in their labels (inside a slice and beside it alike):
        // pale tints under dark text, deep ones under light text.
        let hue = |hue: FileHue| {
            let color = palette.file_hue(hue);
            if palette.is_dark {
                color.lerp_to_gamma(palette.surface_subtle, 0.4)
            } else {
                color.lerp_to_gamma(Color32::WHITE, 0.5)
            }
        };
        Self {
            is_dark: palette.is_dark,
            background: palette.surface_subtle,
            node: palette.surface,
            // A hint of the accent: the renderer turns the border's hue into its Gantt sections'.
            node_border: palette.border_strong.lerp_to_gamma(palette.accent, 0.2),
            text: palette.text,
            line: palette.text_secondary,
            cluster: palette.hover,
            cluster_border: palette.border,
            note: palette.warning_surface,
            note_border: palette.warning,
            muted: palette.pressed,
            series: [
                hue(FileHue::Blue),
                hue(FileHue::Teal),
                hue(FileHue::Amber),
                hue(FileHue::Purple),
                hue(FileHue::Red),
                hue(FileHue::Green),
                hue(FileHue::Cyan),
                hue(FileHue::Orange),
                hue(FileHue::Pink),
                hue(FileHue::Yellow),
                hue(FileHue::Slate),
                hue(FileHue::Blue).lerp_to_gamma(palette.accent, 0.5),
            ],
        }
    }

    /// The theme as text fields for the renderer process: a mode flag, then `#rrggbbaa` colors.
    pub(crate) fn fields(&self) -> Vec<String> {
        let mut fields = Vec::with_capacity(Self::FIELDS);
        fields.push(if self.is_dark { "dark" } else { "light" }.to_owned());
        let colors = [
            self.background,
            self.node,
            self.node_border,
            self.text,
            self.line,
            self.cluster,
            self.cluster_border,
            self.note,
            self.note_border,
            self.muted,
        ];
        fields.extend(colors.iter().chain(&self.series).map(Color32::to_hex));
        fields
    }

    /// The theme [`Self::fields`] wrote; `None` for anything else.
    pub(crate) fn from_fields(fields: &[String]) -> Option<Self> {
        if fields.len() != Self::FIELDS {
            return None;
        }
        let (mode, colors) = fields.split_first()?;
        let colors: Vec<Color32> =
            colors.iter().map(|hex| Color32::from_hex(hex).ok()).collect::<Option<_>>()?;
        let [
            background,
            node,
            node_border,
            text,
            line,
            cluster,
            cluster_border,
            note,
            note_border,
            muted,
            series @ ..,
        ] = colors.as_slice()
        else {
            return None;
        };
        Some(Self {
            is_dark: mode == "dark",
            background: *background,
            node: *node,
            node_border: *node_border,
            text: *text,
            line: *line,
            cluster: *cluster,
            cluster_border: *cluster_border,
            note: *note,
            note_border: *note_border,
            muted: *muted,
            series: series.try_into().ok()?,
        })
    }

    /// The renderer's theme: its modern look (or Mermaid's dark one) in these colors.
    fn renderer_theme(&self) -> Theme {
        // Opaque `#rrggbb`: the renderer mixes some colors itself and reads only that form.
        let css = |color: Color32| format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b());
        let base = if self.is_dark { Theme::dark() } else { Theme::modern() };
        Theme {
            font_family: FONT_FAMILY.to_owned(),
            font_size: FONT_SIZE,
            primary_color: css(self.node),
            primary_text_color: css(self.text),
            primary_border_color: css(self.node_border),
            line_color: css(self.line),
            secondary_color: css(self.muted),
            tertiary_color: css(self.node),
            edge_label_background: css(self.background),
            cluster_background: css(self.cluster),
            cluster_border: css(self.cluster_border),
            background: css(self.background),
            sequence_actor_fill: css(self.node),
            sequence_actor_border: css(self.node_border),
            sequence_actor_line: css(self.line),
            sequence_note_fill: css(self.note),
            sequence_note_border: css(self.note_border),
            sequence_activation_fill: css(self.muted),
            sequence_activation_border: css(self.node_border),
            text_color: css(self.text),
            pie_colors: self.series.map(css),
            pie_title_text_size: FONT_SIZE + 4.0,
            pie_title_text_color: css(self.text),
            pie_section_text_size: FONT_SIZE,
            pie_section_text_color: css(self.text),
            pie_legend_text_size: FONT_SIZE,
            pie_legend_text_color: css(self.text),
            pie_stroke_color: css(self.background),
            pie_stroke_width: 1.5,
            pie_outer_stroke_width: 1.0,
            pie_outer_stroke_color: css(self.node_border),
            pie_opacity: 1.0,
            ..base
        }
    }
}

/// A source too large to draw, or nothing to draw at all: why, before any renderer runs.
pub(crate) fn check_source(source: &str) -> Result<(), DiagramFailure> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(DiagramFailure::new(
            format!("Larger than {} KB, too large to draw", MAX_SOURCE_BYTES / 1024),
            None,
        ));
    }
    let lines = source.lines().count();
    if lines > MAX_SOURCE_LINES {
        return Err(DiagramFailure::new(
            format!("{lines} lines, more than the {MAX_SOURCE_LINES} drawn"),
            None,
        ));
    }
    if source.trim().is_empty() {
        return Err(DiagramFailure::new("The diagram is empty", None));
    }
    Ok(())
}

/// Parses, lays out and draws `source` in `theme`.
pub(crate) fn draw(source: &str, theme: &DiagramTheme) -> Result<Svg, DiagramFailure> {
    check_source(source)?;
    let options = RenderOptions { theme: theme.renderer_theme(), ..RenderOptions::modern() };
    let svg = mermaid_rs_renderer::render_strict(source, options)
        .map_err(|error| failure_of(source, &error))?;
    let size =
        svg_size(&svg).ok_or_else(|| DiagramFailure::new("The renderer drew nothing", None))?;
    if size.x > MAX_EXTENT || size.y > MAX_EXTENT {
        return Err(DiagramFailure::new(
            format!("{:.0} × {:.0} points, too large to draw", size.x, size.y),
            None,
        ));
    }
    Ok(Svg { text: with_size(&svg, size), size })
}

/// The note for a parse error: what is wrong, and where when the renderer knows.
fn failure_of(source: &str, error: &ParseError) -> DiagramFailure {
    match error {
        ParseError::UnexpectedToken { expected, .. } if expected.contains("diagram header") => {
            let line = header_line(source);
            let first = source
                .lines()
                .nth(line as usize - 1)
                .and_then(|text| text.split_whitespace().next());
            match first {
                Some(first) => DiagramFailure::new(
                    format!("Unsupported diagram type \u{201C}{first}\u{201D}"),
                    Some(line),
                ),
                None => DiagramFailure::new("The diagram type is missing", Some(line)),
            }
        }
        ParseError::UnexpectedToken { found, expected, line, .. } => DiagramFailure::new(
            format!("Unexpected \u{201C}{found}\u{201D}, expected {expected}"),
            Some(*line),
        ),
        ParseError::UnknownParticipant { name, line, .. } => {
            DiagramFailure::new(format!("Unknown participant \u{201C}{name}\u{201D}"), Some(*line))
        }
        ParseError::UnclosedSubgraph { opened_at } => DiagramFailure::new(
            "A block is never closed with \u{201C}end\u{201D}",
            Some(*opened_at),
        ),
        ParseError::InvalidDirective { directive, reason, line, .. } => {
            DiagramFailure::new(format!("Invalid {directive} directive: {reason}"), Some(*line))
        }
        other => DiagramFailure::new(other.to_string(), None),
    }
}

/// The 1-based line that names the diagram type: the first one with text that is not a comment
/// or front matter.
fn header_line(source: &str) -> u32 {
    let mut in_front_matter = false;
    for (index, line) in source.lines().enumerate() {
        let line = line.trim();
        if line == "---" {
            in_front_matter = !in_front_matter;
            continue;
        }
        if !(line.is_empty() || in_front_matter || line.starts_with("%%")) {
            return index as u32 + 1;
        }
    }
    1
}

/// The root `<svg>` element's tag: from `<svg` to its `>`.
fn root_tag(svg: &str) -> Option<Range<usize>> {
    let start = svg.find("<svg")?;
    Some(start..start + svg[start..].find('>')?)
}

/// The value of attribute `name` in `tag`, and where it stands (` name="value"`).
fn attribute<'a>(tag: &'a str, name: &str) -> Option<(&'a str, Range<usize>)> {
    let marker = format!(" {name}=\"");
    let start = tag.find(&marker)?;
    let value_start = start + marker.len();
    let value_end = value_start + tag[value_start..].find('"')?;
    Some((&tag[value_start..value_end], start..value_end + 1))
}

/// The root's size in points: its `width` and `height` when both are absolute, else its
/// `viewBox` (some diagrams are drawn `width="100%"`).
fn svg_size(svg: &str) -> Option<Vec2> {
    let tag = &svg[root_tag(svg)?];
    let positive = |length: f32| (length.is_finite() && length > 0.0).then_some(length);
    let length = |name: &str| {
        attribute(tag, name)
            .and_then(|(value, _)| value.trim_end_matches("px").parse::<f32>().ok())
            .and_then(positive)
    };
    if let (Some(width), Some(height)) = (length("width"), length("height")) {
        return Some(vec2(width, height));
    }
    let (view_box, _) = attribute(tag, "viewBox")?;
    let numbers: Vec<f32> = view_box
        .split([' ', ','])
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    match numbers.as_slice() {
        [_, _, width, height] => Some(vec2(positive(*width)?, positive(*height)?)),
        _ => None,
    }
}

/// `svg` with its root sized `size` in absolute points, so it draws at that size.
fn with_size(svg: &str, size: Vec2) -> String {
    let Some(range) = root_tag(svg) else {
        return svg.to_owned();
    };
    let mut tag = svg[range.clone()].to_owned();
    for name in ["width", "height"] {
        if let Some((_, at)) = attribute(&tag, name) {
            tag.replace_range(at, "");
        }
    }
    tag.insert_str("<svg".len(), &format!(" width=\"{}\" height=\"{}\"", size.x, size.y));
    format!("{}{tag}{}", &svg[..range.start], &svg[range.end..])
}

#[cfg(test)]
mod tests {
    use cv_base::theme::Palette;

    use super::{DiagramTheme, MAX_SOURCE_BYTES, check_source, draw, svg_size, with_size};

    fn light() -> DiagramTheme {
        DiagramTheme::from_palette(&Palette::LIGHT)
    }

    #[test]
    fn draws_the_common_diagram_types() {
        let sources = [
            "flowchart LR\n  A[Start] --> B{Choice}\n  B -->|yes| C\n  B -->|no| D",
            "graph TD; A-->B",
            "sequenceDiagram\n  Alice->>Bob: Hello\n  Bob-->>Alice: Hi",
            "classDiagram\n  Animal <|-- Duck\n  Animal : +int age",
            "stateDiagram-v2\n  [*] --> Still\n  Still --> [*]",
            "erDiagram\n  CUSTOMER ||--o{ ORDER : places",
            "gantt\n  title Plan\n  dateFormat YYYY-MM-DD\n  section A\n  Task :a1, 2024-01-01, 3d",
            "pie title Pets\n  \"Dogs\" : 3\n  \"Cats\" : 2",
            "journey\n  title Day\n  section Work\n    Code: 5: Me",
            "gitGraph\n  commit\n  branch dev\n  commit",
            "mindmap\n  root((Root))\n    A\n    B",
            "timeline\n  title History\n  2020 : One",
        ];
        for source in sources {
            let svg =
                draw(source, &light()).unwrap_or_else(|failure| panic!("{source:?}: {failure:?}"));
            assert!(svg.text.contains("<svg"), "{source:?}");
            assert!(svg.size.x > 0.0 && svg.size.y > 0.0, "{source:?}: {:?}", svg.size);
        }
    }

    #[test]
    fn the_diagram_takes_the_theme_colors() {
        let theme = light();
        let svg = draw("flowchart LR\n  A --> B", &theme).expect("a flowchart draws");
        assert!(
            svg.text.to_ascii_lowercase().contains(&theme.node_border.to_hex()[..7]),
            "node borders in the palette's color"
        );
        let dark = DiagramTheme::from_palette(&Palette::DARK);
        let dark_svg = draw("flowchart LR\n  A --> B", &dark).expect("a flowchart draws");
        assert_ne!(
            svg.text, dark_svg.text,
            "a theme switch draws the diagram again in other colors"
        );
    }

    #[test]
    fn errors_name_the_cause_and_the_line() {
        let unknown = draw("%% a comment\n\nfooDiagram\n  x", &light())
            .expect_err("an unknown type is not drawn");
        assert_eq!(unknown.reason, "Unsupported diagram type \u{201C}fooDiagram\u{201D}");
        assert_eq!(unknown.line, Some(3));
        let unclosed = draw("flowchart TD\n  subgraph One\n  A --> B\n", &light())
            .expect_err("an unclosed subgraph is an error");
        assert_eq!(unclosed.line, Some(2), "{unclosed:?}");
        assert!(!unclosed.can_retry);
    }

    #[test]
    fn oversized_and_empty_sources_are_refused_before_drawing() {
        assert!(check_source(&"A-->B\n".repeat(MAX_SOURCE_BYTES / 6 + 1)).is_err());
        assert!(check_source(&"A\n".repeat(2_001)).unwrap_err().reason.contains("2001 lines"));
        assert_eq!(check_source("  \n ").unwrap_err().reason, "The diagram is empty");
        assert!(check_source("graph TD; A-->B").is_ok());
    }

    #[test]
    fn the_theme_survives_the_trip_to_the_renderer() {
        for palette in [Palette::LIGHT, Palette::DARK] {
            let theme = DiagramTheme::from_palette(&palette);
            assert_eq!(DiagramTheme::from_fields(&theme.fields()), Some(theme));
        }
        assert_eq!(DiagramTheme::from_fields(&["light".to_owned(), "#zz".to_owned()]), None);
    }

    #[test]
    fn reads_the_root_size() {
        assert_eq!(
            svg_size(
                r#"<?xml?><svg xmlns="x" width="120.5" height="40px" viewBox="0 0 1 1"><g width="9"/></svg>"#
            ),
            Some(egui::vec2(120.5, 40.0))
        );
        assert_eq!(svg_size(r#"<svg width="0" height="10"></svg>"#), None);
        let relative = r#"<svg xmlns="x" width="100%" viewBox="0 0 482.5 360" style="max-width: 482px;"><rect/></svg>"#;
        assert_eq!(
            svg_size(relative),
            Some(egui::vec2(482.5, 360.0)),
            "a relative width falls back to the view box"
        );
        let sized = with_size(relative, egui::vec2(482.5, 360.0));
        assert_eq!(
            sized,
            r#"<svg width="482.5" height="360" xmlns="x" viewBox="0 0 482.5 360" style="max-width: 482px;"><rect/></svg>"#
        );
    }
}
