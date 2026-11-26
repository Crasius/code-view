//! Fenced code blocks: their text in syntax colors with changed words filled, and the copy
//! button that floats in their corner.

use cv_base::theme::{self, Palette, icons, motion, style as tokens};
use cv_syntax::code_colors::CodeColors;
use cv_widgets::focus;
use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, CursorIcon, FontId, Id, Rect, Sense, Ui, WidgetInfo, vec2};

use super::typeset::Fill;

/// How long a copy button shows its check mark after a copy (the code block's and the diagram's).
pub(crate) const COPIED_SECONDS: f64 = 1.5;

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

/// The copy button in a code block's top-right corner. It is interacted with, not laid out: it
/// floats over the block and takes no room, so showing it on hover never moves anything (laying
/// it out with `ui.put` moved the cursor back up into the block, so everything below jumped up
/// over it while the pointer was inside, and back when it left: the hover flicker). It fades in
/// while the pointer is over the block or the button has keyboard focus, and shows a check mark
/// for a moment after a copy.
pub(super) fn copy_button(ui: &Ui, id: Id, block: Rect, code: &str, palette: &Palette) {
    let size =
        vec2(tokens::CONTROL_HEIGHT - tokens::SPACE_S, tokens::CONTROL_HEIGHT - tokens::SPACE_S);
    let rect = Rect::from_min_size(
        block.right_top() + vec2(-tokens::SPACE_XS - size.x, tokens::SPACE_XS),
        size,
    );
    let response = ui.interact(rect, id, Sense::click());
    let now = ui.input(|input| input.time);
    if response.clicked() {
        ui.ctx().copy_text(code.to_owned());
        ui.ctx().data_mut(|data| data.insert_temp(id.with("copied"), now));
    }
    let copied_at: Option<f64> = ui.ctx().data(|data| data.get_temp(id.with("copied")));
    let is_copied = copied_at.is_some_and(|at| now - at < COPIED_SECONDS);
    if let Some(at) = copied_at.filter(|_| is_copied) {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(
            (at + COPIED_SECONDS - now).max(0.0),
        ));
    }
    let wanted =
        response.hovered() || response.has_focus() || is_copied || ui.rect_contains_pointer(block);
    let shown = motion::toggle(ui.ctx(), id.with("shown"), wanted, motion::FAST);
    let label = if is_copied { "Copied" } else { "Copy code" };
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, label));
    if shown > 0.0 {
        let fill = if response.hovered() { palette.pressed } else { palette.surface_subtle };
        let painter = ui.painter();
        painter.rect_filled(rect, tokens::RADIUS_CONTROL, fill.gamma_multiply(shown));
        let (icon, color) = if is_copied {
            (icons::CHECK, palette.success)
        } else {
            (icons::COPY, palette.text_secondary)
        };
        let font = icons::rest_font(palette, icons::SIZE_SMALL);
        painter.text(rect.center(), Align2::CENTER_CENTER, icon, font, color.gamma_multiply(shown));
    }
    focus::ring(ui, &response, rect);
    if shown > 0.0 {
        response.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(label);
    }
}
