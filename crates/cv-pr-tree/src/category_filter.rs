//! The filter row above the file tree: a multi-select category dropdown ("All files 356",
//! "Code, Tests 344") and the "Hide viewed files" toggle beside it.
//!
//! Inside the dropdown every category is a checkbox row with its icon, label and file count, under
//! an "All files" row. Ctrl+click on a row, or its "Only" button, shows that category alone. Rows
//! are reachable with Tab and the arrow keys; Space or Enter toggles, Ctrl+Enter picks only that
//! row, Esc closes.

use std::collections::{BTreeSet, HashMap};

use egui::{
    Align2, Color32, FontId, Galley, Popup, PopupCloseBehavior, Rect, Response, Sense, Stroke,
    StrokeKind, Ui, pos2, vec2,
};
use review_core::FileCategory;
use review_git::ChangedFile;

use super::FileFilter;
use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, motion, style};
use cv_widgets::numbers::{group_digits, zeroed};
use cv_widgets::{buttons, checkbox, elide, focus, row};

/// Rows of the dropdown list.
const ROW_HEIGHT: f32 = 30.0;
const PADDING_X: f32 = 10.0;
const GAP: f32 = 8.0;
/// Space between the last category row and the hint under the list.
const HINT_GAP: f32 = 6.0;
/// Space between the dropdown and the hide-viewed toggle.
const TOGGLE_GAP: f32 = 4.0;
/// The dropdown never gets narrower than this, even in a very narrow side panel.
const MIN_BUTTON_WIDTH: f32 = 120.0;

pub fn counts(files: &[ChangedFile]) -> HashMap<FileCategory, usize> {
    let mut counts = HashMap::new();
    for file in files {
        *counts.entry(file.category).or_insert(0) += 1;
    }
    counts
}

/// The dropdown and the toggle on one row, `style::CONTROL_HEIGHT` high. Returns true when the
/// filter changed.
pub fn show(
    ui: &mut Ui,
    filter: &mut FileFilter,
    counts: &HashMap<FileCategory, usize>,
    total: usize,
) -> bool {
    let mut changed = false;
    let width = ui.available_width();
    let (row_rect, _) = ui.allocate_exact_size(vec2(width, style::CONTROL_HEIGHT), Sense::hover());
    let toggle_width = buttons::SUBTLE_HEIGHT;
    let button_width = (width - toggle_width - TOGGLE_GAP).max(MIN_BUTTON_WIDTH);
    let button_rect = Rect::from_min_size(row_rect.min, vec2(button_width, style::CONTROL_HEIGHT));
    changed |= dropdown(ui, button_rect, filter, counts, total);

    let toggle_rect = Rect::from_min_size(
        pos2(button_rect.right() + TOGGLE_GAP, row_rect.center().y - buttons::SUBTLE_HEIGHT / 2.0),
        vec2(toggle_width, buttons::SUBTLE_HEIGHT),
    );
    let toggle = ui
        .scope_builder(egui::UiBuilder::new().max_rect(toggle_rect), |ui| {
            buttons::icon_toggle(
                ui,
                icons::EYE,
                icons::EYE_SLASH,
                filter.hide_viewed,
                "Hide viewed files",
            )
        })
        .inner;
    if toggle.clicked() {
        filter.hide_viewed = !filter.hide_viewed;
        changed = true;
    }
    changed
}

/// The categories that have files, in the fixed order.
fn present(counts: &HashMap<FileCategory, usize>) -> Vec<FileCategory> {
    FileCategory::ALL.into_iter().filter(|category| counts.contains_key(category)).collect()
}

/// Selected categories in display order; empty means all.
fn selected(filter: &FileFilter, counts: &HashMap<FileCategory, usize>) -> Vec<FileCategory> {
    present(counts).into_iter().filter(|category| filter.categories.contains(category)).collect()
}

/// Ways to name the selection, longest first; the button shows the first that fits.
fn summaries(selected: &[FileCategory]) -> Vec<String> {
    match selected {
        [] => vec!["All files".to_owned()],
        [only] => vec![only.label().to_owned()],
        [first, rest @ ..] => {
            let names: Vec<&str> = selected.iter().map(|category| category.label()).collect();
            vec![
                names.join(", "),
                format!("{}, +{}", first.label(), rest.len()),
                format!("{} categories", selected.len()),
            ]
        }
    }
}

fn matching_count(
    selected: &[FileCategory],
    counts: &HashMap<FileCategory, usize>,
    total: usize,
) -> usize {
    if selected.is_empty() {
        return total;
    }
    selected.iter().map(|category| counts.get(category).copied().unwrap_or(0)).sum()
}

