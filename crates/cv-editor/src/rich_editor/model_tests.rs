//! Markdown -> document -> Markdown keeps meaning: the document read back from the written
//! Markdown equals the first one, for every construct, on every platform.

use review_core::ProviderKind;

use super::model::{Atom, Container, Document, Kind, ListKind, Mark};
use super::parse::parse;
use super::serialize::serialize;

const PLATFORMS: [ProviderKind; 3] =
    [ProviderKind::GitHub, ProviderKind::GitLab, ProviderKind::AzureDevOps];

/// Parses, writes and parses again; the two documents must be equal. Returns the written text.
fn round_trip(source: &str, platform: ProviderKind) -> String {
    let (written, failure) = try_round_trip(source, platform);
    assert!(failure.is_none(), "{}", failure.unwrap_or_default());
    written
}

/// The written text, and a description of the change when the meaning changed.
fn try_round_trip(source: &str, platform: ProviderKind) -> (String, Option<String>) {
    let mut first = parse(source, platform);
    let written = serialize(&first, platform);
    if platform == ProviderKind::AzureDevOps {
        // Azure DevOps has no `<kbd>`: it is written, and so reads back, as inline code.
        for block in &mut first.blocks {
            let block = std::sync::Arc::make_mut(block);
            let length = block.content.len();
            block.content.map_marks(0..length, |marks| {
                if marks.has(Mark::Kbd) {
                    marks.set(Mark::Kbd, false);
                    marks.set(Mark::Code, true);
                }
            });
        }
    }
    let second = parse(&written, platform);
    let failure = (second != first).then(|| {
        format!(
            "{platform:?}: meaning changed\n--- source\n{source}\n--- written\n{written}\n--- first\n{first:?}\n--- second\n{second:?}\n"
        )
    });
    (written, failure)
}

const CASES: &[&str] = &[
    "plain text",
    "two\nlines in one paragraph",
    "hard  \nbreak and a backslash\\\nbreak",
    "# One\n\n## Two\n\n### Three\n\n#### Four",
    "Setext\n======\n\nSub\n---",
    "**bold** and *italic* and ***both*** and ~~gone~~",
    "**bold across words** then _under scored_ text",
    "mid**word**bold and snake_case_name and 3*4*5",
    "a **bold *nested italic* bold** end",
    "**a***b*",
    "`code` and ``co`de`` and ` `` ` and `  padded  `",
    "[link](https://example.com) and [titled](https://x.test \"Title\") and <https://auto.test/x_y>",
    "[**bold link**](https://x.test) and **[link in bold](https://y.test)**",
    "a bare https://dev.azure.com/org/_git/repo_name/pullrequest/1 url",
    "![alt text](https://img.test/a.png \"Shot\") inline image",
    "- one\n- two\n  - nested\n    - deeper\n- three",
    "1. first\n2. second\n   1. inner\n3. third",
    "5. starts at five\n6. six",
    "- [ ] open\n- [x] done\n  - [ ] nested task",
    "> quoted\n> lines\n>\n> second paragraph",
    "> [!NOTE]\n> An alert",
    "> - list in quote\n>   > quote in list in quote",
    "- item\n\n  second paragraph in item\n\n- next",
    "- item with code\n\n  ```rust\n  fn main() {}\n  ```",
    "```\nplain fence\n```",
    "```rust\nlet a = \"`\";\n```",
    "````\n```inside```\n````",
    "    indented code",
    "---\n\n***\n\n___",
    "| a | b |\n| :- | -: |\n| 1 | 2 \\| pipe |",
    "<details>\n<summary>More</summary>\n\nHidden body\n\n</details>",
    "Press <kbd>Ctrl</kbd>+<kbd>C</kbd>, H<sub>2</sub>O, x<sup>2</sup>, <ins>under</ins>",
    "<u>underlined</u> and <b>bold tag</b> and <span style=\"color:red\">kept</span> raw",
    "<!-- a comment --> visible",
    "escapes: \\* \\_ \\` \\[x\\] \\< 1\\. \\# &amp; &copy;",
    "\\# not a heading\n\n\\- not a list\n\n1\\. not a number\n\n\\> not a quote",
    "line\n\\---\nnot setext",
    "trailing  ",
    "",
    "\n\n",
    "a | b",
    "footnote[^1] here",
];

