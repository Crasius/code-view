//! One row of the file tree: a folder (caret, tri-state viewed mark for every file below it, folder
//! glyph, name, file count) or a file (viewed checkbox, file-type glyph with its change mark, name,
//! comment badge, line counts), with the indent guides of its depth and a tooltip beside the row.
//! Folders and files at one depth share the viewed column, so their marks line up. The folder summary's cards paint
//! their file glyphs, badges and cut names with the same functions.

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, RectAlign, Response, Sense, Stroke, Ui, Vec2,
    pos2, vec2,
};
use review_git::{ChangeKind, ChangedFile};
use review_store::TreeSide;

use super::file_icon::{self, FileIcon};
use super::model::TreeRow;
use super::{FolderMark, TreeEvent, TreeInputs};
use cv_base::platform::shortcut;
use cv_base::theme::{self, Palette, fonts, icons, style};
use cv_widgets::numbers::{compact_count, digit_template, group_digits};
use cv_widgets::{checkbox, focus, row, skeleton};

/// Rows sit on the 4 px grid and touch each other, so hover, selection and indent guides run
/// without gaps.
pub const ROW_HEIGHT: f32 = 28.0;
/// One nesting level; the guide of a folder runs through the middle of its caret column.
const INDENT: f32 = 16.0;
/// Space between the panel edge and the depth-0 caret column.
const INSET: f32 = 4.0;
/// The caret (folders only) and the viewed mark (folders and files) each take one such column.
const LEAD_WIDTH: f32 = 16.0;
/// The viewed column, after the caret column; their hit areas do not overlap.
const CHECK_LEFT: f32 = LEAD_WIDTH + 4.0;
/// Folder or file-type glyph in a fixed slot, followed by the name.
const ICON_LEFT: f32 = CHECK_LEFT + LEAD_WIDTH + 4.0;
const ICON_WIDTH: f32 = 18.0;
/// The change dot on a file glyph's lower right, and the ring of row color that cuts it out.
const MARK_OFFSET: Vec2 = vec2(5.5, 5.0);
const MARK_RADIUS: f32 = 3.5;
const MARK_RING: f32 = 1.5;
/// Line width of the hollow mark for moves and type changes.
const MARK_STROKE: f32 = 1.5;
const NAME_LEFT: f32 = ICON_LEFT + ICON_WIDTH + style::SPACE_S;
/// Keeps the right-hand counts clear of the overlay scroll bar.
const RIGHT_INSET: f32 = style::SPACE_M;
const NAME_SIZE: f32 = 13.0;
const PILL_HEIGHT: f32 = 18.0;

/// The loading skeleton's rows sit where these rows will, so they replace it in place.
pub const SKELETON: skeleton::TreeShape = skeleton::TreeShape {
    row_height: ROW_HEIGHT,
    indent: INDENT,
    inset: INSET,
    icon_center: ICON_LEFT + ICON_WIDTH / 2.0,
    name_left: NAME_LEFT,
};

/// What a click on a row asks for.
pub enum RowAction {
    Tree(TreeEvent),
    /// The caret (or a double click): open or close the folder at this path.
    ToggleFolder(String),
}

/// A row as it showed this frame.
pub struct ShownRow {
    /// What the user did with it.
    pub action: Option<RowAction>,
    /// The pointer is on it (on the row or a control in it).
    pub is_hovered: bool,
}