/// Every present category selected is the same as no filter; keep one representation.
fn normalize(filter: &mut FileFilter, counts: &HashMap<FileCategory, usize>) {
    filter.categories.retain(|category| counts.contains_key(category));
    if filter.categories.len() == present(counts).len() {
        filter.categories.clear();
    }
}

fn dropdown(
    ui: &mut Ui,
    rect: Rect,
    filter: &mut FileFilter,
    counts: &HashMap<FileCategory, usize>,
    total: usize,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let id = ui.make_persistent_id("file_category_dropdown");
    let response = ui.interact(rect, id, Sense::click());
    let picked = selected(filter, counts);
    let count = group_digits(matching_count(&picked, counts, total));
    let popup_id = Popup::default_response_id(&response);
    let is_open = Popup::is_id_open(ui.ctx(), popup_id);
    let is_filtered = !picked.is_empty();

    let body_font = FontId::proportional(style::BODY_SIZE);
    let count_galley =
        ui.painter().layout_no_wrap(count.clone(), body_font.clone(), palette.text_secondary);
    let text_left = rect.left() + PADDING_X + icons::SIZE + GAP;
    let caret_left = rect.right() - PADDING_X - icons::SIZE_SMALL;
    let text_right = caret_left - GAP - count_galley.size().x - GAP;
    let summary_galley = fitting_summary(
        ui,
        &summaries(&picked),
        body_font,
        palette.text,
        (text_right - text_left).max(0.0),
    );
    let summary = summary_galley.text().to_owned();
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "File categories");
        info.current_text_value = Some(format!("{summary}, {count} files"));
        info
    });

    let filtered = paint_field(ui, &response, is_open, is_filtered);
    let icon_color = motion::mix(palette.text_secondary, palette.accent_text, filtered);
    let icon_center = pos2(rect.left() + PADDING_X + icons::SIZE / 2.0, rect.center().y);
    // The funnel stays outlined: filled, it reads as a box. The tint and the outline show the filter.
    ui.painter().text(
        icon_center,
        Align2::CENTER_CENTER,
        icons::FUNNEL_SIMPLE,
        icons::rest_font(&palette, icons::SIZE),
        icon_color,
    );
    let text_color = if is_filtered { palette.accent_text } else { palette.text };
    ui.painter().galley(
        pos2(text_left, rect.center().y - summary_galley.size().y / 2.0),
        summary_galley,
        text_color,
    );
    ui.painter().galley(
        pos2(text_right + GAP, rect.center().y - count_galley.size().y / 2.0),
        count_galley,
        palette.text_secondary,
    );
    let caret = if is_open { icons::CARET_UP } else { icons::CARET_DOWN };
    ui.painter().text(
        pos2(caret_left + icons::SIZE_SMALL / 2.0, rect.center().y),
        Align2::CENTER_CENTER,
        caret,
        icons::rest_font(&palette, icons::SIZE_SMALL),
        palette.text_secondary,
    );
    focus::ring(ui, &response, rect);
    let response = if is_open {
        response
    } else {
        response.on_hover_text(concat!(
            "Filter files by category (",
            shortcut!(command click),
            " a category to show only it)"
        ))
    };

    let focus_first = response.clicked() && !is_open && focus::is_visible(ui.ctx());
    let mut changed = false;
    Popup::menu(&response)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .width(rect.width())
        .show(|ui| {
            ui.set_min_width(rect.width() - 12.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            changed = menu(ui, filter, counts, total, focus_first);
        });
    if changed {
        normalize(filter, counts);
    }
    changed
}

/// The dropdown's field-like face: a hairline outline, the selection tint while a filter is on.
/// Returns how far the filter tint has eased in (0 to 1).
fn paint_field(ui: &Ui, response: &Response, is_open: bool, is_filtered: bool) -> f32 {
    let palette = theme::palette(ui.ctx());
    let hover = row::hover_amount(ui, response);
    let filtered =
        motion::toggle(ui.ctx(), response.id.with("filtered"), is_filtered, motion::FAST);
    let rest = motion::mix(palette.surface, palette.selection, filtered);
    let fill = if response.is_pointer_button_down_on() {
        palette.pressed
    } else {
        motion::mix(rest, palette.hover, hover * 0.6)
    };
    let outline = if is_open {
        palette.accent
    } else {
        motion::mix(palette.border_strong, palette.accent, filtered)
    };
    ui.painter().rect(
        response.rect,
        style::RADIUS_CONTROL,
        fill,
        Stroke::new(1.0, outline),
        StrokeKind::Inside,
    );
    filtered
}

