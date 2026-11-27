//! The Appearance preview: a small sample diff that follows the theme, diff layout, code font size,
//! line wrapping and comments setting as they change. Its rows are painted by the Files tab's own
//! painter (`diff_paint::paint_line`) with the diff's metrics, line styles and wrap rule, in the
//! syntax colors of the diff's grammars (`code_colors`).
//!
//! The file header and the comment are drawn here: the real ones need a loaded pull request (its
//! session, threads and their editor state), which a settings page has none of.

use cv_base::theme::{self, style};
use cv_code::diff_paint::{
    self, Invisibles, LineStyle, Metrics, PaintContext, PaneLine, TEXT_END_PADDING,
};
use cv_code::line_source::CodeLine;
use cv_services::Services;
use cv_syntax::code_colors;
use cv_syntax::highlight::Span;
use egui::{Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use review_git::LineKind;
use review_store::DiffLayout;

const PATH: &str = "src/review/session.rs";
/// Short lines, so both panes of side by side show them whole at the default code size.
const OLD: [&str; 6] = [
    "fn trim(&mut self) {",
    "    let cap = 8;",
    "    if self.n > cap {",
    "        self.pop();",
    "    }",
    "}",
];
const NEW: [&str; 7] = [
    "fn trim(&mut self) {",
    "    let cap = self.limit;",
    "    while self.n > cap {",
    "        self.pop();",
    "    }",
    "    self.save();",
    "}",
];
/// The first line's number in both versions.
const FIRST: u32 = 41;
/// The words a replaced line changes, as the diff's word highlighting marks them.
const CHANGED_WORDS: [(LineKind, usize, &str); 4] = [
    (LineKind::Deleted, 1, "8"),
    (LineKind::Added, 1, "self.limit"),
    (LineKind::Deleted, 2, "if"),
    (LineKind::Added, 2, "while"),
];

/// A line of the sample: which version it is from and its index there.
#[derive(Clone, Copy)]
struct Line {
    kind: LineKind,
    index: usize,
}

const fn context(index: usize) -> Line {
    Line { kind: LineKind::Context, index }
}
const fn added(index: usize) -> Line {
    Line { kind: LineKind::Added, index }
}
const fn deleted(index: usize) -> Line {
    Line { kind: LineKind::Deleted, index }
}

/// Side by side: old on the left, new on the right; `None` is an empty half.
const SPLIT: [(Option<Line>, Option<Line>); 7] = [
    (Some(context(0)), Some(context(0))),
    (Some(deleted(1)), Some(added(1))),
    (Some(deleted(2)), Some(added(2))),
    (Some(context(3)), Some(context(3))),
    (Some(context(4)), Some(context(4))),
    (None, Some(added(5))),
    (Some(context(6)), Some(context(6))),
];
const INLINE: [Line; 9] = [
    context(0),
    deleted(1),
    deleted(2),
    added(1),
    added(2),
    context(3),
    context(4),
    added(5),
    context(6),
];
/// The new line the sample comment is on.
const COMMENTED: usize = 1;

/// The width the sample needs to show its widest line whole in every pane of its widest layout,
/// frame included. Its widest layout, not the current one: the preview keeps its place while the
/// user switches layouts, so the layout control never moves under the pointer.
pub(crate) fn width(ui: &Ui, services: &Services) -> f32 {
    let metrics = Metrics::new(ui, services.settings.code_font_size, FIRST + NEW.len() as u32);
    let widest = OLD.iter().chain(&NEW).map(|line| line.len()).max().unwrap_or_default();
    let text = (widest + 2) as f32 * metrics.char_width;
    let side_by_side = 2.0 * (metrics.text_inset(false) + text);
    let inline = metrics.text_inset(true) + text;
    side_by_side.max(inline).ceil() + 2.0
}

/// How much room the tightest pane had left right of its line's text in the last frame, in
/// points; negative when a line was cut. `None` while wrapping or before the sample showed.
#[cfg(any(test, feature = "test-support"))]
pub fn slack(ctx: &egui::Context) -> Option<f32> {
    ctx.data(|data| data.get_temp(egui::Id::unique(SLACK_ID)))
}

#[cfg(any(test, feature = "test-support"))]
const SLACK_ID: &str = "settings_sample_slack";

pub(crate) fn show(ui: &mut Ui, services: &Services) {
    let settings = &services.settings;
    let palette = theme::palette(ui.ctx());
    let metrics = Metrics::new(ui, settings.code_font_size, FIRST + NEW.len() as u32);
    let sample = Sample {
        metrics: &metrics,
        old_spans: spans(ui.ctx(), &OLD),
        new_spans: spans(ui.ctx(), &NEW),
        layout: settings.diff_layout,
        wrap: settings.wrap_lines,
        slack: std::cell::Cell::new(f32::INFINITY),
    };
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(style::RADIUS_CARD)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            header(ui, settings.diff_layout);
            ui.add_space(style::SPACE_S);
            match settings.diff_layout {
                DiffLayout::SideBySide => {
                    for (left, right) in SPLIT {
                        sample.row(ui, [left, right]);
                        if right.is_some_and(is_commented) {
                            sample.comment(ui, settings.show_comments_in_diff);
                        }
                    }
                }
                DiffLayout::Inline => {
                    for line in INLINE {
                        sample.row(ui, [Some(line), None]);
                        if is_commented(line) {
                            sample.comment(ui, settings.show_comments_in_diff);
                        }
                    }
                }
                DiffLayout::NewOnly => {
                    for line in INLINE.into_iter().filter(|line| line.kind != LineKind::Deleted) {
                        sample.row(ui, [Some(line), None]);
                        if is_commented(line) {
                            sample.comment(ui, settings.show_comments_in_diff);
                        }
                    }
                }
            }
            ui.add_space(style::SPACE_S);
        });
    #[cfg(any(test, feature = "test-support"))]
    {
        let slack = (!sample.wrap).then(|| sample.slack.get());
        ui.ctx().data_mut(|data| match slack {
            Some(slack) => {
                data.insert_temp(egui::Id::unique(SLACK_ID), slack);
            }
            None => {
                data.remove_temp::<f32>(egui::Id::unique(SLACK_ID));
            }
        });
    }
}