/// Paints `row` in a new row at the cursor.
pub(crate) fn show(
    ui: &mut Ui,
    row: &TreeRow,
    inputs: &TreeInputs<'_>,
    columns: &CountColumns,
    side: TreeSide,
) -> ShownRow {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let accessible_name = match row {
        TreeRow::Folder { path, .. } => path.clone(),
        TreeRow::File { index, .. } => inputs.files[*index as usize].path.clone(),
    };
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::TreeItem, true, &accessible_name));
    let action = match row {
        TreeRow::Folder { path, label, depth, file_count, is_collapsed, mark } => {
            let folder = FolderRow {
                path,
                label,
                depth: *depth,
                file_count: *file_count,
                is_collapsed: *is_collapsed,
                is_selected: inputs.selected_folder == Some(path.as_str()),
                mark: *mark,
                side,
            };
            paint_folder(ui, rect, &response, &folder).map(|event| match event {
                FolderRowEvent::Toggle => RowAction::ToggleFolder(path.clone()),
                FolderRowEvent::Select => RowAction::Tree(TreeEvent::SelectFolder(path.clone())),
                FolderRowEvent::ToggleViewed => {
                    RowAction::Tree(TreeEvent::ToggleFolderViewed(path.clone()))
                }
            })
        }
        TreeRow::File { index, depth } => {
            let file_index = *index as usize;
            let file = &inputs.files[file_index];
            let state = FileRowState {
                depth: *depth,
                is_selected: inputs.selected == Some(file_index),
                is_viewed: (inputs.is_viewed)(file),
                comments: inputs.thread_counts.get(&file.path).copied().unwrap_or(0),
                side,
            };
            paint_file(ui, rect, &response, file, &state, columns).map(|event| match event {
                FileRowEvent::Select => RowAction::Tree(TreeEvent::Select(file_index)),
                FileRowEvent::ToggleViewed => RowAction::Tree(TreeEvent::ToggleViewed(file_index)),
            })
        }
    };
    focus::ring(ui, &response, rect);
    ShownRow { action, is_hovered: ui.rect_contains_pointer(rect) }
}

/// Right-aligned "+12  −3" columns shared by every file row, sized for the widest count.
pub struct CountColumns {
    font: FontId,
    /// Widths of the additions and deletions columns; `None` while no line counts are known.
    widths: Option<(f32, f32)>,
}

impl CountColumns {
    const GAP: f32 = 6.0;

    pub fn measure(ui: &Ui, widest: Option<(u32, u32)>) -> Self {
        let font = FontId::proportional(style::SMALL_SIZE);
        let widths = widest.map(|(added, deleted)| {
            let width = |text: String| {
                ui.painter().layout_no_wrap(text, font.clone(), Color32::WHITE).size().x
            };
            (
                width(digit_template('+', u64::from(added))),
                width(digit_template('−', u64::from(deleted))),
            )
        });
        Self { font, widths }
    }

    /// Total width, including the gap before the comment badge.
    fn width(&self) -> f32 {
        self.widths.map_or(0.0, |(added, deleted)| added + Self::GAP + deleted + style::SPACE_S)
    }

    /// `is_quiet`: the lines changed only in whitespace, so the counts step back like zeros.
    fn paint(
        &self,
        painter: &Painter,
        right: f32,
        center_y: f32,
        counts: (u32, u32),
        is_quiet: bool,
        palette: &Palette,
    ) {
        let Some((_, deleted_width)) = self.widths else {
            return;
        };
        let (added, deleted) = counts;
        // A zero carries no news: it stays in place but steps back.
        let color = |value: u32, tint: Color32| {
            if value == 0 || is_quiet { palette.text_disabled } else { tint }
        };
        let text = |x: f32, value: String, color: Color32| {
            painter.text(pos2(x, center_y), Align2::RIGHT_CENTER, value, self.font.clone(), color)
        };
        text(
            right,
            format!("−{}", compact_count(u64::from(deleted))),
            color(deleted, palette.error),
        );
        text(
            right - deleted_width - Self::GAP,
            format!("+{}", compact_count(u64::from(added))),
            color(added, palette.success),
        );
    }
}

enum FileRowEvent {
    Select,
    ToggleViewed,
}

enum FolderRowEvent {
    /// The caret: open or close the folder.
    Toggle,
    /// The row: show the folder's summary.
    Select,
    /// The viewed mark: mark or unmark every file below the folder.
    ToggleViewed,
}

struct FolderRow<'a> {
    path: &'a str,
    label: &'a str,
    depth: u16,
    file_count: u32,
    is_collapsed: bool,
    is_selected: bool,
    mark: FolderMark,
    /// The window edge the tree sits at; the tooltip opens away from it.
    side: TreeSide,
}

struct FileRowState {
    depth: u16,
    is_selected: bool,
    is_viewed: bool,
    comments: usize,
    /// The window edge the tree sits at; the tooltip opens away from it.
    side: TreeSide,
}

/// Left edge of a row's caret column at `depth`.
fn lead_left(rect: Rect, depth: u16) -> f32 {
    rect.left() + INSET + f32::from(depth) * INDENT
}

