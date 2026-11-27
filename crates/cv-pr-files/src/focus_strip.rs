//! The Files tab's bar in focus mode, in place of the pull request header, the tabs and the file
//! header: the previous and next file on the left, the file's path in the middle (where dragging
//! moves the window), and on the right the change counter with the previous and next change, the
//! viewed mark, the layout and whitespace switches, the threads panel with its count and the way
//! out. The file tree switch sits at the end of the tree's side, its sidebar glyph turned to match;
//! its context menu moves the tree to the other side. The keys of the file header keep working
//! (N/P, J/K, Alt+V, Alt+W, Alt+N, Alt+T); Ctrl+B shows or hides the file tree, which stays folded
//! away unless the user wanted it (saved).
//!
//! While a comment's original version shows, the bar says what its banner says instead, with
//! Back to latest, Open on the platform and Retry; Esc goes back to the latest version first and
//! leaves focus mode only after that.

use egui::{Label, RichText, Ui};
use review_core::{DiffSide, ProviderKind};
use review_git::ChangedFile;
use review_store::{DiffLayout, TreeSide};

use super::origin_view::{self, Face};
use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, motion, style};
use cv_pr_diff::diff_view::Direction;
use cv_pr_rendered::file_header::{self, HeaderAction};
use cv_services::Services;
use cv_shell::focus_mode::strip;
use cv_widgets::{buttons, glyph};

/// The widest change counter the slot is sized for, so counting never moves the arrows.
const COUNTER_WIDTH: f32 = 64.0;

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent per-frame facts the tab hands the bar, each read on its own"
)]
pub struct StripInputs<'a> {
    /// The open file; `None` while the summary of `folder` shows.
    pub file: Option<&'a ChangedFile>,
    pub folder: &'a str,
    pub is_viewed: bool,
    /// (1-based change in focus or 0, number of changes) of the diff on screen; `None` without one.
    pub changes: Option<(usize, usize)>,
    /// The diff on screen is a rendered Markdown diff: whitespace does not apply.
    pub is_rendered: bool,
    pub whole_file: Option<DiffSide>,
    /// "Ignore whitespace" left the file without changes: its switch stays usable to show them.
    pub hides_whitespace: bool,
    pub has_previous_file: bool,
    pub has_next_file: bool,
    /// What N or P does now that a step found no change further that way, while it shows.
    pub edge_hint: Option<&'a str>,
    /// Open threads of the pull request, and whether any has news.
    pub open_threads: usize,
    pub has_news: bool,
}

/// What the bar asks of the tab: the file header's actions, and a step to another file.
#[derive(Default)]
pub struct StripAction {
    pub header: HeaderAction,
    pub step_file: Option<Direction>,
}

/// What the bar asks of a comment's original version.
pub enum OriginAction {
    Back,
    Retry,
}

/// Draws the bar at the top of `ui`. Switches that are settings (layout, whitespace, the file tree,
/// the threads panel) change and save them right away, as the file header does.
pub fn show(ui: &mut Ui, inputs: &StripInputs<'_>, services: &mut Services) -> StripAction {
    let mut action = StripAction::default();
    let wants_tree = wants_tree(ui);
    let side = services.settings.file_tree_side;
    let mut changed = false;
    strip::show(ui, "files_focus_strip", |bar| {
        bar.right(|ui| {
            strip::exit_button(ui);
            strip::separator(ui);
            if side == TreeSide::Right {
                changed |= tree_toggle(ui, wants_tree, services);
            }
            changed |= threads_toggle(ui, inputs, services);
            strip::separator(ui);
            changed |= diff_switches(ui, inputs, services, &mut action.header);
            strip::separator(ui);
            if viewed_toggle(ui, inputs) {
                action.header.toggle_viewed = true;
            }
            strip::separator(ui);
            let before = ui.cursor().max.x;
            action.header.navigate = change_arrows(ui, inputs.changes).or(action.header.navigate);
            strip::note(
                ui,
                &inputs.changes.map(file_header::position_text).unwrap_or_default(),
                COUNTER_WIDTH,
            );
            if let Some(text) = inputs.edge_hint {
                // Right to left: the arrows and the counter end where the cursor was before them.
                let anchor = egui::Rect::from_x_y_ranges(
                    ui.cursor().max.x..=before,
                    ui.max_rect().y_range(),
                );
                file_header::edge_hint(ui, anchor, text);
            }
        });
        bar.left(|ui| {
            if side == TreeSide::Left {
                changed |= tree_toggle(ui, wants_tree, services);
            }
            action.step_file = file_steps(ui, inputs);
        });
        let width = bar.middle_width();
        bar.middle(|ui| title(ui, inputs, width));
    });
    if changed {
        services.save_settings();
    }
    action
}

