//! Item-level facts from lexed code: tokens with their lines, attributes, out-of-line module
//! declarations (`mod name;`) and the line ranges of `#[cfg(test)]` items. A brace-matching
//! walk, not a parser: enough to tell test code from shipped code and to resolve module files.

use crate::lexer::{Scanned, is_ident_char};

/// One token: an identifier or keyword, or a single punctuation char, with its 1-based line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    pub line: usize,
}

/// A `mod name;` declaration with its `#[path = "..."]` value, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModDecl {
    pub name: String,
    pub path: Option<String>,
    pub is_test: bool,
    pub line: usize,
}

/// What the module-tree walk and the line count need from one file.
#[derive(Debug, Default)]
pub struct Items {
    /// The whole file is test code (`#![cfg(test)]` at the top).
    pub file_is_test: bool,
    /// Out-of-line modules declared at the top level of the file.
    pub decls: Vec<ModDecl>,
    /// Inline modules (`mod name { ... }`) with their line.
    pub inline_mods: Vec<(String, usize)>,
    /// Inclusive 1-based line ranges of items under `#[cfg(test)]`.
    pub test_ranges: Vec<(usize, usize)>,
}

pub fn tokens(scanned: &Scanned) -> Vec<Token> {
    let mut out = Vec::new();
    for (index, line) in scanned.code.iter().enumerate() {
        let mut word = String::new();
        for c in line.chars() {
            if is_ident_char(c) {
                word.push(c);
                continue;
            }
            if !word.is_empty() {
                out.push(Token { text: std::mem::take(&mut word), line: index + 1 });
            }
            if !c.is_whitespace() {
                out.push(Token { text: c.to_string(), line: index + 1 });
            }
        }
        if !word.is_empty() {
            out.push(Token { text: word, line: index + 1 });
        }
    }
    out
}

/// Reads the items of one file. `source_lines` is the raw text, needed for `#[path = "..."]`
/// values, whose string contents the lexer blanks.
pub fn items(tokens: &[Token], source_lines: &[&str]) -> Items {
    let mut items = Items::default();
    let mut depth = 0usize;
    let mut pending = Pending::default();
    let mut previous: Option<&str> = None;
    let mut i = 0;
    while let Some(token) = tokens.get(i) {
        let text = token.text.as_str();
        if let Some((attr, end)) = attribute(tokens, i, source_lines) {
            if !attr.inner {
                pending.add(&attr, token.line, source_lines);
            } else if attr.is_test && depth == 0 {
                items.file_is_test = true;
            }
            previous = Some("]");
            i = end;
            continue;
        }
        let after_boundary = previous.is_none_or(|p| matches!(p, ";" | "{" | "}" | "]"));
        let starts_item =
            text != "}" && (!pending.is_empty() || (after_boundary && !matches!(text, "{" | ";")));
        if starts_item {
            let attributes = std::mem::take(&mut pending);
            if let Some(end) = record_item(&mut items, tokens, i, depth, &attributes) {
                previous = tokens.get(end).map(|t| t.text.as_str());
                i = end + 1;
                continue;
            }
        }
        match text {
            "{" => depth += 1,
            "}" => depth = depth.saturating_sub(1),
            _ => {}
        }
        previous = Some(text);
        i += 1;
    }
    items
}

#[derive(Default)]
struct Pending {
    first_line: Option<usize>,
    is_test: bool,
    path: Option<String>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.first_line.is_none()
    }

    fn add(&mut self, attr: &Attribute, line: usize, source_lines: &[&str]) {
        self.first_line.get_or_insert(line);
        self.is_test |= attr.is_test;
        if attr.is_path {
            self.path = source_lines.get(line - 1).and_then(|text| quoted(text));
        }
    }
}

