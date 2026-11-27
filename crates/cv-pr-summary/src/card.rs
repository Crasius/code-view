//! One file's card in the folder summary: a header (collapse caret, file-type icon, name, line
//! counts, path, comments and the View button) over the changed lines, which the file view's
//! diff view shows and takes comments on (see `FolderSummary`), or over a Markdown file's rendered
//! diff (see [`super::rendered`]). Only the rows inside the clip rect are painted, so a tall card
//! costs what fits on screen.

use std::borrow::Cow;

use egui::{Align2, Color32, CursorIcon, FontId, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use review_git::ChangedFile;

use super::bodies::{CardBody, Placeholder};
use super::layout::{self, Geometry, Shape};
use super::rendered::RenderedCard;
use cv_base::platform::shortcut;
use cv_base::theme::{self, Palette, fonts, icons, style};
use cv_pr_tree::rows;
use cv_widgets::numbers::group_digits;
use cv_widgets::{focus, row};

const PAD_X: f32 = style::SPACE_M;
/// The gap between the added and the deleted count, and between View's icon and label.
const SMALL_GAP: f32 = 6.0;
const CARET_WIDTH: f32 = 16.0;
const ICON_WIDTH: f32 = 18.0;
const VIEW_WIDTH: f32 = 72.0;
const VIEW_HEIGHT: f32 = 28.0;
const NAME_SIZE: f32 = style::BODY_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAction {
    Toggle,
    View,
    Retry,
    /// Load a change above the inline limit anyway.
    ShowHere,
}

/// What the body of a card shows this frame.
pub enum Body<'a> {
    Loaded(&'a mut CardBody),
    /// A Markdown file's rendered diff, its versions loaded.
    Rendered(&'a mut RenderedCard),
    /// Not loaded yet (or waiting to): placeholder rows the size of the estimate.
    Pending,
    TooLarge,
    Binary,
    /// Line counts say nothing changed in the content: a rename, copy or mode change.
    Unchanged,
    Failed(&'a str),
}

/// The body of a card without a loaded diff.
pub fn placeholder_body<'a>(
    file: &ChangedFile,
    is_forced: bool,
    failure: Option<&'a str>,
) -> Body<'a> {
    if file.is_binary {
        Body::Binary
    } else if layout::is_too_large(file) && !is_forced {
        Body::TooLarge
    } else if layout::is_unchanged(file) {
        Body::Unchanged
    } else {
        failure.map_or(Body::Pending, Body::Failed)
    }
}

pub struct CardView<'a> {
    pub file: &'a ChangedFile,
    /// The card without the gap below it.
    pub rect: Rect,
    pub shape: Shape,
    pub is_collapsed: bool,
    pub is_current: bool,
    pub comments: usize,
    pub body: Body<'a>,
    pub geometry: &'a Geometry,
}

/// What a loaded card's body shows.
pub enum Content<'a> {
    Code(&'a mut CardBody),
    Rendered(&'a mut RenderedCard),
}

/// Paints a loaded card's code or rendered diff in the body rect, clipped to the second rect.
pub type CodeRows<'a> = dyn FnMut(&mut Ui, Rect, Rect, Content<'_>) + 'a;

/// Paints the card and returns what the user did with it; `code` paints a loaded card's rows.
pub fn show(ui: &mut Ui, view: CardView<'_>, code: &mut CodeRows<'_>) -> Option<CardAction> {
    let palette = theme::palette(ui.ctx());
    let rect = view.rect;
    let header = Rect::from_min_size(rect.min, vec2(rect.width(), view.geometry.header));
    let corner = style::RADIUS_CARD;
    ui.painter().rect_filled(rect, corner, palette.surface);
    let mut action = header_row(ui, header, &view, &palette);
    if !view.is_collapsed {
        let body = Rect::from_min_max(pos2(rect.left(), header.bottom()), rect.max);
        ui.painter().hline(rect.x_range(), header.bottom() - 0.5, Stroke::new(1.0, palette.border));
        let clip = body.shrink2(vec2(1.0, 0.0)).intersect(ui.clip_rect());
        if clip.is_positive() {
            let area = BodyArea { body, clip };
            let shown = body_rows(ui, area, view.body, view.shape, view.geometry, view.file, code);
            action = action.or(shown);
        }
    }
    let stroke = if view.is_current && focus::is_visible(ui.ctx()) {
        Stroke::new(2.0, palette.accent)
    } else {
        Stroke::new(1.0, palette.border)
    };
    ui.painter().rect_stroke(rect, corner, stroke, StrokeKind::Inside);
    action
}

