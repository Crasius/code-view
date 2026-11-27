//! Comment composer. The Markdown text is the source of truth (it is what GitHub, GitLab and
//! Azure DevOps store); three modes (`Mode`) show and edit the same `&mut String`:
//! - Rich text (`crate::rich_editor`): the comment as it will look, as `TipTap` writes it in a
//!   browser. It reads the String when it changed since it last wrote it and writes Markdown back
//!   after every edit, so a mode switch never rewrites text nobody edited.
//! - Markdown (`Pass::write`): the source exactly as typed, in one plain text style (fenced code
//!   in the code font so indentation lines up): no styling, hidden markers or drawn widgets that
//!   would hide what gets posted. A toolbar, shortcuts, list continuation, smart paste,
//!   suggestions and @mentions write plain Markdown.
//! - Preview (`Pass::preview`): the exact rendered comment.
//!
//! The seam: [`show`] owns the mode switch, the footer, the submit and cancel keys and the discard
//! guard for all three modes; `Pass::panel` draws the body. Markdown and Preview sit in the
//! composer's own frame under its toolbar; Rich text draws its own frame and toolbar and takes
//! the mode switch at the start of that toolbar, so every mode shows one frame. In Rich text the
//! editor leaves Ctrl+Enter and Esc in the input and [`show`] reads them after it drew.
//!
//! A composer opens in its platform's preferred editor (`preference`, saved in the settings);
//! the switch changes that preference, and after a post or a cancel the composer returns to it.
//! Cancel and Esc ask before discarding only text that differs from where the composer started
//! ([`Options::initial`]).

mod chunks;
mod context;
mod edits;
mod footer;
mod formatting;
mod keys;
mod layout;
mod preference;
mod structure;
mod toolbar;
pub(crate) mod toolbar_layout;

#[cfg(test)]
mod tests;

use cv_base::platform::shortcut;
use cv_base::theme::{self, style};
use cv_markdown::RenderOptions;
use egui::{EventFilter, Id, Key, Modifiers, RichText, TextEdit, Ui};
use review_core::ProviderKind;

pub(crate) use self::context::mention_source;
pub use self::context::{FileContext, Person, PullRequestContext};
use self::edits::{
    PendingCursor, active_formats, indent, newline, run_action, selection_of, smart_paste,
};
pub use self::footer::Outcome;
use self::keys::Requests;
use self::layout::WriteCache;
pub use self::preference::{publish as publish_editors, take_chosen as take_chosen_editors};
use self::toolbar::{Action, Features, Mode};
use self::toolbar_layout::{ROW_HEIGHT, TEXT_MARGIN};
use crate::rich_editor::{self, RichOptions};

/// Posts the text.
const SUBMIT_SHORTCUT: &str = shortcut!(command Enter);

/// How one composer looks and behaves.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent switches of an options struct that callers fill by name over \
              `..Options::default()`"
)]
pub struct Options<'a> {
    /// Calm placeholder, e.g. "Leave a comment".
    pub hint: &'a str,
    /// The primary button, e.g. "Comment" or "Reply".
    pub submit_label: &'a str,
    /// Shows Cancel (with a discard guard when there is text) and lets Esc cancel.
    pub can_cancel: bool,
    /// A post is in flight: the primary button shows progress and is disabled.
    pub is_busy: bool,
    pub grab_focus: bool,
    pub min_rows: usize,
    /// The text the composer started with when it edits a posted comment: cancelling asks first
    /// only when the text differs from it. `None` starts from an empty comment.
    pub initial: Option<&'a str>,
    /// Lines a `suggestion` fence starts with; `Some` only where the platform applies suggestions.
    pub suggestion: Option<&'a str>,
    /// Renders Preview as this platform does; `None` uses the pull request's platform if known.
    pub platform: Option<ProviderKind>,
    /// A second, outlined submit button beside the primary one, e.g. "Reply & resolve". Thread
    /// layout only (`leading_buttons`).
    pub secondary_label: Option<&'a str>,
    /// Replaces the shortcut hint in the footer, e.g. "Draft saved".
    pub footer_hint: Option<&'a str>,
    /// Buttons on the left, the hint on the right (thread composers), instead of the reverse.
    pub leading_buttons: bool,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self {
            hint: "Leave a comment",
            submit_label: "Comment",
            can_cancel: true,
            is_busy: false,
            grab_focus: false,
            min_rows: 3,
            initial: None,
            suggestion: None,
            platform: None,
            secondary_label: None,
            footer_hint: None,
            leading_buttons: false,
        }
    }
}