pub struct Attribute {
    /// `#![...]`: applies to the enclosing item or file.
    pub inner: bool,
    /// `cfg(...)` that holds only in test builds.
    pub is_test: bool,
    /// `path = "..."`.
    pub is_path: bool,
    /// Token index range of the text between the brackets.
    pub body: std::ops::Range<usize>,
}

/// The feature that exports a crate's test helpers to the tests of the crates above it (rule
/// `testing`). The shipped build never turns it on, so code under it counts as test code.
pub const TEST_FEATURE: &str = "test-support";

/// Reads the attribute starting at `start`; returns it and the index after its `]`. None when
/// the token there opens no attribute: an attribute is `#[` or `#![`, while the `#` of a raw
/// identifier (`r#type`) is followed by the name. `source_lines` is the raw text, for the string
/// values of a cfg predicate (`feature = "..."`), whose contents the lexer blanks.
pub fn attribute(
    tokens: &[Token],
    start: usize,
    source_lines: &[&str],
) -> Option<(Attribute, usize)> {
    let text_at = |index: usize| tokens.get(index).map(|t| t.text.as_str());
    if text_at(start) != Some("#") {
        return None;
    }
    let inner = text_at(start + 1) == Some("!");
    let open = start + 1 + usize::from(inner);
    if text_at(open) != Some("[") {
        return None;
    }
    let mut depth = 0usize;
    let mut end = open;
    while let Some(token) = tokens.get(end) {
        match token.text.as_str() {
            "[" => depth += 1,
            "]" => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        end += 1;
    }
    let body = body_texts(tokens, open + 1..end, source_lines);
    let body: Vec<&str> = body.iter().map(String::as_str).collect();
    let is_test = body.first() == Some(&"cfg") && cfg_is_test(body.get(1..).unwrap_or_default());
    let is_path = body.first() == Some(&"path") && body.get(1) == Some(&"=");
    Some((Attribute { inner, is_test, is_path, body: open + 1..end }, end + 1))
}

/// The token texts of an attribute body. The lexer leaves a string literal as two `"` tokens with
/// its contents blanked; each such pair becomes one token `"value"`, read from the raw line: the
/// n-th literal on a line is the n-th quoted value there.
fn body_texts(
    tokens: &[Token],
    body: std::ops::Range<usize>,
    source_lines: &[&str],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_literal = false;
    for index in body {
        let Some(token) = tokens.get(index) else { break };
        if token.text != "\"" {
            out.push(token.text.clone());
            continue;
        }
        in_literal = !in_literal;
        if !in_literal {
            continue;
        }
        let quotes_before = tokens
            .get(..index)
            .unwrap_or_default()
            .iter()
            .rev()
            .take_while(|t| t.line == token.line)
            .filter(|t| t.text == "\"")
            .count();
        let value = source_lines
            .get(token.line - 1)
            .and_then(|line| quoted_values(line).into_iter().nth(quotes_before / 2))
            .unwrap_or_default();
        out.push(format!("\"{value}\""));
    }
    out
}

/// The double-quoted values on a line, in order (escapes skipped, not decoded).
fn quoted_values(line: &str) -> Vec<&str> {
    let mut values = Vec::new();
    let mut start = None;
    let mut escaped = false;
    for (offset, c) in line.char_indices() {
        match (start, c) {
            (Some(_), _) if escaped => escaped = false,
            (Some(_), '\\') => escaped = true,
            (Some(from), '"') => {
                values.push(line.get(from..offset).unwrap_or_default());
                start = None;
            }
            (None, '"') => start = Some(offset + 1),
            _ => {}
        }
    }
    values
}

/// True when a cfg predicate holds only in test builds: `(test)`, `(all(test, windows))`, and
/// the test feature, alone or with `test` (`(any(test, feature = "test-support"))`), also
/// nested in `all(...)`. The predicate is the tokens after `cfg`, string values as one token.
pub fn cfg_is_test(predicate: &[&str]) -> bool {
    let ["(", list @ ..] = predicate else { return false };
    list_entries(list).first().is_some_and(|entry| only_in_tests(entry))
}

/// One cfg predicate, from its first token to its last.
fn only_in_tests(entry: &[&str]) -> bool {
    match entry {
        ["test"] => true,
        ["feature", "=", value] => {
            value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) == Some(TEST_FEATURE)
        }
        ["all", "(", list @ ..] => list_entries(list).iter().any(|entry| only_in_tests(entry)),
        ["any", "(", list @ ..] => {
            let entries = list_entries(list);
            !entries.is_empty() && entries.iter().all(|entry| only_in_tests(entry))
        }
        _ => false,
    }
}

