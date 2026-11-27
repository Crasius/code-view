//! Fenced code blocks: their text in syntax colors with changed words filled, and the buttons
//! that float in a block's corner (the code block's copy button, the diagram's buttons).

use cv_base::theme::{self, Palette, icons, motion, style as tokens};
use cv_syntax::code_colors::CodeColors;
use cv_widgets::{copy_chip, focus};
use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align2, Color32, CursorIcon, FontId, Id, Rect, Response, Sense, Stroke, StrokeKind, Ui,
    WidgetInfo, vec2,
};

use super::typeset::Fill;

pub(super) struct CodeStyle<'a> {
    pub font: FontId,
    pub palette: &'a Palette,
}

/// A code block's text in its syntax colors (plain without `colors`), with `marks` (byte ranges
/// of `code`) filled.
pub(super) fn code_job(
    code: &str,
    colors: Option<&CodeColors>,
    marks: &[Fill],
    style: &CodeStyle<'_>,
) -> LayoutJob {
    let mut tokens: Vec<(usize, theme::syntax::TokenKind)> = Vec::new();
    if let Some(colors) = colors {
        let mut line_start = 0;
        for (index, line) in code.split('\n').enumerate() {
            tokens.push((line_start, theme::syntax::TokenKind::Plain));
            tokens.extend(
                colors
                    .line(index)
                    .filter(|(start, _)| *start <= line.len())
                    .map(|(start, kind)| (line_start + start, kind)),
            );
            line_start += line.len() + 1;
        }
    }
    let mut cuts: Vec<usize> = tokens
        .iter()
        .map(|(start, _)| *start)
        .chain(marks.iter().flat_map(|(range, _)| [range.start, range.end]))
        .chain([0, code.len()])
        .filter(|cut| *cut <= code.len() && code.is_char_boundary(*cut))
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut job = LayoutJob::default();
    let (mut token, mut mark) = (0, 0);
    for window in cuts.windows(2) {
        let (start, end) = (window[0], window[1]);
        while tokens.get(token + 1).is_some_and(|(next, _)| *next <= start) {
            token += 1;
        }
        let kind = tokens
            .get(token)
            .filter(|(at, _)| *at <= start)
            .map_or(theme::syntax::TokenKind::Plain, |(_, kind)| *kind);
        while marks.get(mark).is_some_and(|(range, _)| range.end <= start) {
            mark += 1;
        }
        let fill = marks.get(mark).filter(|(range, _)| range.start <= start).map(|(_, fill)| *fill);
        let format = TextFormat {
            font_id: style.font.clone(),
            color: theme::syntax::color(kind, style.palette),
            background: fill.unwrap_or(Color32::TRANSPARENT),
            ..TextFormat::default()
        };
        job.append(&code[start..end], 0.0, format);
    }
    job
}

/// The copy button in a code block's top-right corner (see [`float_button`]).
pub(super) fn copy_button(ui: &Ui, id: Id, block: Rect, code: &str, palette: &Palette) {
    let button =
        FloatButton { id, slot: 0, icon: icons::COPY, label: "Copy code", copied: Some(id) };
    if float_button(ui, block, &button, palette).clicked() {
        copy_chip::mark_copied(ui.ctx(), id, code.to_owned());
        ui.ctx().request_repaint();
    }
}

/// One button of [`float_button`].
pub(crate) struct FloatButton<'a> {
    pub id: Id,
    /// 0 is the corner, counting leftwards.
    pub slot: usize,
    pub icon: &'a str,
    pub label: &'a str,
    /// A copy button's: the id whose "Copied" confirmation it shows ([`copy_chip::mark_copied`]).
    pub copied: Option<Id>,
}

/// `button` floating over the top-right of `block`. It is interacted with, not laid out: it
/// floats over the block and takes no room, so showing it on hover never moves anything. It fades
/// in while the pointer is over the block or the button has keyboard focus; a copy button shows a
/// check mark for a moment after a copy.
pub(crate) fn float_button(
    ui: &Ui,
    block: Rect,
    button: &FloatButton<'_>,
    palette: &Palette,
) -> Response {
    let side = tokens::CONTROL_HEIGHT - tokens::SPACE_S;
    let step = side + tokens::SPACE_XS;
    let rect = Rect::from_min_size(
        block.right_top()
            + vec2(-tokens::SPACE_XS - side - step * button.slot as f32, tokens::SPACE_XS),
        vec2(side, side),
    );
    let response = ui.interact(rect, button.id, Sense::click());
    let is_copied = button.copied.is_some_and(|id| copy_chip::is_showing_copied(ui, id));
    let wanted =
        response.hovered() || response.has_focus() || is_copied || ui.rect_contains_pointer(block);
    let shown = motion::toggle(ui.ctx(), button.id.with("shown"), wanted, motion::FAST);
    let label = if is_copied { "Copied" } else { button.label };
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, label));
    if shown > 0.0 {
        let fill = if response.hovered() { palette.pressed } else { palette.surface };
        let painter = ui.painter();
        painter.rect_filled(rect, tokens::RADIUS_CONTROL, fill.gamma_multiply(shown));
        painter.rect_stroke(
            rect,
            tokens::RADIUS_CONTROL,
            Stroke::new(1.0, palette.border.gamma_multiply(shown)),
            StrokeKind::Inside,
        );
        let (glyph, color) = if is_copied {
            (icons::CHECK, palette.success)
        } else {
            (button.icon, palette.text_secondary)
        };
        let font = icons::rest_font(palette, icons::SIZE_SMALL);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            glyph,
            font,
            color.gamma_multiply(shown),
        );
    }
    focus::ring(ui, &response, rect);
    if shown > 0.0 {
        response.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(label)
    } else {
        response
    }
}
