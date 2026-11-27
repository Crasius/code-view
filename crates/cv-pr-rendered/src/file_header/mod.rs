//! The bar above the diff: which file, how much changed, and the view controls. It sits outside
//! the scrolled rows, so it stays put while the diff scrolls.
//!
//! Every item's width is known before anything is drawn. The controls take theirs from the right,
//! the title gets the rest, and where the bar is too narrow the least important items shrink to an
//! icon or give way first (see [`Fit`]), so nothing is ever drawn over anything else.
//!
//! The bar keeps one shape for every file: the plan depends only on the bar's width, controls that
//! don't apply to a file (or not yet, while its diff loads) stay in place disabled, and the change
//! counter and line counts have reserved slots. Switching files changes the path text and nothing
//! moves. A step past the last (or first) change says what the next N (or P) does in a callout
//! under the change arrows ([`edge_hint`]), over the diff, so it moves nothing either.

mod controls;

use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Align2, Color32, FontId, Label, Layout, Painter, Pos2, Rect, Response, RichText, Sense,
    Ui, pos2, vec2,
};
use review_core::DiffSide;
use review_git::{ChangedFile, FULL_FILE_CONTEXT};
use review_store::{AppSettings, DiffLayout};

use crate::preview::Preview;
use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, style};
use cv_pr_diff::diff_view::Direction;
use cv_services::Services;
use cv_widgets::elide::text_width;
use cv_widgets::numbers::group_digits;
use cv_widgets::tabs::{self, Segment};
use cv_widgets::{buttons, glyph};

/// The viewed toggle's hints, in this bar and in focus mode's strip.
pub const MARK_VIEWED_HINT: &str = concat!("Mark as viewed (", shortcut!(alt "V"), ")");
pub const UNMARK_VIEWED_HINT: &str = concat!("Viewed: click or ", shortcut!(alt "V"), " to unmark");
pub const PREVIOUS_CHANGE_HINT: &str =
    concat!("Previous change (P or ", shortcut!(shift "F7"), ")");
/// Why the Full width switch rests over a rendered Markdown diff side by side.
const RENDERED_COLUMNS_WIDTH_HINT: &str = "Side by side, each version takes its half's whole width; the reading column applies to one column (Inline, New only, an added or deleted file)";
/// Why the whitespace switch rests while a rendered Markdown diff shows.
pub const RENDERED_WHITESPACE_HINT: &str =
    "The rendered view compares the text as it reads; switch Preview off to compare whitespace";

#[derive(Default)]
pub struct HeaderAction {
    pub toggle_viewed: bool,
    pub layout_changed: bool,
    pub navigate: Option<Direction>,
}

pub struct HeaderInputs<'a> {
    pub file: &'a ChangedFile,
    pub is_viewed: bool,
    /// (1-based change in focus or 0, number of changes) of the diff on screen; `None` while there
    /// is none (loading, an image, a binary file), which disables the diff's controls.
    pub changes: Option<(usize, usize)>,
    /// The diff on screen is a rendered Markdown diff: layout, context and the change arrows
    /// apply (the context counts blocks), whitespace does not, and the wrap switch's slot holds
    /// the reading column's Full width switch.
    pub is_rendered: bool,
    /// An added (`New`) or deleted (`Old`) file, shown whole: no layout or context choice applies.
    pub whole_file: Option<DiffSide>,
    /// "Ignore whitespace" left the file without changes: its switch stays usable to show them.
    pub hides_whitespace: bool,
    /// What N or P does now that a step found no change further that way, while it shows.
    pub edge_hint: Option<&'a str>,
}

