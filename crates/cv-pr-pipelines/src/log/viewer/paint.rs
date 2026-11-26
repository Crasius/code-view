//! How the log viewer paints: one visible line (search mark, number, selection, text) and the
//! job's title in focus mode's bar.

use cv_base::theme::{self, Palette, icons};
use cv_widgets::numbers::group_digits;
use cv_widgets::text_selection::RowSelection;
use cv_widgets::{badges, glyph, mono_text};
use egui::{Align2, FontId, Label, Rect, RichText, Ui, pos2, vec2};
use review_core::PipelineJob;

use super::super::ansi;
use super::super::download::LoadedLog;
use super::selecting;

/// Room in focus mode's bar kept for the job's state and line count after its name.
const STRIP_DETAILS_WIDTH: f32 = 170.0;
/// How a visible line is marked by the search.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Highlight {
    None,
    Match,
    Current,
}

/// What every visible line paints with this frame.
pub struct LineLook<'a> {
    pub font: &'a FontId,
    pub palette: &'a Palette,
    /// Width of the line numbers left of the text.
    pub gutter: f32,
    pub row_height: f32,
    pub fill: selecting::Fill,
}

/// One visible line in `rect`: the search's mark, its number, the selection, its text. Returns
/// the width of the text, for the widest line the view scrolls to.
pub fn paint_line(
    ui: &Ui,
    look: &LineLook<'_>,
    rect: Rect,
    line_index: usize,
    text: &str,
    highlight: Highlight,
    selected: Option<RowSelection>,
) -> f32 {
    let palette = look.palette;
    let mark = match highlight {
        Highlight::Current => Some(palette.warning_surface),
        Highlight::Match => Some(palette.selection),
        Highlight::None => None,
    };
    if let Some(mark) = mark {
        ui.painter().rect_filled(rect, 0, mark);
    }
    let number_at = pos2(rect.left() + look.gutter - 8.0, rect.center().y);
    mono_text::text(
        ui.painter(),
        number_at,
        Align2::RIGHT_CENTER,
        &(line_index + 1).to_string(),
        look.font,
        palette.diff_gutter,
    );
    let galley = mono_text::layout_job(ui.ctx(), ansi::layout(text, palette, look.font));
    let text_width = galley.size().x;
    let text_at = pos2(rect.left() + look.gutter, rect.center().y - galley.size().y / 2.0);
    if let Some(selected) = selected {
        let band = rect.expand2(vec2(0.0, ui.spacing().item_spacing.y / 2.0));
        selecting::paint_line(ui.painter(), &selected, band, text, &galley, text_at, look.fill);
    }
    ui.painter().galley(text_at, galley, palette.text);
    text_width
}

/// The job's state, name and line count in focus mode's bar; the name shortens to fit `width`.
pub fn strip_title(
    ui: &mut Ui,
    title: &str,
    shown: Option<&PipelineJob>,
    log: Option<&LoadedLog>,
    width: f32,
) {
    let palette = theme::palette(ui.ctx());
    if let Some(job) = shown {
        let (glyph, color) = badges::run_status_icon(job.status, &palette);
        // The status is spelled out after the name.
        glyph::decorative(
            ui,
            RichText::new(glyph)
                .font(icons::state_font(&palette, icons::SIZE_SMALL, false))
                .color(color),
        );
    }
    let name = RichText::new(title).font(theme::fonts::semibold_font(theme::style::BODY_SIZE));
    ui.scope(|ui| {
        ui.set_max_width((width - STRIP_DETAILS_WIDTH).max(theme::style::SPACE_L * 4.0));
        ui.add(Label::new(name).truncate().selectable(false));
    });
    let small = |text: String| {
        RichText::new(text).size(theme::style::SMALL_SIZE).color(palette.text_secondary)
    };
    if let Some(job) = shown {
        ui.add(
            Label::new(small(badges::run_status_label(job.status).to_owned())).selectable(false),
        );
    }
    if let Some(log) = log {
        ui.add(
            Label::new(small(format!("{} lines", group_digits(log.file.line_count()))))
                .selectable(false),
        );
    }
}

/// Width of the line numbers left of the text, for `line_count` lines.
pub fn gutter_width(font_size: f32, line_count: usize) -> f32 {
    (group_digits(line_count).len() as f32 + 1.0) * font_size * 0.62 + 12.0
}
