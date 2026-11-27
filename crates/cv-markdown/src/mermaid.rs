//! ```` ```mermaid ```` fences (and Azure DevOps' `::: mermaid` containers, see
//! [`containers_as_fences`]) shown as their diagram, drawn by [`cv_media::mermaid`] in
//! the app's colors and never sent anywhere. The block holds its size while the diagram is drawn
//! (the size an earlier drawing had, else an estimate from the source), and when the size arrives
//! the view keeps what the reader looks at in place. Diagrams shrink to the text width (and a
//! height limit) and open large in the lightbox on a click. Hovering or focusing the block shows
//! Show source, Copy source and Enlarge in its corner; the source view has Show diagram. A source
//! that cannot be drawn shows as code with a note naming the cause, its line, and Copy / Retry.

use std::borrow::Cow;
use std::sync::Arc;

use cv_base::theme::{Palette, icons, motion, style as tokens};
use cv_media::image_state::bump_image_generation;
use cv_media::mermaid::{DiagramFailure, DiagramRequest, DiagramState, DiagramStore, DiagramTheme};
use cv_widgets::{buttons, copy_chip, glyph};
use egui::{
    Align2, Color32, CursorIcon, FontId, Id, Rect, Response, Sense, Stroke, StrokeKind, Ui,
    WidgetInfo, pos2, vec2,
};

use super::images::{self, Anchor};
use super::render::code::{FloatButton, float_button};

/// Diagrams taller than this (at zoom 1) shrink to it; Enlarge shows them whole.
const MAX_HEIGHT: f32 = 640.0;
/// Room around the diagram inside its frame (at zoom 1).
const PADDING: f32 = tokens::SPACE_M;
/// The estimated height of a diagram not drawn yet: per source line, and its bounds (at zoom 1).
const LINE_ESTIMATE: f32 = 22.0;
const MIN_ESTIMATE: f32 = 96.0;
const MAX_ESTIMATE: f32 = 360.0;
/// The width of a diagram not drawn yet (at zoom 1).
const PLACEHOLDER_WIDTH: f32 = 480.0;
const TITLE: &str = "Mermaid diagram";

/// Whether a fence's info string asks for a Mermaid diagram.
pub(super) fn is_mermaid(language: &str) -> bool {
    language.split_whitespace().next().is_some_and(|word| word.eq_ignore_ascii_case("mermaid"))
}

/// The character a [`containers_as_fences`] fence is drawn with. Mermaid never starts a line with
/// `~~~`, so a diagram's own lines cannot end it early.
const CONTAINER_FENCE: char = '~';

/// Azure DevOps writes diagrams as a `::: mermaid` (or `:::mermaid`) container up to a line of
/// colons, beside the ```` ```mermaid ```` fence. Each such container becomes a `~~~` fence of the
/// same length: every colon of its two marker lines turns into a tilde, so bytes and lines stay
/// where they were and the diagram takes the fence's path. The rules follow the container syntax
/// Azure DevOps uses: at most three spaces before the colons, at least three colons, the closing
/// line at least as many colons and nothing else; without one the diagram runs to the end. The word
/// is matched like a fence's language. Inside fenced code a container stays text.
pub(super) fn containers_as_fences(text: &str) -> Cow<'_, str> {
    if !text.contains(":::") {
        return Cow::Borrowed(text);
    }
    let mut out: Option<String> = None;
    let mut open: Option<Open> = None;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent > 3 {
            continue;
        }
        let body = line[indent..].trim_end();
        let run = |marker: u8| body.bytes().take_while(|byte| *byte == marker).count();
        match open {
            Some(Open::Container(colons)) => {
                if run(b':') >= colons && run(b':') == body.len() {
                    tildes(out.get_or_insert_with(|| text.to_owned()), start + indent, body.len());
                    open = None;
                }
            }
            Some(Open::Fence(marker, length)) => {
                if run(marker) >= length && run(marker) == body.len() {
                    open = None;
                }
            }
            None => {
                let colons = run(b':');
                if colons >= 3 && is_mermaid(&body[colons..]) {
                    tildes(out.get_or_insert_with(|| text.to_owned()), start + indent, colons);
                    open = Some(Open::Container(colons));
                } else if let Some(marker) = b"`~".iter().copied().find(|marker| run(*marker) >= 3)
                {
                    open = Some(Open::Fence(marker, run(marker)));
                }
            }
        }
    }
    out.map_or(Cow::Borrowed(text), Cow::Owned)
}

