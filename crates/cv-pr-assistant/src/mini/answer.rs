//! The answer in the mini chat, drawn by the Markdown renderer the card and panel use, with the
//! same citation links. Once its citations are checked, the answer is laid out piece by piece
//! ([`markdown::Units`]) so the reference the user is on can wear a filled chip: the renderer's
//! word fill, in the selection tint, over that link's text.

use cv_base::theme;
use cv_markdown::{self as markdown, Marks, RenderOptions, Units};
use egui::{Color32, Ui};

use super::super::citations;
use super::Reference;

/// The pieces of the last answer laid out, kept for the text they were parsed from.
#[derive(Default)]
pub struct Pieces {
    parsed: Option<(u64, Units)>,
}

/// Draws `text` (the answer of turn `turn`); with `linked`, its citation links are in and
/// `current` gets the fill at `fill` strength (0 to 1).
pub fn show(
    ui: &mut Ui,
    pieces: &mut Pieces,
    text: &str,
    linked: bool,
    turn: usize,
    current: Option<(&Reference, f32)>,
) {
    if !linked {
        markdown::render(ui, ("assistant_mini_answer", turn), text, RenderOptions::default());
        return;
    }
    let key = egui::epaint::util::hash(text);
    if pieces.parsed.as_ref().is_none_or(|(kept, _)| *kept != key) {
        pieces.parsed = Some((key, Units::parse(text, None)));
    }
    let Some((_, units)) = &pieces.parsed else {
        return;
    };
    let palette = theme::palette(ui.ctx());
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    for (index, unit) in units.units.iter().enumerate() {
        if index > 0 {
            ui.add_space(Units::gap((units, index - 1), (units, index)));
        }
        let first = (unit.lines.start as usize).saturating_sub(1);
        let last = (unit.lines.end as usize).saturating_sub(1).min(lines.len());
        let source: String = lines.get(first..last).unwrap_or_default().concat();
        let marks = current.and_then(|(reference, fill)| {
            let ranges = ranges_of(&source, &unit.text, reference);
            let color = palette.selection.gamma_multiply(fill);
            (!ranges.is_empty() && color != Color32::TRANSPARENT).then_some(Marks { ranges, color })
        });
        let id = ui.scope_id().with(("assistant_mini_unit", turn, index));
        units.show(ui, id, index, RenderOptions::default(), marks);
    }
}

/// Where the links to `reference` read in `plain`, the text a piece shows, found from `source`,
/// the Markdown it was made from: each link's text in order, after the one before it.
fn ranges_of(source: &str, plain: &str, reference: &Reference) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut from = 0;
    for link in citations::linked(source) {
        let Some(at) = plain.get(from..).and_then(|rest| rest.find(&link.shown)) else {
            continue;
        };
        let start = from + at;
        from = start + link.shown.len();
        if reference.is(&link) {
            ranges.push(start..from);
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fill_finds_the_reference_in_the_text_the_piece_shows() {
        let source = "Here [`src/a.rs:3`](<codeview-cite:src/a.rs#3-3>) and again \
                      [src/a.rs:3](<codeview-cite:src/a.rs#3-3>), not \
                      [src/b.rs:3](<codeview-cite:src/b.rs#3-3>).";
        let plain = "Here src/a.rs:3 and again src/a.rs:3, not src/b.rs:3.";
        let reference = Reference { path: "src/a.rs".into(), start: 3, end: 3 };
        assert_eq!(ranges_of(source, plain, &reference), [5..15, 26..36]);
        let other = Reference { path: "src/b.rs".into(), start: 3, end: 3 };
        let found = ranges_of(source, plain, &other);
        assert_eq!((found.len(), found.first().cloned()), (1, Some(42..52)));
    }
}
