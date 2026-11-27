//! Backgrounds for custom-painted clickable rows and cards: eased hover, an immediate pressed tint
//! and an animated selection, so state changes glide instead of snapping. And the layouts inside
//! rows: [`with_trailing`] for text that shares the line with buttons, [`centered_lines`] for a
//! name with a line below it beside a centered mark.

use cv_base::theme::{self, Palette, motion};
use egui::{Align, Color32, Layout, Rect, Response, Ui, UiBuilder, pos2, vec2};

/// Width of the accent bar marking the selected row in a list.
const INDICATOR_WIDTH: f32 = 3.0;

/// Full-width list row (run list, job tree, commits). The selected row gets a tinted fill and an
/// accent bar on its left edge that grows in from the middle. Returns the fill painted
/// (transparent when the row shows the surface below), for glyphs cut out of it.
pub fn list_background(ui: &Ui, response: &Response, rect: Rect, is_selected: bool) -> Color32 {
    let palette = theme::palette(ui.ctx());
    let selected =
        motion::toggle(ui.ctx(), response.id.with("row_selected"), is_selected, motion::MEDIUM);
    let fill = state_fill(ui, response, Color32::TRANSPARENT);
    let fill = motion::mix(fill, palette.selection, selected);
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 0, fill);
    }
    if selected > 0.0 {
        let height = rect.height() * selected;
        let bar = Rect::from_center_size(
            pos2(rect.left() + INDICATOR_WIDTH / 2.0, rect.center().y),
            vec2(INDICATOR_WIDTH, height),
        );
        ui.painter().rect_filled(bar, 0, palette.accent.gamma_multiply(selected));
    }
    fill
}

/// Rounded hover surface for cards and rows inside cards; `rest` is the fill when idle.
pub fn card_background(ui: &Ui, response: &Response, rect: Rect, rest: Color32, corner: u8) {
    let fill = state_fill(ui, response, rest);
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, corner, fill);
    }
}

/// `rest` blended towards the hover color while hovered, and the pressed color while held.
pub fn state_fill(ui: &Ui, response: &Response, rest: Color32) -> Color32 {
    let palette = theme::palette(ui.ctx());
    if response.is_pointer_button_down_on() {
        return palette.pressed;
    }
    let hover = hover_amount(ui, response);
    let target = if rest == Color32::TRANSPARENT {
        palette.hover.gamma_multiply(hover)
    } else {
        motion::mix(rest, palette.hover, hover)
    };
    if hover == 0.0 { rest } else { target }
}

/// How far `response`'s toggle is on, eased (0.0 to 1.0).
pub fn toggle_on(ui: &Ui, response: &Response, is_on: bool) -> f32 {
    motion::toggle(ui.ctx(), response.id.with("toggle_on"), is_on, motion::FAST)
}

/// The toggle's background in `rect` at `on`.
pub fn paint_toggle_fill(ui: &Ui, response: &Response, rect: Rect, on: f32) {
    let palette = theme::palette(ui.ctx());
    let fill = motion::mix(state_fill(ui, response, Color32::TRANSPARENT), palette.selection, on);
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, theme::style::RADIUS_CONTROL, fill);
    }
}

/// The toggle's icon and text colour at `on`; the disabled colour when the response is disabled.
pub fn toggle_color(palette: &Palette, response: &Response, on: f32) -> Color32 {
    if response.enabled() {
        motion::mix(palette.text, palette.accent_text, on)
    } else {
        palette.text_disabled
    }
}

/// A row whose `trailing` part (buttons, counts; added right to left, the first one rightmost) is
/// laid out first, and whose `leading` part (left to right) gets the width that is left. Its text
/// then wraps or truncates instead of running under the buttons or past the row: egui grows a
/// column to its widest child, so one long label would push everything after it off-screen.
/// Straight in a vertical layout the row takes all the height left below it and `Align::Center`
/// centers it there, so a row in a column goes in a `ui.horizontal` (or [`with_trailing_block`]).
pub fn with_trailing<T, L>(
    ui: &mut Ui,
    align: Align,
    trailing: impl FnOnce(&mut Ui) -> T,
    leading: impl FnOnce(&mut Ui) -> L,
) -> (T, L) {
    ui.with_layout(Layout::right_to_left(align), |ui| {
        let trailing = trailing(ui);
        let leading = ui.with_layout(Layout::left_to_right(align), leading).inner;
        (trailing, leading)
    })
    .inner
}