const ROW_HEIGHT: f32 = 28.0;
/// The path's claim on the bar before the controls shrink their labels. The same for every file,
/// so the plan never changes with the path; a longer path elides its leading folders first.
const WANTED_PATH_WIDTH: f32 = 260.0;
/// Controls give way (rather than shrink) only to keep the path at least this wide.
const MIN_PATH_WIDTH: f32 = 120.0;
/// The rename source takes at most this share of the room left for the path.
const RENAME_SHARE: f32 = 0.4;
const SEPARATOR_WIDTH: f32 = 6.0;
/// Between a toggle's icon and its label, as in [`buttons`].
const TOGGLE_ICON_GAP: f32 = 6.0;
const PREVIEW_LABEL: &str = "Preview";
const CHIP_HEIGHT: f32 = 22.0;
const CHIP_PADDING: f32 = 8.0;
/// The widest change counter and line counts the slots are sized for; larger numbers grow them.
const WIDEST_COUNTERS: [&str; 3] = ["888 of 888", "888 changes", "No changes"];
const WIDEST_COUNTS: (&str, &str) = ("+8,888", "−8,888");
const CONTEXT_CHOICES: [(u32, &str); 4] =
    [(3, "3 lines"), (10, "10 lines"), (50, "50 lines"), (FULL_FILE_CONTEXT, "Full file")];
/// The same choices for a rendered Markdown diff, which keeps whole blocks around each change.
const RENDERED_CONTEXT_CHOICES: [(u32, &str); 4] =
    [(3, "3 blocks"), (10, "10 blocks"), (50, "50 blocks"), (FULL_FILE_CONTEXT, "Full file")];
/// The layout choice, listed right to left as the bar lays out its controls.
pub const LAYOUT_SEGMENTS: [Segment<'static, DiffLayout>; 3] = [
    Segment::new(DiffLayout::NewOnly, "New only")
        .with_hint(concat!(
            "Only the new version: changed lines marked, removed lines behind the red markers (",
            shortcut!(alt "N"),
            ")"
        ))
        .with_icon(icons::FILE_TEXT),
    Segment::new(DiffLayout::Inline, "Inline")
        .with_hint("Old and new lines in one column")
        .with_icon(icons::ROWS),
    Segment::new(DiffLayout::SideBySide, "Side by side")
        .with_hint("Old version left, new version right")
        .with_icon(icons::COLUMNS),
];
/// [`LAYOUT_SEGMENTS`] with their labels, for where the bar has room for them.
const LABELLED_LAYOUTS: [Segment<'static, DiffLayout>; 3] = {
    let [new_only, inline, side_by_side] = LAYOUT_SEGMENTS;
    [
        Segment { icon: None, ..new_only },
        Segment { icon: None, ..inline },
        Segment { icon: None, ..side_by_side },
    ]
};
/// [`LAYOUT_SEGMENTS`] as (layout, icon, label, tooltip), for the crates that still read tuples.
pub const LAYOUTS: [(DiffLayout, &str, &str, &str); 3] = {
    let [new_only, inline, side_by_side] = LAYOUT_SEGMENTS;
    [layout_tuple(new_only), layout_tuple(inline), layout_tuple(side_by_side)]
};

const fn layout_tuple(
    segment: Segment<'static, DiffLayout>,
) -> (DiffLayout, &'static str, &'static str, &'static str) {
    let icon = segment.icon.expect("every layout has an icon");
    (segment.value, icon, segment.label, segment.hint)
}

pub fn show(
    ui: &mut Ui,
    inputs: &HeaderInputs<'_>,
    services: &mut Services,
    preview: &mut Preview,
) -> HeaderAction {
    let palette = theme::palette(ui.ctx());
    let mut action = HeaderAction::default();
    egui::Frame::new()
        .fill(palette.surface)
        .inner_margin(egui::Margin::symmetric(12, 6))
        .stroke(egui::Stroke::new(1.0, palette.border))
        .show(ui, |ui| {
            let widths = Widths::measure(ui, &services.settings);
            let fit = Fit::choose(&widths, ui.available_width());
            // A right-to-left layout centers on the cross axis, so it needs a bounded height.
            let row = vec2(ui.available_width(), ROW_HEIGHT);
            ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                controls::show(ui, inputs, &fit, &widths, services, preview, &mut action);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    title(ui, inputs.file, &fit, &widths);
                });
            });
        });
    action
}

/// The bar's Preview switch for the folder summary's header, where it switches Markdown cards to
/// their rendered diff: labelled, or as its icon.
pub fn preview_switch(ui: &mut Ui, is_on: bool, is_labelled: bool) -> Response {
    let width = preview_switch_width(ui, is_labelled);
    controls::preview_switch(ui, width, is_on, is_labelled)
}

