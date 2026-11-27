use cv_base::theme::{self, icons, motion, style};
use egui::{Align2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::row;

const PIVOT_HEIGHT: f32 = 40.0;
const PIVOT_PADDING: f32 = 10.0;
/// Gap between two pivot tabs.
const PIVOT_SPACING: f32 = 4.0;
const UNDERLINE_HEIGHT: f32 = 2.0;

/// The count badge of a pivot tab ("Files 356").
#[derive(Clone, PartialEq, Eq)]
pub enum TabCount {
    /// The tab has no count.
    None,
    /// The count is still loading: an empty badge holds its place, so nothing moves when it arrives.
    Loading,
    Ready(String),
}

const COUNT_HEIGHT: f32 = 18.0;
const COUNT_PADDING: f32 = 6.0;
const COUNT_GAP: f32 = 6.0;

/// Azure DevOps-style pivot tabs: text with an accent underline that slides to the selected tab.
/// Returns the index clicked this frame, if any.
pub fn pivot(ui: &mut Ui, labels: &[String], selected: usize) -> Option<usize> {
    let tabs: Vec<(&str, TabCount, &str)> =
        labels.iter().map(|label| (label.as_str(), TabCount::None, "")).collect();
    pivot_with_counts(ui, &tabs, selected)
}

/// The width [`pivot`] takes for these labels, for rows that plan their room before drawing.
pub fn pivot_width(ui: &Ui, labels: &[String]) -> f32 {
    let palette = theme::palette(ui.ctx());
    let tabs: f32 = labels
        .iter()
        .map(|label| {
            let bold = ui.painter().layout_no_wrap(
                label.clone(),
                theme::fonts::semibold_font(style::BODY_SIZE),
                palette.text,
            );
            bold.size().x + PIVOT_PADDING * 2.0
        })
        .sum();
    tabs + PIVOT_SPACING * labels.len().saturating_sub(1) as f32
}

/// Pivot tabs with a count badge after the label, as Azure DevOps shows "Files 356". The badge
/// reserves at least three digits while loading and after, so counts arriving never push the
/// tabs to the right of it. The accessible name stays "Files (356)". Each tab's `(label, count,
/// shortcut)`: a shortcut that is not empty shows in the tab's tooltip, "Files (Ctrl+2)".
pub fn pivot_with_counts(
    ui: &mut Ui,
    tabs: &[(&str, TabCount, &str)],
    selected: usize,
) -> Option<usize> {
    let palette = theme::palette(ui.ctx());
    let mut clicked = None;
    let mut selected_rect = None;
    let count_font = egui::FontId::proportional(style::SMALL_SIZE);
    let min_count_width =
        ui.painter().layout_no_wrap("000".to_owned(), count_font, palette.text).size().x;
    let inner = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = PIVOT_SPACING;
        for (index, (label, count, shortcut)) in tabs.iter().enumerate() {
            let is_selected = index == selected;
            let tab = pivot_tab(ui, (label, count, shortcut), is_selected, min_count_width);
            if is_selected {
                selected_rect = Some(tab.underline);
            }
            if tab.is_clicked {
                clicked = Some(index);
            }
        }
    });
    if let Some(target) = selected_rect {
        let id = inner.response.id.with("pivot_underline");
        let left = motion::glide(ui.ctx(), id.with("left"), target.left(), motion::MEDIUM);
        let right = motion::glide(ui.ctx(), id.with("right"), target.right(), motion::MEDIUM);
        let underline = Rect::from_min_max(pos2(left, target.top()), pos2(right, target.bottom()));
        ui.painter().rect_filled(underline, 1, palette.accent);
    }
    clicked
}

/// One painted pivot tab: whether it was clicked, and where its underline goes when selected.
struct PivotTab {
    is_clicked: bool,
    /// Under the label and its badge: the whole content of the tab.
    underline: Rect,
}