/// The threads panel switch with the open threads' count; true when it changed the setting.
fn threads_toggle(ui: &mut Ui, inputs: &StripInputs<'_>, services: &mut Services) -> bool {
    let threads_open = services.settings.threads_panel.is_open;
    let hint = if threads_open { cv_pr_threads::HIDE_HINT } else { cv_pr_threads::SHOW_HINT };
    let news = if inputs.has_news { ", new replies" } else { "" };
    let name = format!("Threads {}{news}", inputs.open_threads);
    let clicked = strip::labeled_toggle(
        ui,
        icons::CHAT_CIRCLE_TEXT,
        &inputs.open_threads.to_string(),
        threads_open,
        &name,
        hint,
    )
    .clicked();
    if clicked {
        services.settings.threads_panel.is_open = !threads_open;
    }
    clicked
}

/// The open file's viewed mark; returns whether it was clicked.
fn viewed_toggle(ui: &mut Ui, inputs: &StripInputs<'_>) -> bool {
    let hint = if inputs.is_viewed {
        file_header::UNMARK_VIEWED_HINT
    } else {
        file_header::MARK_VIEWED_HINT
    };
    let icon = if inputs.is_viewed { icons::CHECK_SQUARE } else { icons::SQUARE };
    ui.add_enabled_ui(inputs.file.is_some(), |ui| {
        strip::toggle(ui, icon, inputs.is_viewed, egui::Role::CheckBox, hint)
    })
    .inner
    .clicked()
}

/// Next and previous change, usable while the diff on screen has changes.
fn change_arrows(ui: &mut Ui, changes: Option<(usize, usize)>) -> Option<Direction> {
    let has_changes = changes.is_some_and(|(_, total)| total > 0);
    ui.add_enabled_ui(has_changes, |ui| {
        let next = strip::button(ui, icons::ARROW_DOWN, "Next change (N or F7)").clicked();
        let previous =
            strip::button(ui, icons::ARROW_UP, file_header::PREVIOUS_CHANGE_HINT).clicked();
        if previous { Some(Direction::Previous) } else { next.then_some(Direction::Next) }
    })
    .inner
}

/// The previous and next file in the tree.
fn file_steps(ui: &mut Ui, inputs: &StripInputs<'_>) -> Option<Direction> {
    let previous_hint = concat!(
        "Previous file (K or ",
        shortcut!(alt "↑"),
        "); ",
        shortcut!(shift "K"),
        ": previous unviewed file"
    );
    let previous = ui
        .add_enabled_ui(inputs.has_previous_file, |ui| {
            strip::button(ui, icons::CARET_UP, previous_hint)
        })
        .inner
        .clicked();
    let next_hint = concat!(
        "Next file (J or ",
        shortcut!(alt "↓"),
        "); ",
        shortcut!(shift "J"),
        ": next unviewed file"
    );
    let next = ui
        .add_enabled_ui(inputs.has_next_file, |ui| strip::button(ui, icons::CARET_DOWN, next_hint))
        .inner
        .clicked();
    if next { Some(Direction::Next) } else { previous.then_some(Direction::Previous) }
}