pub fn preview_switch_width(ui: &Ui, is_labelled: bool) -> f32 {
    let body = FontId::proportional(style::BODY_SIZE);
    toggle_width(is_labelled.then(|| text_width(ui, PREVIEW_LABEL, &body)))
}

/// An item's width with its label, and as an icon alone.
#[derive(Clone, Copy)]
struct Sizes {
    full: f32,
    compact: f32,
}

impl Sizes {
    fn pick(self, is_full: bool) -> f32 {
        if is_full { self.full } else { self.compact }
    }
}

/// Every item's width, measured with the fonts it is drawn in. None depends on the file.
struct Widths {
    spacing: f32,
    viewed: Sizes,
    preview: Sizes,
    wrap: f32,
    whitespace: f32,
    comments: f32,
    context: f32,
    layouts: Sizes,
    arrow: f32,
    counter: f32,
    copy: f32,
    counts: f32,
}

impl Widths {
    fn measure(ui: &Ui, settings: &AppSettings) -> Self {
        let body = FontId::proportional(style::BODY_SIZE);
        let small = FontId::proportional(style::SMALL_SIZE);
        let spacing = ui.spacing().item_spacing.x;
        let toggle = |label: &str| toggle_width(Some(text_width(ui, label, &body)));
        let icon_toggle = toggle_width(None);
        Self {
            spacing,
            viewed: Sizes {
                full: toggle(viewed_label(false)).max(toggle(viewed_label(true))),
                compact: icon_toggle,
            },
            preview: Sizes { full: toggle(PREVIEW_LABEL), compact: icon_toggle },
            wrap: icon_toggle,
            whitespace: icon_toggle,
            comments: icon_toggle,
            context: context_width(ui, settings),
            layouts: Sizes {
                full: tabs::segments_width(ui, &LABELLED_LAYOUTS),
                compact: tabs::segments_width(ui, &LAYOUT_SEGMENTS),
            },
            arrow: buttons::icon_width(),
            counter: WIDEST_COUNTERS
                .iter()
                .map(|text| tabular_width(ui, text, &small))
                .fold(0.0, f32::max),
            copy: buttons::icon_width(),
            counts: tabular_width(ui, WIDEST_COUNTS.0, &small)
                + spacing
                + tabular_width(ui, WIDEST_COUNTS.1, &small),
        }
    }

    /// Where the context choice and the layout choice go, or the New file / Deleted file chip: as
    /// wide as both choices and the spacing between them.
    fn layout_slot(&self, fit: &Fit) -> f32 {
        let context = if fit.context { self.context + self.spacing } else { 0.0 };
        context + self.layouts.pick(fit.layout_labels)
    }
}

/// What the bar shows at its width. [`Fit::STEPS`] go from least to most important: each one only
/// applies when the bar is still too narrow after the ones before it.
#[derive(Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one switch per part the bar can drop, turned off one by one in Fit::STEPS order"
)]
struct Fit {
    counter: bool,
    counts: bool,
    /// The layout choice with labels, not just icons.
    layout_labels: bool,
    /// "Mark as viewed" and "Preview" with their labels.
    toggle_labels: bool,
    copy: bool,
    context: bool,
    arrows: bool,
    comments: bool,
    whitespace: bool,
    wrap: bool,
}

impl Fit {
    const ALL: Self = Self {
        counter: true,
        counts: true,
        layout_labels: true,
        toggle_labels: true,
        copy: true,
        context: true,
        arrows: true,
        comments: true,
        whitespace: true,
        wrap: true,
    };

    const STEPS: [fn(&mut Self); 10] = [
        |fit| fit.counter = false,
        |fit| fit.counts = false,
        |fit| fit.layout_labels = false,
        |fit| fit.toggle_labels = false,
        |fit| fit.copy = false,
        |fit| fit.context = false,
        |fit| fit.arrows = false,
        |fit| fit.comments = false,
        |fit| fit.whitespace = false,
        |fit| fit.wrap = false,
    ];

    /// The first steps hide counts and shorten labels; the rest remove controls.
    const SHORTENING_STEPS: usize = 4;

