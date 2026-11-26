//! Incremental parse and layout of long comments. The text is cut into chunks at blank lines
//! that start a new top-level block; each chunk is parsed and laid out on its own and reused
//! while its text is unchanged, so typing into a 5,000-line comment parses and lays out the one
//! chunk around the caret instead of the whole text.
//!
//! A cut stays only where the parse of the whole text is back at top level: a chunk that ends
//! inside an unclosed fence or HTML block takes the rest of the text with it, and a text with
//! link reference definitions (which links anywhere may use) stays one chunk.

use std::ops::Range;
use std::sync::Arc;

use egui::Galley;
use egui::text::LayoutJob;

use super::structure::{self, Model};

/// Bytes compared at once while looking for the edited part of the text.
const COMPARE_BLOCK: usize = 4096;

#[derive(Clone)]
struct Chunk {
    /// Bytes of the text; the `\n` between two chunks belongs to neither.
    range: Range<usize>,
    model: Arc<Model>,
    /// The chunk's galley and the layout key it was made with.
    galley: Option<(u64, Arc<Galley>)>,
}

impl Chunk {
    fn parse(text: &str, range: Range<usize>) -> Self {
        Self { model: Arc::new(structure::parse(&text[range.clone()])), galley: None, range }
    }
}

/// The chunks of the text last passed to [`Chunks::update`], and their merged model.
#[derive(Clone, Default)]
pub struct Chunks {
    text: Option<Arc<str>>,
    /// Counts the texts parsed, so a galley is known to be for the current text.
    pub version: u64,
    parts: Arc<Vec<Chunk>>,
    pub model: Arc<Model>,
}

impl Chunks {
    /// Parses what changed since the last text; a no-op while `text` stays the same.
    pub fn update(&mut self, text: &str) {
        if self.text.as_deref() == Some(text) {
            return;
        }
        let old_text = self.text.take().unwrap_or_else(|| Arc::from(""));
        let old = std::mem::take(&mut self.parts);
        let (prefix, suffix) = common_ends(old_text.as_bytes(), text.as_bytes());
        let reuse = |range: &Range<usize>| -> Option<Chunk> {
            // Only a chunk whose bytes all lie in the unchanged start or end has the same text.
            let old_range = if range.end <= prefix {
                range.clone()
            } else if range.start >= text.len() - suffix {
                let shift = text.len() - old_text.len().min(text.len());
                let back = old_text.len() - text.len().min(old_text.len());
                range.start + back - shift..range.end + back - shift
            } else {
                return None;
            };
            let index =
                old.binary_search_by_key(&old_range.start, |chunk| chunk.range.start).ok()?;
            let chunk = &old[index];
            (chunk.range == old_range).then(|| Chunk { range: range.clone(), ..chunk.clone() })
        };
        let chunk_for =
            |range: Range<usize>| reuse(&range).unwrap_or_else(|| Chunk::parse(text, range));

        let ranges = split(text);
        let mut chunks = Vec::with_capacity(ranges.len());
        for (index, range) in ranges.iter().enumerate() {
            let chunk = chunk_for(range.clone());
            if chunk.model.ends_open && index + 1 < ranges.len() {
                chunks.push(chunk_for(range.start..text.len()));
                break;
            }
            chunks.push(chunk);
        }
        if chunks.len() > 1 && chunks.iter().any(|chunk| chunk.model.has_definitions) {
            chunks = vec![chunk_for(0..text.len())];
        }

        let mut model = Model::default();
        for chunk in &chunks {
            model.append(&chunk.model, chunk.range.start);
        }
        self.text = Some(Arc::from(text));
        self.version += 1;
        self.parts = Arc::new(chunks);
        self.model = Arc::new(model);
    }

    /// The galley of the whole text. Chunks without a galley for `key` are laid out with
    /// `layout(chunk text, chunk model)`; `whole` is the job the joined galley reports.
    pub fn galley(
        &mut self,
        key: u64,
        pixels_per_point: f32,
        mut layout: impl FnMut(&str, &Model) -> Arc<Galley>,
        whole: impl FnOnce(&str) -> LayoutJob,
    ) -> Arc<Galley> {
        let text = self.text.clone().unwrap_or_else(|| Arc::from(""));
        let chunks = Arc::make_mut(&mut self.parts);
        let galleys: Vec<Arc<Galley>> = chunks
            .iter_mut()
            .map(|chunk| match &chunk.galley {
                Some((made_with, galley)) if *made_with == key => galley.clone(),
                _ => {
                    let galley = layout(&text[chunk.range.clone()], &chunk.model);
                    chunk.galley = Some((key, galley.clone()));
                    galley
                }
            })
            .collect();
        match galleys.as_slice() {
            [single] => single.clone(),
            // `concat` puts the `\n` between two chunks back, as egui does between paragraphs.
            _ => Arc::new(Galley::concat(Arc::new(whole(&text)), &galleys, pixels_per_point)),
        }
    }
}