/// The comma-separated entries of a predicate list, from the token after its `(` up to the `)`
/// that closes it; tokens after that `)` are ignored. Empty entries (a trailing comma) are skipped.
fn list_entries<'a>(list: &'a [&'a str]) -> Vec<&'a [&'a str]> {
    let mut entries = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (index, token) in list.iter().enumerate() {
        let closes = *token == ")" && depth == 0;
        if closes || (*token == "," && depth == 0) {
            entries.extend(list.get(start..index).filter(|entry| !entry.is_empty()));
            start = index + 1;
            if closes {
                break;
            }
            continue;
        }
        match *token {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ => {}
        }
    }
    entries
}

struct ItemEnd {
    index: usize,
    line: usize,
}

/// Keywords that open an item (after an optional visibility). Inside one, a `,` at bracket depth
/// 0 belongs to generics or a where clause (`impl<A, B>`), so it does not end the item.
const ITEM_KEYWORDS: &[&str] = &[
    "async",
    "const",
    "enum",
    "extern",
    "fn",
    "impl",
    "macro_rules",
    "mod",
    "static",
    "struct",
    "trait",
    "type",
    "union",
    "unsafe",
    "use",
];

/// The last token of the attributed thing starting at `start`, counting `(`, `[` and `{` as
/// depth: a `;` at depth 0, or the `}` that closes a body back to depth 0. A field, variant,
/// match arm or struct-literal field (anything that is not an item) also ends at a `,` at depth
/// 0. A closer that the thing did not open belongs to the enclosing block, so the thing ends
/// before it and the walk still sees that closer.
fn item_end(tokens: &[Token], start: usize) -> ItemEnd {
    let ends_at_comma = !starts_with_item_keyword(tokens, start);
    let mut depth = 0usize;
    let mut last = start;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        let end = ItemEnd { index, line: token.line };
        match token.text.as_str() {
            ";" if depth == 0 => return end,
            "," if depth == 0 && ends_at_comma => return end,
            "(" | "[" | "{" => depth += 1,
            closer @ (")" | "]" | "}") => {
                let Some(inner) = depth.checked_sub(1) else {
                    let line = tokens.get(last).map_or(token.line, |t| t.line);
                    return ItemEnd { index: last, line };
                };
                depth = inner;
                if depth == 0 && closer == "}" {
                    return end;
                }
            }
            _ => {}
        }
        last = index;
    }
    let line = tokens.last().map_or(0, |t| t.line);
    ItemEnd { index: tokens.len(), line }
}

fn starts_with_item_keyword(tokens: &[Token], start: usize) -> bool {
    let i = after_visibility(tokens, start);
    tokens.get(i).is_some_and(|t| ITEM_KEYWORDS.contains(&t.text.as_str()))
}

/// The index after an optional `pub` or `pub(...)` at `start`.
fn after_visibility(tokens: &[Token], start: usize) -> usize {
    let mut i = start;
    if tokens.get(i).is_some_and(|t| t.text == "pub") {
        i += 1;
        if tokens.get(i).is_some_and(|t| t.text == "(") {
            while tokens.get(i).is_some_and(|t| t.text != ")") {
                i += 1;
            }
            i += 1;
        }
    }
    i
}

