//! Above the cards: "3 changed files" with the line totals, Collapse all, Preview and the layout
//! choice, over the folder as a trail of links; and the notice that stands in for the cards when there
//! are none.

use egui::{CursorIcon, FontId, Sense, Ui, vec2};
use review_core::ChangeBreakdown;
use review_store::DiffLayout;

use super::{SummaryEvent, SummaryInputs};
use cv_base::theme::{self, fonts, icons, style};
use cv_pr_rendered::file_header;
use cv_widgets::crumb_fit::{self, CrumbWidth, Spacing};
use cv_widgets::numbers::group_digits;
use cv_widgets::states::{self, Action};
use cv_widgets::tabs::{self, Segment};
use cv_widgets::{buttons, change_breakdown, elide, focus, glyph, row};

/// Room the header keeps for the collapse button and the gap before the title when the layout
/// choice and Preview show their labels.
const LAYOUT_LABELS_SPARE: f32 = 160.0;
const PREVIEW_HINT: &str = "Show the Markdown files rendered";
/// Narrowest a middle level of the folder trail is cut to before levels fold into "…".
const CRUMB_SHORT_WIDTH: f32 = 96.0;
const CRUMB_PADDING: egui::Vec2 = vec2(style::SPACE_XS, 2.0);
const PADDING_Y: i8 = 10;
/// The horizontal gap between the header's items.
const ITEM_GAP: f32 = 6.0;

/// What the user did in the header.
pub enum HeaderAction {
    Event(SummaryEvent),
    /// Collapse every card, or expand them all when every one is collapsed.
    ToggleCollapseAll,
}

/// What the header counts: the cards and their files and lines by kind of file.
pub struct Counts<'a> {
    pub cards: usize,
    pub breakdown: &'a ChangeBreakdown,
    /// Every card is collapsed, so the button expands them.
    pub all_collapsed: bool,
    /// Cards of Markdown files, which Preview shows rendered.
    pub markdown: usize,
}

/// "3 changed files", the line totals and the folder as a trail of links, over the cards.
pub fn show(ui: &mut Ui, inputs: &SummaryInputs<'_>, counts: &Counts<'_>) -> Option<HeaderAction> {
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    egui::Frame::new()
        .fill(palette.surface)
        .inner_margin(egui::Margin::symmetric(style::SPACE_L as i8, PADDING_Y))
        .stroke(egui::Stroke::new(1.0, palette.border))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = vec2(ITEM_GAP, style::SPACE_XS);
            ui.horizontal(|ui| {
                title(ui, counts);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    action = controls(ui, inputs, counts);
                });
            });
            ui.horizontal(|ui| {
                if let Some(folder) = breadcrumbs(ui, inputs.folder) {
                    action = Some(HeaderAction::Event(SummaryEvent::OpenFolder(folder)));
                }
            });
        });
    action
}

/// "3 changed files +120 −34"; hovering or focusing the line totals shows their breakdown.
fn title(ui: &mut Ui, counts: &Counts<'_>) {
    let palette = theme::palette(ui.ctx());
    let title = if counts.cards == 1 {
        "1 changed file".to_owned()
    } else {
        format!("{} changed files", group_digits(counts.cards))
    };
    ui.label(
        egui::RichText::new(title)
            .font(fonts::semibold_font(style::SECTION_SIZE))
            .color(palette.text),
    );
    let lines = counts.breakdown.total();
    if lines.changed() > 0 {
        ui.add_space(style::SPACE_XS);
        let added = format!("+{}", group_digits(lines.added as usize));
        let deleted = format!("−{}", group_digits(lines.deleted as usize));
        let added_label =
            ui.label(egui::RichText::new(&added).size(style::SMALL_SIZE).color(palette.success));
        let deleted_label =
            ui.label(egui::RichText::new(&deleted).size(style::SMALL_SIZE).color(palette.error));
        let rect = added_label.rect.union(deleted_label.rect);
        let totals = ui.interact(
            rect,
            ui.scope_id().with("summary_line_totals"),
            Sense::hover() | Sense::focusable_noninteractive(),
        );
        totals.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{added} {deleted}"))
        });
        focus::ring(ui, &totals, rect.expand(2.0));
        change_breakdown::attach(&totals, counts.breakdown);
    }
}