/// The hit area of a row's caret or viewed checkbox: the whole lead column, not just its glyph.
fn lead_rect(left: f32, center_y: f32) -> Rect {
    Rect::from_center_size(
        pos2(left + LEAD_WIDTH / 2.0, center_y),
        vec2(LEAD_WIDTH + 4.0, ROW_HEIGHT - 4.0),
    )
}

/// One hairline per ancestor folder, through the middle of that folder's caret, so siblings
/// line up visibly at every depth.
fn paint_indent_guides(painter: &Painter, rect: Rect, depth: u16, palette: &Palette) {
    let stroke = Stroke::new(1.0, palette.border);
    for level in 0..depth {
        let x = (lead_left(rect, level) + LEAD_WIDTH / 2.0).round() + 0.5;
        painter.vline(x, rect.y_range(), stroke);
    }
}

/// A folder row: the caret opens and closes it (as does a double click), the row shows its summary.
fn paint_folder(
    ui: &mut Ui,
    rect: Rect,
    response: &Response,
    folder: &FolderRow<'_>,
) -> Option<FolderRowEvent> {
    let palette = theme::palette(ui.ctx());
    row::list_background(ui, response, rect, folder.is_selected);
    let left = lead_left(rect, folder.depth);
    let center_y = rect.center().y;
    let caret_rect = lead_rect(left, center_y);
    let caret_response = ui.interact(caret_rect, response.id.with("caret"), Sense::click());
    let verb = if folder.is_collapsed { "Expand" } else { "Collapse" };
    caret_response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{verb} {}", folder.path))
    });
    focus::ring(ui, &caret_response, caret_rect);
    let painter = ui.painter_at(rect);
    paint_indent_guides(&painter, rect, folder.depth, &palette);
    let caret_color = if caret_response.hovered() { palette.text } else { palette.text_secondary };
    let caret = if folder.is_collapsed { icons::CARET_RIGHT } else { icons::CARET_DOWN };
    painter.text(
        caret_rect.center(),
        Align2::CENTER_CENTER,
        caret,
        FontId::proportional(12.0),
        caret_color,
    );
    let glyph = if folder.is_collapsed { icons::FOLDER } else { icons::FOLDER_OPEN };
    let icon_center = pos2(left + ICON_LEFT + ICON_WIDTH / 2.0, center_y);
    painter.text(
        icon_center,
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(16.0),
        style::folder_icon(&palette),
    );

    let mark = folder_mark(ui, rect, response, folder);
    let count = paint_count_pill(
        &painter,
        pos2(rect.right() - RIGHT_INSET, center_y),
        folder.file_count,
        &palette,
    );
    let name_left = pos2(left + NAME_LEFT, center_y);
    let format = TextFormat::simple(fonts::semibold_font(NAME_SIZE), palette.text);
    paint_elided(&painter, name_left, folder.label, format, count.left() - style::SPACE_S);

    let mut detail = if folder.file_count == 1 {
        "1 changed file".to_owned()
    } else {
        format!("{} changed files", group_digits(folder.file_count as usize))
    };
    match folder.mark {
        FolderMark::None => {}
        FolderMark::Some => detail.push_str(", some viewed"),
        FolderMark::All => detail.push_str(", all viewed"),
    }
    tooltip_beside_row(ui, response, folder.path, detail, folder.side, &palette);
    if mark.clicked() {
        return Some(FolderRowEvent::ToggleViewed);
    }
    if caret_response.clicked() || response.double_clicked() {
        return Some(FolderRowEvent::Toggle);
    }
    response.clicked().then_some(FolderRowEvent::Select)
}

