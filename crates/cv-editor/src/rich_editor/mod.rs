//! Rich comment editor: writes a comment the way it will look, as `TipTap` does in a browser.
//! The caller keeps the comment as Markdown (what GitHub, GitLab and Azure DevOps store); the
//! editor reads it into a document of blocks and marks (`model`), edits that, and writes
//! Markdown back after every change (`serialize`), with inline HTML only for the marks Markdown
//! lacks and only in tags the platform keeps.
//!
//! It is the Rich text mode of the comment composer (`crate::markdown_editor`), which also has
//! Markdown and Preview modes on the same String; the composer draws its footer, owns submit,
//! cancel and the discard guard, and puts its mode switch at the start of this editor's toolbar.
//!
//! API: [`show_with`] draws the frame, the toolbar (with the caller's `leading` content first) and
//! the text area for a `&mut String`. It parses the string on the first show and whenever it
//! changed from outside (detected by a fingerprint of the string the editor last read or wrote:
//! its length and its first and last 64 bytes); a caller that rewrites the middle of the text in
//! place without changing its length calls [`reload`] (the composer does when it returns from
//! Markdown to a changed text). [`text_id`] is the text area's focus id. Ctrl+Enter and Esc are
//! left in the input for the caller (submit and cancel), and Esc does not take the focus away.
//!
//! State (RS-PERF-01): one [`State`] per editor id in egui's temp data, behind an `Arc` so a frame
//! clones a pointer, not the document. It holds the document, the selection, undo history
//! (`history`: block pointers, at most 300 steps), the per-block layout cache (`layout`: entries
//! keyed by block pointer and the layout key, the unused ones dropped when offsets rebuild, a
//! budget of 2048 entries) and the serializer's per-block cache. The state lives as long as egui
//! keeps the id's temp data; a comment editor that is no longer shown is dropped with it when
//! the memory is cleared, and its per-frame cost is zero while it is not drawn. A frame without
//! input costs the visible blocks' cache lookups and a binary search for the first of them.

mod caret;
mod edit;
mod history;
mod input;
mod layout;
mod model;
mod parse;
mod popups;
mod serialize;
mod toolbar;
mod view;

#[cfg(test)]
mod edit_tests;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod walkthrough_tests;

use std::sync::Arc;

use cv_base::theme::{self, style};
use egui::mutex::Mutex;
use egui::{Context, Id, Ui, Vec2, vec2};
use review_core::ProviderKind;

use self::history::History;
use self::input::Frame;
use self::layout::LayoutCache;
use self::model::{Document, Marks, Pos, Selection};
use self::serialize::InlineCache;
use crate::markdown_editor::PullRequestContext;

/// Height of the text area before it starts to scroll.
const MAX_HEIGHT: f32 = 360.0;
/// Inside the text area, as the Markdown composer's.
const MARGIN: Vec2 = vec2(12.0, 10.0);
/// Height one row of `min_rows` reserves.
const ROW_HEIGHT: f32 = 22.0;

/// How one rich editor looks and behaves.
pub struct RichOptions<'a> {
    /// Decides the mention syntax and which HTML tags carry underline, sub, sup and kbd.
    pub platform: ProviderKind,
    /// The text area's accessible name, e.g. "Comment".
    pub label: &'a str,
    /// Calm placeholder while the comment is empty.
    pub hint: &'a str,
    pub min_rows: usize,
    pub grab_focus: bool,
    pub is_enabled: bool,
}

impl Default for RichOptions<'_> {
    fn default() -> Self {
        Self {
            platform: ProviderKind::GitHub,
            label: "Comment",
            hint: "Leave a comment",
            min_rows: 3,
            grab_focus: false,
            is_enabled: true,
        }
    }
}

/// What one frame of the editor did.
pub struct RichOutput {
    /// The Markdown changed this frame.
    pub is_changed: bool,
    pub has_focus: bool,
}

/// The text area's id, for focus.
pub fn text_id(id: Id) -> Id {
    id.with("rich_text")
}

/// Reads the text again at the next show, dropping the undo history.
pub fn reload(ctx: &Context, id: Id) {
    if let Some(handle) = ctx.data(|data| data.get_temp::<Handle>(id.with("rich_state"))) {
        handle.lock().synced = None;
    }
}