fn header_row(
    ui: &mut Ui,
    header: Rect,
    view: &CardView<'_>,
    palette: &Palette,
) -> Option<CardAction> {
    let file = view.file;
    let id = ui.scope_id().with(("summary_card", &file.path));
    let (response, fill) = header_face(ui, header, id, view, palette);
    let center_y = header.center().y;
    let mut x = header.left() + PAD_X;
    let caret = if view.is_collapsed { icons::CARET_RIGHT } else { icons::CARET_DOWN };
    ui.painter().text(
        pos2(x + CARET_WIDTH / 2.0, center_y),
        Align2::CENTER_CENTER,
        caret,
        FontId::proportional(style::SMALL_SIZE),
        palette.text_secondary,
    );
    x += CARET_WIDTH + style::SPACE_XS;
    let row_color = palette.surface.blend(fill);
    rows::paint_change_icon(
        ui.painter(),
        pos2(x + ICON_WIDTH / 2.0, center_y),
        file,
        row_color,
        palette,
    );
    x += ICON_WIDTH + style::SPACE_S;

    // From the right: View, then the comment count.
    let view_rect = Rect::from_min_size(
        pos2(header.right() - PAD_X - VIEW_WIDTH, center_y - VIEW_HEIGHT / 2.0),
        vec2(VIEW_WIDTH, VIEW_HEIGHT),
    );
    let mut right = view_rect.left() - style::SPACE_S;
    if view.comments > 0 {
        let badge =
            rows::paint_comment_badge(ui.painter(), pos2(right, center_y), view.comments, palette);
        right = badge.left() - style::SPACE_S;
    }

    let name_galley = ui.painter().layout_no_wrap(
        file.file_name().to_owned(),
        fonts::semibold_font(NAME_SIZE),
        palette.text,
    );
    let name_width = name_galley.size().x.min((right - x).max(0.0));
    let name_format = egui::text::TextFormat::simple(fonts::semibold_font(NAME_SIZE), palette.text);
    let mut is_cut = rows::paint_elided(
        ui.painter(),
        pos2(x, center_y),
        file.file_name(),
        name_format,
        x + name_width,
    );
    x += name_width + style::SPACE_S;
    if let Some(after) = paint_counts(ui, file, pos2(x, center_y), right, palette) {
        x = after;
    } else {
        is_cut = true;
        x = right;
    }
    let path = match &file.old_path {
        Some(old_path) => format!("{}  {}  {old_path}", file.path, icons::ARROW_LEFT),
        None => file.path.clone(),
    };
    if x < right {
        let format = egui::text::TextFormat::simple(
            FontId::proportional(style::SMALL_SIZE),
            palette.text_secondary,
        );
        is_cut |= rows::paint_elided(ui.painter(), pos2(x, center_y), &path, format, right);
    } else {
        is_cut = true;
    }

    if view_button(ui, view_rect, id.with("view"), file, palette) {
        return Some(CardAction::View);
    }
    // A cut name or path reads in full on hover.
    let response = if is_cut { response.on_hover_text(path) } else { response };
    response.clicked().then_some(CardAction::Toggle)
}

/// The header's hit area, accessible name and tint, eased on hover; returns its response and fill.
fn header_face(
    ui: &mut Ui,
    header: Rect,
    id: egui::Id,
    view: &CardView<'_>,
    palette: &Palette,
) -> (egui::Response, Color32) {
    let response =
        ui.interact(header, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let verb = if view.is_collapsed { "Expand" } else { "Collapse" };
    // The comment badge is painted; a screen reader hears its count with the header.
    let comments = match view.comments {
        0 => String::new(),
        1 => ", 1 open comment".to_owned(),
        count => format!(", {count} open comments"),
    };
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            true,
            format!("{verb} {}{comments}", view.file.path),
        )
    });
    // The top corners stay round over an open card; a collapsed one is all header.
    let rounding = if view.is_collapsed {
        egui::CornerRadius::same(style::RADIUS_CARD)
    } else {
        egui::CornerRadius { nw: style::RADIUS_CARD, ne: style::RADIUS_CARD, sw: 0, se: 0 }
    };
    let fill = row::state_fill(ui, &response, palette.surface_subtle);
    let header_fill = if view.is_collapsed { header.shrink(0.5) } else { header };
    ui.painter().rect_filled(header_fill, rounding, fill);
    focus::ring(ui, &response, header);
    (response, fill)
}