    fn choose(widths: &Widths, width: f32) -> Self {
        let mut fit = Self::ALL;
        for (index, step) in Self::STEPS.iter().enumerate() {
            // The path's whole claim only shortens labels; controls give way only to its minimum.
            let path =
                if index < Self::SHORTENING_STEPS { WANTED_PATH_WIDTH } else { MIN_PATH_WIDTH };
            if fit.controls_width(widths) + path + fit.trailing_width(widths) <= width {
                break;
            }
            step(&mut fit);
        }
        fit
    }

    /// The controls' widths in the order [`controls`] draws them.
    fn controls(&self, widths: &Widths) -> Vec<f32> {
        let mut shown = vec![widths.viewed.pick(self.toggle_labels)];
        shown.extend([widths.preview.pick(self.toggle_labels), SEPARATOR_WIDTH]);
        if self.wrap {
            shown.push(widths.wrap);
        }
        if self.whitespace {
            shown.push(widths.whitespace);
        }
        if self.comments {
            shown.push(widths.comments);
        }
        // A whole file's chip takes the same room as one item: see [`Widths::layout_slot`].
        if self.context {
            shown.push(widths.context);
        }
        shown.push(widths.layouts.pick(self.layout_labels));
        if self.arrows || self.counter {
            shown.push(SEPARATOR_WIDTH);
        }
        if self.arrows {
            shown.extend([widths.arrow, widths.arrow]);
        }
        if self.counter {
            shown.push(widths.counter);
        }
        shown
    }

    /// The controls with the spacing after each.
    fn controls_width(&self, widths: &Widths) -> f32 {
        self.controls(widths).iter().map(|width| width + widths.spacing).sum()
    }

    /// The line counts and the copy button at the end of the title, with the spacing before each.
    fn trailing_width(&self, widths: &Widths) -> f32 {
        let copy = if self.copy { widths.spacing + widths.copy } else { 0.0 };
        let counts = if self.counts { widths.spacing + widths.counts } else { 0.0 };
        copy + counts
    }
}

/// A toggle's width with a label of `label_width`, or as an icon alone: the icon sits in a fixed
/// slot, so the width never depends on the glyph or on the toggle's state.
fn toggle_width(label_width: Option<f32>) -> f32 {
    match label_width {
        Some(label) => icons::SIZE + TOGGLE_ICON_GAP + label + style::SPACE_M * 2.0,
        None => (icons::SIZE + style::SPACE_S * 2.0).max(style::CONTROL_HEIGHT),
    }
}

/// The comments switch's hint, in this bar and in focus mode's strip.
pub fn comments_hint(shows: bool) -> &'static str {
    if shows {
        concat!(
            "Show comments in the diff: on. Off, a count beside a line stands for them (",
            shortcut!(alt "H"),
            ")"
        )
    } else {
        concat!(
            "Show comments in the diff: off. Click a count beside a line to show its comments (",
            shortcut!(alt "H"),
            ")"
        )
    }
}

/// The callout under `anchor` (the change arrows) after a step past the last or first change: one
/// line over the diff, right-aligned with the arrows, so nothing moves when it comes or goes.
pub fn edge_hint(ui: &Ui, anchor: Rect, text: &str) {
    let palette = theme::palette(ui.ctx());
    let at = pos2(anchor.right(), anchor.bottom() + style::SPACE_XS);
    egui::Area::new(egui::Id::unique("files_edge_hint"))
        .order(egui::Order::Tooltip)
        .fixed_pos(at)
        .pivot(Align2::RIGHT_TOP)
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = style::SPACE_S;
                    glyph::decorative(
                        ui,
                        RichText::new(icons::SKIP_FORWARD)
                            .font(icons::state_font(&palette, icons::SIZE_SMALL, false))
                            .color(palette.accent_text),
                    );
                    let label = ui.add(
                        Label::new(RichText::new(text).size(style::SMALL_SIZE).color(palette.text))
                            .extend()
                            .selectable(false),
                    );
                    label.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
                });
            });
        });
}

fn viewed_label(is_viewed: bool) -> &'static str {
    if is_viewed { "Viewed" } else { "Mark as viewed" }
}

