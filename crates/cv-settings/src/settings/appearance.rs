//! Settings → Appearance: the controls of its rows. Every change applies at once and is saved; the
//! preview beside the section ([`super::preview_diff`]) redraws with it.

mod header_modes;
mod miniature;
mod story;

use cv_base::theme::{self, icons, style};
use cv_services::Services;
use cv_shell::zoom;
use cv_widgets::tabs::{self, Segment};
use cv_widgets::{buttons, provider_icon};
use egui::Ui;
use review_core::ProviderKind;
use review_store::{
    CommentEditor, CommentEditors, DiffLayout, FilesStart, FrameRateLimit, PaneSizes,
    PullRequestStart, ThemePreference, TreeSide,
};

use super::rows::{self, AppearanceRow, Row};

/// The control of Appearance row `id`; a switch takes its name from `row`.
pub(crate) fn control(ui: &mut Ui, id: AppearanceRow, row: &Row, services: &mut Services) {
    let settings = &mut services.settings;
    let changed = match id {
        AppearanceRow::Theme => tabs::segments(
            ui,
            &mut settings.theme,
            &[
                Segment::new(ThemePreference::System, "System"),
                Segment::new(ThemePreference::Light, "Light"),
                Segment::new(ThemePreference::Dark, "Dark"),
            ],
        ),
        AppearanceRow::DiffLayout => tabs::segments(
            ui,
            &mut settings.diff_layout,
            &[
                Segment::new(DiffLayout::SideBySide, "Side by side"),
                Segment::new(DiffLayout::Inline, "Inline"),
                Segment::new(DiffLayout::NewOnly, "New only"),
            ],
        ),
        AppearanceRow::CodeFontSize => ui
            .add(
                egui::Slider::new(&mut settings.code_font_size, zoom::CODE_SIZE_RANGE).step_by(0.5),
            )
            .changed(),
        AppearanceRow::WrapLines => rows::switch(ui, &mut settings.wrap_lines, row),
        AppearanceRow::Comments => rows::switch(ui, &mut settings.show_comments_in_diff, row),
        AppearanceRow::SelectionOccurrences => {
            rows::switch(ui, &mut settings.highlight_selection_occurrences, row)
        }
        AppearanceRow::ScrollPastEnd => rows::switch(ui, &mut settings.scroll_past_end, row),
        AppearanceRow::ContextLines => {
            ui.add(egui::Slider::new(&mut settings.context_lines, 0..=50)).changed()
        }
        AppearanceRow::FavoritesLane => {
            rows::switch(ui, &mut settings.favorites.is_lane_shown, row)
        }
        AppearanceRow::RepositorySections => {
            rows::switch(ui, &mut settings.repository_sections.is_grouped, row)
        }
        AppearanceRow::PullRequestStart => pull_request_start(ui, &mut settings.pull_request_start),
        AppearanceRow::FilesStart => files_start(ui, &mut settings.files_start),
        AppearanceRow::TreeSide => tabs::segments(
            ui,
            &mut settings.file_tree_side,
            &[Segment::new(TreeSide::Left, "Left"), Segment::new(TreeSide::Right, "Right")],
        ),
        AppearanceRow::NextUnviewed => rows::switch(ui, &mut settings.open_next_unviewed, row),
        AppearanceRow::PullRequestHeader => header_modes::show(ui, &mut settings.header_modes),
        AppearanceRow::RenderedMarkdown => tabs::segments(
            ui,
            &mut settings.rendered_width.full,
            &[Segment::new(false, "Reading column"), Segment::new(true, "Full width")],
        ),
        AppearanceRow::CommentEditor => comment_editors(ui, &mut settings.comment_editors),
        AppearanceRow::InterfaceScale => {
            let scale = ui
                .add(egui::Slider::new(&mut settings.ui_scale, zoom::UI_SCALE_RANGE).step_by(0.05));
            // Applying the scale while dragging would move the slider under the pointer.
            scale.drag_stopped() || scale.lost_focus()
        }
        AppearanceRow::FrameRate => frame_rate(ui, &mut settings.frame_rate),
        AppearanceRow::PaneSizes => reset_panes(ui, &mut settings.panes),
    };
    if changed {
        services.save_settings();
    }
}

