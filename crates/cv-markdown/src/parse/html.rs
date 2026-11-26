//! The HTML subset inside Markdown the renderer acts on: line breaks and block tags end lines,
//! `<img>` is an image, `<video>` (or its `<source>`) a video, `<a href>` a link, `<ins>`/`<u>`,
//! `<sub>`, `<sup>` and `<kbd>` mark their text (the tags the rich editor writes); every other tag
//! is dropped and its text kept.

use std::sync::Arc;

use super::{ImageSpec, Inline, Length, Style};

/// The tags of HTML in Markdown the renderer acts on.
#[derive(Debug, PartialEq)]
pub(super) enum HtmlTag {
    Break,
    Image(ImageSpec),
    /// `<a>`, with its `href` when it has one.
    Link(Option<String>),
    LinkEnd,
    /// A tag that ends a line of text in a browser (`</p>`, `<div>`, `<hr>`).
    Block,
    /// An opening or closing tag of an HTML mark.
    Mark {
        mark: HtmlMark,
        is_end: bool,
    },
}

/// An inline HTML tag that marks its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HtmlMark {
    /// `<ins>` (GitHub, GitLab) or `<u>` (Azure DevOps).
    Underline,
    Subscript,
    Superscript,
    Kbd,
}

/// How deep a run sits inside each HTML mark, so nested tags close in order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HtmlMarks {
    depths: [u8; 4],
}

impl HtmlMarks {
    pub fn has(self, mark: HtmlMark) -> bool {
        self.depths.get(mark as usize).is_some_and(|depth| *depth > 0)
    }

    pub(super) fn apply(&mut self, mark: HtmlMark, is_end: bool) {
        if let Some(depth) = self.depths.get_mut(mark as usize) {
            *depth = if is_end { depth.saturating_sub(1) } else { depth.saturating_add(1) };
        }
    }
}

/// What one tag (`<img src="a.png" width=200>`, `</a>`) is; `None` for tags the renderer ignores.
pub(super) fn html_tag(tag: &str) -> Option<HtmlTag> {
    let inner = tag.trim().strip_prefix('<')?.strip_suffix('>')?.trim_end_matches('/');
    let (is_end, inner) = match inner.strip_prefix('/') {
        Some(rest) => (true, rest),
        None => (false, inner),
    };
    let (name, attributes) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
    let name = name.to_ascii_lowercase();
    match (name.as_str(), is_end) {
        ("br", _) => Some(HtmlTag::Break),
        ("img" | "video" | "source", false) => {
            let mut image = ImageSpec { video: name != "img", ..ImageSpec::default() };
            for (key, value) in html_attributes(attributes) {
                match key.as_str() {
                    "src" => image.src = value,
                    "alt" => image.alt = value,
                    "title" => image.title = value,
                    "width" => image.width = Length::parse(&value),
                    "height" => image.height = Length::parse(&value),
                    _ => {}
                }
            }
            (!image.src.is_empty()).then_some(HtmlTag::Image(image))
        }
        ("a", false) => {
            let href = html_attributes(attributes)
                .into_iter()
                .find(|(key, _)| key == "href")
                .map(|(_, value)| value);
            Some(HtmlTag::Link(href))
        }
        ("a", true) => Some(HtmlTag::LinkEnd),
        ("ins" | "u", _) => Some(HtmlTag::Mark { mark: HtmlMark::Underline, is_end }),
        ("sub", _) => Some(HtmlTag::Mark { mark: HtmlMark::Subscript, is_end }),
        ("sup", _) => Some(HtmlTag::Mark { mark: HtmlMark::Superscript, is_end }),
        ("kbd", _) => Some(HtmlTag::Mark { mark: HtmlMark::Kbd, is_end }),
        (
            "p" | "div" | "hr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "tr" | "table"
            | "summary",
            _,
        ) => Some(HtmlTag::Block),
        _ => None,
    }
}

