//! Markdown to the editor's document. pulldown-cmark reads the source with the GFM extensions the
//! three platforms share (tables, strikethrough, task lists, alerts); its events fold into flat
//! blocks with their container path. Inline HTML of the subset the serializer writes (`<ins>`,
//! `<u>`, `<sub>`, `<sup>`, `<kbd>`, `<b>`, `<i>`, `<s>`, `<code>`, `<br>`, `<a href>`) becomes
//! marks; every other tag, and every HTML block, is kept verbatim (an `Atom::Raw` or a
//! `Kind::Html` block), so editing a comment never drops what the editor does not understand.

// RS-DONT-04: reads pull request comments, so indexing, slicing and narrowing are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing, clippy::cast_possible_truncation)]

mod html;
mod mentions;

use std::sync::Arc;

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd,
};
use review_core::ProviderKind;

use self::html::{HtmlTag, html_mark, html_tag};
use self::mentions::find_mentions;
use super::model::{
    Alert, Atom, Block, Container, Document, HARD_BREAK, Inline, Kind, Link, ListKind, Mark, Marks,
};
use super::serialize;

/// The document `text` describes, read in `platform`'s dialect (its mention syntax).
pub fn parse(text: &str, platform: ProviderKind) -> Document {
    let source = text.replace("\r\n", "\n").replace('\r', "\n");
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let mut builder = Builder::new(platform);
    for event in Parser::new_ext(&source, options) {
        builder.event(event);
    }
    builder.finish()
}

enum Frame {
    Quote(Option<Alert>),
    List(ListKind),
    Item { task: Option<bool>, head: bool },
}

enum Leaf {
    Text { kind: Kind, inline: Inline },
    Source { kind: Kind, text: String },
}

/// A tag of the HTML subset that is open.
struct HtmlOpen {
    name: String,
    mark: Option<Mark>,
}

struct Table {
    alignments: Vec<Alignment>,
    header: Vec<Inline>,
    rows: Vec<Vec<Inline>>,
    in_head: bool,
    cell: Option<Inline>,
}

struct Image {
    src: String,
    title: String,
    alt: String,
    depth: u32,
}

struct Builder {
    platform: ProviderKind,
    blocks: Vec<Arc<Block>>,
    frames: Vec<Frame>,
    /// Blocks emitted when each frame opened, to give an empty quote or item a paragraph.
    opened_at: Vec<usize>,
    leaf: Option<Leaf>,
    counts: [u16; Mark::ALL.len()],
    links: Vec<Arc<Link>>,
    html: Vec<HtmlOpen>,
    table: Option<Table>,
    image: Option<Image>,
}

