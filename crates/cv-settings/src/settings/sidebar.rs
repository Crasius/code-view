//! The Settings sidebar: the search field, one entry per section and a note that changes save at
//! once. ↑/↓ move between the entries while one has the keyboard, Enter opens it; Ctrl+F (⌘F)
//! focuses the search and Esc there clears it.

use cv_base::theme::{self, icons, motion, style};
use cv_widgets::{badges, buttons, focus, glyph, row};
use egui::{Align2, Color32, Id, Key, Modifiers, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

use super::category::Category;

pub const WIDTH: f32 = 240.0;
const ENTRY_HEIGHT: f32 = 36.0;
const SEARCH_ID: &str = "settings_search";

/// What the page shows: a section, or the rows matching a search.
#[derive(Default)]
pub struct Navigation {
    pub category: Category,
    pub query: String,
}

impl Navigation {
    /// The trimmed search, empty while nobody searches.
    pub fn search(&self) -> &str {
        self.query.trim()
    }

    pub fn open(&mut self, category: Category) {
        self.category = category;
        self.query.clear();
    }

    /// Puts the keyboard into the search field.
    pub fn focus_search(ctx: &egui::Context) {
        ctx.memory_mut(|memory| memory.request_focus(Id::unique(SEARCH_ID)));
    }

    /// The sidebar in `ui`; `problems` counts connections with an access problem.
    pub fn show(&mut self, ui: &mut Ui, categories: &[Category], problems: usize) {
        let palette = theme::palette(ui.ctx());
        ui.spacing_mut().item_spacing.y = style::SPACE_XS;
        ui.label(RichText::new("Settings").font(theme::fonts::semibold_font(style::TITLE_SIZE)));
        ui.add_space(style::SPACE_S);
        self.search_field(ui);
        ui.add_space(style::SPACE_S);
        let searching = !self.search().is_empty();
        let mut ids = Vec::with_capacity(categories.len());
        for category in categories {
            let is_current = !searching && *category == self.category;
            let badge = (*category == Category::Connections && problems > 0).then_some(problems);
            let mut response = entry(ui, *category, is_current, badge);
            if badge.is_some() {
                response = response.on_hover_text(match problems {
                    1 => "A connection needs its token fixed".to_owned(),
                    count => format!("{count} connections need their token fixed"),
                });
            }
            if response.clicked() {
                self.open(*category);
            }
            ids.push(response.id);
        }
        arrow_keys(ui, &ids);
        ui.add_space(style::SPACE_M);
        ui.label(
            RichText::new("Changes save as you make them.")
                .size(style::SMALL_SIZE)
                .color(palette.text_secondary),
        );
    }

    fn search_field(&mut self, ui: &mut Ui) {
        let palette = theme::palette(ui.ctx());
        let id = Id::unique(SEARCH_ID);
        let frame = egui::Frame::new()
            .fill(palette.surface)
            .stroke(Stroke::new(1.0, palette.border_strong))
            .corner_radius(style::RADIUS_CONTROL)
            .inner_margin(egui::Margin::symmetric(style::SPACE_S as i8, 0));
        frame.show(ui, |ui| {
            let size = egui::vec2(ui.available_width(), style::CONTROL_HEIGHT);
            ui.allocate_ui_with_layout(
                size,
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = style::SPACE_XS;
                    glyph::decorative(
                        ui,
                        badges::icon_text(
                            icons::MAGNIFYING_GLASS,
                            palette.text_secondary,
                            icons::SIZE,
                        ),
                    );
                    let has_text = !self.query.is_empty();
                    let clear_width =
                        if has_text { buttons::icon_width() + style::SPACE_XS } else { 0.0 };
                    let field = egui::TextEdit::singleline(&mut self.query)
                        .id(id)
                        .hint_text("Search settings…")
                        .align(egui::Align2::LEFT_CENTER)
                        .frame(egui::Frame::NONE)
                        .desired_width(ui.available_width() - clear_width);
                    let response = ui.add(field);
                    let is_escaped =
                        response.lost_focus() && ui.input(|input| input.key_pressed(Key::Escape));
                    if is_escaped {
                        self.query.clear();
                    }
                    if has_text && buttons::icon(ui, icons::X, "Clear search (Esc)").clicked() {
                        self.query.clear();
                        response.request_focus();
                    }
                    // ↓ from the search goes to the entries.
                    if response.has_focus()
                        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::ArrowDown))
                    {
                        ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::Next));
                    }
                },
            );
        });
    }
}