/// The block [`containers_as_fences`] is inside while it walks the lines.
#[derive(Clone, Copy)]
enum Open {
    /// A code fence of this marker byte and length: containers inside it stay text.
    Fence(u8, usize),
    /// A diagram container opened with this many colons.
    Container(usize),
}

/// Turns `count` colons at `at` into the container fence's character (both are one byte).
fn tildes(text: &mut String, at: usize, count: usize) {
    text.replace_range(at..at + count, &CONTAINER_FENCE.to_string().repeat(count));
}

/// One diagram block, as the renderer hands it over.
pub(super) struct Diagram<'a> {
    pub id: Id,
    pub code: &'a str,
    pub palette: &'a Palette,
    pub scale: f32,
    /// A change color around the diagram (the rendered diff's added and removed blocks).
    pub border: Option<Color32>,
    /// Who keeps the reader's place when the block changes height above the view.
    pub anchor: Anchor,
}

/// Lays out the diagram at the cursor, or its source with `source` (which draws the code block
/// and returns its rect).
pub(super) fn show(ui: &mut Ui, diagram: &Diagram<'_>, source: impl FnOnce(&mut Ui) -> Rect) {
    let ctx = ui.ctx().clone();
    let theme = DiagramTheme::from_palette(diagram.palette);
    let request = DiagramRequest { source: diagram.code, theme: &theme };
    let store = DiagramStore::of(&ctx);
    let source_id = source_view_id(diagram.code);
    let wants_source = ctx.data(|data| data.get_temp::<bool>(source_id)).unwrap_or(false);
    // Only a block on screen gets a texture; one measured out of view only learns its size.
    let pixel_scale =
        (ui.is_visible() && !wants_source).then(|| diagram.scale * ctx.pixels_per_point());
    let state = store.state(&ctx, &request, pixel_scale);
    let rect = ui
        .vertical(|ui| match &state {
            DiagramState::Failed(failure) => {
                failure_note(ui, diagram, failure, &store, &request);
                ui.add_space(tokens::SPACE_XS * diagram.scale);
                source(ui);
            }
            _ if wants_source => {
                let block = source(ui);
                let toggle = corner_button(ui, diagram, block, 1, icons::GRAPH, "Show diagram");
                if toggle.clicked() {
                    set_source_view(&ctx, source_id, false);
                }
            }
            _ => show_diagram(ui, diagram, &state, source_id),
        })
        .response
        .rect;
    images::keep_anchor(ui, diagram.id, rect, diagram.anchor);
}

/// The diagram in its frame: drawn, or the placeholder of its size while it is drawn.
fn show_diagram(ui: &mut Ui, diagram: &Diagram<'_>, state: &DiagramState, source_id: Id) {
    let padding = PADDING * diagram.scale;
    let width = ui.available_width();
    let room = (width - 2.0 * padding).max(1.0);
    let (natural, texture) = match state {
        DiagramState::Ready { texture, size } => (Some(*size * diagram.scale), Some(texture)),
        DiagramState::Drawing { size } => (size.map(|size| size * diagram.scale), None),
        DiagramState::Failed(_) => (None, None),
    };
    let shown = match natural {
        Some(natural) => natural * fit(natural, room, MAX_HEIGHT * diagram.scale),
        None => vec2(
            room.min(PLACEHOLDER_WIDTH * diagram.scale),
            estimate_height(diagram.code) * diagram.scale,
        ),
    };
    let (rect, response) =
        ui.allocate_exact_size(vec2(width, shown.y + 2.0 * padding), Sense::click());
    let is_ready = texture.is_some();
    let opacity = motion::reveal(ui.ctx(), diagram.id.with("reveal"), is_ready);
    response.widget_info(|| {
        WidgetInfo::labeled(
            egui::Role::Image,
            true,
            if is_ready { TITLE } else { "Drawing the Mermaid diagram" },
        )
    });
    if ui.is_rect_visible(rect) {
        let image =
            Rect::from_min_size(pos2(rect.center().x - shown.x / 2.0, rect.top() + padding), shown);
        paint_frame(ui, diagram, rect, image, texture, opacity);
    }
    // Enlarge only what the frame shows smaller than it is.
    let enlarge = match (state, natural) {
        (DiagramState::Ready { texture, .. }, Some(natural)) if natural.x > shown.x + 0.5 => {
            Some((texture, natural))
        }
        _ => None,
    };
    diagram_actions(ui, diagram, &response, enlarge, source_id);
}

