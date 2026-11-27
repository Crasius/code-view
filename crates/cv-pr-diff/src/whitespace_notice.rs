//! Whitespace-only changes in words: the banner above a diff whose every change is whitespace,
//! line endings, the final newline or the byte order mark, and the notice for a file that
//! "Ignore whitespace" leaves without changes. Both carry the switch that shows the other view.

use egui::{Label, RichText, Sense, Ui, vec2};
use review_git::{EdgeChange, WhitespaceSummary};

use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, style};
use cv_pr_session::web_link::WebLink;
use cv_widgets::numbers::group_digits;
use cv_widgets::states::{self, Action};
use cv_widgets::{buttons, glyph};

/// One line of text and the action, whatever the summary says: the diff below never moves when
/// the text changes.
const BANNER_HEIGHT: f32 = 32.0;

pub const SHOW_LABEL: &str = "Show whitespace changes";
pub const HIDE_LABEL: &str = "Hide whitespace changes";

/// What changed, in one sentence: "Line endings changed from LF to CRLF (all 412 lines).",
/// "Only whitespace changed: indentation tabs → spaces in 37 lines.", or a short combined list.
pub fn describe(summary: &WhitespaceSummary) -> String {
    let mut parts = Vec::new();
    if let Some(endings) = line_endings(summary) {
        parts.push(endings);
    }
    let counted = [
        (summary.tabs_to_spaces, "indentation tabs → spaces in"),
        (summary.spaces_to_tabs, "indentation spaces → tabs in"),
        (summary.reindented, "indentation in"),
        (summary.trailing, "trailing whitespace in"),
        (summary.inner, "spacing within"),
    ];
    for (count, what) in counted {
        if count > 0 {
            parts.push(Part::Lines(format!("{what} {}", lines(count))));
        }
    }
    if let Some(change) = summary.final_newline {
        parts.push(Part::Edge("newline at end of file", change));
    }
    if let Some(change) = summary.bom {
        parts.push(Part::Edge("byte order mark (BOM)", change));
    }
    match parts.as_slice() {
        [Part::Endings { sentence, .. }] => format!("{sentence}."),
        [Part::Edge(what, change)] => {
            format!("{}.", capitalized(&format!("{what} {}", edge(*change))))
        }
        [Part::Lines(fragment)] => format!("Only whitespace changed: {fragment}."),
        _ => {
            let fragments: Vec<String> = parts.iter().map(Part::fragment).collect();
            format!("Only whitespace changed: {}.", join_list(&fragments))
        }
    }
}

enum Part {
    /// Line endings: the sentence when they are all that changed, the fragment in a list.
    Endings {
        sentence: String,
        fragment: String,
    },
    Lines(String),
    Edge(&'static str, EdgeChange),
}

impl Part {
    fn fragment(&self) -> String {
        match self {
            Part::Endings { fragment, .. } | Part::Lines(fragment) => fragment.clone(),
            Part::Edge(what, change) => format!("{what} {}", edge(*change)),
        }
    }
}

fn line_endings(summary: &WhitespaceSummary) -> Option<Part> {
    let changed = summary.to_crlf + summary.to_lf;
    let all = summary.whole_file && changed == summary.lines;
    let part = match (summary.to_crlf, summary.to_lf) {
        (0, 0) => return None,
        (count, 0) | (0, count) => {
            let (from, to) = if summary.to_crlf > 0 { ("LF", "CRLF") } else { ("CRLF", "LF") };
            let counted = lines(count);
            let (scope, fragment_scope) = if all {
                (format!("(all {counted})"), format!("in all {counted}"))
            } else {
                (format!("in {counted}"), format!("in {counted}"))
            };
            Part::Endings {
                sentence: format!("Line endings changed from {from} to {to} {scope}"),
                fragment: format!("line endings {from} → {to} {fragment_scope}"),
            }
        }
        (to_crlf, to_lf) => Part::Endings {
            sentence: format!(
                "Line endings changed in {}: {} to CRLF, {} to LF",
                lines(changed),
                group_digits(to_crlf as usize),
                group_digits(to_lf as usize)
            ),
            fragment: format!("line endings in {}", lines(changed)),
        },
    };
    Some(part)
}

fn lines(count: u32) -> String {
    match count {
        1 => "1 line".to_owned(),
        count => format!("{} lines", group_digits(count as usize)),
    }
}

fn edge(change: EdgeChange) -> &'static str {
    match change {
        EdgeChange::Added => "added",
        EdgeChange::Removed => "removed",
    }
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters
        .next()
        .map_or_else(String::new, |first| first.to_uppercase().chain(characters).collect())
}