#[test]
fn every_construct_keeps_its_meaning_on_every_platform() {
    let failures: Vec<String> = PLATFORMS
        .iter()
        .flat_map(|platform| CASES.iter().filter_map(|source| try_round_trip(source, *platform).1))
        .collect();
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn crlf_sources_read_like_lf() {
    let lf = parse("# Title\n\n- a\n- b\n\n```\ncode\n```", ProviderKind::GitHub);
    let crlf = parse("# Title\r\n\r\n- a\r\n- b\r\n\r\n```\r\ncode\r\n```", ProviderKind::GitHub);
    assert_eq!(crlf, lf);
    round_trip("a\r\nb\r\n\r\nc", ProviderKind::GitHub);
}

#[test]
fn canonical_markdown_is_written_unchanged() {
    for source in [
        "# Title\n\nSome **bold** and *italic* text with `code`.",
        "- one\n- two\n  - nested\n- [ ] task\n- [x] done",
        "1. first\n2. second",
        "> quote\n>\n> more",
        "```rust\nfn main() {}\n```",
        "---",
        "| a | b |\n| --- | --- |\n| 1 | 2 |",
        "Thanks @octocat, see #12.",
    ] {
        assert_eq!(serialize(&parse(source, ProviderKind::GitHub), ProviderKind::GitHub), source);
    }
}

#[test]
fn html_marks_follow_each_platforms_allowlist() {
    let source = "<ins>u</ins> <sub>b</sub> <sup>p</sup> <kbd>K</kbd>";
    let github = round_trip(source, ProviderKind::GitHub);
    assert_eq!(github, "<ins>u</ins> <sub>b</sub> <sup>p</sup> <kbd>K</kbd>");
    let gitlab = round_trip(source, ProviderKind::GitLab);
    assert_eq!(gitlab, github);
    let azure = serialize(&parse(source, ProviderKind::AzureDevOps), ProviderKind::AzureDevOps);
    assert_eq!(azure, "<u>u</u> <sub>b</sub> <sup>p</sup> `K`", "no <kbd> on Azure DevOps");
}

#[test]
fn unknown_html_is_kept_verbatim() {
    let source =
        "a <span class=\"x\">b</span> c\n\n<div align=\"center\">\n  <img src=\"x.png\">\n</div>";
    let doc = parse(source, ProviderKind::GitHub);
    let raw: Vec<&Atom> =
        doc.blocks[0].content.runs().iter().filter_map(|run| run.atom.as_ref()).collect();
    assert_eq!(
        raw,
        [&Atom::Raw("<span class=\"x\">".into()), &Atom::Raw("</span>".into())],
        "tags outside the subset stay as they were"
    );
    assert_eq!(doc.blocks[1].kind, Kind::Html);
    assert_eq!(serialize(&doc, ProviderKind::GitHub), source);
}

#[test]
fn marks_span_words_and_leave_their_edge_spaces() {
    let mut doc = parse("a bold b", ProviderKind::GitHub);
    std::sync::Arc::make_mut(&mut doc.blocks[0])
        .content
        .map_marks(1..7, |marks| marks.set(Mark::Bold, true));
    assert_eq!(serialize(&doc, ProviderKind::GitHub), "a **bold** b");
}

#[test]
fn mentions_are_atoms_in_each_platforms_syntax() {
    let github = parse("hi @octo-cat and a@mail.test", ProviderKind::GitHub);
    let atoms: Vec<&Atom> =
        github.blocks[0].content.runs().iter().filter_map(|run| run.atom.as_ref()).collect();
    assert_eq!(atoms, [&Atom::Mention { key: "octo-cat".into() }], "an email is no mention");
    round_trip("hi @octo-cat and a@mail.test", ProviderKind::GitHub);

    for id in ["0f8fad5b-d9cb-469f-a165-70867728950e", "f8fad5b0-d9cb-469f-a165-70867728950e"] {
        let source = format!("ping @<{id}> please");
        let azure = parse(&source, ProviderKind::AzureDevOps);
        let atoms: Vec<&Atom> =
            azure.blocks[0].content.runs().iter().filter_map(|run| run.atom.as_ref()).collect();
        assert_eq!(atoms, [&Atom::Mention { key: id.into() }], "{source}");
        assert_eq!(serialize(&azure, ProviderKind::AzureDevOps), source);
    }
}

#[test]
fn nested_lists_and_quotes_keep_their_containers() {
    let doc = parse("> - a\n>   - b\n\n1. x", ProviderKind::GitHub);
    let nesting: Vec<&[Container]> = doc.blocks.iter().map(|block| &block.nesting[..]).collect();
    let bullet = |head| Container::Item { list: ListKind::Bullet, task: None, head };
    assert_eq!(
        nesting,
        [
            &[Container::Quote(None), bullet(true)][..],
            &[Container::Quote(None), bullet(false), bullet(true)][..],
            &[Container::Item { list: ListKind::Ordered(1), task: None, head: true }][..],
        ]
    );
}

#[test]
fn empty_items_and_quotes_keep_a_block() {
    for source in ["-", "- a\n-\n- c", ">", "1. a\n2."] {
        let doc = parse(source, ProviderKind::GitHub);
        let has_container = doc.blocks.iter().any(|block| !block.nesting.is_empty());
        assert!(has_container, "{source:?} keeps its empty item or quote");
        round_trip(source, ProviderKind::GitHub);
    }
    assert_eq!(parse("", ProviderKind::GitHub), Document::default());
}