/// The frame's fill and border, the drawn diagram fading in over `image`, and the drawing note
/// fading out under it.
fn paint_frame(
    ui: &Ui,
    diagram: &Diagram<'_>,
    rect: Rect,
    image: Rect,
    texture: Option<&egui::TextureHandle>,
    opacity: f32,
) {
    let palette = diagram.palette;
    let painter = ui.painter();
    painter.rect_filled(rect, tokens::RADIUS_CONTROL, palette.surface_subtle);
    if let Some(texture) = texture {
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        painter.image(texture.id(), image, uv, Color32::WHITE.gamma_multiply(opacity));
    }
    if opacity < 1.0 {
        let faded = 1.0 - opacity;
        let font = FontId::proportional(tokens::SMALL_SIZE * diagram.scale);
        let color = palette.text_disabled.gamma_multiply(faded);
        painter.text(
            image.center(),
            Align2::CENTER_CENTER,
            format!("{}  Drawing diagram", icons::GRAPH),
            font,
            color,
        );
    }
    let stroke = match diagram.border {
        Some(border) => Stroke::new(2.0, border),
        None => Stroke::new(1.0, palette.border),
    };
    painter.rect_stroke(rect, tokens::RADIUS_CONTROL, stroke, StrokeKind::Inside);
}

/// Copy source, Show source and (with `enlarge`: the texture and its natural size) Enlarge, as
/// corner buttons, a click on the diagram and its context menu.
fn diagram_actions(
    ui: &Ui,
    diagram: &Diagram<'_>,
    response: &Response,
    enlarge: Option<(&egui::TextureHandle, egui::Vec2)>,
    source_id: Id,
) {
    let ctx = ui.ctx().clone();
    let can_enlarge = enlarge.is_some();
    let open_enlarged = || {
        if let Some((texture, natural)) = enlarge {
            images::open_lightbox(
                &ctx,
                cv_media::picture::Picture::still(texture.clone()),
                natural,
                TITLE.to_owned(),
                None,
            );
        }
    };
    let rect = response.rect;
    let copy = corner_button(ui, diagram, rect, 0, icons::COPY, "Copy source");
    let toggle = corner_button(ui, diagram, rect, 1, icons::CODE, "Show source");
    let enlarged =
        can_enlarge && corner_button(ui, diagram, rect, 2, icons::CORNERS_OUT, "Enlarge").clicked();
    if copy.clicked() {
        copy_source(&ctx, diagram);
    }
    if toggle.clicked() {
        set_source_view(&ctx, source_id, true);
    }
    if enlarged || (can_enlarge && response.clicked()) {
        open_enlarged();
    }
    if can_enlarge {
        response
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text_at_pointer("Click to enlarge");
    }
    response.context_menu(|ui| {
        if can_enlarge
            && buttons::menu_item(ui, icons::CORNERS_OUT, "Enlarge", None, None).clicked()
        {
            open_enlarged();
            ui.close();
        }
        if buttons::menu_item(ui, icons::CODE, "Show source", None, None).clicked() {
            set_source_view(ui.ctx(), source_id, true);
            ui.close();
        }
        if buttons::menu_item(ui, icons::COPY, "Copy source", None, None).clicked() {
            copy_source(ui.ctx(), diagram);
            ui.close();
        }
    });
}

/// The share a diagram of `natural` size is shown at: never larger than it is, never wider than
/// `room` or taller than `max_height`.
fn fit(natural: egui::Vec2, room: f32, max_height: f32) -> f32 {
    (room / natural.x.max(1.0)).min(max_height / natural.y.max(1.0)).min(1.0)
}

/// The height (at zoom 1) a diagram not drawn yet holds: its source lines tell roughly how much.
fn estimate_height(code: &str) -> f32 {
    (code.lines().filter(|line| !line.trim().is_empty()).count() as f32 * LINE_ESTIMATE)
        .clamp(MIN_ESTIMATE, MAX_ESTIMATE)
}

/// "Diagram not drawn: <cause>, line 3." above the source, with Copy source and, when drawing
/// again may help, Retry.
fn failure_note(
    ui: &mut Ui,
    diagram: &Diagram<'_>,
    failure: &Arc<DiagramFailure>,
    store: &DiagramStore,
    request: &DiagramRequest<'_>,
) {
    let palette = diagram.palette;
    let size = tokens::SMALL_SIZE * diagram.scale;
    let line = failure.line.map(|line| format!(", line {line}")).unwrap_or_default();
    let text = format!("Diagram not drawn: {}{line}.", failure.reason.trim_end_matches('.'));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = tokens::SPACE_S * diagram.scale;
        glyph::decorative(
            ui,
            egui::RichText::new(icons::WARNING)
                .font(icons::rest_font(palette, size + 2.0))
                .color(palette.warning),
        );
        ui.label(
            egui::RichText::new(text)
                .font(FontId::proportional(size))
                .color(palette.text_secondary),
        );
        if buttons::subtle(ui, icons::COPY, "Copy source").clicked() {
            copy_source(ui.ctx(), diagram);
        }
        if failure.can_retry && buttons::subtle(ui, icons::ARROW_CLOCKWISE, "Retry").clicked() {
            store.retry(ui.ctx(), request);
        }
    });
}