/// The longest summary that fits `width`, else the shortest one cut with "…".
fn fitting_summary(
    ui: &Ui,
    candidates: &[String],
    font: FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<Galley> {
    for candidate in candidates {
        let galley = ui.painter().layout_no_wrap(candidate.clone(), font.clone(), color);
        if galley.size().x <= width {
            return galley;
        }
    }
    let last = candidates.last().cloned().unwrap_or_default();
    let mut job = egui::text::LayoutJob::simple_singleline(last, font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    ui.painter().layout_job(job)
}

/// The list: "All files", one row per present category, and a hint. Returns true on a change.
fn menu(
    ui: &mut Ui,
    filter: &mut FileFilter,
    counts: &HashMap<FileCategory, usize>,
    total: usize,
    focus_first: bool,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let categories = present(counts);
    let is_all = filter.categories.is_empty();
    let mut changed = false;
    let count_width = count_column_width(ui, total);

    let all = ListRow {
        icon: icons::GIT_DIFF,
        label: "All files",
        count: total,
        is_checked: is_all,
        has_only: false,
    };
    let all_response = all.show(ui, "all", count_width);
    if focus_first {
        all_response.row.request_focus();
    }
    if all_response.row.clicked() && !is_all {
        filter.categories.clear();
        changed = true;
    }

    let line_y = ui.cursor().top() + 3.5;
    ui.add_space(style::SPACE_S);
    ui.painter().hline(ui.max_rect().x_range(), line_y, Stroke::new(1.0, palette.border));

    for category in &categories {
        let count = counts.get(category).copied().unwrap_or(0);
        let is_checked = is_all || filter.categories.contains(category);
        let entry = ListRow {
            icon: category_icon(*category),
            label: category.label(),
            count,
            is_checked,
            has_only: categories.len() > 1,
        };
        let response = entry.show(ui, category.label(), count_width);
        let is_only_click = response.row.clicked() && ui.input(|input| input.modifiers.command);
        if response.only.is_some_and(|only| only.clicked()) || is_only_click {
            filter.categories = BTreeSet::from([*category]);
            changed = true;
        } else if response.row.clicked() {
            toggle(filter, &categories, *category);
            changed = true;
        }
    }

    ui.add_space(HINT_GAP);
    let hint =
        egui::RichText::new(concat!(shortcut!(command click), " or Only shows one category"))
            .size(style::SMALL_SIZE)
            .color(palette.text_secondary);
    ui.horizontal(|ui| {
        ui.add_space(PADDING_X);
        ui.add(egui::Label::new(hint).wrap());
    });
    ui.add_space(style::SPACE_XS);
    changed
}

/// Checks or unchecks one category. From "all", unchecking one keeps the others; unchecking the
/// last checked one returns to all files, since an empty selection would show nothing.
fn toggle(filter: &mut FileFilter, categories: &[FileCategory], category: FileCategory) {
    if filter.categories.is_empty() {
        filter.categories = categories.iter().copied().filter(|other| *other != category).collect();
    } else if !filter.categories.remove(&category) {
        filter.categories.insert(category);
    }
}

/// Room for the widest count, so every row's counts line up and never shift.
fn count_column_width(ui: &Ui, total: usize) -> f32 {
    elide::text_width(ui, &zeroed(total as u64), &FontId::proportional(style::SMALL_SIZE))
}

struct ListRow<'a> {
    icon: &'a str,
    label: &'a str,
    count: usize,
    is_checked: bool,
    has_only: bool,
}

struct RowResponse {
    row: Response,
    only: Option<Response>,
}

impl ListRow<'_> {
    /// `salt` tells the rows apart.
    fn show(&self, ui: &mut Ui, salt: &str, count_width: f32) -> RowResponse {
        let palette = theme::palette(ui.ctx());
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
        let id = ui.scope_id().with(("category_row", salt));
        let row = ui.interact(rect, id, Sense::click());
        let description = format!("{}, {} files", self.label, group_digits(self.count));
        row.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::CheckBox, true, self.is_checked, &description)
        });

        let only_galley = ui.painter().layout_no_wrap(
            "Only".to_owned(),
            FontId::proportional(style::SMALL_SIZE),
            palette.accent_text,
        );
        let count_right = rect.right() - PADDING_X;
        let only_rect = Rect::from_min_max(
            pos2(
                count_right - count_width - GAP - only_galley.size().x - 2.0 * 6.0,
                rect.top() + 5.0,
            ),
            pos2(count_right - count_width - GAP, rect.bottom() - 5.0),
        );
        let only = self.has_only.then(|| {
            let only = ui.interact(only_rect, id.with("only"), Sense::click());
            only.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Only {}", self.label))
            });
            only.on_hover_text(format!("Show only {}", self.label))
        });

        let is_engaged = row.contains_pointer()
            || row.has_focus()
            || only.as_ref().is_some_and(egui::Response::has_focus);
        let fill = row::state_fill(ui, &row, Color32::TRANSPARENT);
        if fill != Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
        }
        let mut x = rect.left() + PADDING_X;
        let mark = checkbox::Mark::from(self.is_checked);
        let center = pos2(x + icons::SIZE / 2.0, rect.center().y);
        let color = checkbox::color(&palette, mark, false);
        checkbox::paint(ui.painter(), &palette, center, icons::SIZE, mark, color);
        x += icons::SIZE + GAP;
        ui.painter().text(
            pos2(x + icons::SIZE / 2.0, rect.center().y),
            Align2::CENTER_CENTER,
            self.icon,
            icons::rest_font(&palette, icons::SIZE),
            palette.text_secondary,
        );
        x += icons::SIZE + GAP;
        let label_right =
            if self.has_only { only_rect.left() - GAP } else { count_right - count_width - GAP };
        let mut job = egui::text::LayoutJob::simple_singleline(
            self.label.to_owned(),
            FontId::proportional(style::BODY_SIZE),
            palette.text,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width((label_right - x).max(0.0));
        let label = ui.painter().layout_job(job);
        ui.painter().galley(pos2(x, rect.center().y - label.size().y / 2.0), label, palette.text);
        ui.painter().text(
            pos2(count_right, rect.center().y),
            Align2::RIGHT_CENTER,
            group_digits(self.count),
            FontId::proportional(style::SMALL_SIZE),
            palette.text_secondary,
        );
        if let Some(only) = &only {
            // Always laid out, only painted while the row is engaged: nothing moves on hover.
            if is_engaged {
                paint_only(ui, only, only_galley);
            }
            focus::ring(ui, only, only_rect);
        }
        focus::ring(ui, &row, rect);
        RowResponse { row, only }
    }
}