/// From the right: the layout choice, Preview, then Collapse all (or Expand all).
fn controls(ui: &mut Ui, inputs: &SummaryInputs<'_>, counts: &Counts<'_>) -> Option<HeaderAction> {
    // Rightmost, so it stays put whatever the collapse button says.
    let is_labelled = has_label_room(ui);
    let picked = layout_choice(ui, inputs.layout, is_labelled);
    // Only for a folder with Markdown files: elsewhere it would switch nothing.
    let previews = counts.markdown > 0
        && file_header::preview_switch(ui, inputs.preview, is_labelled)
            .on_hover_text(PREVIEW_HINT)
            .clicked();
    let (icon, label) = if counts.all_collapsed {
        (icons::CARET_DOWN, "Expand all")
    } else {
        (icons::CARET_RIGHT, "Collapse all")
    };
    let toggles = counts.cards > 0 && buttons::subtle(ui, icon, label).clicked();
    if let Some(layout) = picked {
        return Some(HeaderAction::Event(SummaryEvent::SwitchLayout(layout)));
    }
    if previews {
        return Some(HeaderAction::Event(SummaryEvent::SwitchPreview(!inputs.preview)));
    }
    toggles.then_some(HeaderAction::ToggleCollapseAll)
}

/// The header has the room for the layout choice's and Preview's labels.
fn has_label_room(ui: &Ui) -> bool {
    let labelled = tabs::segments_width(ui, &layout_segments(true))
        + file_header::preview_switch_width(ui, true)
        + ui.spacing().item_spacing.x;
    ui.available_width() >= labelled + LAYOUT_LABELS_SPARE
}

/// The file header's layouts but New only (the file view's own), with their labels or icons.
fn layout_segments(is_labelled: bool) -> Vec<Segment<'static, DiffLayout>> {
    file_header::LAYOUTS
        .iter()
        .filter(|(layout, ..)| *layout != DiffLayout::NewOnly)
        .map(|&(layout, icon, label, hint)| {
            let segment = Segment::new(layout, label).with_hint(hint);
            if is_labelled { segment } else { segment.with_icon(icon) }
        })
        .collect()
}

/// The file header's Inline / Side by side choice: labelled where the header has the room, icons
/// otherwise. Returns the layout picked.
fn layout_choice(ui: &mut Ui, current: DiffLayout, is_labelled: bool) -> Option<DiffLayout> {
    // New only shows inline here.
    let mut chosen =
        if current == DiffLayout::SideBySide { DiffLayout::SideBySide } else { DiffLayout::Inline };
    tabs::segments(ui, &mut chosen, &layout_segments(is_labelled)).then_some(chosen)
}

/// The trail's levels as (folder path, label): "All files" for the root, then one per folder
/// down to `folder`.
fn crumb_levels(folder: &str) -> Vec<(String, String)> {
    let mut levels = vec![(String::new(), "All files".to_owned())];
    let mut path = String::new();
    for segment in folder.split('/').filter(|segment| !segment.is_empty()) {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(segment);
        levels.push((path.clone(), segment.to_owned()));
    }
    levels
}

/// "All files / src / app": every level above the folder leads to its summary. A trail wider than
/// the row cuts long middle levels, then folds them into a "…" whose list leads to each, as the
/// title bar's trail does ([`crumb_fit`]). Returns the folder clicked.
fn breadcrumbs(ui: &mut Ui, folder: &str) -> Option<String> {
    let palette = theme::palette(ui.ctx());
    let levels = crumb_levels(folder);
    let last = levels.len() - 1;
    let font = FontId::proportional(style::BODY_SIZE);
    let text_width = |text: &str| {
        ui.painter().layout_no_wrap(text.to_owned(), font.clone(), palette.text).size().x
    };
    let widths: Vec<CrumbWidth> = levels
        .iter()
        .enumerate()
        .map(|(index, (_, label))| {
            let full = text_width(label) + CRUMB_PADDING.x * 2.0;
            let is_middle = index > 0 && index < last;
            CrumbWidth { full, short: if is_middle { full.min(CRUMB_SHORT_WIDTH) } else { full } }
        })
        .collect();
    let separator = FontId::proportional(style::TINY_SIZE);
    let spacing = Spacing {
        separator: ui
            .painter()
            .layout_no_wrap(icons::CARET_RIGHT.to_owned(), separator.clone(), palette.text_disabled)
            .size()
            .x
            + ui.spacing().item_spacing.x * 2.0,
        overflow: text_width("…") + CRUMB_PADDING.x * 2.0,
    };
    let shown = crumb_fit::fit(&widths, 0, spacing, ui.available_width());
    let hidden: Vec<usize> = (0..levels.len()).filter(|index| shown[*index].is_none()).collect();
    let mut clicked = None;
    for (index, ((path, label), width)) in levels.iter().zip(&shown).enumerate() {
        let Some(width) = *width else {
            if hidden.first() == Some(&index) {
                crumb_separator(ui, &separator);
                clicked = clicked.or(hidden_levels(ui, &levels, &hidden, spacing.overflow));
            }
            continue;
        };
        if index > 0 {
            crumb_separator(ui, &separator);
        }
        if crumb(ui, path, label, width, index == last) {
            clicked = Some(path.clone());
        }
    }
    clicked
}