/// Records the item starting at `start`. Returns the index of its last token when it is test
/// code, so the walk skips its body.
fn record_item(
    items: &mut Items,
    tokens: &[Token],
    start: usize,
    depth: usize,
    attributes: &Pending,
) -> Option<usize> {
    let line = tokens.get(start).map_or(0, |t| t.line);
    if let Some((name, is_decl)) = module_name(tokens, start) {
        if !is_decl {
            items.inline_mods.push((name, line));
        } else if depth == 0 {
            let path = attributes.path.clone();
            items.decls.push(ModDecl { name, path, is_test: attributes.is_test, line });
        }
    }
    if !attributes.is_test {
        return None;
    }
    let end = item_end(tokens, start);
    items.test_ranges.push((attributes.first_line.unwrap_or(line), end.line));
    Some(end.index)
}

/// `mod name;` or `mod name {` after an optional visibility, starting at `start`.
/// Returns the name and whether it is an out-of-line declaration.
fn module_name(tokens: &[Token], start: usize) -> Option<(String, bool)> {
    let i = after_visibility(tokens, start);
    if tokens.get(i)?.text != "mod" {
        return None;
    }
    let name = tokens.get(i + 1)?.text.clone();
    let is_decl = tokens.get(i + 2)?.text == ";";
    Some((name, is_decl))
}