/// The bar while a comment's original version shows: what the banner says (where dragging moves
/// the window), Retry while the version is missing, the comment on the platform, Back to latest,
/// the way out of focus mode, and the file tree switch at the end of the tree's side.
pub fn show_origin(
    ui: &mut Ui,
    face: &Face,
    web_url: Option<&str>,
    platform: ProviderKind,
    services: &mut Services,
) -> Option<OriginAction> {
    let wants_tree = wants_tree(ui);
    let side = services.settings.file_tree_side;
    let mut changed = false;
    let mut action = None;
    strip::show(ui, "files_focus_strip", |bar| {
        bar.right(|ui| {
            strip::exit_button(ui);
            strip::separator(ui);
            if side == TreeSide::Right {
                changed |= tree_toggle(ui, wants_tree, services);
                strip::separator(ui);
            }
            if strip::labeled_button(
                ui,
                icons::ARROW_LEFT,
                origin_view::BACK_LABEL,
                "Back to the latest version (Esc)",
            )
            .clicked()
            {
                action = Some(OriginAction::Back);
            }
            if let Some(url) = web_url
                && strip::button(
                    ui,
                    cv_widgets::provider_icon::glyph(platform),
                    &format!("Open on {}", platform.label()),
                )
                .clicked()
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
            if let Some(reason) = &face.missing
                && strip::button(ui, icons::ARROW_CLOCKWISE, &format!("Retry: {reason}")).clicked()
            {
                action = Some(OriginAction::Retry);
            }
        });
        if side == TreeSide::Left {
            bar.left(|ui| changed |= tree_toggle(ui, wants_tree, services));
        }
        let width = bar.middle_width();
        bar.middle(|ui| origin_title(ui, face, width));
    });
    if changed {
        services.save_settings();
    }
    action
}

/// Ctrl+B, unless a text field has the keys.
fn wants_tree(ui: &mut Ui) -> bool {
    !ui.ctx().text_edit_focused()
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::B))
}

/// The file tree switch; true when it changed a setting. Its sidebar glyph has its pane on the
/// tree's side. Its context menu moves the tree to the other side, also while the tree is hidden.
fn tree_toggle(ui: &mut Ui, wants_tree: bool, services: &mut Services) -> bool {
    let settings = &mut services.settings;
    let shows_tree = settings.focus_file_tree;
    let tree_hint = if shows_tree {
        concat!("Hide the file tree (", shortcut!(command "B"), ")")
    } else {
        concat!("Show the file tree (", shortcut!(command "B"), ")")
    };
    let toggle = strip::toggle(ui, "", shows_tree, egui::Role::Switch, tree_hint);
    let turn = match settings.file_tree_side {
        TreeSide::Left => 0.0,
        TreeSide::Right => std::f32::consts::PI,
    };
    paint_turned(ui, &toggle, icons::SIDEBAR_SIMPLE, shows_tree, turn);
    let mut changed = false;
    toggle.context_menu(|ui| {
        let (icon, label) = cv_pr_tree::flip_control(settings.file_tree_side);
        if buttons::menu_item(ui, icon, label, None, None).clicked() {
            settings.file_tree_side = settings.file_tree_side.flipped();
            changed = true;
            ui.close();
        }
    });
    if toggle.clicked() || wants_tree {
        settings.focus_file_tree = !shows_tree;
        changed = true;
    }
    changed
}

/// `icon` turned by `turn` radians about its center on `toggle`, in the color [`strip::toggle`]
/// gives its icons. The sidebar glyph is symmetric top to bottom, so half a turn mirrors it left to
/// right.
fn paint_turned(ui: &Ui, toggle: &egui::Response, icon: &str, is_on: bool, turn: f32) {
    let palette = theme::palette(ui.ctx());
    let color = if toggle.enabled() {
        let on = motion::toggle(ui.ctx(), toggle.id.with("strip_on"), is_on, motion::FAST);
        motion::mix(palette.text, palette.accent_text, on)
    } else {
        palette.text_disabled
    };
    let font = icons::state_font(&palette, icons::SIZE, is_on);
    let galley = ui.painter().layout_no_wrap(icon.to_owned(), font, color);
    // A text shape turns about its top-left corner: place that corner so the center stays put.
    let corner = toggle.rect.center() - egui::emath::Rot2::from_angle(turn) * (galley.size() / 2.0);
    let shape = egui::epaint::TextShape::new(corner, galley, color).with_angle(turn);
    ui.painter().add(shape);
}

