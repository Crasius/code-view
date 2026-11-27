//! `path:line` and `path:start-end` in an answer: found in the agent's Markdown, checked against
//! the checkout (the file is there and has the line), and turned into links that open the Files
//! tab at that line. Fenced code stays as written; inline code that is only a citation becomes a
//! link with its code look kept. How those links show and open is [`links`].
#![warn(clippy::string_slice, clippy::indexing_slicing, clippy::cast_possible_truncation)]

pub(super) mod links;

use std::ops::Range;
use std::path::Path;

/// The link target scheme the assistant's resolver knows.
const SCHEME: &str = "codeview-cite:";
/// Longer ranges are not a citation of lines but a number that happens to follow a colon.
const MAX_RANGE: u32 = 5_000;
/// Files larger than this are not read to check the line; being there is enough.
const MAX_CHECKED_BYTES: u64 = 8 * 1024 * 1024;

/// One citation in an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Citation {
    pub(crate) path: String,
    /// 1-based, inclusive.
    pub(crate) start: u32,
    pub(crate) end: u32,
    /// What the link replaces: the citation, or the whole code span around it.
    pub(crate) bytes: Range<usize>,
}

/// Every citation of `text` outside fenced code, in order.
pub(crate) fn find(text: &str) -> Vec<Citation> {
    let mut found = Vec::new();
    let mut fence: Option<&str> = None;
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_start();
        let marker = ["```", "~~~"].into_iter().find(|marker| body.starts_with(marker));
        match (fence, marker) {
            (Some(open), Some(close)) if open == close => fence = None,
            (None, Some(open)) => fence = Some(open),
            (None, None) => find_in_line(line, line_start, &mut found),
            (Some(_), _) => {}
        }
        line_start += line.len();
    }
    found
}

fn is_path_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/')
}

/// The backtick code spans of one line, as byte ranges of the line including the backticks.
fn code_spans(line: &[u8]) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut at = 0;
    while let Some(open) = line.get(at..).and_then(|rest| rest.iter().position(|b| *b == b'`')) {
        let open = at + open;
        let Some(close) =
            line.get(open + 1..).and_then(|rest| rest.iter().position(|b| *b == b'`'))
        else {
            break;
        };
        let close = open + 1 + close;
        spans.push(open..close + 1);
        at = close + 1;
    }
    spans
}

fn find_in_line(line: &str, offset: usize, found: &mut Vec<Citation>) {
    let bytes = line.as_bytes();
    let spans = code_spans(bytes);
    let mut at = 0;
    while at < bytes.len() {
        let is_start = bytes.get(at).is_some_and(|byte| is_path_byte(*byte))
            && (at == 0 || bytes.get(at - 1).is_some_and(|byte| !is_path_byte(*byte)));
        if !is_start {
            at += 1;
            continue;
        }
        let Some((citation, end)) = citation_at(line, at) else {
            at += 1;
            continue;
        };
        at = end;
        let span = spans.iter().find(|span| span.contains(&citation.bytes.start));
        let replaced = match span {
            // Inline code that is exactly the citation becomes the link; other code stays code.
            Some(span) => {
                let inner = line.get(span.start + 1..span.end - 1).unwrap_or_default();
                let cited = line.get(citation.bytes.clone()).unwrap_or_default();
                if inner.trim() != cited {
                    continue;
                }
                span.clone()
            }
            None => citation.bytes.clone(),
        };
        let is_link_target =
            replaced.start >= 2 && line.get(replaced.start - 2..replaced.start) == Some("](");
        let is_link_text = line.get(replaced.end..).is_some_and(|rest| rest.starts_with("]("));
        if is_link_target || is_link_text {
            continue;
        }
        found.push(Citation { bytes: offset + replaced.start..offset + replaced.end, ..citation });
    }
}

