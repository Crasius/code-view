//! The inline HTML the parser reads as marks and links: one tag at a time, its name and `href`.

// RS-DONT-04: reads pull request comments, so indexing, slicing and narrowing are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing, clippy::cast_possible_truncation)]

use crate::rich_editor::model::Mark;

pub fn html_mark(name: &str) -> Option<Mark> {
    Some(match name {
        "ins" | "u" => Mark::Underline,
        "sub" => Mark::Subscript,
        "sup" => Mark::Superscript,
        "kbd" => Mark::Kbd,
        "b" | "strong" => Mark::Bold,
        "i" | "em" => Mark::Italic,
        "s" | "del" | "strike" => Mark::Strike,
        "code" => Mark::Code,
        _ => return None,
    })
}

pub struct HtmlTag {
    pub name: String,
    pub is_end: bool,
    pub href: Option<String>,
}

/// The name of a single tag (`<kbd>`, `</ins>`, `<a href="x">`); `None` for comments,
/// declarations and anything that is not one tag.
pub fn html_tag(html: &str) -> Option<HtmlTag> {
    let inner = html.trim().strip_prefix('<')?.strip_suffix('>')?;
    let (is_end, inner) = match inner.strip_prefix('/') {
        Some(rest) => (true, rest),
        None => (false, inner),
    };
    let inner = inner.trim_end_matches('/').trim();
    let name_end = inner.find(|ch: char| ch.is_whitespace()).unwrap_or(inner.len());
    let (name, attributes) = inner.split_at_checked(name_end)?;
    if name.is_empty() || !name.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return None;
    }
    Some(HtmlTag { name: name.to_ascii_lowercase(), is_end, href: attribute(attributes, "href") })
}

/// The value of attribute `wanted` in `attributes` (`href="x" title='y'`).
fn attribute(attributes: &str, wanted: &str) -> Option<String> {
    let mut rest = attributes.trim_start();
    while !rest.is_empty() {
        let name_end = rest.find(|ch: char| ch.is_whitespace() || ch == '=').unwrap_or(rest.len());
        let (name, after) = rest.split_at_checked(name_end.max(1))?;
        let after = after.trim_start();
        let Some(value_part) = after.strip_prefix('=') else {
            rest = after;
            continue;
        };
        let value_part = value_part.trim_start();
        let (value, remaining) = match value_part.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                let body = value_part.get(1..)?;
                body.split_once(quote).unwrap_or((body, ""))
            }
            _ => value_part.split_once(char::is_whitespace).unwrap_or((value_part, "")),
        };
        if name.eq_ignore_ascii_case(wanted) {
            return Some(value.replace("&amp;", "&").replace("&quot;", "\""));
        }
        rest = remaining.trim_start();
    }
    None
}