fn selected_context(settings: &AppSettings, is_rendered: bool) -> String {
    let (choices, unit) =
        if is_rendered { (RENDERED_CONTEXT_CHOICES, "blocks") } else { (CONTEXT_CHOICES, "lines") };
    choices.iter().find(|(lines, _)| *lines == settings.context_lines).map_or_else(
        || format!("{} {unit}", settings.context_lines),
        |(_, label)| (*label).to_owned(),
    )
}

/// Wide enough for every choice and the selected one, in lines and in blocks, so picking another
/// or opening a rendered diff keeps the size.
fn context_width(ui: &Ui, settings: &AppSettings) -> f32 {
    let body = FontId::proportional(style::BODY_SIZE);
    let selected = [false, true]
        .map(|is_rendered| text_width(ui, &selected_context(settings, is_rendered), &body));
    let widest = CONTEXT_CHOICES
        .iter()
        .chain(&RENDERED_CONTEXT_CHOICES)
        .map(|(_, label)| text_width(ui, label, &body))
        .fold(selected[0].max(selected[1]), f32::max);
    let spacing = ui.spacing();
    widest + spacing.icon_spacing + spacing.icon_width + spacing.button_padding.x * 2.0
}

pub fn position_text((current, total): (usize, usize)) -> String {
    match (current, total) {
        (_, 0) => "No changes".to_owned(),
        (0, 1) => "1 change".to_owned(),
        (0, total) => format!("{} changes", group_digits(total)),
        (current, total) => format!("{current} of {}", group_digits(total)),
    }
}

fn counts_text(file: &ChangedFile) -> Option<(String, String)> {
    Some((
        format!("+{}", group_digits(file.additions? as usize)),
        format!("−{}", group_digits(file.deletions? as usize)),
    ))
}

/// Folder in secondary text, file name emphasized and the rename source on the left; the line
/// counts and the copy button at the right end, so they stay put from file to file. The path and
/// rename source shorten to the room the end leaves them.
fn title(ui: &mut Ui, file: &ChangedFile, fit: &Fit, widths: &Widths) {
    let palette = theme::palette(ui.ctx());
    let small = FontId::proportional(style::SMALL_SIZE);
    let counts = counts_text(file);
    // Counts beyond the reserved slot widen it into the path's room, never into the controls.
    let counts_width = counts.as_ref().map_or(widths.counts, |(additions, deletions)| {
        let width = tabular_width(ui, additions, &small)
            + widths.spacing
            + tabular_width(ui, deletions, &small);
        widths.counts.max(width)
    });
    let trailing =
        fit.trailing_width(widths) + if fit.counts { counts_width - widths.counts } else { 0.0 };
    let room = (ui.available_width() - trailing).max(0.0);
    let end =
        Rect::from_min_max(pos2(ui.cursor().min.x + room, ui.max_rect().top()), ui.max_rect().max);

    let renamed = file.old_path.as_deref().map(rename_text);
    let rename_width =
        renamed.as_ref().map_or(0.0, |text| text_width(ui, text, &small).min(room * RENAME_SHARE));
    let path_room = if renamed.is_some() { room - rename_width - widths.spacing } else { room };
    // Too little room even for an ellipsis: leave the path out rather than draw it over the controls.
    if let Some(job) = path_job(ui, &file.path, path_room) {
        let path = within(ui, path_room, |ui| ui.add(Label::new(job).truncate()))
            .on_hover_text(&file.path);
        path.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &file.path));
        if let (Some(old_path), Some(text)) = (&file.old_path, renamed) {
            let text = RichText::new(text).color(palette.text_secondary).size(style::SMALL_SIZE);
            let renamed_from = format!("Renamed from {old_path}");
            // Its tooltip says it in words; egui's own for a cut label would repeat the arrow.
            let label = within(ui, rename_width, |ui| {
                ui.add(Label::new(text).truncate().show_tooltip_when_elided(false))
            });
            glyph::speak_as(&label, &renamed_from);
            label.on_hover_text(renamed_from);
        }
    }

    ui.scope_builder(
        egui::UiBuilder::new().max_rect(end).layout(Layout::right_to_left(Align::Center)),
        |ui| {
            if fit.copy && buttons::icon(ui, icons::COPY, "Copy path").clicked() {
                ui.ctx().copy_text(file.path.clone());
            }
            if fit.counts {
                let (rect, response) =
                    ui.allocate_exact_size(vec2(counts_width, ROW_HEIGHT), Sense::hover());
                // Lines are counted after the file list arrives; the slot waits empty until then.
                let Some((additions, deletions)) = counts else {
                    return;
                };
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Label,
                        true,
                        format!("{additions} {deletions}"),
                    )
                });
                let right = pos2(rect.right(), rect.center().y);
                let deletions_left =
                    paint_tabular(ui.painter(), right, &deletions, &small, palette.error);
                paint_tabular(
                    ui.painter(),
                    pos2(deletions_left - widths.spacing, right.y),
                    &additions,
                    &small,
                    palette.success,
                );
            }
        },
    );
}

