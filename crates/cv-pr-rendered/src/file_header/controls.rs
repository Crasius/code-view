//! The bar's controls, drawn from the right: the viewed mark and Preview, the diff's switches
//! (wrap or the reading column's width, whitespace, comments), the context and layout choices or
//! the New file / Deleted file chip, and the change arrows with their counter. Each switch saves
//! its setting when it changed; [`Fit`] says which of them show.

use egui::{Align2, Color32, FontId, Rect, Response, Sense, Ui, pos2, vec2};
use review_core::DiffSide;
use review_git::ChangedFile;
use review_store::{AppSettings, DiffLayout};

use super::{
    CHIP_HEIGHT, CHIP_PADDING, CONTEXT_CHOICES, Fit, HeaderAction, HeaderInputs, LAYOUTS,
    MARK_VIEWED_HINT, PREVIEW_LABEL, PREVIOUS_CHANGE_HINT, RENDERED_COLUMNS_WIDTH_HINT,
    RENDERED_CONTEXT_CHOICES, RENDERED_WHITESPACE_HINT, ROW_HEIGHT, SEPARATOR_WIDTH,
    TOGGLE_ICON_GAP, UNMARK_VIEWED_HINT, Widths, comments_hint, paint_tabular, position_text,
    selected_context, tabular_width, text_width, viewed_label,
};
use crate::preview::{self, Preview};
use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, motion, style};
use cv_pr_diff::diff_view::Direction;
use cv_services::Services;
use cv_widgets::{buttons, focus, row, tabs};

pub(super) fn show(
    ui: &mut Ui,
    inputs: &HeaderInputs<'_>,
    fit: &Fit,
    widths: &Widths,
    services: &mut Services,
    preview: &mut Preview,
    action: &mut HeaderAction,
) {
    if viewed_toggle(ui, inputs.is_viewed, fit, widths) {
        action.toggle_viewed = true;
    }
    preview_toggle(ui, inputs.file, fit, widths, preview);
    separator(ui);

    // The diff's controls wait, disabled, until there is a diff on screen.
    let settings = &mut services.settings;
    let mut changed = false;
    if fit.wrap && inputs.is_rendered {
        changed |= full_width_switch(ui, inputs, widths.wrap, settings, preview);
    } else if fit.wrap {
        changed |= wrap_switch(ui, inputs.changes.is_some(), widths.wrap, settings);
    }
    if fit.whitespace {
        changed |= whitespace_switch(ui, inputs, widths.whitespace, settings);
    }
    if fit.comments {
        changed |= comments_switch(ui, inputs.changes.is_some(), widths.comments, settings);
    }
    // Known from the file's kind alone, so it shows at once. Only one of the two is drawn: an empty
    // group would still take the spacing after it and push the rest of the bar along.
    if let Some(side) = inputs.whole_file {
        whole_file_chip(ui, side, widths.layout_slot(fit));
    } else {
        ui.add_enabled_ui(inputs.changes.is_some(), |ui| {
            layout_choices(ui, fit, widths, settings, inputs.is_rendered, &mut changed, action);
        });
    }
    if changed || action.layout_changed {
        services.save_settings();
    }
    if !fit.arrows && !fit.counter {
        return;
    }
    separator(ui);
    let before = ui.cursor().max.x;
    if fit.arrows {
        action.navigate = change_arrows(ui, inputs.changes).or(action.navigate);
    }
    if fit.counter {
        position_counter(ui, inputs.changes, widths.counter);
    }
    if let Some(text) = inputs.edge_hint {
        // Right to left: the arrows and the counter end where the cursor was before them.
        let row = ui.max_rect();
        let anchor = Rect::from_x_y_ranges(ui.cursor().max.x..=before, row.y_range());
        super::edge_hint(ui, anchor, text);
    }
}

/// "Mark as viewed": returns whether it was clicked.
fn viewed_toggle(ui: &mut Ui, is_viewed: bool, fit: &Fit, widths: &Widths) -> bool {
    let face = Face {
        icon: viewed_icon(is_viewed),
        label: fit.toggle_labels.then(|| viewed_label(is_viewed)),
    };
    let viewed = toggle(
        ui,
        widths.viewed.pick(fit.toggle_labels),
        &face,
        is_viewed,
        egui::Role::CheckBox,
        "Viewed",
    );
    let hint = if is_viewed { UNMARK_VIEWED_HINT } else { MARK_VIEWED_HINT };
    viewed.on_hover_text(hint).clicked()
}