/// One section entry: icon, title, an optional problem count; the current one tinted with an
/// accent bar, as the navigation rail marks its section.
fn entry(
    ui: &mut Ui,
    category: Category,
    is_current: bool,
    badge: Option<usize>,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ENTRY_HEIGHT), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Tab, true, is_current, category.title())
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let current = motion::toggle(ui.ctx(), response.id.with("current"), is_current, motion::FAST);
    let fill = motion::mix(
        row::state_fill(ui, &response, Color32::TRANSPARENT),
        palette.selection,
        current,
    );
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, style::RADIUS_CONTROL, fill);
    }
    if current > 0.0 {
        let bar = Rect::from_center_size(
            pos2(rect.left() + 1.5, rect.center().y),
            vec2(3.0, (rect.height() - 14.0) * current),
        );
        ui.painter().rect_filled(bar, 2, palette.accent);
    }
    let icon_center = pos2(rect.left() + style::SPACE_M + icons::SIZE / 2.0, rect.center().y);
    ui.painter().text(
        icon_center,
        Align2::CENTER_CENTER,
        category.icon(),
        icons::state_font(&palette, icons::SIZE, is_current),
        if is_current { palette.accent_text } else { palette.text_secondary },
    );
    let font = if is_current {
        theme::fonts::semibold_font(style::BODY_SIZE)
    } else {
        egui::FontId::proportional(style::BODY_SIZE)
    };
    ui.painter().text(
        pos2(icon_center.x + icons::SIZE / 2.0 + style::SPACE_S, rect.center().y),
        Align2::LEFT_CENTER,
        category.title(),
        font,
        palette.text,
    );
    if let Some(count) = badge {
        problem_badge(ui, rect, count);
    }
    focus::ring(ui, &response, rect);
    response
}

/// A warning-colored count at the entry's right end; the entry's tooltip says what it counts.
fn problem_badge(ui: &Ui, entry: Rect, count: usize) {
    let palette = theme::palette(ui.ctx());
    let galley = ui.painter().layout_no_wrap(
        count.to_string(),
        theme::fonts::semibold_font(style::SMALL_SIZE),
        palette.text,
    );
    let size = vec2(galley.size().x.max(galley.size().y) + style::SPACE_S, galley.size().y + 2.0);
    let rect = Rect::from_center_size(
        pos2(entry.right() - style::SPACE_S - size.x / 2.0, entry.center().y),
        size,
    );
    ui.painter().rect_filled(rect, size.y / 2.0, palette.warning_surface);
    ui.painter().rect_stroke(
        rect,
        size.y / 2.0,
        Stroke::new(1.0, palette.warning),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, palette.text);
}

/// ↑/↓ while an entry has the keyboard move it to the entry above or below.
fn arrow_keys(ui: &Ui, ids: &[Id]) {
    let Some(at) = ids.iter().position(|id| ui.memory(|memory| memory.has_focus(*id))) else {
        return;
    };
    let (down, up) = ui.input_mut(|input| {
        (
            input.consume_key(Modifiers::NONE, Key::ArrowDown),
            input.consume_key(Modifiers::NONE, Key::ArrowUp),
        )
    });
    let target = if down {
        (at + 1).min(ids.len() - 1)
    } else if up {
        at.saturating_sub(1)
    } else {
        return;
    };
    if let Some(id) = ids.get(target) {
        ui.memory_mut(|memory| memory.request_focus(*id));
    }
}