/// Each line's syntax spans, from the diff's grammar for Rust; empty until they are worked out.
fn spans(ctx: &egui::Context, lines: &[&str]) -> Option<Vec<Vec<Span>>> {
    let colors = code_colors::colors(ctx, "rust", &lines.join("\n"))?;
    Some(
        (0..lines.len())
            .map(|index| colors.line(index).map(|(start, kind)| Span::new(start, kind)).collect())
            .collect(),
    )
}

fn is_commented(line: Line) -> bool {
    line.kind == LineKind::Added && line.index == COMMENTED
}

/// The file's path, its counts and the layout's name, as a file header shows them.
fn header(ui: &mut Ui, layout: DiffLayout) {
    let palette = theme::palette(ui.ctx());
    egui::Frame::new()
        .fill(palette.surface_subtle)
        .inner_margin(egui::Margin::symmetric(style::SPACE_M as i8, style::SPACE_S as i8))
        .corner_radius(egui::CornerRadius {
            nw: style::RADIUS_CARD,
            ne: style::RADIUS_CARD,
            sw: 0,
            se: 0,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                cv_widgets::row::with_trailing(
                    ui,
                    egui::Align::Center,
                    |ui| {
                        let name = match layout {
                            DiffLayout::SideBySide => "Side by side",
                            DiffLayout::Inline => "Inline",
                            DiffLayout::NewOnly => "New only",
                        };
                        ui.label(
                            RichText::new(name)
                                .size(style::SMALL_SIZE)
                                .color(palette.text_secondary),
                        );
                    },
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(PATH)
                                    .font(theme::fonts::semibold_font(style::BODY_SIZE)),
                            )
                            .truncate(),
                        );
                        ui.label(RichText::new("+3").color(palette.success));
                        ui.label(RichText::new("−2").color(palette.error));
                    },
                );
            });
        });
}

struct Sample<'a> {
    metrics: &'a Metrics,
    old_spans: Option<Vec<Vec<Span>>>,
    new_spans: Option<Vec<Vec<Span>>>,
    layout: DiffLayout,
    wrap: bool,
    /// The least room any pane had right of its text (see `slack`).
    slack: std::cell::Cell<f32>,
}