/// Rendered text always wraps; the wrap switch's slot holds the reading column's switch instead.
/// Side by side each half always takes its whole width: the switch rests, filled. Returns whether
/// a setting changed.
fn full_width_switch(
    ui: &mut Ui,
    inputs: &HeaderInputs<'_>,
    width: f32,
    settings: &mut AppSettings,
    preview: &mut Preview,
) -> bool {
    let is_columns = settings.diff_layout == DiffLayout::SideBySide && inputs.whole_file.is_none();
    let fills = is_columns || preview.fills_width();
    let switch = ui
        .add_enabled_ui(!is_columns, |ui| {
            let face = Face { icon: icons::ARROWS_OUT_LINE_HORIZONTAL, label: None };
            toggle(ui, width, &face, fills, egui::Role::Switch, "Full width")
        })
        .inner
        .on_disabled_hover_text(RENDERED_COLUMNS_WIDTH_HINT);
    let clicked = switch.on_hover_text(width_hint(fills)).clicked();
    if clicked {
        preview.switch_width(&mut settings.rendered_width);
    }
    clicked
}

/// "Wrap long lines", usable once a diff is on screen (`has_diff`). Returns whether it changed.
fn wrap_switch(ui: &mut Ui, has_diff: bool, width: f32, settings: &mut AppSettings) -> bool {
    let wrap = ui
        .add_enabled_ui(has_diff, |ui| {
            let face = Face { icon: icons::ARROW_ELBOW_DOWN_LEFT, label: None };
            toggle(ui, width, &face, settings.wrap_lines, egui::Role::Switch, "Wrap long lines")
        })
        .inner;
    let clicked =
        wrap.on_hover_text(concat!("Wrap long lines (", shortcut!(alt "Z"), ")")).clicked();
    if clicked {
        settings.wrap_lines = !settings.wrap_lines;
    }
    clicked
}

/// "Ignore whitespace": not for rendered text, but usable to show the changes it hid. Returns
/// whether it changed.
fn whitespace_switch(
    ui: &mut Ui,
    inputs: &HeaderInputs<'_>,
    width: f32,
    settings: &mut AppSettings,
) -> bool {
    let is_rendered = inputs.is_rendered;
    let is_enabled = (inputs.changes.is_some() && !is_rendered) || inputs.hides_whitespace;
    let ignore = ui
        .add_enabled_ui(is_enabled, |ui| {
            let face = Face { icon: icons::PARAGRAPH, label: None };
            toggle(
                ui,
                width,
                &face,
                settings.ignore_whitespace,
                egui::Role::Switch,
                "Ignore whitespace",
            )
        })
        .inner;
    let ignore =
        if is_rendered { ignore.on_disabled_hover_text(RENDERED_WHITESPACE_HINT) } else { ignore };
    let hint = if settings.ignore_whitespace {
        concat!(
            "Ignore whitespace: on. Lines that differ only in whitespace or line endings count as unchanged (",
            shortcut!(alt "W"),
            ")"
        )
    } else {
        concat!(
            "Ignore whitespace: lines that differ only in whitespace or line endings count as unchanged (",
            shortcut!(alt "W"),
            ")"
        )
    };
    let clicked = ignore.on_hover_text(hint).clicked();
    if clicked {
        settings.ignore_whitespace = !settings.ignore_whitespace;
    }
    clicked
}

/// "Show comments in the diff". Both the text and the rendered diff show the comments; the icon
/// keeps its slot either way. Returns whether it changed.
fn comments_switch(ui: &mut Ui, has_diff: bool, width: f32, settings: &mut AppSettings) -> bool {
    let shows = settings.show_comments_in_diff;
    let comments = ui
        .add_enabled_ui(has_diff, |ui| {
            let face = Face { icon: icons::CHAT_CIRCLE_TEXT, label: None };
            toggle(ui, width, &face, shows, egui::Role::Switch, "Show comments in the diff")
        })
        .inner;
    let clicked = comments.on_hover_text(comments_hint(shows)).clicked();
    if clicked {
        settings.show_comments_in_diff = !shows;
    }
    clicked
}