/// A folder's tri-state viewed mark in the viewed column, between the caret and the folder glyph,
/// where the files' checkboxes sit: checked when every file below it is
/// viewed, a dash when some are, an empty box when none are. The shape tells the states apart, not
/// only the color.
fn folder_mark(ui: &mut Ui, rect: Rect, response: &Response, folder: &FolderRow<'_>) -> Response {
    let palette = theme::palette(ui.ctx());
    let mark_rect = lead_rect(lead_left(rect, folder.depth) + CHECK_LEFT, rect.center().y);
    let mark = ui.interact(mark_rect, response.id.with("viewed"), Sense::click());
    let is_all = folder.mark == FolderMark::All;
    mark.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::CheckBox,
            true,
            is_all,
            format!("Viewed folder {}", folder.path),
        )
    });
    if folder.mark == FolderMark::Some {
        ui.ctx().accesskit_node_builder(mark.id, |node| {
            node.set_toggled(egui::accesskit::Toggled::Mixed);
        });
    }
    focus::ring(ui, &mark, mark_rect);
    let shown = match folder.mark {
        FolderMark::All => checkbox::Mark::On,
        FolderMark::Some => checkbox::Mark::Mixed,
        FolderMark::None => checkbox::Mark::Off,
    };
    let painter = ui.painter_at(rect);
    let color = mark_color(&palette, shown, mark.hovered());
    checkbox::paint(&painter, &palette, mark_rect.center(), icons::SIZE, shown, color);
    mark.on_hover_ui(|ui| {
        let verb = if is_all { "Unmark" } else { "Mark" };
        let key = shortcut!(alt "V");
        ui.label(format!(
            "{verb} every file in {} as viewed ({key} on the open folder)",
            folder.path
        ));
    })
}

/// Number of files below a folder, as a quiet neutral pill so it never reads as a line count.
fn paint_count_pill(painter: &Painter, right_center: Pos2, count: u32, palette: &Palette) -> Rect {
    let galley = painter.layout_no_wrap(
        group_digits(count as usize),
        FontId::proportional(style::SMALL_SIZE),
        palette.text_secondary,
    );
    let size = vec2((galley.size().x + 2.0 * 6.0).max(PILL_HEIGHT), PILL_HEIGHT);
    let pill =
        Rect::from_min_size(pos2(right_center.x - size.x, right_center.y - size.y / 2.0), size);
    painter.rect_filled(pill, PILL_HEIGHT / 2.0, palette.border);
    let text_pos = pill.center() - galley.size() / 2.0;
    painter.galley(text_pos, galley, palette.text_secondary);
    pill
}

/// Single-line text cut with "…" so it never runs into the counts on the right.
/// True when `text` was cut, so the caller can offer it in full on hover.
pub fn paint_elided(
    painter: &Painter,
    left_center: Pos2,
    text: &str,
    format: TextFormat,
    right: f32,
) -> bool {
    let color = format.color;
    let mut job = LayoutJob::single_section(text.to_owned(), format);
    job.wrap = TextWrapping::truncate_at_width((right - left_center.x).max(0.0));
    let galley = painter.layout_job(job);
    let is_cut = galley.elided;
    painter.galley(pos2(left_center.x, left_center.y - galley.size().y / 2.0), galley, color);
    is_cut
}

fn paint_file(
    ui: &mut Ui,
    rect: Rect,
    response: &Response,
    file: &ChangedFile,
    state: &FileRowState,
    columns: &CountColumns,
) -> Option<FileRowEvent> {
    let palette = theme::palette(ui.ctx());
    let fill = row::list_background(ui, response, rect, state.is_selected);
    let left = lead_left(rect, state.depth);
    let center_y = rect.center().y;
    let checkbox = viewed_checkbox(ui, rect, response, file, state);

    let painter = ui.painter_at(rect);
    let change = change_look(file.kind, &palette);
    let icon_center = pos2(left + ICON_LEFT + ICON_WIDTH / 2.0, center_y);
    let row_color = palette.surface.blend(fill);
    paint_file_icon(
        &painter,
        icon_center,
        file_icon::file_icon(&file.path),
        &change,
        state.is_selected,
        row_color,
        &palette,
    );

    let counts_right = rect.right() - RIGHT_INSET;
    if let (Some(additions), Some(deletions)) = (file.additions, file.deletions) {
        columns.paint(
            &painter,
            counts_right,
            center_y,
            (additions, deletions),
            file.whitespace_only,
            &palette,
        );
    }
    let mut name_right = counts_right - columns.width();
    if state.comments > 0 {
        let badge =
            paint_comment_badge(&painter, pos2(name_right, center_y), state.comments, &palette);
        name_right = badge.left() - style::SPACE_S;
    }
    let name_color =
        if state.is_viewed || change.is_deleted { palette.text_secondary } else { palette.text };
    let mut format = TextFormat::simple(FontId::proportional(NAME_SIZE), name_color);
    if change.is_deleted {
        format.strikethrough = Stroke::new(1.0, name_color);
    }
    paint_elided(
        &painter,
        pos2(left + NAME_LEFT, center_y),
        file.file_name(),
        format,
        name_right - style::SPACE_XS,
    );

    let summary = row_summary(file, &change, state);
    tooltip_beside_row(ui, response, &file.path, summary, state.side, &palette);
    if checkbox.clicked() {
        return Some(FileRowEvent::ToggleViewed);
    }
    if response.clicked() {
        return Some(FileRowEvent::Select);
    }
    None
}