fn crumb_separator(ui: &mut Ui, font: &FontId) {
    let palette = theme::palette(ui.ctx());
    glyph::decorative(
        ui,
        egui::RichText::new(icons::CARET_RIGHT).font(font.clone()).color(palette.text_disabled),
    );
}

/// One level of the trail, cut to `width`: a link to the folder's summary, or the current folder
/// as plain text. Returns true when it was clicked.
fn crumb(ui: &mut Ui, path: &str, label: &str, width: f32, is_current: bool) -> bool {
    let palette = theme::palette(ui.ctx());
    let font = FontId::proportional(style::BODY_SIZE);
    let galley =
        elide::line(ui.painter(), label, font, palette.text, width - CRUMB_PADDING.x * 2.0);
    let is_cut = galley.elided;
    let sense = if is_current { Sense::hover() } else { Sense::click() };
    let (rect, response) = ui.allocate_exact_size(galley.size() + CRUMB_PADDING * 2.0, sense);
    if is_current {
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{label} (current folder)"))
        });
        ui.painter().galley(rect.min + CRUMB_PADDING, galley, palette.text);
        if is_cut {
            response.on_hover_text(path);
        }
        return false;
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, label));
    let response =
        response.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(if path.is_empty() {
            "Summary of all changed files".to_owned()
        } else {
            format!("Summary of {path}")
        });
    row::card_background(ui, &response, rect, egui::Color32::TRANSPARENT, style::RADIUS_CONTROL);
    let color = if response.hovered() { palette.accent_text } else { palette.text_secondary };
    ui.painter().galley(rect.min + CRUMB_PADDING, galley, color);
    focus::ring(ui, &response, rect);
    response.clicked()
}

/// The "…" standing for the folded levels of the folder trail; its list leads to each of them.
fn hidden_levels(
    ui: &mut Ui,
    levels: &[(String, String)],
    hidden: &[usize],
    width: f32,
) -> Option<String> {
    let palette = theme::palette(ui.ctx());
    let galley = ui.painter().layout_no_wrap(
        "…".to_owned(),
        FontId::proportional(style::BODY_SIZE),
        palette.text_secondary,
    );
    let (rect, response) = ui
        .allocate_exact_size(vec2(width, galley.size().y + CRUMB_PADDING.y * 2.0), Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Show hidden folders"));
    let response =
        response.on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Show hidden folders");
    row::card_background(ui, &response, rect, egui::Color32::TRANSPARENT, style::RADIUS_CONTROL);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, palette.text_secondary);
    focus::ring(ui, &response, rect);
    let mut picked = None;
    egui::Popup::menu(&response).show(|ui| {
        for &index in hidden {
            let (path, label) = &levels[index];
            if buttons::menu_item(ui, icons::FOLDER, label, None, None)
                .on_hover_text(format!("Summary of {path}"))
                .clicked()
            {
                picked = Some(path.clone());
            }
        }
    });
    picked
}

/// Nothing to summarize: no changed files, or none the filters let through.
pub fn empty_state(ui: &mut Ui, inputs: &SummaryInputs<'_>) -> Option<SummaryEvent> {
    let palette = theme::palette(ui.ctx());
    if inputs.is_filtered {
        let actions = [Action::secondary(icons::X, "Clear filters")];
        let detail = "No changed file here matches the category, search or viewed filter.";
        return states::notice(
            ui,
            icons::FUNNEL,
            palette.text_secondary,
            "No files match",
            detail,
            &actions,
        )
        .map(|_| SummaryEvent::ClearFilters);
    }
    let actions = [Action::secondary(icons::FOLDER_OPEN, "Show all changed files")];
    let detail = "This folder has no changed files in this comparison.";
    states::notice(ui, icons::FOLDER, palette.text_secondary, "No changes here", detail, &actions)
        .map(|_| SummaryEvent::OpenFolder(String::new()))
}