/// The first double-quoted value on a line.
fn quoted(line: &str) -> Option<String> {
    let start = line.find('"')? + 1;
    let len = line.get(start..)?.find('"')?;
    line.get(start..start + len).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::scan;

    fn read(text: &str) -> Items {
        let lines: Vec<&str> = text.lines().collect();
        items(&tokens(&scan(text)), &lines)
    }

    #[test]
    fn reads_declarations_with_cfg_and_path() {
        let items = read(
            "mod plain;\npub(crate) mod shared;\n#[cfg(test)]\nmod tests;\n\
             #[cfg(all(test, windows))]\n#[path = \"other_tests.rs\"]\nmod other;\n",
        );
        let summary: Vec<(&str, Option<&str>, bool)> =
            items.decls.iter().map(|d| (d.name.as_str(), d.path.as_deref(), d.is_test)).collect();
        assert_eq!(
            summary,
            [
                ("plain", None, false),
                ("shared", None, false),
                ("tests", None, true),
                ("other", Some("other_tests.rs"), true),
            ]
        );
    }

    #[test]
    fn test_ranges_cover_inline_test_modules_and_items() {
        let items = read(
            "fn shipped() {}\n#[cfg(test)]\nmod tests {\n    fn a() { if x { } }\n}\n\
             impl X {\n    #[cfg(test)]\n    fn helper() {}\n}\n#[cfg(test)]\nuse std::fmt;\n",
        );
        assert_eq!(items.test_ranges, [(2, 5), (7, 8), (10, 11)]);
        assert_eq!(items.inline_mods, [("tests".to_owned(), 3)]);
    }

    /// Regression: a cfg(test) field ran on to the struct's closing brace and swallowed it, so
    /// the depth stayed one too high and a later `#[cfg(test)] mod tests;` was not recorded.
    #[test]
    fn cfg_test_fields_and_variants_end_at_their_comma() {
        let items = read(
            "pub struct S {\n    a: u8,\n    #[cfg(test)]\n    b: HashMap<u8, (u8, u16)>,\n    c: u8,\n}\n\
             enum E {\n    A,\n    #[cfg(test)]\n    B { x: u8 },\n    C,\n}\n\
             #[cfg(test)]\n#[path = \"big_tests.rs\"]\nmod tests;\n",
        );
        assert_eq!(items.test_ranges, [(3, 4), (9, 10), (13, 15)]);
        let decls: Vec<(&str, bool)> =
            items.decls.iter().map(|d| (d.name.as_str(), d.is_test)).collect();
        assert_eq!(decls, [("tests", true)]);
    }

    /// Regression: the last cfg(test) field of a struct literal, without a trailing comma,
    /// ended at the literal's closing brace and swallowed it.
    #[test]
    fn cfg_test_struct_literal_fields_leave_the_closing_brace() {
        let items = read(
            "fn f() -> S {\n    S {\n        a: 1,\n        #[cfg(test)]\n        b: g(1, 2)\n    }\n}\n\
             #[cfg(test)]\nmod tests;\n",
        );
        assert_eq!(items.test_ranges, [(4, 5), (8, 9)]);
        assert_eq!(items.decls.len(), 1, "{:?}", items.decls);
    }

    #[test]
    fn cfg_test_items_keep_commas_of_generics_and_brackets() {
        let items = read(
            "#[cfg(test)]\nimpl<A, B> X<A, B> where A: C, B: C {\n    fn f() {}\n}\n\
             #[cfg(test)]\nfn g(a: [u8; 2], b: u8) {}\nfn shipped() {}\n",
        );
        assert_eq!(items.test_ranges, [(1, 4), (5, 6)]);
    }

    /// Regression: the `#` of a raw identifier was read as an attribute that ran to the next `]`
    /// in the file, swallowing the braces and the `#[cfg(test)]` between.
    #[test]
    fn raw_identifiers_are_not_attributes() {
        let items = read(
            "pub struct Event {\n    pub r#type: String,\n}\n\
             #[cfg(test)]\n#[path = \"big_tests.rs\"]\nmod tests;\n",
        );
        assert_eq!(items.test_ranges, [(4, 6)]);
        assert_eq!(items.decls[0].path.as_deref(), Some("big_tests.rs"));
        assert!(items.decls[0].is_test);
    }

    /// Whether `#[cfg<predicate>]` marks test code, read the way the walk reads it.
    fn holds(predicate: &str) -> bool {
        let text = format!("#[cfg{predicate}]\nfn a() {{}}\n");
        let lines: Vec<&str> = text.lines().collect();
        attribute(&tokens(&scan(&text)), 0, &lines).is_some_and(|(attr, _)| attr.is_test)
    }

    #[test]
    fn only_test_only_cfgs_count_as_test() {
        assert!(holds("(test)"));
        assert!(holds("(all(windows, test))"));
        assert!(holds("(all(windows, all(unix, test)))"));
        assert!(!holds("(not(test))"));
        assert!(!holds("(any(test, windows))"));
        assert!(!holds("(all(not(test), windows))"));
    }

    /// `test-support` exports test helpers to other crates' tests; the shipped build never has
    /// it, so its items are test lines. Any other feature is shipped code.
    #[test]
    fn the_test_support_feature_counts_as_test() {
        assert!(holds("(any(test, feature = \"test-support\"))"));
        assert!(holds("(feature = \"test-support\")"));
        assert!(holds("(all(windows, any(test, feature = \"test-support\")))"));
        assert!(!holds("(any(test, feature = \"x\"))"));
        assert!(!holds("(feature = \"wgpu\")"));
        assert!(!holds("(not(any(test, feature = \"test-support\")))"));
        assert!(!holds("(any(test, feature = \"test-support\", windows))"));
    }

    /// The n-th literal on a line is read for the n-th quote pair, so a value before the
    /// attribute on the same line does not shift it.
    #[test]
    fn string_values_are_read_from_their_own_position() {
        let items = read(
            "const A: &str = \"x\"; #[cfg(any(test, feature = \"test-support\"))]\nfn probe() {}\n\
             #[cfg(any(test, feature = \"other\"))]\nfn shipped() {}\n",
        );
        assert_eq!(items.test_ranges, [(1, 2)]);
    }

    #[test]
    fn inner_cfg_test_marks_the_whole_file() {
        assert!(read("//! Docs.\n#![cfg(test)]\nfn a() {}\n").file_is_test);
        assert!(!read("#![cfg(not(test))]\nfn a() {}\n").file_is_test);
    }
}