/// The "Only" button of an engaged row.
fn paint_only(ui: &Ui, only: &Response, label: std::sync::Arc<Galley>) {
    let palette = theme::palette(ui.ctx());
    let fill = row::state_fill(ui, only, palette.surface);
    ui.painter().rect(
        only.rect,
        style::RADIUS_CONTROL,
        fill,
        Stroke::new(1.0, palette.border),
        StrokeKind::Inside,
    );
    ui.painter().galley(only.rect.center() - label.size() / 2.0, label, palette.accent_text);
}

pub fn category_icon(category: FileCategory) -> &'static str {
    match category {
        FileCategory::Code => icons::CODE,
        FileCategory::Tests => icons::FLASK,
        FileCategory::Docs => icons::BOOK_OPEN_TEXT,
        FileCategory::Config => icons::SLIDERS_HORIZONTAL,
        FileCategory::Generated => icons::ROBOT,
        FileCategory::Assets => icons::IMAGE,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use review_core::FileCategory::{self, *};

    use super::{FileFilter, normalize, summaries, toggle};

    fn counts() -> HashMap<FileCategory, usize> {
        HashMap::from([(Code, 300), (Tests, 44), (Docs, 12)])
    }

    #[test]
    fn summaries_shorten_step_by_step() {
        assert_eq!(summaries(&[]), ["All files"]);
        assert_eq!(summaries(&[Config]), ["Config & build"]);
        assert_eq!(
            summaries(&[Code, Tests, Docs]),
            ["Code, Tests, Docs", "Code, +2", "3 categories"]
        );
    }

    #[test]
    fn unchecking_from_all_keeps_the_others_and_the_last_returns_to_all() {
        let counts = counts();
        let present = [Code, Tests, Docs];
        let mut filter = FileFilter::default();
        toggle(&mut filter, &present, Tests);
        assert_eq!(filter.categories, BTreeSet::from([Code, Docs]));
        toggle(&mut filter, &present, Code);
        toggle(&mut filter, &present, Docs);
        normalize(&mut filter, &counts);
        assert!(filter.categories.is_empty(), "nothing checked means every file");
        toggle(&mut filter, &present, Tests);
        toggle(&mut filter, &present, Tests);
        normalize(&mut filter, &counts);
        assert!(filter.categories.is_empty(), "every category checked is the same as all");
    }
}