/// The citation starting at byte `at` of `line`, and the byte after it.
fn citation_at(line: &str, at: usize) -> Option<(Citation, usize)> {
    let bytes = line.as_bytes();
    let path_len = bytes.get(at..)?.iter().take_while(|byte| is_path_byte(**byte)).count();
    let path_end = at + path_len;
    let raw_path = line.get(at..path_end)?;
    let path = raw_path.strip_prefix("./").unwrap_or(raw_path);
    let is_path = (path.contains('/') || path.contains('.'))
        && !path.contains("//")
        && !path.starts_with('/')
        && !path.ends_with('.')
        && !path.ends_with('/')
        && path.bytes().any(|byte| byte.is_ascii_alphabetic());
    if !is_path || bytes.get(path_end) != Some(&b':') {
        return None;
    }
    let (start, mut end_at) = number_at(line, path_end + 1)?;
    let mut end = start;
    let rest = line.get(end_at..).unwrap_or_default();
    let dash = ["-", "–"].into_iter().find(|dash| rest.starts_with(dash));
    if let Some(dash) = dash
        && let Some((last, after)) = number_at(line, end_at + dash.len())
    {
        end = last;
        end_at = after;
    }
    let is_range = start >= 1 && end >= start && end - start <= MAX_RANGE;
    is_range.then(|| (Citation { path: path.to_owned(), start, end, bytes: at..end_at }, end_at))
}

/// The decimal number at byte `at`, and the byte after it.
fn number_at(line: &str, at: usize) -> Option<(u32, usize)> {
    let digits = line.as_bytes().get(at..)?.iter().take_while(|byte| byte.is_ascii_digit()).count();
    let number = line.get(at..at + digits)?.parse().ok()?;
    Some((number, at + digits))
}

/// `text` with `citations`, found in it, as Markdown links to [`target`].
pub(crate) fn link(text: &str, citations: &[Citation]) -> String {
    let mut linked = String::with_capacity(text.len() + citations.len() * 32);
    let mut at = 0;
    for citation in citations {
        let (Some(before), Some(shown)) =
            (text.get(at..citation.bytes.start), text.get(citation.bytes.clone()))
        else {
            continue;
        };
        linked.push_str(before);
        linked.push('[');
        linked.push_str(shown);
        linked.push_str("](<");
        linked.push_str(&target(&citation.path, citation.start, citation.end));
        linked.push_str(">)");
        at = citation.bytes.end;
    }
    linked.push_str(text.get(at..).unwrap_or_default());
    linked
}

/// The link target of a citation.
pub(crate) fn target(path: &str, start: u32, end: u32) -> String {
    format!("{SCHEME}{path}#{start}-{end}")
}

/// The lines a citation link names: a file of the checkout and a 1-based, inclusive range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitedLines {
    pub path: String,
    pub first: u32,
    pub last: u32,
}

/// Path and lines of a citation link's target; `None` for every other target.
pub fn parse_target(target: &str) -> Option<CitedLines> {
    let (path, lines) = target.strip_prefix(SCHEME)?.rsplit_once('#')?;
    let (first, last) = lines.split_once('-')?;
    Some(CitedLines { path: path.to_owned(), first: first.parse().ok()?, last: last.parse().ok()? })
}

/// A citation link [`link`] wrote into an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Linked {
    /// The link's text as it reads, without the backticks of a code span.
    pub(crate) shown: String,
    pub(crate) lines: CitedLines,
    /// Where the link starts in the text.
    pub(crate) at: usize,
}

/// Every citation link of an answer [`link`] wrote, in order.
pub(crate) fn linked(text: &str) -> Vec<Linked> {
    let opener = format!("](<{SCHEME}");
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = text.get(from..).and_then(|rest| rest.find(&opener)) {
        let close = from + offset;
        let target_at = close + 3;
        let Some(target_end) = text.get(target_at..).and_then(|rest| rest.find('>')) else {
            break;
        };
        from = target_at + target_end;
        let open = text.get(..close).and_then(|before| before.rfind('['));
        let target = text.get(target_at..from).and_then(parse_target);
        if let (Some(open), Some(lines)) = (open, target) {
            let shown = text.get(open + 1..close).unwrap_or_default().trim_matches('`').to_owned();
            found.push(Linked { shown, lines, at: open });
        }
    }
    found
}