/// Per-editor UI state kept between frames.
#[derive(Clone)]
struct EditorState {
    mode: Mode,
    is_confirming_discard: bool,
    is_mention_open: bool,
    /// The text's hash when Rich text was left, to read it again on return when it changed.
    rich_left_at: Option<u64>,
}

impl EditorState {
    fn new(mode: Mode) -> Self {
        Self { mode, is_confirming_discard: false, is_mention_open: false, rich_left_at: None }
    }
}

/// Shows the composer for `text`; returns how this frame ended it, `None` while the user writes.
pub fn show(ui: &mut Ui, id: Id, text: &mut String, options: &Options<'_>) -> Option<Outcome> {
    let state_id = id.with("editor_state");
    let cache_id = id.with("write");
    let people = PullRequestContext::current(ui.ctx());
    let platform = options.platform.or(people.as_ref().map(|context| context.kind));
    let mut state = ui
        .data(|data| data.get_temp::<EditorState>(state_id))
        .unwrap_or_else(|| EditorState::new(preference::mode(ui.ctx(), platform)));
    let pass = Pass {
        id,
        edit_id: id.with("text"),
        rich_id: id.with("rich"),
        platform,
        options,
        people: people.as_ref(),
        features: Features {
            can_mention: people.as_ref().is_some_and(|context| !context.people.is_empty()),
            can_suggest: options.suggestion.is_some(),
        },
    };
    let edit_id = pass.edit_id;
    let mut cache = WriteCache::load(ui, cache_id);

    PendingCursor::restore(ui.ctx(), edit_id, text);
    let has_focus = ui.memory(|memory| memory.has_focus(edit_id));
    let mut requests = if has_focus && state.mode == Mode::Markdown {
        keys::take_requests(ui, state.is_mention_open, &pass.features)
    } else {
        Requests::default()
    };
    let shown_mode = state.mode;
    let is_changed = pass.panel(ui, text, &mut cache, &mut state, &mut requests, has_focus);
    if state.mode != shown_mode {
        pass.switched(ui, text, &mut state, shown_mode);
    }
    if options.grab_focus && !has_focus && state.mode == Mode::Markdown {
        ui.memory_mut(|memory| memory.request_focus(edit_id));
    }

    let content = footer::Content::of(text, options.initial);
    ui.add_space(style::SPACE_S);
    let action = footer::show(ui, options, state.is_confirming_discard, content.has_text());
    let keys = footer::Keys { submit: requests.submit, cancel: requests.cancel };
    let draft = footer::Draft { content, was_confirming: state.is_confirming_discard, is_changed };
    let resolution = footer::resolve(action, keys, draft, options);
    state.is_confirming_discard = resolution.is_confirming_discard;
    let text_id = pass.text_id(state.mode);
    if resolution.keeps_editing {
        ui.memory_mut(|memory| memory.request_focus(text_id));
    }
    match resolution.outcome {
        Some(Outcome::Submitted { .. } | Outcome::Cancelled) => {
            pass.set_mode(ui.ctx(), text, &mut state, preference::mode(ui.ctx(), platform));
        }
        Some(Outcome::Dismissed) => ui.memory_mut(|memory| memory.surrender_focus(text_id)),
        None => {}
    }
    cache.store(ui, cache_id);
    ui.data_mut(|data| data.insert_temp(state_id, state));
    resolution.outcome
}

/// What one frame of an editor works with, besides its text, caches and state.
struct Pass<'a> {
    id: Id,
    edit_id: Id,
    /// The rich editor's id; its text area has `rich_editor::text_id(rich_id)`.
    rich_id: Id,
    /// Rich text is offered only where the platform is known.
    platform: Option<ProviderKind>,
    options: &'a Options<'a>,
    people: Option<&'a PullRequestContext>,
    features: Features,
}

