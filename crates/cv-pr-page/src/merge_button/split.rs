//! The split button's two halves: where they sit, how they read to screen readers, and the main
//! part's width, wide enough for every label so the button never resizes.

use egui::{Color32, Rect, Response, Sense, Ui, pos2, vec2};

use super::{CHEVRON_WIDTH, FAILED_LABEL, ICON_GAP, PADDING_X, auto_complete, complete_label};
use cv_base::theme::{icons, style};
use cv_pr_session::merge_choice;
use cv_pr_session::session::PullRequestSession;

/// The split button's two halves this frame.
#[derive(Clone, Copy)]
pub(super) struct SplitParts {
    pub id: egui::Id,
    pub main_width: f32,
    /// The main part arms and confirms.
    pub is_main_enabled: bool,
    /// The chevron opens the dropdown.
    pub is_chevron_enabled: bool,
    pub is_open: bool,
}

impl SplitParts {
    /// Places the button and its two halves, named `label` and "Merge options"; returns the
    /// whole button's rect, the main part and the chevron.
    pub fn interact(self, ui: &mut Ui, label: &str) -> (Rect, Response, Response) {
        let (rect, _) = ui.allocate_exact_size(
            vec2(self.main_width + CHEVRON_WIDTH, style::CONTROL_HEIGHT),
            Sense::hover(),
        );
        let main_rect =
            Rect::from_min_max(rect.min, pos2(rect.left() + self.main_width, rect.bottom()));
        let chevron_rect = Rect::from_min_max(pos2(main_rect.right(), rect.top()), rect.max);
        let sense = |is_enabled| if is_enabled { Sense::click() } else { Sense::hover() };
        let main = ui.interact(main_rect, self.id.with("main"), sense(self.is_main_enabled));
        let chevron =
            ui.interact(chevron_rect, self.id.with("chevron"), sense(self.is_chevron_enabled));
        main.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, self.is_main_enabled, label)
        });
        chevron.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::Role::Button,
                self.is_chevron_enabled,
                self.is_open,
                "Merge options",
            )
        });
        (rect, main, chevron)
    }
}

/// The main part's width: every label the platform can show fits, so loading or switching
/// never resizes the button.
pub(super) fn main_width(ui: &Ui, session: &PullRequestSession, label: &str) -> f32 {
    let kind = session.kind;
    let font = egui::FontId::proportional(style::BODY_SIZE);
    let text_width = |text: &str| {
        ui.painter().layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size().x
    };
    let reserved = session
        .capabilities
        .merge_strategies
        .iter()
        .map(|strategy| merge_choice::button_label(*strategy, kind))
        .chain([auto_complete::set_label(kind), auto_complete::cancel_label(kind)])
        .map(text_width)
        .fold(text_width(complete_label(kind)).max(text_width(FAILED_LABEL)), f32::max);
    PADDING_X * 2.0 + icons::SIZE + ICON_GAP + reserved.max(text_width(label))
}
