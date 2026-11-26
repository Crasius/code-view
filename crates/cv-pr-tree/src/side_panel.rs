//! The file tree pane of the Files tab: search (Ctrl+P's go to file, see `goto`), the category
//! dropdown with the hide-viewed toggle and the switch that moves the pane to the other window
//! edge, the review progress (`progress`) and the file tree. The header keeps an 8 px rhythm and never overlaps itself:
//! every piece is placed in a rect measured for it, and what does not fit beside the progress text
//! moves to its own line.

use egui::{FontId, Rect, Sense, TextEdit, Ui, pos2, vec2};
use review_store::TreeSide;

use super::progress::{self, BAR_HEIGHT, PROGRESS_HEIGHT, Progress};
use super::{FileTree, TreeEvent, TreeInputs, category_filter, flip_control, goto, rows};
use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, style};
use cv_widgets::{buttons, glyph, skeleton};

/// Inner margin of the panel.
const MARGIN: i8 = 8;
/// Space between the filter row and the side switch, as between the filter row's own controls.
const FLIP_GAP: f32 = 4.0;

/// The file tree pane of the Files tab, `width` points wide, filling `ui`, which the caller placed
/// at the tree's window edge with the splitter on its inner edge. `is_counting_lines`: the line
/// totals are still on their way.
pub fn show(
    ui: &mut Ui,
    width: f32,
    tree: &mut FileTree,
    inputs: &TreeInputs<'_>,
    is_counting_lines: bool,
) -> Option<TreeEvent> {
    let palette = theme::palette(ui.ctx());
    let files = inputs.files;
    let mut event = None;
    egui::Panel::right("files_side_panel")
        .resizable(false)
        .exact_size(width)
        .frame(
            egui::Frame::new()
                .fill(palette.surface)
                .inner_margin(egui::Margin { left: MARGIN, right: MARGIN, top: MARGIN, bottom: 0 })
                .stroke(egui::Stroke::new(1.0, palette.border)),
        )
        .show(ui, |ui| {
            // Exact gaps below instead of the default item spacing.
            ui.spacing_mut().item_spacing.y = 0.0;
            let search = ui.add(
                TextEdit::singleline(&mut tree.filter.search)
                    .id(goto::filter_id())
                    .hint_text(format!("{}  Filter files", icons::MAGNIFYING_GLASS))
                    .min_size(vec2(0.0, style::CONTROL_HEIGHT))
                    .align(egui::Align2::LEFT_CENTER)
                    .desired_width(f32::INFINITY),
            );
            glyph::hint_as(&search, "Filter files");
            if tree.goto.take_focus_request() {
                search.request_focus();
            }
            search.on_hover_text(concat!(
                "Filter files. ",
                shortcut!(command "P"),
                " goes to a file: ↓ and ↑ pick it, Enter opens it"
            ));
            ui.add_space(style::SPACE_S);
            let counts = category_filter::counts(files);
            let side = tree.side();
            let flip = filters_and_flip(ui, side, |ui| {
                category_filter::show(ui, &mut tree.filter, &counts, files.len());
            });
            ui.add_space(style::SPACE_M);
            let viewed_count = files.iter().filter(|file| (inputs.is_viewed)(file)).count();
            let progress = Progress {
                viewed: viewed_count,
                total: files.len(),
                is_counting_lines,
                vote: inputs.vote,
            };
            let step = progress::show(ui, &progress, tree.breakdown());
            ui.add_space(style::SPACE_S);
            // Full-bleed divider: from panel edge to panel edge, not just the content width.
            let y = ui.cursor().top() + 0.5;
            let x_range = ui.max_rect().expand2(vec2(f32::from(MARGIN), 0.0)).x_range();
            ui.painter().hline(x_range, y, egui::Stroke::new(1.0, palette.border));
            ui.add_space(1.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            event = tree
                .show(ui, inputs)
                .or(flip.then_some(TreeEvent::FlipSide))
                .or(step.map(TreeEvent::Review));
        });
    event
}

/// The category row, and right of it the switch that moves the pane away from `side`. Returns
/// whether the switch was clicked.
fn filters_and_flip(ui: &mut Ui, side: TreeSide, filters: impl FnOnce(&mut Ui)) -> bool {
    let row = ui.available_rect_before_wrap();
    let flip_left = row.right() - buttons::SUBTLE_HEIGHT;
    let filters_rect = Rect::from_min_max(row.min, pos2(flip_left - FLIP_GAP, row.bottom()));
    ui.scope_builder(egui::UiBuilder::new().max_rect(filters_rect), filters);
    let flip_rect = Rect::from_min_size(
        pos2(flip_left, row.top() + (style::CONTROL_HEIGHT - buttons::SUBTLE_HEIGHT) / 2.0),
        vec2(buttons::SUBTLE_HEIGHT, buttons::SUBTLE_HEIGHT),
    );
    let (icon, hint) = flip_control(side);
    ui.scope_builder(egui::UiBuilder::new().max_rect(flip_rect), |ui| buttons::icon(ui, icon, hint))
        .inner
        .clicked()
}

/// The panel while the changed files load: the same frame, width and header heights as
/// [`show`], with placeholders, so the real panel replaces it without moving anything. After a
/// failure the placeholders hold still.
pub fn show_skeleton(ui: &mut Ui, width: f32, is_failed: bool) -> Rect {
    let palette = theme::palette(ui.ctx());
    let panel = egui::Panel::right("files_side_panel")
        .resizable(false)
        .exact_size(width)
        .frame(
            egui::Frame::new()
                .fill(palette.surface)
                .inner_margin(egui::Margin { left: MARGIN, right: MARGIN, top: MARGIN, bottom: 0 })
                .stroke(egui::Stroke::new(1.0, palette.border)),
        )
        .show(ui, |ui| {
            if is_failed {
                skeleton::still(ui, skeleton_content);
            } else {
                skeleton_content(ui);
            }
        });
    panel.response.rect
}

/// The header placeholders and the tree rows of [`show_skeleton`].
fn skeleton_content(ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    ui.spacing_mut().item_spacing.y = 0.0;
    let content = ui.available_width();
    // The search field and the category row, as empty outlines with a placeholder.
    control_outline(ui, content, 96.0);
    ui.add_space(style::SPACE_S);
    control_outline(ui, (content - style::CONTROL_HEIGHT - style::SPACE_S).max(0.0) * 0.6, 120.0);
    ui.add_space(style::SPACE_M);
    let line_height =
        ui.fonts_mut(|fonts| fonts.row_height(&FontId::proportional(style::SMALL_SIZE)));
    let (line, _) = ui.allocate_exact_size(vec2(content, line_height), Sense::hover());
    skeleton::paint_bar(
        ui,
        Rect::from_min_size(
            pos2(line.left(), line.center().y - 5.0),
            vec2(110.0_f32.min(content), 10.0),
        ),
    );
    ui.add_space(style::SPACE_XS);
    let (strip, _) = ui.allocate_exact_size(vec2(content, PROGRESS_HEIGHT), Sense::hover());
    ui.painter().rect_filled(
        Rect::from_center_size(strip.center(), vec2(strip.width(), BAR_HEIGHT)),
        BAR_HEIGHT / 2.0,
        palette.border,
    );
    ui.add_space(style::SPACE_S);
    let y = ui.cursor().top() + 0.5;
    let x_range = ui.max_rect().expand2(vec2(f32::from(MARGIN), 0.0)).x_range();
    ui.painter().hline(x_range, y, egui::Stroke::new(1.0, palette.border));
    ui.add_space(1.0);
    skeleton::tree_rows(ui, &rows::SKELETON);
}

/// A control's outline, `width` wide and [`style::CONTROL_HEIGHT`] high, with a placeholder bar.
fn control_outline(ui: &mut Ui, width: f32, bar_width: f32) {
    let palette = theme::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(width, style::CONTROL_HEIGHT), Sense::hover());
    ui.painter().rect_stroke(
        rect,
        style::RADIUS_CONTROL,
        egui::Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );
    let bar = Rect::from_min_size(
        pos2(rect.left() + 10.0, rect.center().y - 5.0),
        vec2(bar_width.min(rect.width() - 20.0).max(0.0), 10.0),
    );
    skeleton::paint_bar(ui, bar);
}
