//! Folds pulldown-cmark's events into the block tree: a stack of open containers (quotes, lists,
//! items, tables) and the inline being written, plus the source range of each top-level block.

use std::ops::Range;
use std::sync::Arc;

use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, Tag, TagEnd};

use super::html::{HtmlTag, html_inline, html_tag};
use super::{Block, BlockSource, ImageSpec, Inline, Item, Parsed, Style};

enum Container {
    Root(Vec<Block>),
    Quote(Option<BlockQuoteKind>, Vec<Block>),
    List(Option<u64>, Vec<Item>),
    Item(Item),
    Table { alignments: Vec<Alignment>, header: Vec<Inline>, rows: Vec<Vec<Inline>>, in_head: bool },
}

#[derive(Default)]
pub(super) struct Builder {
    stack: Vec<Container>,
    inline: Option<Inline>,
    heading: Option<u8>,
    /// The open code block's info string and text.
    code: Option<(String, String)>,
    html: Option<String>,
    style: Style,
    strong: u32,
    emphasis: u32,
    strike: u32,
    /// The image whose alt text is being read, and how deep images nest inside it.
    image: Option<ImageSpec>,
    images: u32,
    /// Links an inline HTML `<a>` replaced, to restore at its `</a>`.
    html_links: Vec<Option<String>>,
    /// Source of the top-level block being built, and of the items of a top-level list.
    root_source: BlockSource,
    /// Sources of the finished top-level blocks.
    sources: Vec<BlockSource>,
}