/// A file row's viewed checkbox in the viewed column (its caret column stays empty), with the
/// indent guides it sits after.
fn viewed_checkbox(
    ui: &mut Ui,
    rect: Rect,
    response: &Response,
    file: &ChangedFile,
    state: &FileRowState,
) -> Response {
    let palette = theme::palette(ui.ctx());
    let checkbox_rect = lead_rect(lead_left(rect, state.depth) + CHECK_LEFT, rect.center().y);
    let viewed = ui.interact(checkbox_rect, response.id.with("viewed"), Sense::click());
    viewed.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::CheckBox,
            true,
            state.is_viewed,
            format!("Viewed {}", file.path),
        )
    });
    focus::ring(ui, &viewed, checkbox_rect);
    let painter = ui.painter_at(rect);
    paint_indent_guides(&painter, rect, state.depth, &palette);
    let shown = checkbox::Mark::from(state.is_viewed);
    let color = mark_color(&palette, shown, viewed.hovered());
    checkbox::paint(&painter, &palette, checkbox_rect.center(), icons::SIZE, shown, color);
    viewed
}

/// The app's checkbox colors, but an empty box at rest recedes further: every row of a long tree
/// has one, and only the checked ones should catch the eye.
fn mark_color(palette: &Palette, mark: checkbox::Mark, is_hovered: bool) -> Color32 {
    match mark {
        checkbox::Mark::Off if !is_hovered => palette.text_disabled,
        checkbox::Mark::Off => palette.text_secondary,
        checkbox::Mark::On | checkbox::Mark::Mixed => checkbox::color(palette, mark, is_hovered),
    }
}

/// "Modified, 4 added, 3 deleted, 2 comments, viewed" (with "only whitespace or line endings
/// changed" after the counts where that is so) for the tooltip.
fn row_summary(file: &ChangedFile, change: &ChangeLook, state: &FileRowState) -> String {
    let mut parts = vec![match &file.old_path {
        Some(old_path) => format!("{} from {old_path}", change.name),
        None => change.name.to_owned(),
    }];
    if let (Some(additions), Some(deletions)) = (file.additions, file.deletions) {
        parts.push(format!(
            "{} added, {} deleted",
            group_digits(additions as usize),
            group_digits(deletions as usize)
        ));
        if file.whitespace_only {
            parts.push("only whitespace or line endings changed".to_owned());
        }
    } else if file.is_binary {
        parts.push("binary".to_owned());
    }
    match state.comments {
        0 => {}
        1 => parts.push("1 comment".to_owned()),
        count => parts.push(format!("{} comments", group_digits(count))),
    }
    if state.is_viewed {
        parts.push("viewed".to_owned());
    }
    parts.join(", ")
}

/// Path tooltips open beside the row, over the diff (away from the window edge the tree sits at),
/// so they never hide the rows around it.
/// The detail line is also the row's accessible description.
fn tooltip_beside_row(
    ui: &Ui,
    response: &Response,
    path: &str,
    detail: String,
    side: TreeSide,
    palette: &Palette,
) {
    ui.ctx().accesskit_node_builder(response.id, |node| node.set_description(detail.clone()));
    let (towards_diff, alternatives): (_, &'static [RectAlign]) = match side {
        TreeSide::Left => (RectAlign::RIGHT, &[RectAlign::RIGHT_START, RectAlign::LEFT]),
        TreeSide::Right => (RectAlign::LEFT, &[RectAlign::LEFT_START, RectAlign::RIGHT]),
    };
    let mut tooltip = egui::Tooltip::for_enabled(response).gap(style::SPACE_S);
    tooltip.popup = tooltip.popup.align(towards_diff).align_alternatives(alternatives);
    tooltip.show(|ui| {
        ui.label(egui::RichText::new(path).size(NAME_SIZE).color(palette.text));
        ui.label(egui::RichText::new(detail).size(style::SMALL_SIZE).color(palette.text_secondary));
    });
}