/// Where the choice between diagram and source is kept: by source, so the diff's measuring pass
/// and its painting pass agree on it.
fn source_view_id(code: &str) -> Id {
    Id::unique(("review.markdown.mermaid.source", egui::epaint::util::hash(code)))
}

fn set_source_view(ctx: &egui::Context, id: Id, show_source: bool) {
    ctx.data_mut(|data| data.insert_temp(id, show_source));
    // The block changes height: views that keep laid-out heights measure again.
    bump_image_generation(ctx);
    ctx.request_repaint();
}

fn copy_source(ctx: &egui::Context, diagram: &Diagram<'_>) {
    copy_chip::mark_copied(ctx, diagram.id, diagram.code.to_owned());
    ctx.request_repaint();
}

/// Button `slot` (0 is the corner, counting leftwards) of the diagram's corner buttons
/// ([`float_button`]). Slot 0 of the source view is the code block's own copy button.
fn corner_button(
    ui: &Ui,
    diagram: &Diagram<'_>,
    block: Rect,
    slot: usize,
    icon: &str,
    label: &str,
) -> Response {
    let button = FloatButton {
        id: diagram.id.with(("button", slot)),
        slot,
        icon,
        label,
        copied: (icon == icons::COPY).then_some(diagram.id),
    };
    float_button(ui, block, &button, diagram.palette)
}

#[cfg(test)]
mod tests {
    use egui::vec2;

    use super::{containers_as_fences, fit, is_mermaid};

    #[test]
    fn azure_devops_containers_become_fences_of_the_same_length() {
        let cases = [
            ("::: mermaid\ngraph LR;\n  A-->B\n:::\n", "~~~ mermaid\ngraph LR;\n  A-->B\n~~~\n"),
            (
                ":::mermaid\r\nA-->B\r\n   :::  \r\nafter",
                "~~~mermaid\r\nA-->B\r\n   ~~~  \r\nafter",
            ),
            ("Text\n::: Mermaid\nA-->B\n:::", "Text\n~~~ Mermaid\nA-->B\n~~~"),
            // A shorter line of colons is the diagram's; the container runs to its own length.
            (":::: mermaid\nA-->B\n:::\n::::", "~~~~ mermaid\nA-->B\n:::\n~~~~"),
            // Unclosed, the diagram runs to the end, as the container does.
            ("::: mermaid\nA-->B\n", "~~~ mermaid\nA-->B\n"),
        ];
        for (text, fenced) in cases {
            let out = containers_as_fences(text);
            assert_eq!(out, fenced, "{text:?}");
            assert_eq!(out.len(), text.len());
        }
    }

    #[test]
    fn other_colons_stay_text() {
        for text in [
            "::: moniker range=\"azure-devops\"\ntext\n:::",
            ":::mermaidx\nA\n:::",
            ":: mermaid\nA\n::",
            "    ::: mermaid\n    A\n    :::",
            "```md\n::: mermaid\nA-->B\n:::\n```\n",
            "~~~~\n::: mermaid\n~~~\n:::\n~~~~\n",
            "Only text: a::: mermaid.",
        ] {
            assert_eq!(containers_as_fences(text), text, "{text:?}");
        }
    }

    #[test]
    fn only_mermaid_fences_become_diagrams() {
        assert!(is_mermaid("mermaid"));
        assert!(is_mermaid("Mermaid title=\"x\""));
        assert!(!is_mermaid(""));
        assert!(!is_mermaid("mermaidjs"));
        assert!(!is_mermaid("rust"));
    }

    #[test]
    fn diagrams_shrink_to_the_text_and_never_grow() {
        assert_eq!(fit(vec2(200.0, 100.0), 600.0, 640.0), 1.0, "a small diagram keeps its size");
        assert_eq!(fit(vec2(1200.0, 300.0), 600.0, 640.0), 0.5, "a wide one fits the width");
        assert_eq!(fit(vec2(300.0, 1280.0), 600.0, 640.0), 0.5, "a tall one the height limit");
    }
}
