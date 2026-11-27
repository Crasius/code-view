//! RS-READ-04 file length, counted in shipped lines (a file's lines minus its `#[cfg(test)]`
//! items; test-only files count zero). No file is over 700 lines unless it is in the shrink-only
//! allowlist, whatever its length: an entry caps its file, may only go down, and goes away once
//! the file is back within 700 lines. Above 400 lines a split by topic is review; `--lengths`
//! lists those files.

use std::collections::BTreeMap;

use crate::finding::{Finding, Rule};

/// The allowlist, relative to the repository root.
pub const ALLOWLIST: &str = "tools/repo-lint/file-lengths.allow";
/// Above this, `--lengths` lists a file for a split by topic (review, no finding).
pub const SPLIT_BY_TOPIC: usize = 400;
/// Above this, a file needs an allowlist entry and may not grow past it.
pub const NO_GROWTH: usize = 700;

/// Allowlist entries: path to its line cap.
pub type Allowlist = BTreeMap<String, usize>;

/// Parses `<lines> <path>` lines; a line that starts with `#` is a comment, and an entry carries
/// none, so `shrink` can rewrite entries without losing text. Returns the bad lines as errors.
pub fn parse_allowlist(text: &str) -> Result<Allowlist, String> {
    let mut entries = Allowlist::new();
    for (index, raw) in text.lines().enumerate() {
        let line = without_comment(raw);
        if line.is_empty() {
            continue;
        }
        let Some((count, path)) = parse_entry(line) else {
            return Err(format!(
                "{ALLOWLIST}:{}: expected `<lines> <path>`, got `{line}`",
                index + 1
            ));
        };
        if entries.insert(path.to_owned(), count).is_some() {
            return Err(format!("{ALLOWLIST}:{}: {path} is listed twice", index + 1));
        }
    }
    Ok(entries)
}

fn without_comment(raw: &str) -> &str {
    let line = raw.trim();
    if line.starts_with('#') { "" } else { line }
}

fn parse_entry(line: &str) -> Option<(usize, &str)> {
    let (count, path) = line.split_once(' ')?;
    let path = path.trim();
    let plain = !path.is_empty() && !path.contains(|c: char| c == '#' || c.is_whitespace());
    plain.then_some((count.parse().ok()?, path))
}

/// The finding for one file of `lines` shipped lines with its allowlist entry, if any.
pub fn check(path: &str, lines: usize, entry: Option<usize>) -> Option<Finding> {
    let split = "split it by topic into sibling modules";
    match entry {
        None if lines > NO_GROWTH => Some(Finding::file(
            Rule::FileLength,
            path,
            format!(
                "{lines} shipped lines, over {NO_GROWTH} without an allowlist entry: {split} (RS-READ-04)"
            ),
        )),
        Some(_) if lines <= NO_GROWTH => Some(Finding::file(
            Rule::LengthAllowlist,
            path,
            format!(
                "now {lines} shipped lines, within {NO_GROWTH}: remove its entry from {ALLOWLIST}"
            ),
        )),
        Some(cap) if lines > cap => Some(Finding::file(
            Rule::FileLength,
            path,
            format!(
                "grew to {lines} shipped lines, the allowlist caps it at {cap}: {split} (RS-READ-04)"
            ),
        )),
        Some(cap) if lines < cap => Some(Finding::file(
            Rule::LengthAllowlist,
            path,
            format!(
                "shrank to {lines} lines: lower its entry from {cap} (`repo-lint --shrink-allowlist`)"
            ),
        )),
        _ => None,
    }
}

/// Entries that are new or higher than at the base revision: the allowlist only shrinks. A file
/// that git reports as renamed since the base (`renamed_from`: new path to old path) keeps the
/// entry of its old path, so a move is no new entry, while a raise on the way still is.
pub fn check_growth(
    current: &Allowlist,
    base: &Allowlist,
    renamed_from: &BTreeMap<String, String>,
) -> Vec<Finding> {
    current
        .iter()
        .filter_map(|(path, &cap)| {
            let moved =
                renamed_from.get(path).and_then(|old_path| Some((old_path, *base.get(old_path)?)));
            let change = match (base.get(path), moved) {
                (Some(&old), _) if cap > old => format!("entry raised from {old} to {cap}"),
                (None, Some((old_path, old))) if cap > old => {
                    format!("entry raised from {old} to {cap} (moved from {old_path})")
                }
                (Some(_), _) | (None, Some(_)) => return None,
                (None, None) => format!("new entry ({cap})"),
            };
            let message =
                format!("{path}: {change}; the allowlist only shrinks: split the file instead");
            Some(Finding::file(Rule::LengthAllowlist, ALLOWLIST, message))
        })
        .collect()
}

