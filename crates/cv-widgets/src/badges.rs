use egui::ecolor::HexColor;
use egui::{Color32, Response, RichText, Ui};

use cv_base::theme::{self, Palette, icons, style};
use review_core::{Label, PullRequestState, ReviewVote, RunStatus};

/// A rounded chip with text, used for states, labels and counts.
pub fn pill(ui: &mut Ui, text: &str, fill: Color32, text_color: Color32) -> Response {
    // Measured before allocating, so wrapping layouts move the whole chip to the next row.
    let font = egui::FontId::proportional(theme::style::SMALL_SIZE);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, text_color);
    let padding = egui::vec2(8.0, 2.0);
    let (rect, response) =
        ui.allocate_exact_size(galley.size() + padding * 2.0, egui::Sense::hover());
    ui.painter().rect_filled(rect, style::RADIUS_CHIP, fill);
    ui.painter().galley(rect.min + padding, galley, text_color);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
    response
}

pub fn state_pill(ui: &mut Ui, state: PullRequestState, is_draft: bool) -> Response {
    let palette = theme::palette(ui.ctx());
    let (text, fill) = match (state, is_draft) {
        (PullRequestState::Open, true) => ("Draft", palette.text_secondary),
        (PullRequestState::Open, false) => ("Active", palette.accent),
        (PullRequestState::Merged, _) => ("Merged", palette.success),
        (PullRequestState::Closed, _) => ("Abandoned", palette.error),
    };
    pill(ui, text, fill, palette.on_accent)
}

pub fn label_chip(ui: &mut Ui, label: &Label) -> Response {
    let palette = theme::palette(ui.ctx());
    let response = pill(ui, &label.name, label_fill(label, &palette), palette.text);
    match label.description.as_deref() {
        Some(description) if !description.is_empty() => response.on_hover_text(description),
        _ => response,
    }
}

/// The label's own color, tinted so `palette.text` stays readable on it.
pub fn label_fill(label: &Label, palette: &Palette) -> Color32 {
    let base = label
        .color
        .as_deref()
        .and_then(|hex| HexColor::from_str_without_hash(hex.trim_start_matches('#')).ok())
        .map_or(palette.text_secondary, |hex| hex.color());
    if palette.is_dark { base.gamma_multiply(0.45) } else { base.gamma_multiply(0.22) }
}

pub fn vote_icon(vote: ReviewVote, palette: &Palette) -> (&'static str, Color32) {
    match vote {
        ReviewVote::Approved | ReviewVote::ApprovedWithSuggestions => {
            (icons::CHECK_CIRCLE, palette.success)
        }
        ReviewVote::WaitingForAuthor => (icons::CLOCK, palette.warning),
        ReviewVote::ChangesRequested => (icons::X_CIRCLE, palette.error),
        ReviewVote::None => (icons::CIRCLE_DASHED, palette.text_disabled),
    }
}

pub fn vote_label(vote: ReviewVote) -> &'static str {
    match vote {
        ReviewVote::None => "No vote",
        ReviewVote::Approved => "Approved",
        ReviewVote::ApprovedWithSuggestions => "Approved with suggestions",
        ReviewVote::WaitingForAuthor => "Waiting for author",
        ReviewVote::ChangesRequested => "Changes requested",
    }
}

pub fn run_status_icon(status: RunStatus, palette: &Palette) -> (&'static str, Color32) {
    match status {
        RunStatus::Queued => (icons::CLOCK, palette.text_secondary),
        RunStatus::Running => (icons::CIRCLE_NOTCH, palette.accent),
        RunStatus::Succeeded => (icons::CHECK_CIRCLE, palette.success),
        RunStatus::Failed => (icons::X_CIRCLE, palette.error),
        RunStatus::Canceled => (icons::PROHIBIT, palette.text_secondary),
        RunStatus::Skipped => (icons::SKIP_FORWARD, palette.text_disabled),
        RunStatus::Neutral => (icons::MINUS_CIRCLE, palette.text_secondary),
    }
}

pub fn run_status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Queued => "Queued",
        RunStatus::Running => "Running",
        RunStatus::Succeeded => "Succeeded",
        RunStatus::Failed => "Failed",
        RunStatus::Canceled => "Canceled",
        RunStatus::Skipped => "Skipped",
        RunStatus::Neutral => "Neutral",
    }
}

pub fn icon_text(icon: &str, color: Color32, size: f32) -> RichText {
    RichText::new(icon).color(color).size(size)
}

#[cfg(test)]
mod tests {
    use cv_base::theme::Palette;
    use egui::Color32;
    use review_core::Label;

    use super::label_fill;

    fn fill(color: Option<&str>) -> Color32 {
        let label =
            Label { name: "bug".into(), color: color.map(str::to_owned), description: None };
        label_fill(&label, &Palette::LIGHT)
    }

    #[test]
    fn a_label_is_tinted_with_its_hex_color_with_or_without_the_hash() {
        let red = Color32::from_rgb(0xD7, 0x3A, 0x4A).gamma_multiply(0.22);
        assert_eq!(fill(Some("d73a4a")), red);
        assert_eq!(fill(Some("#d73a4a")), red);
        assert_eq!(fill(Some("#f00")), Color32::from_rgb(0xFF, 0, 0).gamma_multiply(0.22));
        let fallback = Palette::LIGHT.text_secondary.gamma_multiply(0.22);
        assert_eq!(fill(Some("not a colour")), fallback);
        assert_eq!(fill(None), fallback);
    }
}