fn pivot_tab(
    ui: &mut Ui,
    (label, count, shortcut): (&str, &TabCount, &str),
    is_selected: bool,
    min_count_width: f32,
) -> PivotTab {
    let palette = theme::palette(ui.ctx());
    // Every tab reserves its semibold width, so selecting one never shifts the others.
    let bold = ui.painter().layout_no_wrap(
        label.to_owned(),
        theme::fonts::semibold_font(style::BODY_SIZE),
        palette.text,
    );
    let count_galley = match count {
        TabCount::Ready(text) => Some(ui.painter().layout_no_wrap(
            text.clone(),
            egui::FontId::proportional(style::SMALL_SIZE),
            palette.text_secondary,
        )),
        TabCount::None | TabCount::Loading => None,
    };
    let badge_width = match count {
        TabCount::None => 0.0,
        TabCount::Loading | TabCount::Ready(_) => {
            count_galley.as_ref().map_or(0.0, |galley| galley.size().x).max(min_count_width)
                + COUNT_PADDING * 2.0
        }
    };
    let badge_space = if badge_width > 0.0 { COUNT_GAP + badge_width } else { 0.0 };
    let size = vec2(bold.size().x + badge_space + PIVOT_PADDING * 2.0, PIVOT_HEIGHT);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    // The name is built only when egui asks for it (accessibility on), not every frame.
    response.widget_info(|| {
        let accessible = match count {
            TabCount::Ready(text) => format!("{label} ({text})"),
            TabCount::None | TabCount::Loading => label.to_owned(),
        };
        egui::WidgetInfo::selected(egui::Role::Tab, true, is_selected, accessible)
    });
    if !shortcut.is_empty() {
        // Built only while hovered.
        response = response.on_hover_ui(|ui| {
            ui.label(format!("{label} ({shortcut})"));
        });
    }
    let hover = row::hover_amount(ui, &response);
    let pill = rect.shrink2(vec2(0.0, 6.0));
    let fill = if response.is_pointer_button_down_on() {
        palette.pressed
    } else {
        palette.hover.gamma_multiply(hover)
    };
    ui.painter().rect_filled(pill, style::RADIUS_CONTROL, fill);
    let (font, color) = if is_selected {
        (theme::fonts::semibold_font(style::BODY_SIZE), palette.text)
    } else {
        (
            egui::FontId::proportional(style::BODY_SIZE),
            motion::mix(palette.text_secondary, palette.text, hover),
        )
    };
    let label_left = rect.left() + PIVOT_PADDING;
    ui.painter().text(pos2(label_left, rect.center().y), Align2::LEFT_CENTER, label, font, color);
    if badge_width > 0.0 {
        let badge = Rect::from_min_size(
            pos2(label_left + bold.size().x + COUNT_GAP, rect.center().y - COUNT_HEIGHT / 2.0),
            vec2(badge_width, COUNT_HEIGHT),
        );
        ui.painter().rect_filled(badge, COUNT_HEIGHT / 2.0, palette.border);
        if let Some(galley) = count_galley {
            ui.painter().galley(
                badge.center() - galley.size() / 2.0,
                galley,
                palette.text_secondary,
            );
        }
    }
    crate::focus::ring(ui, &response, pill);
    PivotTab {
        is_clicked: response.clicked(),
        underline: Rect::from_center_size(
            pos2(rect.center().x, rect.bottom() - UNDERLINE_HEIGHT / 2.0),
            vec2(bold.size().x + badge_space, UNDERLINE_HEIGHT),
        ),
    }
}

/// One option of a segmented control: its value, its label (the accessible name, and the tooltip's
/// lead when an icon shows instead), an optional tooltip and an optional icon.
#[derive(Clone, Copy)]
pub struct Segment<'a, T> {
    pub value: T,
    pub label: &'a str,
    /// Empty shows no tooltip.
    pub hint: &'a str,
    /// Shown instead of the label, for where the labels don't fit.
    pub icon: Option<&'a str>,
}

impl<'a, T: Copy> Segment<'a, T> {
    pub const fn new(value: T, label: &'a str) -> Self {
        Self { value, label, hint: "", icon: None }
    }