impl Sample<'_> {
    /// One row: the old and new pane side by side, else the one pane (`panes[0]`) full width.
    fn row(&self, ui: &mut Ui, panes: [Option<Line>; 2]) {
        let width = ui.available_width();
        let left_width =
            if self.layout == DiffLayout::SideBySide { (width / 2.0).floor() } else { width };
        let pane_widths = [left_width, width - left_width];
        let shown = if self.layout == DiffLayout::SideBySide { 2 } else { 1 };
        let wrap_columns = pane_widths.map(|width| self.wrap_columns(width));
        let rows = (0..shown)
            .filter_map(|slot| Some(rows(panes[slot]?, wrap_columns[slot])))
            .max()
            .unwrap_or(1);
        let height = rows as f32 * self.metrics.line_height;
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let palette = theme::palette(ui.ctx());
        let inset = self.metrics.text_inset(self.layout == DiffLayout::Inline);
        for slot in 0..shown {
            if let Some(line) = panes[slot] {
                let room =
                    pane_widths[slot] - inset - text(line).len() as f32 * self.metrics.char_width;
                self.slack.set(self.slack.get().min(room));
            }
        }
        let mut left = rect.left();
        for slot in 0..shown {
            let pane = Rect::from_min_size(pos2(left, rect.top()), vec2(pane_widths[slot], height));
            left = pane.right();
            let painter = ui.painter().with_clip_rect(pane.intersect(ui.clip_rect()));
            let context =
                PaintContext { painter: &painter, palette: &palette, metrics: self.metrics };
            let line = panes[slot];
            let words = line.and_then(changed_words);
            let pane_line = PaneLine {
                line: line.map(|line| self.code_line(line, slot)),
                has_two_numbers: self.layout == DiffLayout::Inline,
                style: if self.layout == DiffLayout::NewOnly {
                    LineStyle::Focus
                } else {
                    LineStyle::Diff
                },
                words: words.as_slice(),
                h_offset: 0.0,
                wrap_width: wrap_columns[slot]
                    .map(|columns| columns as f32 * self.metrics.char_width + 0.5),
                is_hovered: false,
                hide_marker: false,
                selection: None,
                range: None,
                invisibles: Invisibles::default(),
                syntax_mix: 1.0,
            };
            diff_paint::paint_line(&context, pane, &pane_line);
        }
        if shown == 2 {
            diff_paint::paint_divider(ui.painter(), rect.left() + left_width, rect, &palette);
        }
    }

    /// The sample thread under its line, in that line's pane and right of its gutter, so it never
    /// covers the other pane: the card in place, or the count that stands for it.
    fn comment(&self, ui: &mut Ui, is_shown: bool) {
        let palette = theme::palette(ui.ctx());
        let width = ui.available_width();
        let pane_left =
            if self.layout == DiffLayout::SideBySide { (width / 2.0).floor() } else { 0.0 };
        let indent = pane_left + self.metrics.text_inset(self.layout == DiffLayout::Inline);
        ui.add_space(style::SPACE_XS);
        ui.horizontal(|ui| {
            ui.add_space(indent);
            if !is_shown {
                cv_widgets::badges::pill(ui, "1 comment", palette.selection, palette.text);
                return;
            }
            egui::Frame::new()
                .fill(palette.surface_subtle)
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(style::RADIUS_CARD)
                .inner_margin(egui::Margin::same(style::SPACE_S as i8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width() - style::SPACE_M);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Reviewer")
                                .font(theme::fonts::semibold_font(style::SMALL_SIZE)),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new("Could this cap move into the settings?")
                                    .size(style::SMALL_SIZE),
                            )
                            .wrap(),
                        );
                    });
                });
        });
        ui.add_space(style::SPACE_XS);
    }

    /// Columns per visual row of a pane `width` wide when wrapping, by the diff's own rule.
    fn wrap_columns(&self, width: f32) -> Option<usize> {
        let metrics = self.metrics;
        self.wrap.then(|| {
            let code_area = width - metrics.text_inset(self.layout == DiffLayout::Inline);
            let code_width = (code_area - TEXT_END_PADDING).max(metrics.char_width * 8.0);
            ((code_width / metrics.char_width).floor() as usize).max(8)
        })
    }

    /// `line` as pane `slot` shows it: side by side's left pane numbers the old version.
    fn code_line(&self, line: Line, slot: usize) -> CodeLine<'_> {
        let index = line.index as u32;
        // The sample adds one line (new index 5), so the lines after it sit one higher in the new
        // version.
        let old_of_new = if line.index > 5 { index - 1 } else { index };
        let (old_line, new_line) = match line.kind {
            LineKind::Deleted => (FIRST + index, 0),
            LineKind::Added => (0, FIRST + index),
            LineKind::Context | LineKind::Hunk => (FIRST + old_of_new, FIRST + index),
        };
        let is_old = line.kind == LineKind::Deleted
            || (slot == 0 && self.layout == DiffLayout::SideBySide && line.kind != LineKind::Added);
        let spans = if line.kind == LineKind::Deleted { &self.old_spans } else { &self.new_spans };
        let spans = spans.as_ref().and_then(|spans| spans.get(line.index));
        CodeLine {
            text: text(line),
            kind: line.kind,
            old_line,
            new_line,
            number: if is_old { old_line } else { new_line },
            spans: spans.map_or(&[], Vec::as_slice),
            has_file_spans: spans.is_some(),
        }
    }
}

/// How many visual rows `line` takes; the sample is ASCII without tabs.
fn rows(line: Line, wrap_columns: Option<usize>) -> usize {
    let columns = text(line).len();
    wrap_columns.map_or(1, |per_row| columns.div_ceil(per_row).max(1))
}

fn text(line: Line) -> &'static str {
    let lines: &[&'static str] = if line.kind == LineKind::Deleted { &OLD } else { &NEW };
    lines.get(line.index).copied().unwrap_or_default()
}

/// The changed word of a replaced line, as a byte range of its text.
fn changed_words(line: Line) -> Option<std::ops::Range<usize>> {
    let text = text(line);
    CHANGED_WORDS
        .iter()
        .find(|(kind, index, _)| *kind == line.kind && *index == line.index)
        .and_then(|(_, _, word)| text.find(word).map(|start| start..start + word.len()))
}