impl Builder {
    fn new(platform: ProviderKind) -> Self {
        Self {
            platform,
            blocks: Vec::new(),
            frames: Vec::new(),
            opened_at: Vec::new(),
            leaf: None,
            counts: [0; Mark::ALL.len()],
            links: Vec::new(),
            html: Vec::new(),
            table: None,
            image: None,
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                if let Some(image) = &mut self.image {
                    image.alt.push_str(&code);
                } else {
                    let marks = self.marks().with(Mark::Code);
                    self.inline().push_text(&code, &marks);
                }
            }
            Event::Html(html) => match &mut self.leaf {
                Some(Leaf::Source { text, .. }) => text.push_str(&html),
                _ => self.inline_html(&html),
            },
            Event::InlineHtml(html) => self.inline_html(&html),
            Event::SoftBreak => self.push_text("\n"),
            Event::HardBreak => self.push_text(&HARD_BREAK.to_string()),
            Event::Rule => {
                self.close_leaf();
                self.push_block(Kind::Rule, Inline::default());
            }
            Event::TaskListMarker(checked) => {
                if let Some(Frame::Item { task, .. }) = self.frames.last_mut() {
                    *task = Some(checked);
                }
            }
            Event::FootnoteReference(name) => {
                let marks = self.marks();
                self.inline().push_atom(Atom::Raw(format!("[^{name}]")), &marks);
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                let marks = self.marks();
                self.inline().push_atom(Atom::Raw(format!("${math}$")), &marks);
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.open_leaf(Kind::Paragraph),
            Tag::Heading { level, .. } => self.open_leaf(Kind::Heading(level as u8)),
            Tag::CodeBlock(kind) => {
                self.close_leaf();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info.trim().to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.leaf = Some(Leaf::Source { kind: Kind::Code(language), text: String::new() });
            }
            Tag::HtmlBlock => {
                self.close_leaf();
                self.leaf = Some(Leaf::Source { kind: Kind::Html, text: String::new() });
            }
            Tag::BlockQuote(kind) => self.open_frame(Frame::Quote(kind.map(alert))),
            Tag::List(start) => {
                self.open_frame(Frame::List(start.map_or(ListKind::Bullet, ListKind::Ordered)));
            }
            Tag::Item => self.open_frame(Frame::Item { task: None, head: true }),
            Tag::Table(alignments) => {
                self.close_leaf();
                self.table = Some(Table {
                    alignments,
                    header: Vec::new(),
                    rows: Vec::new(),
                    in_head: false,
                    cell: None,
                });
            }
            Tag::TableHead => self.with_table(|table| table.in_head = true),
            Tag::TableRow => self.with_table(|table| table.rows.push(Vec::new())),
            Tag::TableCell => self.with_table(|table| table.cell = Some(Inline::default())),
            Tag::Emphasis => self.count(Mark::Italic, true),
            Tag::Strong => self.count(Mark::Bold, true),
            Tag::Strikethrough => self.count(Mark::Strike, true),
            Tag::Superscript => self.count(Mark::Superscript, true),
            Tag::Subscript => self.count(Mark::Subscript, true),
            Tag::Link { dest_url, title, .. } => self
                .links
                .push(Arc::new(Link { url: dest_url.into_string(), title: title.into_string() })),
            Tag::Image { dest_url, title, .. } => match &mut self.image {
                Some(image) => image.depth += 1,
                None => {
                    self.image = Some(Image {
                        src: dest_url.into_string(),
                        title: title.into_string(),
                        alt: String::new(),
                        depth: 1,
                    });
                }
            },
            Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => self.close_leaf(),
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::HtmlBlock => {
                self.close_leaf();
            }
            TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item => self.close_frame(),
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    let source = serialize::table_source(
                        &table.alignments,
                        &table.header,
                        &table.rows,
                        self.platform,
                    );
                    self.push_block(Kind::Table, Inline::plain(&source));
                }
            }
            TagEnd::TableHead => self.with_table(|table| table.in_head = false),
            TagEnd::TableCell => {
                let platform = self.platform;
                self.with_table(|table| {
                    let mut cell = table.cell.take().unwrap_or_default();
                    find_mentions(&mut cell, platform);
                    if table.in_head {
                        table.header.push(cell);
                    } else if let Some(row) = table.rows.last_mut() {
                        row.push(cell);
                    }
                });
            }
            TagEnd::Emphasis => self.count(Mark::Italic, false),
            TagEnd::Strong => self.count(Mark::Bold, false),
            TagEnd::Strikethrough => self.count(Mark::Strike, false),
            TagEnd::Superscript => self.count(Mark::Superscript, false),
            TagEnd::Subscript => self.count(Mark::Subscript, false),
            TagEnd::Link => {
                self.links.pop();
            }
            TagEnd::Image => self.end_image(),
            TagEnd::TableRow
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_) => {}
        }
    }

    fn with_table(&mut self, change: impl FnOnce(&mut Table)) {
        if let Some(table) = &mut self.table {
            change(table);
        }
    }

    fn count(&mut self, mark: Mark, is_open: bool) {
        if let Some(count) = self.counts.get_mut(mark as usize) {
            *count = if is_open { count.saturating_add(1) } else { count.saturating_sub(1) };
        }
    }

    fn marks(&self) -> Marks {
        let mut marks = Marks::default();
        for (mark, count) in Mark::ALL.iter().zip(self.counts) {
            marks.set(*mark, count > 0);
        }
        marks.link = self.links.last().cloned();
        marks
    }

    fn text(&mut self, text: &str) {
        if let Some(image) = &mut self.image {
            image.alt.push_str(text);
            return;
        }
        if let Some(Leaf::Source { text: source, .. }) = &mut self.leaf {
            source.push_str(text);
            return;
        }
        self.push_text(text);
    }

    fn push_text(&mut self, text: &str) {
        let marks = self.marks();
        self.inline().push_text(text, &marks);
    }

    /// The inline being written: the open table cell, or the open text block (a tight list
    /// item's text has no paragraph around it, so one opens here).
    fn inline(&mut self) -> &mut Inline {
        if self.table.as_ref().is_some_and(|table| table.cell.is_some()) {
            return self
                .table
                .as_mut()
                .and_then(|table| table.cell.as_mut())
                .expect("cell is open");
        }
        if !matches!(self.leaf, Some(Leaf::Text { .. })) {
            self.open_leaf(Kind::Paragraph);
        }
        match &mut self.leaf {
            Some(Leaf::Text { inline, .. }) => inline,
            _ => unreachable!("a text leaf was just opened"),
        }
    }

    fn end_image(&mut self) {
        let Some(image) = &mut self.image else { return };
        image.depth = image.depth.saturating_sub(1);
        if image.depth > 0 {
            return;
        }
        if let Some(image) = self.image.take() {
            let marks = self.marks();
            let atom = Atom::Image { src: image.src, alt: image.alt, title: image.title };
            self.inline().push_atom(atom, &marks);
        }
    }

    /// A tag of the subset opens or closes a mark; any other tag stays as written.
    fn inline_html(&mut self, html: &str) {
        let tag = html_tag(html);
        match tag {
            Some(HtmlTag { name, is_end: false, href }) => {
                if name == "br" {
                    self.push_text(&HARD_BREAK.to_string());
                    return;
                }
                if name == "a" {
                    if let Some(url) = href {
                        self.links.push(Arc::new(Link { url, title: String::new() }));
                        self.html.push(HtmlOpen { name, mark: None });
                        return;
                    }
                } else if let Some(mark) = html_mark(&name) {
                    self.count(mark, true);
                    self.html.push(HtmlOpen { name, mark: Some(mark) });
                    return;
                }
            }
            Some(HtmlTag { name, is_end: true, .. }) => {
                if let Some(index) = self.html.iter().rposition(|open| open.name == name) {
                    let open = self.html.remove(index);
                    match open.mark {
                        Some(mark) => self.count(mark, false),
                        None => {
                            self.links.pop();
                        }
                    }
                    return;
                }
            }
            None => {}
        }
        let marks = self.marks();
        self.inline().push_atom(Atom::Raw(html.to_owned()), &marks);
    }

    fn open_leaf(&mut self, kind: Kind) {
        self.close_leaf();
        self.leaf = Some(Leaf::Text { kind, inline: Inline::default() });
    }

    fn close_leaf(&mut self) {
        match self.leaf.take() {
            Some(Leaf::Text { kind, mut inline }) => {
                find_mentions(&mut inline, self.platform);
                self.push_block(kind, inline);
            }
            Some(Leaf::Source { kind, text }) => {
                let text = text.strip_suffix('\n').unwrap_or(&text);
                self.push_block(kind, Inline::plain(text));
            }
            None => {}
        }
    }

    fn open_frame(&mut self, frame: Frame) {
        self.close_leaf();
        self.frames.push(frame);
        self.opened_at.push(self.blocks.len());
    }

    /// Ends a quote, list or item; a quote or item that held nothing keeps an empty paragraph.
    fn close_frame(&mut self) {
        self.close_leaf();
        let opened = self.opened_at.last().copied().unwrap_or_default();
        let is_empty = self.blocks.len() == opened;
        if is_empty && matches!(self.frames.last(), Some(Frame::Quote(_) | Frame::Item { .. })) {
            self.push_block(Kind::Paragraph, Inline::default());
        }
        self.frames.pop();
        self.opened_at.pop();
    }

    /// The block in the open containers; the items it starts lose their pending marker.
    fn push_block(&mut self, kind: Kind, content: Inline) {
        let mut nesting = Vec::new();
        let mut list = ListKind::Bullet;
        for frame in &mut self.frames {
            match frame {
                Frame::Quote(alert) => nesting.push(Container::Quote(*alert)),
                Frame::List(kind) => list = *kind,
                Frame::Item { task, head } => {
                    nesting.push(Container::Item { list, task: *task, head: *head });
                    *head = false;
                }
            }
        }
        self.blocks.push(Arc::new(Block { kind, nesting, content }));
    }

    fn finish(mut self) -> Document {
        self.close_leaf();
        while !self.frames.is_empty() {
            self.close_frame();
        }
        let mut document = Document { blocks: self.blocks };
        document.normalize();
        document
    }
}

fn alert(kind: BlockQuoteKind) -> Alert {
    match kind {
        BlockQuoteKind::Note => Alert::Note,
        BlockQuoteKind::Tip => Alert::Tip,
        BlockQuoteKind::Important => Alert::Important,
        BlockQuoteKind::Warning => Alert::Warning,
        BlockQuoteKind::Caution => Alert::Caution,
    }
}