/// Next and previous change, usable while the diff on screen has changes.
fn change_arrows(ui: &mut Ui, changes: Option<(usize, usize)>) -> Option<Direction> {
    let has_changes = changes.is_some_and(|(_, total)| total > 0);
    let next = ui
        .add_enabled_ui(has_changes, |ui| {
            buttons::icon(ui, icons::ARROW_DOWN, "Next change (N or F7)")
        })
        .inner;
    let previous = ui
        .add_enabled_ui(has_changes, |ui| buttons::icon(ui, icons::ARROW_UP, PREVIOUS_CHANGE_HINT))
        .inner;
    if previous.clicked() {
        Some(Direction::Previous)
    } else {
        next.clicked().then_some(Direction::Next)
    }
}

/// "2 of 7": the change in focus, right-aligned in a slot `width` wide or as wide as the text.
fn position_counter(ui: &mut Ui, changes: Option<(usize, usize)>, width: f32) {
    let palette = theme::palette(ui.ctx());
    let text = changes.map(position_text).unwrap_or_default();
    let font = FontId::proportional(style::SMALL_SIZE);
    let width = width.max(tabular_width(ui, &text, &font));
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
    paint_tabular(
        ui.painter(),
        pos2(rect.right(), rect.center().y),
        &text,
        &font,
        palette.text_secondary,
    );
}

/// The width switch's tooltip: what it shows now and what a click does.
fn width_hint(fills: bool) -> &'static str {
    if fills {
        "Full width: the text fills the pane. Click for a centered reading column, or drag the text's right edge inward"
    } else {
        "Reading column: drag its edges to resize it, double-click one to reset. Click to fill the pane"
    }
}

/// The context lines (blocks, rendered) and the layout of a changed file.
fn layout_choices(
    ui: &mut Ui,
    fit: &Fit,
    widths: &Widths,
    settings: &mut AppSettings,
    is_rendered: bool,
    changed: &mut bool,
    action: &mut HeaderAction,
) {
    if fit.context {
        let (choices, hint) = if is_rendered {
            (
                RENDERED_CONTEXT_CHOICES,
                "Unchanged blocks shown around each change; the rest fold away",
            )
        } else {
            (CONTEXT_CHOICES, "Unchanged lines shown around each change")
        };
        egui::ComboBox::from_id_salt("context_lines")
            .width(widths.context)
            .selected_text(selected_context(settings, is_rendered))
            .show_ui(ui, |ui| {
                for (lines, label) in choices {
                    *changed |=
                        ui.selectable_value(&mut settings.context_lines, lines, label).changed();
                }
            })
            .response
            .on_hover_text(hint);
    }
    action.layout_changed = if fit.layout_labels {
        tabs::segmented_with_hints(
            ui,
            &mut settings.diff_layout,
            &LAYOUTS.map(|(layout, _, label, hint)| (layout, label, hint)),
        )
    } else {
        tabs::segmented_icons(ui, &mut settings.diff_layout, &LAYOUTS)
    };
}

/// Always in the same place: off and disabled for files without a preview, on and disabled for
/// images, which always show as images.
fn preview_toggle(
    ui: &mut Ui,
    file: &ChangedFile,
    fit: &Fit,
    widths: &Widths,
    preview: &mut Preview,
) {
    let is_on = preview.is_active(file);
    let is_optional = preview::is_optional(file);
    let width = widths.preview.pick(fit.toggle_labels);
    let response = ui
        .add_enabled_ui(is_optional, |ui| preview_switch(ui, width, is_on, fit.toggle_labels))
        .inner;
    let disabled_hint = if preview::can_preview(file) {
        "Images always show as a preview"
    } else {
        "Images, SVG and Markdown files have a preview"
    };
    if response
        .on_hover_text("Show the rendered file")
        .on_disabled_hover_text(disabled_hint)
        .clicked()
    {
        preview.set_enabled(!is_on);
    }
}

/// The Preview switch `width` wide, labelled or as its icon: the file's in this bar, the folder's in
/// the summary's header.
pub(super) fn preview_switch(ui: &mut Ui, width: f32, is_on: bool, is_labelled: bool) -> Response {
    let face = Face { icon: icons::EYE, label: is_labelled.then_some(PREVIEW_LABEL) };
    toggle(ui, width, &face, is_on, egui::Role::Switch, "Preview")
}

/// A vertical hairline between groups of controls.
fn separator(ui: &mut Ui) {
    ui.add(egui::Separator::default().vertical().spacing(SEPARATOR_WIDTH));
}