/// "+12 −3" from `left_center`, whole or not at all: they never run under the comment badge or
/// View at `right`. Returns where the path starts, or `None` when the counts did not fit. A pure
/// rename has no line counts worth showing (its body says so) and leaves `x` as it is.
fn paint_counts(
    ui: &Ui,
    file: &ChangedFile,
    left_center: egui::Pos2,
    right: f32,
    palette: &Palette,
) -> Option<f32> {
    let (x, center_y) = (left_center.x, left_center.y);
    let (Some(additions), Some(deletions)) = (file.additions, file.deletions) else {
        return Some(x);
    };
    if additions + deletions == 0 {
        return Some(x);
    }
    let font = FontId::proportional(style::SMALL_SIZE);
    let added = ui.painter().layout_no_wrap(
        format!("+{}", group_digits(additions as usize)),
        font.clone(),
        palette.success,
    );
    let deleted = ui.painter().layout_no_wrap(
        format!("−{}", group_digits(deletions as usize)),
        font,
        palette.error,
    );
    let counts_right = x + added.size().x + SMALL_GAP + deleted.size().x;
    if counts_right > right {
        return None;
    }
    ui.painter().galley(pos2(x, center_y - added.size().y / 2.0), added, palette.success);
    ui.painter().galley(
        pos2(counts_right - deleted.size().x, center_y - deleted.size().y / 2.0),
        deleted,
        palette.error,
    );
    Some(counts_right + style::SPACE_M)
}

/// "View": opens the file in the file view, as selecting it in the tree does.
fn view_button(
    ui: &mut Ui,
    rect: Rect,
    id: egui::Id,
    file: &ChangedFile,
    palette: &Palette,
) -> bool {
    let response = ui
        .interact(rect, id, Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(format!(
            concat!("Open {} in the file view (", shortcut!(Enter), ")"),
            file.path
        ));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("View {}", file.path))
    });
    let fill = row::state_fill(ui, &response, palette.surface);
    ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
    ui.painter().rect_stroke(
        rect,
        style::RADIUS_CONTROL,
        Stroke::new(1.0, palette.border_strong),
        StrokeKind::Inside,
    );
    let font = FontId::proportional(style::BODY_SIZE);
    let label = ui.painter().layout_no_wrap("View".to_owned(), font, palette.text);
    let icon_font = icons::rest_font(palette, icons::SIZE_SMALL);
    let content = icons::SIZE_SMALL + SMALL_GAP + label.size().x;
    let left = rect.center().x - content / 2.0;
    ui.painter().text(
        pos2(left + icons::SIZE_SMALL / 2.0, rect.center().y),
        Align2::CENTER_CENTER,
        icons::ARROW_SQUARE_OUT,
        icon_font,
        palette.text,
    );
    ui.painter().galley(
        pos2(left + icons::SIZE_SMALL + SMALL_GAP, rect.center().y - label.size().y / 2.0),
        label,
        palette.text,
    );
    focus::ring(ui, &response, rect);
    response.clicked()
}

/// A card's body: where it is on screen and the part of it inside the view.
#[derive(Clone, Copy)]
struct BodyArea {
    body: Rect,
    clip: Rect,
}

fn body_rows(
    ui: &mut Ui,
    area: BodyArea,
    content: Body<'_>,
    shape: Shape,
    geometry: &Geometry,
    file: &ChangedFile,
    code: &mut CodeRows<'_>,
) -> Option<CardAction> {
    let BodyArea { body, clip } = area;
    let notice = match content {
        Body::Loaded(card) => match card.placeholder {
            Some(Placeholder::Binary) => Notice::binary(),
            Some(Placeholder::Lfs) => Notice {
                icon: icons::DATABASE,
                text: "Stored with Git LFS: only the pointer to the content changed.".into(),
                label: "Open file",
                action: CardAction::View,
            },
            Some(Placeholder::Unchanged) => Notice::unchanged(file),
            Some(Placeholder::TooLarge { changed }) => Notice::too_large(changed),
            None => {
                code(ui, body, clip, Content::Code(card));
                return None;
            }
        },
        Body::Rendered(card) => {
            code(ui, body, clip, Content::Rendered(card));
            return None;
        }
        Body::Pending => {
            pending_rows(ui, body, clip, shape, geometry, &theme::palette(ui.ctx()));
            return None;
        }
        Body::TooLarge => Notice::too_large(
            file.additions.unwrap_or(0).saturating_add(file.deletions.unwrap_or(0)),
        ),
        Body::Binary => Notice::binary(),
        Body::Unchanged => Notice::unchanged(file),
        Body::Failed(message) => Notice {
            icon: icons::WARNING,
            text: format!("Couldn't load the diff: {message}").into(),
            label: "Retry",
            action: CardAction::Retry,
        },
    };
    placeholder(ui, body, file, &notice)
}

/// Why a card shows no code, and the action that helps.
struct Notice {
    icon: &'static str,
    text: Cow<'static, str>,
    label: &'static str,
    action: CardAction,
}

impl Notice {
    fn binary() -> Self {
        Self {
            icon: icons::FILE,
            text: "Binary file: there are no lines to compare.".into(),
            label: "Open file",
            action: CardAction::View,
        }
    }

    /// A change of `changed` lines, above what a card shows inline.
    fn too_large(changed: u32) -> Self {
        Self {
            icon: icons::FILE_TEXT,
            text: format!(
                "Large change: {} lines. The file view shows it in full.",
                group_digits(changed as usize)
            )
            .into(),
            label: "Show here",
            action: CardAction::ShowHere,
        }
    }