/// The banner's words with the clock of an earlier version, then the file.
fn origin_title(ui: &mut Ui, face: &Face, width: f32) {
    let palette = theme::palette(ui.ctx());
    ui.set_max_width(width);
    glyph::decorative(
        ui,
        RichText::new(icons::CLOCK_COUNTER_CLOCKWISE)
            .font(icons::state_font(&palette, icons::SIZE_SMALL, false))
            .color(palette.warning),
    );
    let label = ui.add(
        Label::new(RichText::new(&face.text).size(style::BODY_SIZE).color(palette.text))
            .truncate()
            .selectable(false),
    );
    label.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &face.text));
    ui.add(
        Label::new(RichText::new(&face.path).size(style::SMALL_SIZE).color(palette.text_secondary))
            .truncate()
            .selectable(false),
    );
}

/// Layout and whitespace: the file header's switches, as icons. Returns true when one changed.
fn diff_switches(
    ui: &mut Ui,
    inputs: &StripInputs<'_>,
    services: &mut Services,
    header: &mut HeaderAction,
) -> bool {
    let has_diff = inputs.changes.is_some();
    let settings = &mut services.settings;
    let mut changed = false;
    let whitespace = ui
        .add_enabled_ui((has_diff && !inputs.is_rendered) || inputs.hides_whitespace, |ui| {
            strip::toggle(
                ui,
                icons::PARAGRAPH,
                settings.ignore_whitespace,
                egui::Role::Switch,
                concat!("Ignore whitespace (", shortcut!(alt "W"), ")"),
            )
        })
        .inner;
    let whitespace = if inputs.is_rendered {
        whitespace.on_disabled_hover_text(file_header::RENDERED_WHITESPACE_HINT)
    } else {
        whitespace
    };
    if whitespace.clicked() {
        settings.ignore_whitespace = !settings.ignore_whitespace;
        changed = true;
    }
    let shows = settings.show_comments_in_diff;
    let comments = ui
        .add_enabled_ui(has_diff, |ui| {
            strip::toggle(
                ui,
                icons::CHAT_CIRCLE_TEXT,
                shows,
                egui::Role::Switch,
                file_header::comments_hint(shows),
            )
        })
        .inner;
    if comments.clicked() {
        settings.show_comments_in_diff = !shows;
        changed = true;
    }
    // A whole added or deleted file has one version: there is no layout to choose. The folder
    // summary shows its cards inline or side by side; New only is the file view's own.
    let is_summary = inputs.file.is_none();
    let can_choose = has_diff && inputs.whole_file.is_none();
    for segment in file_header::LAYOUT_SEGMENTS {
        let layout = segment.value;
        let is_on = settings.diff_layout == layout;
        let hint = match layout {
            DiffLayout::NewOnly => concat!("New only (", shortcut!(alt "N"), ")"),
            DiffLayout::Inline | DiffLayout::SideBySide => segment.label,
        };
        let icon = segment.icon.expect("every layout has an icon");
        let is_enabled = if is_summary { layout != DiffLayout::NewOnly } else { can_choose };
        let toggle = ui
            .add_enabled_ui(is_enabled, |ui| {
                strip::toggle(ui, icon, is_on, egui::Role::RadioButton, hint)
            })
            .inner;
        if toggle.clicked() && !is_on {
            settings.diff_layout = layout;
            header.layout_changed = true;
            changed = true;
        }
    }
    changed
}

/// The open file's path (folders elided first), or which summary shows.
fn title(ui: &mut Ui, inputs: &StripInputs<'_>, width: f32) {
    if let Some(file) = inputs.file {
        if let Some(job) = file_header::path_job(ui, &file.path, width) {
            ui.add(Label::new(job).truncate().selectable(false));
        }
        return;
    }
    let palette = theme::palette(ui.ctx());
    let text = if inputs.folder.is_empty() {
        "All changed files".to_owned()
    } else {
        format!("{}/", inputs.folder)
    };
    ui.add(
        Label::new(RichText::new(text).size(style::BODY_SIZE).color(palette.text_secondary))
            .truncate()
            .selectable(false),
    );
}