/// `name=value` pairs of a tag: names lowercased, values unquoted, the common entities decoded.
fn html_attributes(text: &str) -> Vec<(String, String)> {
    let mut attributes = Vec::new();
    let mut rest = text.trim_start();
    while !rest.is_empty() {
        // At least one byte, so a stray `=` (the only match at 0 after the trim) becomes a name
        // instead of looping.
        let name_end =
            rest.find(|ch: char| ch.is_whitespace() || ch == '=').unwrap_or(rest.len()).max(1);
        let Some((name, after_name)) = rest.split_at_checked(name_end) else { break };
        let name = name.to_ascii_lowercase();
        rest = after_name.trim_start();
        let mut value = String::new();
        if let Some(after) = rest.strip_prefix('=') {
            let after = after.trim_start();
            let mut chars = after.chars();
            let (raw, remaining) = if let Some(quote @ ('"' | '\'')) = chars.next() {
                let body = chars.as_str();
                body.split_once(quote).unwrap_or((body, ""))
            } else {
                after.split_once(char::is_whitespace).unwrap_or((after, ""))
            };
            value = decode_entities(raw);
            rest = remaining.trim_start();
        }
        attributes.push((name, value));
    }
    attributes
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    text.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// An HTML block as the renderer shows it: GitHub and Azure DevOps render HTML, this renderer
/// shows its text (whitespace collapsed as in a browser, lines ended by `<br>` and block tags, e.g.
/// the `<summary>` of a `<details>` block), its images and its links.
pub(super) fn html_inline(html: &str) -> Inline {
    let mut inline = Inline::default();
    let mut style = Style::default();
    let mut links: Vec<Option<String>> = Vec::new();
    let mut text = String::new();
    let flush = |text: &mut String, inline: &mut Inline, style: &Style| {
        if !text.is_empty() {
            inline.push(text, style);
            text.clear();
        }
    };
    let mut rest = html;
    while !rest.is_empty() {
        let Some((before, from_open)) = rest.find('<').and_then(|open| rest.split_at_checked(open))
        else {
            push_collapsed(&mut text, rest);
            break;
        };
        push_collapsed(&mut text, before);
        let Some((tag, after)) =
            from_open.find('>').and_then(|close| from_open.split_at_checked(close + 1))
        else {
            push_collapsed(&mut text, from_open);
            break;
        };
        rest = after;
        match html_tag(tag) {
            Some(HtmlTag::Break | HtmlTag::Block) => {
                let kept = text.trim_end_matches(' ').len();
                text.truncate(kept);
                let after_content = !text.is_empty()
                    || inline.runs.last().is_some_and(|run| !run.text.ends_with('\n'));
                if after_content && !text.ends_with('\n') {
                    text.push('\n');
                }
            }
            Some(HtmlTag::Image(image)) => {
                flush(&mut text, &mut inline, &style);
                let alt = image.alt.clone();
                inline.push(&alt, &Style { image: Some(Arc::new(image)), ..style.clone() });
            }
            Some(HtmlTag::Link(href)) => {
                flush(&mut text, &mut inline, &style);
                links.push(std::mem::replace(&mut style.link, href));
            }
            Some(HtmlTag::LinkEnd) => {
                flush(&mut text, &mut inline, &style);
                if let Some(previous) = links.pop() {
                    style.link = previous;
                }
            }
            Some(HtmlTag::Mark { mark, is_end }) => {
                flush(&mut text, &mut inline, &style);
                style.html.apply(mark, is_end);
            }
            None => {}
        }
    }
    flush(&mut text, &mut inline, &style);
    trim_inline(&mut inline);
    inline
}

/// Appends `text` with each run of whitespace as one space, as a browser shows HTML text.
fn push_collapsed(out: &mut String, text: &str) {
    for ch in text.chars() {
        if !ch.is_whitespace() {
            out.push(ch);
        } else if !out.is_empty() && !out.ends_with([' ', '\n']) {
            out.push(' ');
        }
    }
}

/// Drops whitespace at the start and the end of an inline, and text runs left empty.
fn trim_inline(inline: &mut Inline) {
    if let Some(first) = inline.runs.first_mut().filter(|run| run.style.image.is_none()) {
        first.text = first.text.trim_start().to_owned();
    }
    if let Some(last) = inline.runs.last_mut().filter(|run| run.style.image.is_none()) {
        last.text = last.text.trim_end().to_owned();
    }
    inline.runs.retain(|run| run.style.image.is_some() || !run.text.is_empty());
}