/// Lines stacked top to bottom as one block, centered on the height of the horizontal row they
/// sit in, as a mark or a caret beside them is. `ui.vertical` there takes the row's whole height
/// and starts at its top, so a two-line block sat flush under the row's top border. The block's
/// height is measured; when it changes (a font size, a line more) the frame is laid out again
/// before it shows. A block taller than the row starts at its top and grows it. In a vertical
/// layout the lines simply stack.
pub fn centered_lines<R>(ui: &mut Ui, add_lines: impl FnOnce(&mut Ui) -> R) -> R {
    if !ui.layout().is_horizontal() {
        return ui.vertical(add_lines).inner;
    }
    let id = ui.next_auto_id().with("centered_lines");
    let known = ui.data(|data| data.get_temp::<f32>(id));
    let slot = ui.available_rect_before_wrap();
    let top = known.map_or(slot.top(), |height| (slot.center().y - height / 2.0).max(slot.top()));
    let block = Rect::from_min_max(pos2(slot.left(), top), slot.max);
    let shown = ui.scope_builder(
        UiBuilder::new().max_rect(block).layout(Layout::top_down(Align::Min)),
        add_lines,
    );
    let height = shown.response.rect.height();
    if known != Some(height) {
        ui.data_mut(|data| data.insert_temp(id, height));
        ui.ctx().request_discard("a centered block of lines was measured");
    }
    shown.inner
}

/// A horizontal row of its own: `trailing` controls (as in [`with_trailing`]) beside a `leading`
/// block of lines (a name and its description), both centered on the row, which is as tall as
/// the taller of them. The controls are laid out first, before the block's height is known, so
/// the block's height of the frame before sets the row's; a change redoes the frame.
pub fn with_trailing_block<T, L>(
    ui: &mut Ui,
    trailing: impl FnOnce(&mut Ui) -> T,
    leading: impl FnOnce(&mut Ui) -> L,
) -> (T, L) {
    ui.horizontal(|ui| {
        let id = ui.scope_id().with("trailing_block");
        let known = ui.data(|data| data.get_temp::<f32>(id));
        if let Some(height) = known {
            ui.set_min_height(height);
        }
        let (trailing, (leading, height)) = with_trailing(ui, Align::Center, trailing, |ui| {
            centered_lines(ui, |ui| {
                let leading = leading(ui);
                (leading, ui.min_rect().height())
            })
        });
        if known != Some(height) {
            ui.data_mut(|data| data.insert_temp(id, height));
            ui.ctx().request_discard("a row's block of lines was measured");
        }
        (trailing, leading)
    })
    .inner
}

/// 0..1, easing in while the pointer is over `response` and out after it leaves.
pub fn hover_amount(ui: &Ui, response: &Response) -> f32 {
    motion::toggle(
        ui.ctx(),
        response.id.with("row_hover"),
        response.hovered() && response.enabled(),
        motion::FAST,
    )
}

