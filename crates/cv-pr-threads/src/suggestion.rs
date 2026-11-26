//! Splits a comment into Markdown and suggestion blocks (fenced code tagged `suggestion`), which
//! the card shows as a "Suggested change" panel. None of the three platforms documents an API to
//! apply a suggestion as a commit (GitLab's apply endpoint takes suggestion ids its REST notes do
//! not carry), so the panel is read-only and offers to copy the code.

use cv_editor::markdown_editor::FileContext;
use review_core::{DiffSide, ThreadAnchor};

/// The commented head-side lines for a new suggestion block. All three platforms render
/// suggestions on lines of the new file (GitHub "Suggested changes", GitLab "Suggest changes",
/// Azure DevOps "Suggest change"); a comment on a deleted line has nothing to replace, and a range
/// replaces all its lines, so it has to start and end in the new file.
pub fn suggestion_lines(ctx: &egui::Context, anchor: &ThreadAnchor) -> Option<String> {
    let end = anchor.new_line?;
    let start = match &anchor.range {
        Some(range) if range.side == DiffSide::New => range.start_new_line?,
        Some(_) => return None,
        None => end,
    };
    let file = FileContext::current(ctx)?;
    if file.path != anchor.path {
        return None;
    }
    file.new_lines(start..=end)
}

/// "line 12" or "lines 12–18", numbered in the file the lines were picked in.
pub fn lines_label(anchor: &ThreadAnchor) -> String {
    let Some(range) = &anchor.range else {
        let line = anchor.new_line.or(anchor.old_line).unwrap_or(0);
        return format!("line {line}");
    };
    let in_side = |line: Option<u32>, other: Option<u32>| line.or(other).unwrap_or(0);
    let (start, end) = match range.side {
        DiffSide::Old => (
            in_side(range.start_old_line, range.start_new_line),
            in_side(anchor.old_line, anchor.new_line),
        ),
        DiffSide::New => (
            in_side(range.start_new_line, range.start_old_line),
            in_side(anchor.new_line, anchor.old_line),
        ),
    };
    format!("lines {start}–{end}")
}

/// [`lines_label`] starting with a capital, for headings and pills.
pub fn lines_title(anchor: &ThreadAnchor) -> String {
    let label = lines_label(anchor);
    let mut characters = label.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().chain(characters).collect())
        .unwrap_or_default()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Part<'a> {
    Markdown(&'a str),
    Suggestion(String),
}

/// The parts of `body` in order. A fence opens with ```` ```suggestion ```` or GitLab's
/// ```` ```suggestion:-0+0 ```` (any number of backticks, at least three) and closes with a fence
/// at least as long.
pub fn split(body: &str) -> Vec<Part<'_>> {
    let mut parts = Vec::new();
    let mut markdown_start = 0;
    let mut offset = 0;
    let mut open: Option<(usize, usize, Vec<&str>)> = None;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']).trim_start();
        match &mut open {
            None => {
                let ticks = trimmed.chars().take_while(|character| *character == '`').count();
                if ticks >= 3 && is_suggestion_tag(trimmed[ticks..].trim()) {
                    if offset > markdown_start {
                        push_markdown(&mut parts, &body[markdown_start..offset]);
                    }
                    open = Some((ticks, offset, Vec::new()));
                }
            }
            Some((ticks, _, lines)) => {
                let closing = trimmed.chars().take_while(|character| *character == '`').count();
                if closing >= *ticks && trimmed[closing..].trim().is_empty() {
                    parts.push(Part::Suggestion(lines.join("\n")));
                    open = None;
                    markdown_start = offset + line.len();
                } else {
                    lines.push(line.trim_end_matches(['\r', '\n']));
                }
            }
        }
        offset += line.len();
    }
    // An unclosed fence stays Markdown, as the platforms render it.
    let rest_start = open.map_or(markdown_start, |(_, start, _)| start);
    push_markdown(&mut parts, &body[rest_start..]);
    parts
}

/// `suggestion`, or GitLab's `suggestion:-2+1` with the lines above and below the commented one it
/// replaces (its "Insert suggestion" always writes them, as `suggestion:-0+0`).
fn is_suggestion_tag(info: &str) -> bool {
    let Some(rest) = info.strip_prefix("suggestion") else {
        return false;
    };
    let Some(offsets) = rest.strip_prefix(":-") else {
        return rest.is_empty();
    };
    let is_number = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    offsets.split_once('+').is_some_and(|(above, below)| is_number(above) && is_number(below))
}

fn push_markdown<'a>(parts: &mut Vec<Part<'a>>, text: &'a str) {
    if !text.trim().is_empty() {
        parts.push(Part::Markdown(text));
    }
}

/// The comment contains a suggestion block (for the panel's "suggested change" note).
pub fn has_suggestion(body: &str) -> bool {
    split(body).iter().any(|part| matches!(part, Part::Suggestion(_)))
}

#[cfg(test)]
mod tests {
    use review_core::LineRange;

    use super::*;

    #[test]
    fn names_the_line_or_the_lines() {
        let mut anchor = ThreadAnchor {
            path: "a.rs".into(),
            old_path: None,
            old_line: Some(7),
            new_line: Some(9),
            range: None,
        };
        assert_eq!(lines_label(&anchor), "line 9");
        assert_eq!(lines_title(&anchor), "Line 9");
        anchor.range = Some(LineRange {
            start_old_line: Some(3),
            start_new_line: Some(5),
            side: DiffSide::Old,
            detail: None,
        });
        assert_eq!(lines_label(&anchor), "lines 3–7");
    }

    #[test]
    fn splits_suggestions_from_the_text_around_them() {
        let body = "Right. Something like this?\n```suggestion\nif (charge.Amount <= 0) return;\nvar number = 1;\n```\nThoughts?";
        assert_eq!(
            split(body),
            [
                Part::Markdown("Right. Something like this?\n"),
                Part::Suggestion("if (charge.Amount <= 0) return;\nvar number = 1;".into()),
                Part::Markdown("Thoughts?"),
            ]
        );
        assert!(has_suggestion(body));
    }

    #[test]
    fn gitlab_suggestions_with_line_offsets_are_suggestions() {
        assert_eq!(
            split("```suggestion:-0+0\nlet a = 2;\n```"),
            [Part::Suggestion("let a = 2;".into())],
            "GitLab's Insert suggestion writes the offsets"
        );
        assert_eq!(
            split("```suggestion:-2+1\none\ntwo\n```"),
            [Part::Suggestion("one\ntwo".into())]
        );
        assert_eq!(
            split("```suggestions\nx\n```"),
            [Part::Markdown("```suggestions\nx\n```")],
            "only the suggestion tag itself"
        );
    }

    #[test]
    fn plain_code_and_unclosed_fences_stay_markdown() {
        assert_eq!(split("```rust\nlet a = 1;\n```"), [Part::Markdown("```rust\nlet a = 1;\n```")]);
        assert_eq!(
            split("text\n```suggestion\nnever closed"),
            [Part::Markdown("text\n"), Part::Markdown("```suggestion\nnever closed")]
        );
        assert_eq!(
            split("````suggestion\nremove this\n````\n"),
            [Part::Suggestion("remove this".into())]
        );
        assert_eq!(
            split("```suggestion\n```"),
            [Part::Suggestion(String::new())],
            "an empty suggestion deletes the lines"
        );
    }
}