/// The tab a pull request opens on the first time.
fn pull_request_start(ui: &mut Ui, start: &mut PullRequestStart) -> bool {
    tabs::segments(
        ui,
        start,
        &[
            Segment::new(PullRequestStart::Overview, "Overview")
                .with_hint("The description, checks, reviewers and the conversation"),
            Segment::new(PullRequestStart::Files, "Files")
                .with_hint("The diff of the first file in the file tree"),
        ],
    )
}

/// What the Files tab shows when nothing particular was asked for.
fn files_start(ui: &mut Ui, start: &mut FilesStart) -> bool {
    tabs::segments(
        ui,
        start,
        &[
            Segment::new(FilesStart::AllFiles, "All files")
                .with_hint("The folder view: every changed file, one below the other"),
            Segment::new(FilesStart::FirstFile, "First file").with_hint(
                "The first file of the file tree; the folder view stays a click on a folder away",
            ),
        ],
    )
}

/// Rich text or Markdown per platform: the platform's icon and name, then its choice, the choices
/// lined up in one column.
fn comment_editors(ui: &mut Ui, editors: &mut CommentEditors) -> bool {
    let palette = theme::palette(ui.ctx());
    let mut changed = false;
    egui::Grid::new("comment_editors")
        .num_columns(2)
        .spacing(egui::vec2(style::SPACE_L, style::SPACE_S))
        .show(ui, |ui| {
            for kind in ProviderKind::ALL {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = style::SPACE_S;
                    let size = egui::vec2(icons::SIZE, icons::SIZE);
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    provider_icon::paint(ui.painter(), &palette, rect.center(), icons::SIZE, kind);
                    ui.label(kind.label());
                });
                let mut editor = editors.get(kind);
                let options = [
                    Segment::new(CommentEditor::Rich, "Rich text")
                        .with_hint("Write the comment as it will look"),
                    Segment::new(CommentEditor::Markdown, "Markdown")
                        .with_hint("Write the Markdown source"),
                ];
                if tabs::segments(ui, &mut editor, &options) {
                    editors.set(kind, editor);
                    changed = true;
                }
                ui.end_row();
            }
        });
    changed
}

/// "Reset pane sizes", usable while a pane has another size; returns whether it reset them.
fn reset_panes(ui: &mut Ui, panes: &mut PaneSizes) -> bool {
    let is_default = *panes == PaneSizes::DEFAULT;
    let reset = ui.add_enabled_ui(!is_default, |ui| {
        buttons::secondary_icon(ui, icons::ARROW_COUNTER_CLOCKWISE, "Reset pane sizes")
    });
    if reset.inner.clicked() {
        *panes = PaneSizes::DEFAULT;
    }
    reset.inner.clicked()
}

/// The presets as one segmented control; applies live, since the pacer reads the setting each frame.
fn frame_rate(ui: &mut Ui, limit: &mut FrameRateLimit) -> bool {
    let labels: Vec<(FrameRateLimit, String, String)> = FrameRateLimit::PRESETS
        .iter()
        .map(|fps| {
            (FrameRateLimit::Fps(*fps), fps.to_string(), format!("Up to {fps} frames per second"))
        })
        .chain(std::iter::once((
            FrameRateLimit::MatchDisplay,
            "Match display".to_owned(),
            "As many frames per second as the monitor refreshes".to_owned(),
        )))
        .collect();
    let options: Vec<Segment<'_, FrameRateLimit>> = labels
        .iter()
        .map(|(limit, label, hint)| Segment::new(*limit, label).with_hint(hint))
        .collect();
    tabs::segments(ui, limit, &options)
}