    fn unchanged(file: &ChangedFile) -> Self {
        Self {
            icon: icons::FILE_TEXT,
            text: unchanged_text(file),
            label: "Open file",
            action: CardAction::View,
        }
    }
}

fn unchanged_text(file: &ChangedFile) -> Cow<'static, str> {
    // With whitespace ignored, a whitespace-only change has no lines left to show.
    if file.whitespace_only {
        return "Only whitespace or line endings changed, and whitespace is hidden.".into();
    }
    match &file.old_path {
        Some(old_path) => format!("Moved from {old_path} with the same content.").into(),
        None => "No changes to the content, only to the file mode or type.".into(),
    }
}

/// One line saying why there is no code, with the action that helps.
fn placeholder(ui: &mut Ui, body: Rect, file: &ChangedFile, notice: &Notice) -> Option<CardAction> {
    let Notice { icon, text, label, action } = notice;
    let (text, label, action) = (text.as_ref(), *label, *action);
    let palette = theme::palette(ui.ctx());
    let center_y = body.center().y;
    let left = body.left() + PAD_X + CARET_WIDTH + style::SPACE_XS;
    ui.painter().text(
        pos2(left + ICON_WIDTH / 2.0, center_y),
        Align2::CENTER_CENTER,
        icon,
        icons::rest_font(&palette, icons::SIZE),
        palette.text_secondary,
    );
    let button_width = ui
        .painter()
        .layout_no_wrap(
            label.to_owned(),
            FontId::proportional(style::BODY_SIZE),
            Color32::PLACEHOLDER,
        )
        .size()
        .x
        + 2.0 * style::SPACE_M;
    let button = Rect::from_min_size(
        pos2(body.right() - PAD_X - button_width, center_y - VIEW_HEIGHT / 2.0),
        vec2(button_width, VIEW_HEIGHT),
    );
    let format = egui::text::TextFormat::simple(
        FontId::proportional(style::BODY_SIZE),
        palette.text_secondary,
    );
    let text_room = (left + ICON_WIDTH + style::SPACE_S)..=(button.left() - style::SPACE_M);
    let is_cut = rows::paint_elided(
        ui.painter(),
        pos2(*text_room.start(), center_y),
        text,
        format,
        *text_room.end(),
    );
    if is_cut {
        // A cut notice reads in full on hover.
        let rows = (center_y - VIEW_HEIGHT / 2.0)..=(center_y + VIEW_HEIGHT / 2.0);
        let id = ui.scope_id().with(("summary_placeholder_text", &file.path));
        ui.interact(Rect::from_x_y_ranges(text_room, rows), id, Sense::hover()).on_hover_text(text);
    }
    let response = ui
        .interact(button, ui.scope_id().with(("summary_placeholder", &file.path)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(text);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{label} {}", file.path))
    });
    let fill = row::state_fill(ui, &response, Color32::TRANSPARENT);
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(button, style::RADIUS_CONTROL, fill);
    }
    ui.painter().text(
        button.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(style::BODY_SIZE),
        palette.accent_text,
    );
    focus::ring(ui, &response, button);
    response.clicked().then_some(action)
}

/// Muted bars the size of the rows to come, without animation: a summary may sit waiting while
/// the window is in the background, and nothing should repaint for it then.
fn pending_rows(
    ui: &Ui,
    body: Rect,
    clip: Rect,
    shape: Shape,
    geometry: &Geometry,
    palette: &Palette,
) {
    let Shape::Rows { lines, .. } = shape else {
        return;
    };
    let painter = ui.painter().with_clip_rect(clip);
    let shade = if palette.is_dark { palette.surface_subtle } else { palette.pressed };
    let first = ((clip.top() - body.top()) / geometry.line).floor().max(0.0) as u32;
    let last = (((clip.bottom() - body.top()) / geometry.line).ceil().max(0.0) as u32).min(lines);
    for index in first..last {
        let top = body.top() + index as f32 * geometry.line;
        let height = (geometry.line * 0.45).round();
        let y = top + (geometry.line - height) / 2.0;
        let seed = index as usize;
        let gutter = Rect::from_min_size(pos2(body.left() + 12.0, y), vec2(22.0, height));
        let indent = [0.0, 4.0, 8.0, 8.0, 12.0, 4.0, 0.0, 8.0][seed % 8] * 7.0;
        let width = (60.0 + ((seed * 37) % 380) as f32).min(body.width() - 160.0).max(20.0);
        let code = Rect::from_min_size(pos2(body.left() + 110.0 + indent, y), vec2(width, height));
        painter.rect_filled(gutter, 2, shade);
        painter.rect_filled(code, 2, shade);
    }
}
