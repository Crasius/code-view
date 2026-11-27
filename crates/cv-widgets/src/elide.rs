//! Text cut to fit a width with an ellipsis, and the width a text takes. Plain lines are cut at
//! the end ([`line`], [`job`]). References (`owner/repo#59`, `group/project!12`)
//! are cut in the middle so the number stays readable: `owner…/repo#59`, then `re…#59`.
//!
//! Rows lay out their trailing parts (pills, durations, buttons) first and hand the remaining
//! width to these helpers; the full text belongs in the row's tooltip.

use std::sync::Arc;

use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, FontId, Galley, Painter, Ui};

const ELLIPSIS: &str = "…";

/// One line of `text`, cut with … at `max_width`. `galley.elided` says whether it was cut.
pub fn line(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    job(painter, LayoutJob::simple_singleline(text.to_owned(), font, color), max_width)
}

/// The same for a prepared job.
pub fn job(painter: &Painter, mut job: LayoutJob, max_width: f32) -> Arc<Galley> {
    job.wrap = TextWrapping::truncate_at_width(max_width.max(1.0));
    painter.layout_job(job)
}

/// The width of `text` laid out on one unwrapped line in `font`.
pub fn text_width(ui: &Ui, text: &str, font: &FontId) -> f32 {
    ui.fonts_mut(|fonts| {
        fonts.layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER).size().x
    })
}

/// `reference` cut to fit `width` when painted in `font`.
pub fn reference(ui: &Ui, reference: &str, font: &FontId, width: f32) -> String {
    fit_reference(reference, width, |text| text_width(ui, text, font))
}

/// The longest form of `text` that `measure` says fits `width`, in this order of preference:
/// the whole text; the owner cut (`contoso-pl…/data-loader#59`); the name cut (`data…#59`);
/// the number alone. Text without a trailing `#123` or `!123` is cut at the end.
fn fit_reference(text: &str, width: f32, measure: impl Fn(&str) -> f32) -> String {
    if measure(text) <= width {
        return text.to_owned();
    }
    let fits = |candidate: &str| measure(candidate) <= width;
    let Some(number_start) = number_start(text) else {
        return longest(text, |head| format!("{head}{ELLIPSIS}"), &fits)
            .unwrap_or_else(|| ELLIPSIS.to_owned());
    };
    let (path, number) = text.split_at(number_start);
    if let Some((owner, name)) = path.rsplit_once('/')
        && let Some(fitted) =
            longest(owner, |head| format!("{head}{ELLIPSIS}/{name}{number}"), &fits)
    {
        return fitted;
    }
    let name = path.rsplit_once('/').map_or(path, |(_, name)| name);
    longest(name, |head| format!("{head}{ELLIPSIS}{number}"), &fits)
        .unwrap_or_else(|| number.to_owned())
}

/// Byte index of a trailing `#123` / `!123`.
fn number_start(text: &str) -> Option<usize> {
    let start = text.rfind(['#', '!'])?;
    let digits = &text[start + 1..];
    (!digits.is_empty() && digits.chars().all(|char| char.is_ascii_digit())).then_some(start)
}

/// `shape(prefix)` for the longest prefix of `source` (by characters, empty included) that fits.
fn longest(
    source: &str,
    shape: impl Fn(&str) -> String,
    fits: &dyn Fn(&str) -> bool,
) -> Option<String> {
    let ends: Vec<usize> =
        source.char_indices().map(|(index, _)| index).chain([source.len()]).collect();
    // Binary search over prefix lengths: shorter prefixes never get wider.
    let (mut low, mut high) = (0usize, ends.len() - 1);
    if !fits(&shape(&source[..ends[low]])) {
        return None;
    }
    while low < high {
        let middle = (low + high).div_ceil(2);
        if fits(&shape(&source[..ends[middle]])) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    Some(shape(&source[..ends[low]]))
}

#[cfg(test)]
mod tests {
    use egui::{Color32, FontId};

    use super::{fit_reference, line, text_width};

    /// Runs `check` in one frame of a fresh context.
    fn in_a_frame(check: impl FnOnce(&egui::Ui)) {
        let ctx = egui::Context::default();
        let mut check = Some(check);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(check) = check.take() {
                check(ui);
            }
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn a_long_line_is_cut_to_fit_and_a_short_one_stays_whole() {
        in_a_frame(|ui| {
            let font = FontId::proportional(14.0);
            let text = "a branch name far too long for the room it is given";
            let long = line(ui.painter(), text, font.clone(), Color32::WHITE, 80.0);
            assert!(long.elided);
            assert!(long.size().x <= 80.0, "{}", long.size().x);
            assert_eq!(long.rows.len(), 1);
            let short = line(ui.painter(), "main", font.clone(), Color32::WHITE, 80.0);
            assert!(!short.elided);
            assert_eq!(short.size().x, text_width(ui, "main", &font));
        });
    }

    #[test]
    fn a_text_is_as_wide_as_its_length_and_the_empty_text_has_none() {
        in_a_frame(|ui| {
            let font = FontId::proportional(14.0);
            let short = text_width(ui, "main", &font);
            assert!(short > 0.0);
            assert!(text_width(ui, "main and more", &font) > short);
            assert_eq!(text_width(ui, "", &font), 0.0);
        });
    }

    /// One unit per character.
    fn fit(text: &str, width: usize) -> String {
        fit_reference(text, width as f32, |text| text.chars().count() as f32)
    }

    #[test]
    fn a_reference_that_fits_stays_whole() {
        assert_eq!(fit("team/app#59", 11), "team/app#59");
    }

    #[test]
    fn the_owner_goes_first_and_the_number_last() {
        let reference = "contoso-platform-tools/data-loader#59";
        assert_eq!(fit(reference, 26), "contoso-pl…/data-loader#59");
        assert_eq!(fit(reference, 16), "…/data-loader#59");
        assert_eq!(fit(reference, 10), "data-l…#59");
        assert_eq!(fit(reference, 4), "…#59");
        assert_eq!(fit(reference, 2), "#59");
    }

    #[test]
    fn gitlab_and_plain_texts() {
        assert_eq!(fit("group/sub/project!1234", 19), "group…/project!1234");
        assert_eq!(fit("group/sub/project!1234", 12), "projec…!1234");
        assert_eq!(fit("PROJ-1234 ticket", 6), "PROJ-…");
        assert_eq!(fit("issue #12 of many", 8), "issue #…");
    }
}