/// Byte ranges of the chunks: the text is cut at the first `\n` of each blank line run that
/// is followed by an unindented line. Lists, quotes, paragraphs and indented code all end
/// there; fences and HTML blocks may not, which [`Chunks::update`] checks.
fn split(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut search = 0;
    while let Some(offset) = text[search..].find("\n\n") {
        let cut = search + offset;
        let mut next = cut + 1;
        while bytes.get(next) == Some(&b'\n') {
            next += 1;
        }
        if cut > start && next < bytes.len() && !matches!(bytes[next], b' ' | b'\t' | b'\r') {
            ranges.push(start..cut);
            start = cut + 1;
        }
        search = next;
    }
    ranges.push(start..text.len());
    ranges
}

/// Lengths of the common start and end of `old` and `new`, which never overlap.
fn common_ends(old: &[u8], new: &[u8]) -> (usize, usize) {
    let max = old.len().min(new.len());
    let mut prefix = 0;
    while prefix + COMPARE_BLOCK <= max
        && old[prefix..prefix + COMPARE_BLOCK] == new[prefix..prefix + COMPARE_BLOCK]
    {
        prefix += COMPARE_BLOCK;
    }
    prefix += old[prefix..max].iter().zip(&new[prefix..max]).take_while(|(a, b)| a == b).count();
    let max = max - prefix;
    let (old_end, new_end) = (old.len(), new.len());
    let mut suffix = 0;
    while suffix + COMPARE_BLOCK <= max
        && old[old_end - suffix - COMPARE_BLOCK..old_end - suffix]
            == new[new_end - suffix - COMPARE_BLOCK..new_end - suffix]
    {
        suffix += COMPARE_BLOCK;
    }
    suffix += old[old_end - max..old_end - suffix]
        .iter()
        .rev()
        .zip(new[new_end - max..new_end - suffix].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (prefix, suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(text: &str) -> Vec<&str> {
        split(text).into_iter().map(|range| &text[range]).collect()
    }

    #[test]
    fn cuts_before_unindented_blocks_only() {
        assert_eq!(texts("a\n\nb"), ["a", "\nb"]);
        assert_eq!(texts("a\n\n\nb\n"), ["a", "\n\nb\n"]);
        assert_eq!(texts("- item\n\n  more\n\nnext"), ["- item\n\n  more", "\nnext"]);
        assert_eq!(texts("\n\nstart"), ["\n\nstart"]);
        assert_eq!(texts("end\n\n"), ["end\n\n"]);
        assert_eq!(texts(""), [""]);
    }

    #[test]
    fn finds_the_edited_part() {
        assert_eq!(common_ends(b"abcdef", b"abXdef"), (2, 3));
        assert_eq!(common_ends(b"aaa", b"aaaa"), (3, 0));
        assert_eq!(common_ends(b"", b"x"), (0, 0));
        let long = "x".repeat(10_000);
        let edited = format!("{}y{}", &long[..5_000], &long[5_000..]);
        let (prefix, suffix) = common_ends(long.as_bytes(), edited.as_bytes());
        assert!(prefix + suffix == long.len() && prefix >= 5_000);
    }

    /// The chunked model equals a parse of the whole text, before and after edits that open
    /// and close a fence across chunks.
    #[test]
    fn matches_a_whole_parse() {
        let base = "# Title\n\nSome **bold** text with `code`.\n\n- a\n- [x] b\n\n> quote\n\n```rust\nlet x = 1;\n\nlet y = 2;\n```\n\n1. one\n\n2. two @someone #12\n";
        let mut chunks = Chunks::default();
        let mut text = base.to_owned();
        let edits: [&dyn Fn(&mut String); 5] = [
            &|text| text.insert(text.find("Some").unwrap_or(0), 'x'),
            &|text| text.insert_str(text.find("- a").unwrap_or(0), "```\n\n"),
            &|text| text.push_str("\n```\n\ntail"),
            &|text| text.insert_str(text.find("> quote").unwrap_or(0), "<!-- open\n\n"),
            &|text| text.insert_str(0, "[ref]: https://example.test\n\n[ref]\n\n"),
        ];
        for edit in [&(|_: &mut String| {}) as &dyn Fn(&mut String)].into_iter().chain(edits) {
            edit(&mut text);
            chunks.update(&text);
            let whole = structure::parse(&text);
            let chunked = &chunks.model;
            // A heading of the whole text may end with the `\n` between two chunks.
            let headings = |model: &Model| {
                let trim = |range: &Range<usize>| {
                    range.start..range.start + text[range.clone()].trim_end_matches('\n').len()
                };
                model.headings.iter().map(trim).collect::<Vec<_>>()
            };
            assert_eq!(headings(&whole), headings(chunked), "headings of {text:?}");
            assert_eq!(whole.spans, chunked.spans, "spans of {text:?}");
            assert_eq!(whole.code_blocks, chunked.code_blocks, "code blocks of {text:?}");
        }
    }
}