/// `folder/` (with its slash, empty at the root) and the file name.
fn split_path(path: &str) -> (&str, &str) {
    path.rfind('/').map_or(("", path), |slash| path.split_at(slash + 1))
}

fn rename_text(old_path: &str) -> String {
    format!("{}  {old_path}", icons::ARROW_LEFT)
}

/// The path as it fits `width`: whole; else with its leading folders elided ("…/Ai/Plan.cs"); else
/// the file name alone, which the label truncates at its end. `None` without room for an ellipsis.
pub fn path_job(ui: &Ui, path: &str, width: f32) -> Option<LayoutJob> {
    let palette = theme::palette(ui.ctx());
    let body = FontId::proportional(style::BODY_SIZE);
    let bold = theme::fonts::semibold_font(style::BODY_SIZE);
    if width < text_width(ui, "…", &body) {
        return None;
    }
    let (folder, name) = split_path(path);
    let room_for_folder = width - text_width(ui, name, &bold);
    let shown_folder = if text_width(ui, folder, &body) <= room_for_folder {
        Some(folder.to_owned())
    } else {
        // Whole folders from the left give way first, so what stays reads as a path.
        folder
            .match_indices('/')
            .map(|(slash, _)| format!("…{}", &folder[slash..]))
            .find(|shorter| text_width(ui, shorter, &body) <= room_for_folder)
    };
    let mut job = LayoutJob::default();
    if let Some(folder) = shown_folder.filter(|folder| !folder.is_empty()) {
        job.append(&folder, 0.0, TextFormat::simple(body, palette.text_secondary));
    }
    job.append(name, 0.0, TextFormat::simple(bold, palette.text));
    Some(job)
}

/// Adds a widget that takes at most `width`.
fn within(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui) -> Response) -> Response {
    ui.scope(|ui| {
        ui.set_max_width(width);
        add(ui)
    })
    .inner
}

/// The width of `text` with every digit as wide as the widest one, as [`paint_tabular`] draws it.
fn tabular_width(ui: &Ui, text: &str, font: &FontId) -> f32 {
    let digit = digit_width(ui.painter(), font);
    text.chars()
        .map(|character| {
            if character.is_ascii_digit() {
                digit
            } else {
                char_width(ui.painter(), character, font)
            }
        })
        .sum()
}

/// Draws `text` ending at `right` (vertically centered there) with tabular digits, so a counter
/// that counts never jitters; returns where the text starts.
fn paint_tabular(painter: &Painter, right: Pos2, text: &str, font: &FontId, color: Color32) -> f32 {
    let digit = digit_width(painter, font);
    let mut x = right.x;
    for character in text.chars().rev() {
        let advance =
            if character.is_ascii_digit() { digit } else { char_width(painter, character, font) };
        x -= advance;
        let center = pos2(x + advance / 2.0, right.y);
        painter.text(center, Align2::CENTER_CENTER, character, font.clone(), color);
    }
    x
}

fn digit_width(painter: &Painter, font: &FontId) -> f32 {
    ('0'..='9').map(|digit| char_width(painter, digit, font)).fold(0.0, f32::max)
}

fn char_width(painter: &Painter, character: char, font: &FontId) -> f32 {
    painter.layout_no_wrap(character.to_string(), font.clone(), Color32::PLACEHOLDER).size().x
}