/// What a toggle shows: its icon, and its label where the bar has room for it.
struct Face<'a> {
    icon: &'a str,
    label: Option<&'a str>,
}

/// A borderless on/off button, tinted while on, its icon in the theme's weight and Fill while on.
/// The caller gives its width, so a state with a shorter label keeps the size of the longer one.
fn toggle(
    ui: &mut Ui,
    width: f32,
    face: &Face<'_>,
    is_on: bool,
    role: egui::Role,
    label: &str,
) -> Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(width, style::CONTROL_HEIGHT), Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(role, response.enabled(), is_on, label));
    let on = motion::toggle(ui.ctx(), response.id.with("toggle_on"), is_on, motion::FAST);
    let fill =
        motion::mix(row::state_fill(ui, &response, Color32::TRANSPARENT), palette.selection, on);
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
    }
    // A disabled toggle fades with its `Ui`.
    let color = motion::mix(palette.text, palette.accent_text, on);
    let icon_font = icons::state_font(&palette, icons::SIZE, is_on);
    let Some(text) = face.label else {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, face.icon, icon_font, color);
        focus::ring(ui, &response, rect);
        return response;
    };
    let galley =
        ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(style::BODY_SIZE), color);
    let content = icons::SIZE + TOGGLE_ICON_GAP + galley.size().x;
    let icon_center = pos2(rect.center().x - content / 2.0 + icons::SIZE / 2.0, rect.center().y);
    ui.painter().text(icon_center, Align2::CENTER_CENTER, face.icon, icon_font, color);
    let text_left = icon_center.x + icons::SIZE / 2.0 + TOGGLE_ICON_GAP;
    ui.painter().galley(pos2(text_left, rect.center().y - galley.size().y / 2.0), galley, color);
    focus::ring(ui, &response, rect);
    response
}

fn viewed_icon(is_viewed: bool) -> &'static str {
    if is_viewed { icons::CHECK_SQUARE } else { icons::SQUARE }
}

fn whole_file_text(side: DiffSide) -> (&'static str, &'static str, &'static str) {
    match side {
        DiffSide::New => (
            icons::FILE_PLUS,
            "New file",
            "Added in this pull request, so there is no old version to compare: the new content is shown whole. Changed files keep your layout.",
        ),
        DiffSide::Old => (
            icons::FILE_MINUS,
            "Deleted file",
            "Deleted in this pull request, so there is no new version: the removed content is shown whole. Changed files keep your layout.",
        ),
    }
}

/// "New file" or "Deleted file", centered where the context and layout choices would be: the file
/// has one version, so it is shown whole, and the layout the user picked applies again on the next
/// changed file. The slot keeps the choices' width, so nothing beside it moves; the label shows
/// whenever it fits the slot, even where the choices are down to icons.
fn whole_file_chip(ui: &mut Ui, side: DiffSide, slot_width: f32) {
    let palette = theme::palette(ui.ctx());
    let (glyph, label, hint) = whole_file_text(side);
    let color = match side {
        DiffSide::New => palette.success,
        DiffSide::Old => palette.error,
    };
    let font = FontId::proportional(style::SMALL_SIZE);
    let labeled = format!("{glyph}  {label}");
    let text = if text_width(ui, &labeled, &font) + CHIP_PADDING * 2.0 <= slot_width {
        labeled
    } else {
        glyph.to_owned()
    };
    let (slot, _) = ui.allocate_exact_size(vec2(slot_width, CHIP_HEIGHT), Sense::hover());
    let chip_width = (text_width(ui, &text, &font) + CHIP_PADDING * 2.0).min(slot.width());
    let chip = Rect::from_center_size(slot.center(), vec2(chip_width, CHIP_HEIGHT));
    let response = ui.interact(chip, ui.scope_id().with("whole_file_chip"), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, label));
    ui.painter().rect_filled(chip, style::RADIUS_CHIP, chip_fill(&palette, color));
    ui.painter().text(chip.center(), Align2::CENTER_CENTER, text, font, color);
    response.on_hover_text(hint);
}

/// A soft fill of `color` for a status chip on the header surface.
fn chip_fill(palette: &theme::Palette, color: Color32) -> Color32 {
    let amount = if palette.is_dark { 0.18 } else { 0.10 };
    cv_code::diff_paint::mix(palette.surface, color, amount)
}