/// Shows the editor for the Markdown in `text`, with `leading` drawn at the start of the toolbar
/// row (the composer's mode switch).
pub fn show_with(
    ui: &mut Ui,
    id: Id,
    text: &mut String,
    options: &RichOptions<'_>,
    leading: impl FnOnce(&mut Ui),
) -> RichOutput {
    let handle =
        ui.data_mut(|data| data.get_temp_mut_or_default::<Handle>(id.with("rich_state")).clone());
    let mut state = handle.lock();
    state.sync(text, options.platform);
    let text_id = text_id(id);
    let has_focus = ui.memory(|memory| memory.has_focus(text_id));
    let palette = theme::palette(ui.ctx());
    let border = if has_focus {
        egui::Stroke::new(2.0, palette.accent)
    } else {
        egui::Stroke::new(1.0, palette.border_strong)
    };
    let frame = egui::Frame::new().fill(palette.surface).corner_radius(style::RADIUS_CARD);
    let shown = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        let clicked = egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(6, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    leading(ui);
                    let active = toolbar::Active::of(&state);
                    let can_mention = PullRequestContext::current(ui.ctx())
                        .is_some_and(|context| !context.people.is_empty());
                    toolbar::show(ui, &active, can_mention, options.is_enabled)
                })
                .inner
            })
            .inner;
        let divider_y = ui.cursor().top();
        ui.painter().hline(
            ui.max_rect().x_range(),
            divider_y,
            egui::Stroke::new(1.0, palette.border),
        );
        if clicked.is_some() {
            ui.memory_mut(|memory| memory.request_focus(text_id));
        }
        let min_height = options.min_rows as f32 * ROW_HEIGHT + 2.0 * MARGIN.y;
        let max_height = MAX_HEIGHT.min(ui.ctx().content_rect().height() * 0.5).max(min_height);
        egui::ScrollArea::vertical()
            .id_salt(id.with("rich_scroll"))
            .max_height(max_height)
            .auto_shrink([false, true])
            .show_viewport(ui, |ui, viewport| {
                let body = view::Body {
                    text_id,
                    options,
                    has_focus,
                    page_height: viewport.height(),
                    min_height,
                };
                body.show(ui, &mut state, clicked)
            })
            .inner
    });
    ui.painter().rect_stroke(
        shown.response.rect,
        style::RADIUS_CARD,
        border,
        egui::StrokeKind::Inside,
    );
    let (caret_rect, origin) = shown.inner;
    let mut is_changed = state.version != state.written;
    let frame = Frame { ui, origin, page_height: 0.0, now: ui.input(|input| input.time) };
    if let Some(command) = popups::show(ui, id, &mut state, caret_rect, &frame) {
        let ran = input::run(&mut state, &frame, vec![command]);
        is_changed |= ran.is_changed;
        ui.memory_mut(|memory| memory.request_focus(text_id));
    }
    if options.grab_focus && !has_focus && matches!(state.popup, Popup::None) {
        ui.memory_mut(|memory| memory.request_focus(text_id));
    }
    if state.version != state.written {
        *text = state.write();
        is_changed = true;
    }
    RichOutput { is_changed, has_focus }
}

type Handle = Arc<Mutex<State>>;

/// A popup the editor shows.
#[derive(Debug, Clone, PartialEq)]
enum Popup {
    None,
    /// The link popover; `is_new` until its field took focus.
    Link {
        url: String,
        is_new: bool,
    },
    /// The mention list, for the `@` at `start`.
    Mention {
        start: Pos,
        selected: usize,
    },
}

/// A cheap stand-in for the text last read or written (see the module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    length: usize,
    hash: u64,
}

impl Fingerprint {
    fn of(text: &str) -> Self {
        let bytes = text.as_bytes();
        let head = bytes.get(..bytes.len().min(64)).unwrap_or_default();
        let tail = bytes.get(bytes.len().saturating_sub(64)..).unwrap_or_default();
        Self { length: bytes.len(), hash: egui::epaint::util::hash((head, tail)) }
    }
}

/// One editor's state between frames.
struct State {
    doc: Document,
    selection: Selection,
    /// Marks for the next typed text, after a mark toggle at a bare caret or an input rule.
    stored: Option<Marks>,
    history: History,
    layout: LayoutCache,
    serializer: InlineCache,
    platform: ProviderKind,
    /// The text last read or written; `None` reads the text at the next show.
    synced: Option<Fingerprint>,
    /// Counts edits; `written` is the count the text was last written at.
    version: u64,
    written: u64,
    /// The column vertical moves keep (screen x).
    column: Option<f32>,
    /// The IME composition shown in the text: where it starts and its length in chars.
    preedit: Option<(Pos, usize)>,
    popup: Popup,
    scroll_to_caret: bool,
    /// The plain text for accessibility, at an edit count.
    value: Option<(u64, Arc<str>)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            doc: Document::default(),
            selection: Selection::caret(Pos::new(0, 0)),
            stored: None,
            history: History::default(),
            layout: LayoutCache::default(),
            serializer: InlineCache::default(),
            platform: ProviderKind::GitHub,
            synced: None,
            version: 0,
            written: 0,
            column: None,
            preedit: None,
            popup: Popup::None,
            scroll_to_caret: false,
            value: None,
        }
    }
}

impl State {
    /// Reads `text` when it is not the text the editor last read or wrote.
    fn sync(&mut self, text: &str, platform: ProviderKind) {
        let fingerprint = Fingerprint::of(text);
        if self.synced == Some(fingerprint) && self.platform == platform {
            return;
        }
        self.platform = platform;
        self.doc = parse::parse(text, platform);
        self.selection = Selection::caret(self.doc.end());
        self.history = History::default();
        self.stored = None;
        self.preedit = None;
        self.popup = Popup::None;
        self.synced = Some(fingerprint);
        self.version += 1;
        self.written = self.version;
        self.layout.invalidate();
    }

    /// Marks the document changed.
    fn changed(&mut self) {
        self.version += 1;
        self.layout.invalidate();
        self.scroll_to_caret = true;
    }

    /// The document as Markdown; remembers it as the synced text.
    fn write(&mut self) -> String {
        let text = serialize::serialize_with(&self.doc, self.platform, &mut self.serializer);
        self.synced = Some(Fingerprint::of(&text));
        self.written = self.version;
        text
    }

    /// The text a screen reader reads: blocks on lines of their own, atoms as their source.
    fn value(&mut self) -> Arc<str> {
        if let Some((version, value)) = &self.value
            && *version == self.version
        {
            return value.clone();
        }
        let lines: Vec<String> = self
            .doc
            .blocks
            .iter()
            .map(|block| {
                block
                    .content
                    .runs()
                    .iter()
                    .map(|run| match &run.atom {
                        Some(atom) => serialize::atom_source(atom, self.platform),
                        None => run.text.replace(model::HARD_BREAK, "\n"),
                    })
                    .collect()
            })
            .collect();
        let value: Arc<str> = lines.join("\n").into();
        self.value = Some((self.version, value.clone()));
        value
    }
}