impl Pass<'_> {
    /// The focus id of the text area `mode` shows; Preview has none and keeps the Markdown one.
    fn text_id(&self, mode: Mode) -> Id {
        match mode {
            Mode::Rich => rich_editor::text_id(self.rich_id),
            Mode::Markdown | Mode::Preview => self.edit_id,
        }
    }

    /// After the switch went from `from` to `state.mode`: the new body takes the focus, and the
    /// platform's preferred editor becomes the one chosen.
    fn switched(&self, ui: &Ui, text: &str, state: &mut EditorState, from: Mode) {
        let to = state.mode;
        state.mode = from;
        self.set_mode(ui.ctx(), text, state, to);
        if let Some(platform) = self.platform {
            preference::choose(ui.ctx(), platform, to);
        }
        if to != Mode::Preview {
            ui.memory_mut(|memory| memory.request_focus(self.text_id(to)));
        }
    }

    /// Changes the mode. Coming back to Rich text after the text changed elsewhere, the rich
    /// editor reads it again: its own check misses an edit that kept the text's length and ends.
    fn set_mode(&self, ctx: &egui::Context, text: &str, state: &mut EditorState, mode: Mode) {
        let hash = || egui::epaint::util::hash(text);
        if state.mode == Mode::Rich && mode != Mode::Rich {
            state.rich_left_at = Some(hash());
        }
        if mode == Mode::Rich && state.mode != Mode::Rich && state.rich_left_at != Some(hash()) {
            rich_editor::reload(ctx, self.rich_id);
        }
        state.mode = mode;
    }

    /// The body of the current mode: the rich editor, or the framed panel with the toolbar,
    /// divider and the write area or the preview. Runs the requested edits first; returns
    /// whether the text changed this frame.
    fn panel(
        &self,
        ui: &mut Ui,
        text: &mut String,
        cache: &mut WriteCache,
        state: &mut EditorState,
        requests: &mut Requests,
        has_focus: bool,
    ) -> bool {
        if state.mode == Mode::Rich
            && let Some(platform) = self.platform
        {
            return self.rich(ui, text, state, platform, requests);
        }
        toolbar_layout::framed(ui, has_focus, |ui| {
            if let Some(action) = self.toolbar(ui, text, cache, state) {
                requests.action = Some(action);
            }
            let is_edited = self.run_requests(ui, text, cache, requests);
            let is_written = match state.mode {
                Mode::Preview => {
                    self.preview(ui, text);
                    false
                }
                Mode::Markdown | Mode::Rich => self.write(ui, text, cache, state),
            };
            is_edited || is_written
        })
    }

    /// The rich editor in its own frame, the mode switch leading its toolbar. It leaves Ctrl+Enter
    /// and Esc in the input; while it has the focus they submit and cancel. Returns whether the
    /// Markdown changed.
    fn rich(
        &self,
        ui: &mut Ui,
        text: &mut String,
        state: &mut EditorState,
        platform: ProviderKind,
        requests: &mut Requests,
    ) -> bool {
        let options = RichOptions {
            platform,
            label: self.options.submit_label,
            hint: self.options.hint,
            min_rows: self.options.min_rows,
            grab_focus: self.options.grab_focus,
            is_enabled: !self.options.is_busy,
        };
        let output = rich_editor::show_with(ui, self.rich_id, text, &options, |ui| {
            toolbar::mode_switch(ui, self.id, &mut state.mode, true);
        });
        if output.has_focus {
            ui.input_mut(|input| {
                requests.submit = input.consume_key(Modifiers::COMMAND, Key::Enter);
                requests.cancel = input.consume_key(Modifiers::NONE, Key::Escape);
            });
        }
        output.is_changed
    }

    /// The mode switch and the Markdown formatting buttons; returns the button clicked.
    fn toolbar(
        &self,
        ui: &mut Ui,
        text: &str,
        cache: &mut WriteCache,
        state: &mut EditorState,
    ) -> Option<Action> {
        toolbar_layout::toolbar_row(ui, |ui| {
            toolbar::mode_switch(ui, self.id, &mut state.mode, self.platform.is_some());
            let model = cache.model_for(text);
            let active = active_formats(text, &model, selection_of(ui, self.edit_id, text));
            toolbar::show(ui, &active, &self.features, state.mode == Mode::Markdown)
        })
    }

    /// The toolbar's or a shortcut's action, the paste, Enter and Tab the editor took from the
    /// input; returns whether one of them changed the text.
    fn run_requests(
        &self,
        ui: &Ui,
        text: &mut String,
        cache: &mut WriteCache,
        requests: &mut Requests,
    ) -> bool {
        let edit_id = self.edit_id;
        let mut is_changed = false;
        if let Some(action) = requests.action.take() {
            is_changed |= run_action(ui, edit_id, text, cache, action, self.options);
        }
        if let Some(paste) = requests.paste.take() {
            is_changed |= smart_paste(ui, edit_id, text, cache, &paste);
        }
        if requests.enter {
            is_changed |= newline(ui, edit_id, text, cache);
        }
        if let Some(direction) = requests.tab {
            is_changed |= indent(ui, edit_id, text, cache, direction);
        }
        is_changed
    }

    /// The scrolling text area; returns whether the text changed.
    fn write(
        &self,
        ui: &mut Ui,
        text: &mut String,
        cache: &mut WriteCache,
        state: &mut EditorState,
    ) -> bool {
        let (_, max_height) = toolbar_layout::text_area_heights(ui.ctx(), self.options.min_rows);
        egui::ScrollArea::vertical()
            .id_salt(self.id.with("scroll"))
            .max_height(max_height)
            .auto_shrink([false, true])
            .show(ui, |ui| self.write_area(ui, text, cache, state))
            .inner
    }

    /// The comment as it will be posted, rendered as its platform renders it.
    fn preview(&self, ui: &mut Ui, text: &str) {
        let palette = theme::palette(ui.ctx());
        egui::Frame::new().inner_margin(TEXT_MARGIN).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(self.options.min_rows as f32 * ROW_HEIGHT);
            ui.spacing_mut().item_spacing.y = 6.0;
            if text.trim().is_empty() {
                ui.label(RichText::new("Nothing to preview yet.").color(palette.text_secondary));
                return;
            }
            let render =
                self.platform.map_or_else(RenderOptions::default, RenderOptions::for_platform);
            cv_markdown::render(ui, self.id.with("preview"), text, render);
        });
    }

    /// Shows the text edit, with mention completion where people are known; returns whether the
    /// user changed the text (typing, undo, an accepted mention).
    fn write_area(
        &self,
        ui: &mut Ui,
        text: &mut String,
        cache: &mut WriteCache,
        state: &mut EditorState,
    ) -> bool {
        let (edit_id, options) = (self.edit_id, self.options);
        let palette = theme::palette(ui.ctx());
        let caret_byte = formatting::byte_offset(text, selection_of(ui, edit_id, text).start);
        let model = cache.model_for(text);
        let tab_indents =
            formatting::list_at(text, caret_byte).is_some() || model.is_in_code_block(caret_byte);
        let event_filter = EventFilter {
            tab: tab_indents,
            horizontal_arrows: true,
            vertical_arrows: true,
            escape: true,
        };
        let (output, is_accepted) = {
            let mut layouter = layout::layouter(cache, palette);
            let text_edit = TextEdit::multiline(text)
                .id(edit_id)
                .hint_text(options.hint)
                .desired_rows(options.min_rows)
                .desired_width(f32::INFINITY)
                .frame(egui::Frame::NONE.inner_margin(TEXT_MARGIN))
                .event_filter(event_filter)
                .layouter(&mut layouter);
            match self.people {
                Some(context) if !context.people.is_empty() => {
                    let completion =
                        egui::CompletionPopup::new(edit_id)
                            .show(ui, text_edit, |query| mention_suggestions(context, query.word));
                    state.is_mention_open = completion.is_open;
                    // An accepted suggestion replaces the word without marking the edit changed.
                    (completion.text_edit, completion.accepted.is_some())
                }
                _ => {
                    state.is_mention_open = false;
                    (text_edit.show(ui), false)
                }
            }
        };
        let response = &output.response.response;
        ui.ctx().accesskit_node_builder(edit_id, |node| node.set_label(options.submit_label));
        response.changed() || is_accepted
    }
}

fn mention_suggestions(context: &PullRequestContext, word: &str) -> Vec<egui::Suggestion> {
    let Some(query) = word.strip_prefix('@') else {
        return Vec::new();
    };
    if query.starts_with('<') {
        return Vec::new();
    }
    context
        .matching(query)
        .into_iter()
        .filter_map(|person| {
            let mention = context.mention(person)?;
            let login = person.login.clone().unwrap_or_default();
            Some(
                egui::Suggestion::new(format!("{mention} "))
                    .content(person.display_name.clone())
                    .description(login),
            )
        })
        .collect()
}