    /// A tooltip, for an option whose label alone does not say enough, or that has a shortcut.
    #[must_use]
    pub const fn with_hint(self, hint: &'a str) -> Self {
        Self { hint, ..self }
    }

    #[must_use]
    pub const fn with_icon(self, icon: &'a str) -> Self {
        Self { icon: Some(icon), ..self }
    }
}

/// The width [`segments`] takes for these options.
pub fn segments_width<T>(ui: &Ui, options: &[Segment<'_, T>]) -> f32 {
    options.iter().map(|segment| segment_width(ui, segment.label, segment.icon)).sum()
}

/// Segmented control for small sets of mutually exclusive options.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let segments: Vec<Segment<'_, T>> =
        options.iter().map(|(option, label)| Segment::new(*option, label)).collect();
    self::segments(ui, value, &segments)
}

/// [`segmented`] with a tooltip per option (`(value, label, tooltip)`, an empty tooltip shows none),
/// for options whose label alone does not say enough, or that have a shortcut.
pub fn segmented_with_hints<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str, &str)],
) -> bool {
    let segments: Vec<Segment<'_, T>> = options
        .iter()
        .map(|(option, label, hint)| Segment::new(*option, label).with_hint(hint))
        .collect();
    self::segments(ui, value, &segments)
}

/// [`segmented_with_hints`] showing an icon per option (`(value, icon, label, tooltip)`), for where
/// the labels don't fit. Each label stays the option's accessible name and leads its tooltip.
pub fn segmented_icons<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str, &str, &str)],
) -> bool {
    let segments: Vec<Segment<'_, T>> = options
        .iter()
        .map(|(option, icon, label, hint)| {
            Segment::new(*option, label).with_hint(hint).with_icon(icon)
        })
        .collect();
    self::segments(ui, value, &segments)
}

/// The width [`segmented_with_hints`] takes for these labels.
pub fn segmented_width(ui: &Ui, labels: &[&str]) -> f32 {
    let segments: Vec<Segment<'_, ()>> =
        labels.iter().map(|label| Segment::new((), label)).collect();
    segments_width(ui, &segments)
}

/// The width [`segmented_icons`] takes for these icons.
pub fn segmented_icons_width(ui: &Ui, icons: &[&str]) -> f32 {
    let segments: Vec<Segment<'_, ()>> =
        icons.iter().map(|icon| Segment::new((), "").with_icon(icon)).collect();
    segments_width(ui, &segments)
}

/// Width of one segment. An icon segment is a fixed icon slot. A label segment is sized for its
/// semibold (selected) face with every count widened to two digits, so neither selecting a segment
/// nor a count ticking from 9 to 10 resizes the control.
fn segment_width(ui: &Ui, label: &str, icon: Option<&str>) -> f32 {
    if icon.is_some() {
        return icons::SIZE + style::SPACE_M * 2.0;
    }
    let font = theme::fonts::semibold_font(style::BODY_SIZE);
    let galley =
        ui.painter().layout_no_wrap(reserve_digits(label), font, theme::palette(ui.ctx()).text);
    galley.size().x + style::SPACE_L * 2.0
}