impl Builder {
    pub(super) fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        if self.stack.is_empty() {
            self.stack.push(Container::Root(Vec::new()));
        }
        let starts_block = match &event {
            Event::Rule => true,
            Event::Start(tag) => is_block_tag(tag),
            _ => false,
        };
        if starts_block && self.stack.len() == 1 && self.code.is_none() {
            self.flush_inline();
            self.root_source = BlockSource { bytes: range.clone(), items: Vec::new() };
        }
        if matches!(event, Event::Start(Tag::Item))
            && matches!(self.stack.as_slice(), [Container::Root(_), Container::List(..)])
        {
            self.root_source.items.push(range);
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(text) => {
                if let Some(image) = &mut self.image {
                    image.alt.push_str(&text);
                    return;
                }
                let style = Style { code: true, ..self.style.clone() };
                self.inline().push(&text, &style);
            }
            Event::InlineHtml(html) => self.inline_html(&html),
            Event::Html(html) => self.html.get_or_insert_with(String::new).push_str(&html),
            Event::SoftBreak => {
                let style = self.style.clone();
                self.inline().push(" ", &style);
            }
            Event::HardBreak => {
                let style = self.style.clone();
                self.inline().push("\n", &style);
            }
            Event::Rule => {
                self.flush_inline();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(checked) => {
                if let Some(Container::Item(item)) = self.stack.last_mut() {
                    item.task = Some(checked);
                }
            }
            Event::InlineMath(text) | Event::DisplayMath(text) => {
                let style = Style { code: true, ..self.style.clone() };
                self.inline().push(&text, &style);
            }
            Event::FootnoteReference(_) => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph | Tag::HtmlBlock | Tag::MetadataBlock(_) => self.flush_inline(),
            Tag::Heading { level, .. } => {
                self.flush_inline();
                self.heading = Some(level as u8);
            }
            Tag::BlockQuote(kind) => {
                self.flush_inline();
                self.stack.push(Container::Quote(kind, Vec::new()));
            }
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info.trim().to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((language, String::new()));
            }
            Tag::List(start) => {
                self.flush_inline();
                self.stack.push(Container::List(start, Vec::new()));
            }
            Tag::Item => {
                self.flush_inline();
                self.stack.push(Container::Item(Item::default()));
            }
            Tag::Table(alignments) => {
                self.flush_inline();
                self.stack.push(Container::Table {
                    alignments,
                    header: Vec::new(),
                    rows: Vec::new(),
                    in_head: false,
                });
            }
            Tag::TableHead => {
                if let Some(Container::Table { in_head, .. }) = self.stack.last_mut() {
                    *in_head = true;
                }
            }
            Tag::TableRow => {
                if let Some(Container::Table { rows, .. }) = self.stack.last_mut() {
                    rows.push(Vec::new());
                }
            }
            Tag::TableCell => self.inline = Some(Inline::default()),
            Tag::Emphasis => self.set_count(|builder| &mut builder.emphasis, 1),
            Tag::Strong => self.set_count(|builder| &mut builder.strong, 1),
            Tag::Strikethrough => self.set_count(|builder| &mut builder.strike, 1),
            Tag::Link { dest_url, .. } => self.style.link = Some(dest_url.into_string()),
            Tag::Image { dest_url, title, .. } => {
                self.images += 1;
                if self.image.is_none() {
                    self.image = Some(ImageSpec {
                        src: dest_url.into_string(),
                        title: title.into_string(),
                        ..ImageSpec::default()
                    });
                }
            }
            Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::MetadataBlock(_) => self.flush_inline(),
            TagEnd::Heading(_) => {
                let inline = self.inline.take().unwrap_or_default();
                let level = self.heading.take().unwrap_or(1);
                self.push_block(Block::Heading(level, inline));
            }
            TagEnd::HtmlBlock => self.end_html_block(),
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                if let Some(Container::Quote(alert, blocks)) = self.stack.pop() {
                    self.push_block(Block::Quote { alert, blocks });
                }
            }
            TagEnd::CodeBlock => {
                let (language, code) = self.code.take().unwrap_or_default();
                let code = code.strip_suffix('\n').unwrap_or(&code).to_owned();
                self.push_block(Block::Code { language, code });
            }
            TagEnd::List(_) => {
                self.flush_inline();
                if let Some(Container::List(start, items)) = self.stack.pop() {
                    self.push_block(Block::List { start, items });
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                if let Some(Container::Item(item)) = self.stack.pop()
                    && let Some(Container::List(_, items)) = self.stack.last_mut()
                {
                    items.push(item);
                }
            }
            TagEnd::Table => {
                if let Some(Container::Table { alignments, header, rows, .. }) = self.stack.pop() {
                    self.push_block(Block::Table { alignments, header, rows });
                }
            }
            TagEnd::TableHead => {
                if let Some(Container::Table { in_head, .. }) = self.stack.last_mut() {
                    *in_head = false;
                }
            }
            TagEnd::TableCell => self.end_cell(),
            TagEnd::Emphasis => self.set_count(|builder| &mut builder.emphasis, -1),
            TagEnd::Strong => self.set_count(|builder| &mut builder.strong, -1),
            TagEnd::Strikethrough => self.set_count(|builder| &mut builder.strike, -1),
            TagEnd::Link => self.style.link = None,
            TagEnd::Image => self.end_image(),
            TagEnd::TableRow
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript => {}
        }
    }

    /// An HTML block shows as a paragraph of its text, images and links, unless nothing of it shows.
    fn end_html_block(&mut self) {
        self.flush_inline();
        if let Some(html) = self.html.take() {
            let inline = html_inline(&html);
            if !inline.is_blank() {
                self.push_block(Block::Paragraph(inline));
            }
        }
    }

    /// The finished cell joins the header or the open row.
    fn end_cell(&mut self) {
        let cell = self.inline.take().unwrap_or_default();
        if let Some(Container::Table { header, rows, in_head, .. }) = self.stack.last_mut() {
            if *in_head {
                header.push(cell);
            } else if let Some(row) = rows.last_mut() {
                row.push(cell);
            }
        }
    }

    /// The outermost image ends: it becomes a run; images nested in its alt text do not.
    fn end_image(&mut self) {
        self.images = self.images.saturating_sub(1);
        if self.images == 0
            && let Some(image) = self.image.take()
        {
            self.push_image(image);
        }
    }

    fn text(&mut self, text: &str) {
        if let Some((_, code)) = &mut self.code {
            code.push_str(text);
            return;
        }
        if let Some(html) = &mut self.html {
            html.push_str(text);
            return;
        }
        if let Some(image) = &mut self.image {
            image.alt.push_str(text);
            return;
        }
        // Bare URLs are linked once the block is complete, see `link_plain_text`.
        let style = self.style.clone();
        self.inline().push(text, &style);
    }

    fn set_count(&mut self, counter: impl Fn(&mut Self) -> &mut u32, delta: i32) {
        let count = counter(self);
        *count = count.saturating_add_signed(delta);
        self.style.strong = self.strong > 0;
        self.style.emphasis = self.emphasis > 0;
        self.style.strike = self.strike > 0;
    }

    fn inline(&mut self) -> &mut Inline {
        self.inline.get_or_insert_with(Inline::default)
    }

    /// An image run in the current style, linked when it sits inside a link.
    fn push_image(&mut self, image: ImageSpec) {
        let alt = image.alt.clone();
        let style = Style { image: Some(Arc::new(image)), ..self.style.clone() };
        self.inline().push(&alt, &style);
    }

    /// Inline HTML inside a paragraph: `<br>` breaks the line, `<img>` is an image, `<a href>`
    /// links what follows up to its `</a>`, the HTML marks mark it up to their end tag. Other
    /// tags are dropped, their text stays.
    fn inline_html(&mut self, html: &str) {
        match html_tag(html) {
            Some(HtmlTag::Break) => {
                let style = self.style.clone();
                self.inline().push("\n", &style);
            }
            Some(HtmlTag::Image(image)) => self.push_image(image),
            Some(HtmlTag::Link(href)) => {
                let previous = std::mem::replace(&mut self.style.link, href);
                self.html_links.push(previous);
            }
            Some(HtmlTag::LinkEnd) => {
                if let Some(previous) = self.html_links.pop() {
                    self.style.link = previous;
                }
            }
            Some(HtmlTag::Mark { mark, is_end }) => self.style.html.apply(mark, is_end),
            Some(HtmlTag::Block) | None => {}
        }
    }

    /// Ends the open paragraph; tight list items have text without a paragraph around it.
    fn flush_inline(&mut self) {
        if self.heading.is_some() {
            return;
        }
        if let Some(inline) = self.inline.take()
            && !inline.is_blank()
        {
            self.push_block(Block::Paragraph(inline));
        }
    }

    fn push_block(&mut self, block: Block) {
        match self.stack.last_mut() {
            Some(Container::Root(blocks)) => {
                blocks.push(block);
                self.sources.push(std::mem::take(&mut self.root_source));
            }
            Some(Container::Quote(_, blocks)) => blocks.push(block),
            Some(Container::Item(item)) => item.blocks.push(block),
            Some(Container::List(_, items)) => items.push(Item { task: None, blocks: vec![block] }),
            Some(Container::Table { .. }) | None => {}
        }
    }

    pub(super) fn finish(mut self) -> Parsed {
        let blocks = self.finish_blocks();
        Parsed { blocks, sources: self.sources }
    }

    fn finish_blocks(&mut self) -> Vec<Block> {
        self.flush_inline();
        debug_assert_eq!(self.stack.len(), 1, "pulldown-cmark balances its events");
        match self.stack.drain(..).next() {
            Some(Container::Root(blocks)) => blocks,
            _ => Vec::new(),
        }
    }
}

/// Tags that open a block of their own (not inline markup inside one).
fn is_block_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::CodeBlock(_)
            | Tag::HtmlBlock
            | Tag::List(_)
            | Tag::Table(_)
            | Tag::FootnoteDefinition(_)
            | Tag::MetadataBlock(_)
            | Tag::DefinitionList
    )
}