/// Rewrites the allowlist with every entry lowered to its file's current length and entries of
/// files within 700 lines (or gone) dropped. It never adds or raises an entry.
pub fn shrink(text: &str, lengths: &BTreeMap<String, usize>) -> String {
    let mut out: Vec<String> = Vec::new();
    for raw in text.lines() {
        let Some((cap, path)) = parse_entry(without_comment(raw)) else {
            out.push(raw.to_owned());
            continue;
        };
        let Some(&now) = lengths.get(path) else {
            continue;
        };
        if now > NO_GROWTH {
            out.push(format!("{} {path}", cap.min(now)));
        }
    }
    out.push(String::new());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule_of(path: &str, lines: usize, entry: Option<usize>) -> Option<Rule> {
        check(path, lines, entry).map(|finding| finding.rule)
    }

    #[test]
    fn unlisted_files_stay_within_700_lines() {
        assert_eq!(rule_of("a.rs", 700, None), None);
        assert_eq!(rule_of("a.rs", 701, None), Some(Rule::FileLength));
        assert_eq!(rule_of("a.rs", 1001, None), Some(Rule::FileLength));
    }

    #[test]
    fn listed_files_may_not_grow_and_must_ratchet_down() {
        assert_eq!(rule_of("a.rs", 900, Some(900)), None);
        assert_eq!(rule_of("a.rs", 901, Some(900)), Some(Rule::FileLength));
        assert_eq!(rule_of("a.rs", 899, Some(900)), Some(Rule::LengthAllowlist));
        assert_eq!(rule_of("a.rs", 650, Some(900)), Some(Rule::LengthAllowlist));
    }

    #[test]
    fn parses_entries_and_rejects_bad_lines() {
        let entries = parse_allowlist(
            "# header\n1200 crates/a/src/big.rs\n\n  # indented\n800 crates/b/src/x.rs\n",
        );
        assert_eq!(
            entries.expect("valid").into_iter().collect::<Vec<_>>(),
            [("crates/a/src/big.rs".to_owned(), 1200), ("crates/b/src/x.rs".to_owned(), 800)]
        );
        assert!(parse_allowlist("800 a.rs # note\n").is_err());
        assert!(parse_allowlist("big.rs 1200\n").is_err());
        assert!(parse_allowlist("800 a.rs\n900 a.rs\n").is_err());
    }

    #[test]
    fn growth_against_the_base_is_refused() {
        let base: Allowlist =
            [("a.rs".to_owned(), 900), ("b.rs".to_owned(), 800)].into_iter().collect();
        let current: Allowlist =
            [("a.rs".to_owned(), 950), ("b.rs".to_owned(), 790), ("c.rs".to_owned(), 720)]
                .into_iter()
                .collect();
        let messages: Vec<String> = check_growth(&current, &base, &BTreeMap::new())
            .into_iter()
            .map(|f| f.message)
            .collect();
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages[0].starts_with("a.rs: entry raised from 900 to 950"), "{messages:?}");
        assert!(messages[1].starts_with("c.rs: new entry (720)"), "{messages:?}");
    }

    /// A move keeps its cap; raising it on the way, or a new path with no listed origin, is still
    /// growth.
    #[test]
    fn a_renamed_file_keeps_the_entry_of_its_old_path() {
        let base: Allowlist =
            [("old/a.rs".to_owned(), 900), ("old/b.rs".to_owned(), 800)].into_iter().collect();
        let current: Allowlist = [
            ("new/a.rs".to_owned(), 900),
            ("new/b.rs".to_owned(), 810),
            ("new/c.rs".to_owned(), 720),
        ]
        .into_iter()
        .collect();
        let renamed_from: BTreeMap<String, String> = [
            ("new/a.rs".to_owned(), "old/a.rs".to_owned()),
            ("new/b.rs".to_owned(), "old/b.rs".to_owned()),
            ("new/c.rs".to_owned(), "old/unlisted.rs".to_owned()),
        ]
        .into_iter()
        .collect();
        let messages: Vec<String> =
            check_growth(&current, &base, &renamed_from).into_iter().map(|f| f.message).collect();
        assert_eq!(
            messages,
            [
                "new/b.rs: entry raised from 800 to 810 (moved from old/b.rs); the allowlist only \
                 shrinks: split the file instead",
                "new/c.rs: new entry (720); the allowlist only shrinks: split the file instead",
            ]
        );
    }

    #[test]
    fn shrink_lowers_and_drops_but_never_raises() {
        let text = "# keep me\n900 a.rs\n800 b.rs\n800 gone.rs\n750 c.rs\n";
        let lengths: BTreeMap<String, usize> =
            [("a.rs".to_owned(), 850), ("b.rs".to_owned(), 640), ("c.rs".to_owned(), 780)]
                .into_iter()
                .collect();
        assert_eq!(shrink(text, &lengths), "# keep me\n850 a.rs\n750 c.rs\n");
    }
}