/// Segmented control for small sets of mutually exclusive options; true when the value changed.
pub fn segments<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[Segment<'_, T>],
) -> bool {
    let palette = theme::palette(ui.ctx());
    let mut changed = false;
    let inner = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for segment in options {
            let is_selected = *value == segment.value;
            let size = vec2(segment_width(ui, segment.label, segment.icon), style::CONTROL_HEIGHT);
            let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::Role::RadioButton,
                    true,
                    is_selected,
                    segment.label,
                )
            });
            let tooltip = match (segment.icon, segment.hint) {
                (None, hint) => hint.to_owned(),
                (Some(_), "") => segment.label.to_owned(),
                (Some(_), hint) => format!("{}: {hint}", segment.label),
            };
            if !tooltip.is_empty() {
                response = response.on_hover_text(tooltip);
            }
            let selected = motion::toggle(
                ui.ctx(),
                response.id.with("segment_selected"),
                is_selected,
                motion::FAST,
            );
            let rest = row::state_fill(ui, &response, palette.surface);
            let fill = motion::mix(rest, palette.selection, selected);
            ui.painter().rect_filled(rect.shrink(1.0), style::RADIUS_CONTROL, fill);
            let (text, font) = match segment.icon {
                // The icon weight follows the theme and turns Fill when selected; its slot is fixed.
                Some(icon) => {
                    (icon.to_owned(), icons::state_font(&palette, icons::SIZE, is_selected))
                }
                None if is_selected => {
                    (segment.label.to_owned(), theme::fonts::semibold_font(style::BODY_SIZE))
                }
                None => (segment.label.to_owned(), egui::FontId::proportional(style::BODY_SIZE)),
            };
            let color = motion::mix(palette.text, palette.accent_text, selected);
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, font, color);
            crate::focus::ring(ui, &response, rect);
            if response.clicked() && !is_selected {
                *value = segment.value;
                changed = true;
            }
        }
    });
    ui.painter().rect_stroke(
        inner.response.rect,
        style::RADIUS_CONTROL,
        Stroke::new(1.0, palette.border_strong),
        StrokeKind::Inside,
    );
    changed
}

/// `label` with every run of digits widened to at least two zeros: the text a segment is sized for.
fn reserve_digits(label: &str) -> String {
    let mut sized = String::with_capacity(label.len() + 2);
    let mut run = 0;
    for character in label.chars().chain(std::iter::once('\0')) {
        if character.is_ascii_digit() {
            run += 1;
            continue;
        }
        if run > 0 {
            sized.push_str(&"0".repeat(run.max(2)));
            run = 0;
        }
        if character != '\0' {
            sized.push(character);
        }
    }
    sized
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use cv_base::theme::{self, icons};
    use egui::{Event, Modifiers, PointerButton};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use review_store::ThemePreference;

    use super::{Segment, reserve_digits};

    #[test]
    fn counts_reserve_two_digits() {
        assert_eq!(reserve_digits("Active 3"), "Active 00");
        assert_eq!(reserve_digits("All 128"), "All 000");
        assert_eq!(reserve_digits("Light"), "Light");
    }

    const LAYOUTS: [Segment<'static, u8>; 3] = [
        Segment::new(0, "Inline"),
        Segment::new(1, "Side by side").with_hint("Ctrl+Shift+L"),
        Segment::new(2, "Unified").with_icon(icons::ROWS),
    ];

    /// A press and release on the centre of `label`, where the last frame put it.
    fn click(harness: &mut Harness<'_>, label: &str) {
        let at = harness.get_by_label(label).rect().center();
        for pressed in [true, false] {
            harness.event(Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        harness.run();
    }

    /// A click picks its option, and the control is as wide as [`super::segments_width`] plans.
    #[test]
    fn segments_pick_the_clicked_option_at_their_planned_width() {
        let picked = Rc::new(Cell::new(0_u8));
        let widths = Rc::new(Cell::new((0.0, 0.0)));
        let (value, planned) = (picked.clone(), widths.clone());
        let mut harness = Harness::builder()
            .with_size([600.0, 80.0])
            .with_context_setup(|ctx| {
                theme::fonts::install(ctx);
                theme::activate(ctx, ThemePreference::Light);
            })
            .build_ui(move |ui| {
                let mut current = value.get();
                let left = ui.cursor().left();
                let changed = super::segments(ui, &mut current, &LAYOUTS);
                assert_eq!(changed, current != value.get());
                value.set(current);
                let painted = ui.min_rect().right() - left;
                planned.set((painted, super::segments_width(ui, &LAYOUTS)));
            });
        harness.run();
        click(&mut harness, "Side by side");
        assert_eq!(picked.get(), 1);
        click(&mut harness, "Unified");
        assert_eq!(picked.get(), 2, "an icon segment keeps its label as its name");
        let (painted, planned) = widths.get();
        assert!((painted - planned).abs() < 0.5, "painted {painted}, planned {planned}");
    }
}