/// Whether `path` is a file of `checkout` with at least `start` lines. Runs on the background
/// runtime: it reads the file. A path that leads out of the checkout is never a citation.
pub(crate) fn is_in_checkout(checkout: &Path, path: &str, start: u32) -> bool {
    if path.split('/').any(|part| part == "..") {
        return false;
    }
    let file = checkout.join(path);
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    if metadata.len() > MAX_CHECKED_BYTES {
        return true;
    }
    std::fs::read(&file).is_ok_and(|bytes| {
        let lines = bytes.split(|byte| *byte == b'\n').count();
        let ends_with_newline = bytes.last() == Some(&b'\n');
        let lines = if ends_with_newline { lines - 1 } else { lines };
        u32::try_from(lines).unwrap_or(u32::MAX) >= start
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cited(text: &str) -> Vec<(String, u32, u32, &str)> {
        find(text)
            .into_iter()
            .map(|citation| {
                let shown = text.get(citation.bytes.clone()).unwrap_or_default();
                (citation.path, citation.start, citation.end, shown)
            })
            .collect()
    }

    #[test]
    fn finds_lines_and_ranges_in_prose_and_inline_code() {
        let text = "The decision sits in `Billing/TaxTreatment.cs:48`, and src/a.rs:3-7 or \
                    ./lib/x.ts:12–14 saves it (see README.md:2:5).";
        assert_eq!(
            cited(text),
            [
                ("Billing/TaxTreatment.cs".into(), 48, 48, "`Billing/TaxTreatment.cs:48`"),
                ("src/a.rs".into(), 3, 7, "src/a.rs:3-7"),
                ("lib/x.ts".into(), 12, 14, "./lib/x.ts:12–14"),
                ("README.md".into(), 2, 2, "README.md:2"),
            ]
        );
    }

    #[test]
    fn leaves_urls_times_code_and_links_alone() {
        let text = "Open https://example.com:8080/a or localhost:3000 at 10:30. \
                    `let x = map.get(a.b:3)` and [a.rs:4](https://x.test/a.rs).\n\
                    ```\nsrc/in_fence.rs:9\n```\nafter.rs:1";
        assert_eq!(cited(text), [("after.rs".into(), 1, 1, "after.rs:1")]);
        assert!(find("src/a.rs:0 and src/b.rs:9-3").is_empty(), "no line 0, no backward range");
    }

    #[test]
    fn links_only_what_was_accepted_and_keeps_the_rest() {
        let text = "See `src/a.rs:3` and src/gone.rs:9 or src/b.rs:1-2.";
        let mut citations = find(text);
        citations.retain(|citation| citation.path != "src/gone.rs");
        let linked = link(text, &citations);
        assert_eq!(
            linked,
            "See [`src/a.rs:3`](<codeview-cite:src/a.rs#3-3>) and src/gone.rs:9 or \
             [src/b.rs:1-2](<codeview-cite:src/b.rs#1-2>)."
        );
        assert_eq!(
            parse_target("codeview-cite:src/b.rs#1-2"),
            Some(CitedLines { path: "src/b.rs".into(), first: 1, last: 2 })
        );
        assert_eq!(parse_target("https://x.test"), None);
    }

    #[test]
    fn reads_the_links_it_wrote_back_in_order() {
        let text = "See `src/a.rs:3` and src/b.rs:1-2, [a page](https://x.test).";
        let linked = linked(&link(text, &find(text)));
        let read: Vec<(&str, &str, u32, u32)> = linked
            .iter()
            .map(|link| {
                (link.shown.as_str(), link.lines.path.as_str(), link.lines.first, link.lines.last)
            })
            .collect();
        assert_eq!(read, [("src/a.rs:3", "src/a.rs", 3, 3), ("src/b.rs:1-2", "src/b.rs", 1, 2)]);
        assert_eq!(linked[0].at, 4, "where the link starts");
    }

    #[test]
    fn checks_the_file_and_its_length_inside_the_checkout() {
        let scratch = cv_testkit::ScratchDir::new("assistant-citations");
        std::fs::create_dir_all(scratch.path().join("src")).unwrap();
        std::fs::write(scratch.path().join("src/a.rs"), "one\ntwo\nthree\n").unwrap();
        assert!(is_in_checkout(scratch.path(), "src/a.rs", 3));
        assert!(!is_in_checkout(scratch.path(), "src/a.rs", 4), "past the last line");
        assert!(!is_in_checkout(scratch.path(), "src/missing.rs", 1));
        assert!(!is_in_checkout(scratch.path(), "src", 1), "a folder is no file");
        assert!(!is_in_checkout(scratch.path(), "../outside.rs", 1));
    }
}