/// The file-type glyph in its family's color, Fill on the selected row. A change other than a plain
/// edit adds a mark on the glyph's lower right, cut out of the row color so it reads on any fill:
/// a solid dot where content came or went (added, deleted), a ring where the file moved or changed
/// type, so the two groups differ in shape as well as color. The tooltip and the accessible
/// description name the change in words. A deleted file's glyph steps back to secondary text,
/// like its struck-through name.
fn paint_file_icon(
    painter: &Painter,
    center: Pos2,
    icon: FileIcon,
    change: &ChangeLook,
    is_selected: bool,
    row_color: Color32,
    palette: &Palette,
) {
    let color = if change.is_deleted { palette.text_secondary } else { palette.file_hue(icon.hue) };
    painter.text(
        center,
        Align2::CENTER_CENTER,
        icon.glyph,
        icons::state_font(palette, icons::SIZE, is_selected),
        color,
    );
    let Some(mark) = change.mark else {
        return;
    };
    let dot = center + MARK_OFFSET;
    painter.circle_filled(dot, MARK_RADIUS + MARK_RING, row_color);
    match mark {
        Mark::Dot(color) => {
            painter.circle_filled(dot, MARK_RADIUS, color);
        }
        Mark::Ring(color) => {
            painter.circle_stroke(
                dot,
                MARK_RADIUS - MARK_STROKE / 2.0,
                Stroke::new(MARK_STROKE, color),
            );
        }
    }
}

pub fn paint_comment_badge(
    painter: &Painter,
    right_center: Pos2,
    count: usize,
    palette: &Palette,
) -> Rect {
    let text = format!("{} {}", icons::CHAT_CIRCLE_TEXT, group_digits(count));
    let galley =
        painter.layout_no_wrap(text, FontId::proportional(style::SMALL_SIZE), palette.accent_text);
    let size = vec2(galley.size().x + 2.0 * 6.0, PILL_HEIGHT);
    let pill =
        Rect::from_min_size(pos2(right_center.x - size.x, right_center.y - size.y / 2.0), size);
    let strength = if palette.is_dark { 0.22 } else { 0.12 };
    painter.rect_filled(pill, PILL_HEIGHT / 2.0, palette.accent.gamma_multiply(strength));
    painter.galley(pill.center() - galley.size() / 2.0, galley, palette.accent_text);
    pill
}

/// A file's type glyph with its change mark, as its tree row shows it, on a row of `row_color`.
pub fn paint_change_icon(
    painter: &Painter,
    center: Pos2,
    file: &ChangedFile,
    row_color: Color32,
    palette: &Palette,
) {
    let change = change_look(file.kind, palette);
    paint_file_icon(
        painter,
        center,
        file_icon::file_icon(&file.path),
        &change,
        false,
        row_color,
        palette,
    );
}

struct ChangeLook {
    name: &'static str,
    /// None for a plain edit, the common case that needs no flag.
    mark: Option<Mark>,
    is_deleted: bool,
}

#[derive(Clone, Copy)]
enum Mark {
    Dot(Color32),
    Ring(Color32),
}

fn change_look(kind: ChangeKind, palette: &Palette) -> ChangeLook {
    let (name, mark) = match kind {
        ChangeKind::Added => ("Added", Some(Mark::Dot(palette.success))),
        ChangeKind::Modified => ("Modified", None),
        ChangeKind::Deleted => ("Deleted", Some(Mark::Dot(palette.error))),
        ChangeKind::Renamed => ("Renamed", Some(Mark::Ring(palette.warning))),
        ChangeKind::Copied => ("Copied", Some(Mark::Ring(palette.warning))),
        ChangeKind::TypeChanged => ("Type changed", Some(Mark::Ring(palette.text_secondary))),
    };
    ChangeLook { name, mark, is_deleted: kind == ChangeKind::Deleted }
}