/// Asserts that `block` sits centered in `row`: its top and bottom insets differ by at most 1 pt,
/// and neither is below the small spacing step.
#[cfg(any(test, feature = "test-support"))]
pub fn assert_centered_in(row: Rect, block: Rect) {
    let (top, bottom) = (block.top() - row.top(), row.bottom() - block.bottom());
    assert!((top - bottom).abs() <= 1.0, "top inset {top} and bottom inset {bottom} differ");
    let small = theme::style::SPACE_S;
    assert!(top.min(bottom) >= small, "insets {top} and {bottom} below {small}");
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use cv_base::theme::{self, style};
    use egui::{Align, Layout, Rect, Sense, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use review_store::ThemePreference;

    /// A row of two-line height with a mark and `lines` small lines beside it; the row's rect is
    /// reported through `row`.
    fn row_harness(lines: Rc<Cell<usize>>, row: Rc<Cell<Rect>>) -> Harness<'static> {
        Harness::builder()
            .with_size([400.0, 120.0])
            .with_context_setup(|ctx| {
                theme::fonts::install(ctx);
                theme::activate(ctx, ThemePreference::Light);
            })
            .build_ui(move |ui| {
                let size = vec2(ui.available_width(), style::ROW_TWO_LINE);
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                row.set(rect);
                let mut content = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .layout(Layout::left_to_right(Align::Center)),
                );
                content.label("Mark");
                super::centered_lines(&mut content, |ui| {
                    ui.spacing_mut().item_spacing.y = style::SPACE_LINE;
                    for index in 0..lines.get() {
                        let line = format!("Line {index}");
                        ui.label(egui::RichText::new(line).size(style::SMALL_SIZE));
                    }
                });
            })
    }

    fn block(harness: &Harness<'_>, lines: usize) -> Rect {
        (1..lines).fold(harness.get_by_label("Line 0").rect(), |block, index| {
            block.union(harness.get_by_label(&format!("Line {index}")).rect())
        })
    }

    /// The lines sit centered from the first frame on, and again once a line more shows.
    #[test]
    fn centers_the_lines_in_the_row_and_follows_their_count() {
        let (lines, row) = (Rc::new(Cell::new(2)), Rc::new(Cell::new(Rect::NOTHING)));
        let mut harness = row_harness(lines.clone(), row.clone());
        harness.step();
        super::assert_centered_in(row.get(), block(&harness, 2));
        lines.set(1);
        harness.step();
        super::assert_centered_in(row.get(), block(&harness, 1));
    }

    /// A checkbox beside a block taller than itself centers on the whole block, not on its first
    /// line, from the first frame on and again once a line more shows.
    #[test]
    fn centers_a_control_on_the_block_beside_it() {
        let lines = Rc::new(Cell::new(3));
        let shown = lines.clone();
        let mut harness = Harness::builder()
            .with_size([400.0, 160.0])
            .with_context_setup(|ctx| {
                theme::fonts::install(ctx);
                theme::activate(ctx, ThemePreference::Light);
            })
            .build_ui(move |ui| {
                let mut value = true;
                super::with_trailing_block(
                    ui,
                    |ui| crate::checkbox::checkbox(ui, &mut value, "Box"),
                    |ui| {
                        for index in 0..shown.get() {
                            ui.label(format!("Line {index}"));
                        }
                    },
                );
            });
        for count in [3, 4] {
            lines.set(count);
            harness.step();
            let control = harness.get_by_label("Box").rect().center().y;
            let block = block(&harness, count).center().y;
            assert!(
                (control - block).abs() <= 1.0,
                "{count} lines: box at {control}, block at {block}"
            );
        }
    }

    /// One frame at `time` with a toggle that is `is_on`: how far it is on, and its colour
    /// enabled and disabled.
    fn toggle_frame(
        ctx: &egui::Context,
        time: f64,
        is_on: bool,
    ) -> (f32, egui::Color32, egui::Color32) {
        let mut seen = (0.0, egui::Color32::PLACEHOLDER, egui::Color32::PLACEHOLDER);
        let input = egui::RawInput { time: Some(time), ..egui::RawInput::default() };
        let mut output = ctx.run_ui(input, |ui| {
            let palette = theme::palette(ui.ctx());
            let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(28.0, 28.0));
            let response = ui.interact(rect, egui::Id::unique("toggle"), Sense::click());
            let on = super::toggle_on(ui, &response, is_on);
            let disabled = ui
                .add_enabled_ui(false, |ui| {
                    ui.interact(rect, egui::Id::unique("disabled"), Sense::click())
                })
                .inner;
            seen = (
                on,
                super::toggle_color(&palette, &response, on),
                super::toggle_color(&palette, &disabled, on),
            );
        });
        output.textures_delta.clear();
        seen
    }

    #[test]
    fn a_toggle_eases_on_and_a_disabled_one_takes_the_disabled_colour() {
        let ctx = egui::Context::default();
        theme::activate(&ctx, ThemePreference::Light);
        let palette = theme::palette(&ctx);
        let (off, off_color, disabled) = toggle_frame(&ctx, 0.0, false);
        assert_eq!((off, off_color), (0.0, palette.text));
        assert_eq!(disabled, palette.text_disabled);
        // egui eases by at most one frame's time per frame, so the clock runs at 60 frames a second.
        let frame = 1.0 / 60.0;
        let (easing, ..) = toggle_frame(&ctx, frame, true);
        assert!(0.0 < easing && easing < 1.0, "one frame in it is {easing}");
        let mut time = frame;
        let (on, on_color, disabled) = loop {
            time += frame;
            let seen = toggle_frame(&ctx, time, true);
            if time > frame + f64::from(theme::motion::FAST) {
                break seen;
            }
        };
        assert_eq!((on, on_color), (1.0, palette.accent_text));
        assert_eq!(disabled, palette.text_disabled);
    }
}
