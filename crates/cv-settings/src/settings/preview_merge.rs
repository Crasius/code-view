//! The Merging preview: the merge button and its list of types as a pull request on the chosen
//! platform shows them with the favourite set here. It draws with the merge button's own painter
//! (`merge_paint`) and picks with its own rule (`merge_choice::preselect`) on a branch that
//! allows every type the platform can merge with. Inert: nothing in it can merge.

use cv_base::theme::{self, icons, style};
use cv_pr_session::merge_choice::{self, Scope};
use cv_pr_session::merge_paint::{self as paint, CHEVRON_WIDTH, ICON_GAP, Look, PADDING_X, Row};
use cv_services::Services;
use cv_widgets::elide;
use cv_widgets::tabs::{self, Segment};
use egui::{RichText, Sense, Stroke, Ui, vec2};
use review_core::{MergePolicy, MergeStrategy, ProviderKind};

use super::merging;

const MENU_WIDTH: f32 = 360.0;
const TARGET: &str = "main";

/// `previewed` is the platform last worked on in the section; the preview's own switch changes it.
pub fn show(ui: &mut Ui, services: &Services, previewed: &mut Option<ProviderKind>) {
    let palette = theme::palette(ui.ctx());
    let kinds = merging::connected_kinds(services);
    let Some(first) = kinds.first().copied() else {
        return;
    };
    let mut kind = previewed.filter(|kind| kinds.contains(kind)).unwrap_or(first);
    if kinds.len() > 1 {
        let options: Vec<Segment<'_, ProviderKind>> =
            kinds.iter().map(|kind| Segment::new(*kind, kind.label())).collect();
        tabs::segments(ui, &mut kind, &options);
        ui.add_space(style::SPACE_M);
    }
    *previewed = Some(kind);
    let policy = MergePolicy::reported(merging::choices(kind).to_vec(), None);
    // No repository: only the platform favourite and the platform's own rule apply.
    let scope = Scope { kind, connection_id: "", repository: "" };
    let Some((preselected, _)) =
        merge_choice::preselect(&policy, None, &services.settings.merge, scope)
    else {
        return;
    };
    ui.label(
        RichText::new(format!("A pull request on {} into {TARGET}", kind.label()))
            .color(palette.text_secondary),
    );
    ui.add_space(style::SPACE_S);
    button(ui, kind, preselected);
    ui.add_space(style::SPACE_S);
    menu(ui, kind, &policy, preselected);
    ui.add_space(style::SPACE_S);
    let note = if services.settings.merge.platform_favourite(kind).is_some() {
        "Types the branch policy forbids stay listed but disabled, with the reason."
    } else {
        "No favourite: the platform's default, else the first type the branch allows."
    };
    ui.add(
        egui::Label::new(RichText::new(note).size(style::SMALL_SIZE).color(palette.text_secondary))
            .wrap(),
    );
}

/// The split button at rest, as wide as the platform's longest label like the real one.
fn button(ui: &mut Ui, kind: ProviderKind, strategy: MergeStrategy) {
    let label = merge_choice::button_label(strategy, kind);
    let font = egui::FontId::proportional(style::BODY_SIZE);
    let reserved = merging::choices(kind)
        .iter()
        .map(|choice| elide::text_width(ui, merge_choice::button_label(*choice, kind), &font))
        .fold(0.0, f32::max);
    let main_width = PADDING_X * 2.0 + icons::SIZE + ICON_GAP + reserved;
    let (rect, response) = ui.allocate_exact_size(
        vec2(main_width + CHEVRON_WIDTH, style::CONTROL_HEIGHT),
        Sense::hover(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Image, true, format!("Merge button: {label}"))
    });
    let (main_rect, chevron_rect) = rect.split_left_right_at_x(rect.left() + main_width);
    let main = ui.interact(main_rect, response.id.with("main"), Sense::hover());
    let chevron = ui.interact(chevron_rect, response.id.with("chevron"), Sense::hover());
    let look = Look {
        strategy: Some(strategy),
        label,
        remaining: None,
        is_enabled: true,
        is_busy: false,
        is_open: true,
    };
    paint::paint(ui, &main, &chevron, rect, &look);
}

/// The dropdown's types with the merge button's own rows, the preselected one selected.
fn menu(ui: &mut Ui, kind: ProviderKind, policy: &MergePolicy, preselected: MergeStrategy) {
    let palette = theme::palette(ui.ctx());
    let width = MENU_WIDTH.min(ui.available_width());
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(style::RADIUS_POPUP)
        .inner_margin(egui::Margin::same(style::SPACE_S as i8))
        .show(ui, |ui| {
            ui.set_width(width - 2.0 * style::SPACE_S);
            for (index, strategy) in policy.allowed.iter().enumerate() {
                let shown = Row {
                    strategy: *strategy,
                    kind,
                    is_selected: *strategy == preselected,
                    is_default: policy.default == Some(*strategy),
                    number: Some(index + 1),
                    explanation: merge_choice::explain(*strategy, kind, policy, TARGET),
                };
                paint::strategy_row(ui, &shown);
            }
        });
}