/// "a", "a and b", "a, b and c".
fn join_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The strip above a whitespace-only diff: what changed, and the switch that hides (or, while
/// "Ignore whitespace" is on, shows) such changes. Returns `true` when the switch was clicked.
pub fn banner(ui: &mut Ui, summary: &WhitespaceSummary, ignores_whitespace: bool) -> bool {
    let palette = theme::palette(ui.ctx());
    let text = describe(summary);
    let mut clicked = false;
    egui::Frame::new()
        .fill(palette.surface_subtle)
        .stroke(egui::Stroke::NONE)
        .inner_margin(egui::Margin::symmetric(12, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), BANNER_HEIGHT),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    let (icon, label) = if ignores_whitespace {
                        (icons::EYE, SHOW_LABEL)
                    } else {
                        (icons::EYE_SLASH, HIDE_LABEL)
                    };
                    let hint = if ignores_whitespace {
                        "Turn off Ignore whitespace"
                    } else {
                        concat!("Turn on Ignore whitespace (", shortcut!(alt "W"), ")")
                    };
                    clicked = buttons::subtle(ui, icon, label).on_hover_text(hint).clicked();
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        glyph::decorative(
                            ui,
                            RichText::new(icons::INFO)
                                .font(icons::state_font(&palette, icons::SIZE_SMALL, false))
                                .color(palette.text_secondary),
                        );
                        let response = ui.add(
                            Label::new(
                                RichText::new(&text).color(palette.text).size(style::BODY_SIZE),
                            )
                            .truncate()
                            .sense(Sense::hover()),
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Label, true, &text)
                        });
                    });
                },
            );
        });
    clicked
}

/// "Ignore whitespace" left this file without changes: say so and offer to show them. Returns
/// `Some(true)` for "Show whitespace changes", `Some(false)` for the platform link.
pub fn hidden_notice(ui: &mut Ui, web_link: Option<&WebLink>) -> Option<bool> {
    let mut actions = vec![Action::primary(icons::EYE, SHOW_LABEL)];
    if let Some(link) = web_link {
        actions.push(Action::secondary(link.icon, &link.label));
    }
    let detail =
        "Every change in this file is whitespace or a line ending, which Ignore whitespace hides.";
    states::message_with_actions(ui, icons::EYE_SLASH, "Only whitespace changed", detail, &actions)
        .map(|index| index == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(change: impl FnOnce(&mut WhitespaceSummary)) -> WhitespaceSummary {
        let mut summary = WhitespaceSummary::default();
        change(&mut summary);
        summary
    }

    #[test]
    fn names_a_single_kind_in_plain_words() {
        let all = summary(|s| (s.lines, s.to_crlf, s.whole_file) = (412, 412, true));
        assert_eq!(describe(&all), "Line endings changed from LF to CRLF (all 412 lines).");
        let some = summary(|s| (s.lines, s.to_lf) = (3, 3));
        assert_eq!(describe(&some), "Line endings changed from CRLF to LF in 3 lines.");
        let mixed = summary(|s| (s.lines, s.to_crlf, s.to_lf) = (5, 2, 3));
        assert_eq!(describe(&mixed), "Line endings changed in 5 lines: 2 to CRLF, 3 to LF.");
        let tabs = summary(|s| (s.lines, s.tabs_to_spaces) = (37, 37));
        assert_eq!(
            describe(&tabs),
            "Only whitespace changed: indentation tabs → spaces in 37 lines."
        );
        let trailing = summary(|s| (s.lines, s.trailing) = (1, 1));
        assert_eq!(describe(&trailing), "Only whitespace changed: trailing whitespace in 1 line.");
        let newline = summary(|s| (s.lines, s.final_newline) = (1, Some(EdgeChange::Removed)));
        assert_eq!(describe(&newline), "Newline at end of file removed.");
        let bom = summary(|s| (s.lines, s.bom) = (1, Some(EdgeChange::Removed)));
        assert_eq!(describe(&bom), "Byte order mark (BOM) removed.");
    }

    #[test]
    fn combines_several_kinds_into_one_short_list() {
        let combined = summary(|s| {
            (s.lines, s.to_crlf, s.trailing, s.final_newline) = (4, 4, 2, Some(EdgeChange::Added));
        });
        assert_eq!(
            describe(&combined),
            "Only whitespace changed: line endings LF → CRLF in 4 lines, trailing whitespace in 2 lines and newline at end of file added."
        );
        let grouped = summary(|s| {
            (s.lines, s.reindented, s.inner, s.to_crlf, s.whole_file) =
                (1_200, 1_200, 3, 1_200, true);
        });
        assert_eq!(
            describe(&grouped),
            "Only whitespace changed: line endings LF → CRLF in all 1,200 lines, indentation in 1,200 lines and spacing within 3 lines."
        );
    }
}
