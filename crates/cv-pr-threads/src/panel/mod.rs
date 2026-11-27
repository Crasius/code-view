//! All threads of the pull request beside the diff: filter chips (Active, Unread, Mine, Resolved),
//! threads grouped by file with "General" for pull request comments, and a footer to start a
//! general comment or step to the next unread thread. ↑↓ or N/P step through the list once the
//! panel was clicked or opened by Alt+T (which also selects the first unread, else the first
//! active thread); every step shows the thread in its file. R opens the selected thread's reply
//! box, Shift+R resolves or reopens it, Esc hands the keys back to the diff.
//!
//! Only the rows near the view are laid out ([`lazy_list`]), and the selected row wherever it
//! is while it scrolls into view: a review with hundreds of threads scrolls at the cost of a few.

#[cfg(test)]
mod frame_budget;
mod list_row;
#[cfg(test)]
mod listed_tests;
#[cfg(test)]
mod walkthrough;

use std::collections::HashMap;

use cv_base::platform::shortcut;
use cv_base::theme::{self, icons, style};
use cv_editor::markdown_editor::{self, Outcome};
use cv_pr_session::session::PullRequestSession;
use cv_widgets::{buttons, focus, row};
use egui::{Align, Key, Layout, Margin, Modifiers, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use review_core::{ReviewThread, ThreadStatus};
use review_store::ThreadFilters;

use self::list_row::thread_row;
use super::posts::PostState;
use super::{GENERAL_KEY, ThreadEvent, ThreadsUi, lazy_list};

/// The panel's toggle, in the tab bar and in focus mode's strip.
pub const SHOW_HINT: &str = concat!("Show all threads (", shortcut!(alt "T"), ")");
pub const HIDE_HINT: &str = concat!("Hide all threads (", shortcut!(alt "T"), ")");

const MARGIN: i8 = 16;
const ROW_AVATAR: f32 = 24.0;
const CHIP_HEIGHT: f32 = 26.0;
const GENERAL: &str = "General";

#[derive(Default)]
pub struct PanelState {
    pub selected: Option<String>,
    /// Who the arrow keys, N/P, R and Shift+R go to.
    keys: Keys,
    pub(super) is_writing_general: bool,
    /// Scroll the selected row into view next frame.
    scroll_to_selected: bool,
    /// The heights of the list's rows by thread, and of its group headers by group, for laying out
    /// only those near the view.
    heights: lazy_list::Heights,
    group_heights: lazy_list::Heights,
    /// Each changed file's place in tree order, by path, read for `file_order_version`.
    file_ranks: HashMap<String, usize>,
    file_ranks_version: Option<u64>,
}

/// Who has the panel's keys.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Keys {
    /// The diff, until the panel is clicked or opened by Alt+T.
    #[default]
    Diff,
    /// The panel: it was clicked last.
    Panel,
    /// The panel, opened by Alt+T: the first unread (else the first active) thread gets selected
    /// once the threads are there.
    PanelPicksFirst,
}

impl PanelState {
    /// The panel opened by its key: it takes the keys and selects a thread to start from.
    pub fn take_keys(&mut self) {
        self.keys = Keys::PanelPicksFirst;
    }

    /// The panel closed: the keys go back to the diff.
    pub fn release_keys(&mut self) {
        self.keys = Keys::Diff;
    }
}

pub enum PanelEvent {
    /// Show the thread in its file.
    Reveal {
        path: String,
        thread_id: String,
    },
    /// Show the version of the file an outdated thread was written on.
    ShowOrigin {
        thread_id: String,
    },
    Close,
    /// Filters changed: save the settings.
    FiltersChanged,
    /// A reply or status change on a general thread shown in the panel.
    Thread(ThreadEvent),
    PostGeneral,
}

pub struct PanelInputs<'a> {
    pub threads: &'a [ReviewThread],
    /// Changed files in tree order, for the order of the groups. The panel reads it only when
    /// `file_order_version` changed, so the order costs nothing per frame.
    pub file_order: &'a mut dyn Iterator<Item = &'a str>,
    /// Changes whenever `file_order` lists other files or another order.
    pub file_order_version: u64,
    pub filters: &'a mut ThreadFilters,
    pub width: f32,
}

/// One row of the list: a thread and the group it is in.
struct Entry<'a> {
    thread: &'a ReviewThread,
    group: &'a str,
    /// A new thread on its way: listed as it will be, without actions.
    is_pending: bool,
}

/// Lays the panel out on the right of `ui`; returns what the user did and the panel's rect (for
/// the splitter on its left edge).
pub fn show(
    ui: &mut Ui,
    threads: &mut ThreadsUi,
    mut inputs: PanelInputs<'_>,
) -> (Vec<PanelEvent>, Rect) {
    let palette = theme::palette(ui.ctx());
    let mut events = Vec::new();
    // The Mine chip and filter need the user.
    threads.request_me();
    // New threads on their way are listed where they will land, marked "Posting…".
    let pending: Vec<ReviewThread> =
        threads.pending_threads().into_iter().map(|(_, thread)| thread).collect();
    read_file_ranks(&mut threads.panel, &mut inputs);
    let panel = egui::Panel::right("threads_panel")
        .resizable(false)
        .exact_size(inputs.width)
        .frame(egui::Frame::new().fill(palette.surface).stroke(Stroke::new(1.0, palette.border)))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let visible = visible_entries(threads, inputs.threads, *inputs.filters, &pending);
            header(ui, threads, &mut inputs, &mut events);
            keyboard(ui, threads, &visible, &mut events);
            egui::Panel::bottom("threads_panel_footer")
                .resizable(false)
                .frame(
                    egui::Frame::new()
                        .inner_margin(Margin::symmetric(MARGIN, 10))
                        .stroke(Stroke::new(1.0, palette.border)),
                )
                .show(ui, |ui| footer(ui, threads, &visible, &mut events));
            list(ui, threads, &visible, &mut inputs, &mut events);
        });
    let rect = panel.response.rect;
    let pressed_at =
        ui.input(|input| input.pointer.press_origin().filter(|_| input.pointer.any_pressed()));
    if let Some(origin) = pressed_at {
        threads.panel.keys = if rect.contains(origin) { Keys::Panel } else { Keys::Diff };
    }
    (events, rect)
}

/// The tab bar's "Threads" toggle (Alt+T): the number of active threads and a dot while any
/// thread has news. Returns true when clicked.
pub(crate) fn toggle(
    ui: &mut Ui,
    all: &[ReviewThread],
    threads: &ThreadsUi,
    is_open: bool,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let counts = toggle_counts(all, threads);
    let (active, has_news) = (counts.active, counts.unread > 0);
    let label = format!("Threads {active}");
    let font = egui::FontId::proportional(style::BODY_SIZE);
    let color = if is_open { palette.accent_text } else { palette.text };
    let galley = ui.painter().layout_no_wrap(label.clone(), font, color);
    let dot = if has_news { 12.0 } else { 0.0 };
    let size = vec2(
        style::SPACE_M * 2.0 + icons::SIZE + 6.0 + galley.size().x + dot,
        buttons::SUBTLE_HEIGHT,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let news = if has_news { ", new replies" } else { "" };
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, true, is_open, format!("{label}{news}"))
    });
    let rest = if is_open { palette.selection } else { egui::Color32::TRANSPARENT };
    ui.painter().rect_filled(rect, style::RADIUS_CONTROL, row::state_fill(ui, &response, rest));
    let icon_center = pos2(rect.left() + style::SPACE_M + icons::SIZE / 2.0, rect.center().y);
    ui.painter().text(
        icon_center,
        egui::Align2::CENTER_CENTER,
        icons::CHAT_CIRCLE_TEXT,
        icons::state_font(&palette, icons::SIZE, is_open),
        color,
    );
    let text_left = icon_center.x + icons::SIZE / 2.0 + 6.0;
    let text_right = text_left + galley.size().x;
    ui.painter().galley(pos2(text_left, rect.center().y - galley.size().y / 2.0), galley, color);
    if has_news {
        ui.painter().circle_filled(
            pos2(text_right + 8.0, rect.center().y - 5.0),
            3.5,
            palette.accent,
        );
    }
    focus::ring(ui, &response, rect);
    let tooltip = if is_open { HIDE_HINT } else { SHOW_HINT };
    response.on_hover_text(tooltip).clicked()
}

/// `toggle` for the threads `session` loaded: the tab bar's "Threads" toggle; true when clicked.
pub fn panel_toggle(
    ui: &mut Ui,
    session: &PullRequestSession,
    threads: &ThreadsUi,
    is_open: bool,
) -> bool {
    let all = session.threads.ready().map_or(&[][..], Vec::as_slice);
    toggle(ui, all, threads, is_open)
}

fn group_of(thread: &ReviewThread) -> &str {
    thread.anchor.as_ref().map_or(GENERAL, |anchor| anchor.path.as_str())
}

fn is_mine(thread: &ReviewThread, me: Option<&str>) -> bool {
    me.is_some_and(|me| thread.comments.iter().any(|comment| comment.author.id == me))
}

/// Whether the panel lists `thread` under `filters`: never a system notice or a thread without
/// comments; with no status filter on, both statuses.
pub(crate) fn is_listed(
    thread: &ReviewThread,
    filters: ThreadFilters,
    threads: &ThreadsUi,
) -> bool {
    let any_status = !filters.active && !filters.resolved;
    let shows_status = match thread.status {
        ThreadStatus::Active => filters.active || any_status,
        ThreadStatus::Resolved => filters.resolved || any_status,
    };
    let me = threads.known_me().map(|me| me.id.as_str());
    !thread.is_system
        && !thread.comments.is_empty()
        && shows_status
        && (!filters.unread || threads.is_unread(thread))
        && (!filters.mine || is_mine(thread, me))
}

/// The panel's threads per filter toggle; `mine` is `None` while the user is not known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToggleCounts {
    pub active: usize,
    pub resolved: usize,
    pub unread: usize,
    pub mine: Option<usize>,
}

/// Per toggle, how many of the threads the panel lists it would add.
pub fn toggle_counts(all: &[ReviewThread], threads: &ThreadsUi) -> ToggleCounts {
    let me = threads.known_me().map(|me| me.id.as_str());
    let mut counts = ToggleCounts { active: 0, resolved: 0, unread: 0, mine: me.map(|_| 0) };
    for thread in all.iter().filter(|thread| is_listed(thread, ThreadFilters::ALL, threads)) {
        match thread.status {
            ThreadStatus::Active => counts.active += 1,
            ThreadStatus::Resolved => counts.resolved += 1,
        }
        counts.unread += usize::from(threads.is_unread(thread));
        if let Some(mine) = counts.mine.as_mut() {
            *mine += usize::from(is_mine(thread, me));
        }
    }
    counts
}

/// Reads the files' places in tree order when the order changed; a path listed twice keeps its
/// first place.
fn read_file_ranks(panel: &mut PanelState, inputs: &mut PanelInputs<'_>) {
    if panel.file_ranks_version == Some(inputs.file_order_version) {
        return;
    }
    panel.file_ranks.clear();
    for (index, path) in (&mut *inputs.file_order).enumerate() {
        panel.file_ranks.entry(path.to_owned()).or_insert(index);
    }
    panel.file_ranks_version = Some(inputs.file_order_version);
}

/// The `loaded` threads `filters` let through, grouped by file in tree order, General last;
/// `pending` (new threads on their way, the user's own and active) after the loaded ones of their
/// place.
fn visible_entries<'a>(
    threads: &ThreadsUi,
    loaded: &'a [ReviewThread],
    filters: ThreadFilters,
    pending: &'a [ReviewThread],
) -> Vec<Entry<'a>> {
    let any_status = !filters.active && !filters.resolved;
    let loaded = loaded
        .iter()
        .filter(|thread| is_listed(thread, filters, threads))
        .map(|thread| Entry { thread, group: group_of(thread), is_pending: false });
    let shows_pending = (filters.active || any_status) && !filters.unread;
    let pending = pending.iter().filter(|_| shows_pending).map(|thread| Entry {
        thread,
        group: group_of(thread),
        is_pending: true,
    });
    let mut entries: Vec<Entry<'a>> = loaded.chain(pending).collect();
    // Groups outside `file_order` go after those in it, by name; General last.
    let rank = |group: &'a str| {
        if group == GENERAL {
            return (usize::MAX, "");
        }
        (threads.panel.file_ranks.get(group).copied().unwrap_or(usize::MAX - 1), group)
    };
    let line = |thread: &ReviewThread| {
        thread.anchor.as_ref().and_then(|anchor| anchor.new_line.or(anchor.old_line)).unwrap_or(0)
    };
    entries.sort_by_key(|entry| (rank(entry.group), line(entry.thread)));
    entries
}

fn header(
    ui: &mut Ui,
    threads: &ThreadsUi,
    inputs: &mut PanelInputs<'_>,
    events: &mut Vec<PanelEvent>,
) {
    let palette = theme::palette(ui.ctx());
    egui::Frame::new().inner_margin(Margin { left: MARGIN, right: 10, top: 12, bottom: 12 }).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Threads").font(theme::fonts::semibold_font(style::BODY_SIZE)),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if buttons::icon(
                        ui,
                        icons::X,
                        concat!("Close threads panel (", shortcut!(alt "T"), ")"),
                    )
                    .clicked()
                    {
                        events.push(PanelEvent::Close);
                    }
                    ui.add(
                        egui::Label::new(
                            RichText::new("↑↓ or N/P to step")
                                .size(style::SMALL_SIZE)
                                .color(palette.text_secondary),
                        )
                        .truncate(),
                    );
                });
            });
            let counts = toggle_counts(inputs.threads, threads);
            let filters = &mut *inputs.filters;
            let mut changed = false;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                changed |= chip(ui, "Active", Some(counts.active), &mut filters.active);
                changed |= chip(ui, "Unread", Some(counts.unread), &mut filters.unread);
                changed |= chip(ui, "Mine", counts.mine, &mut filters.mine);
                changed |= chip(ui, "Resolved", Some(counts.resolved), &mut filters.resolved);
            });
            if changed {
                events.push(PanelEvent::FiltersChanged);
            }
        },
    );
    let y = ui.cursor().top();
    ui.painter().hline(ui.max_rect().x_range(), y + 0.5, Stroke::new(1.0, palette.border));
    ui.add_space(1.0);
}

/// A filter chip: tinted while on, outlined while off. Reported as a toggle button.
fn chip(ui: &mut Ui, label: &str, count: Option<usize>, is_on: &mut bool) -> bool {
    let palette = theme::palette(ui.ctx());
    let text = match count {
        Some(count) => format!("{label} {count}"),
        None => label.to_owned(),
    };
    let color = if *is_on { palette.accent_text } else { palette.text };
    let galley = ui.painter().layout_no_wrap(
        text.clone(),
        egui::FontId::proportional(style::SMALL_SIZE),
        color,
    );
    let (rect, response) =
        ui.allocate_exact_size(vec2(galley.size().x + 20.0, CHIP_HEIGHT), Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, *is_on, &text));
    let rest = if *is_on { palette.selection } else { egui::Color32::TRANSPARENT };
    let fill = row::state_fill(ui, &response, rest);
    let stroke = if *is_on { Stroke::NONE } else { Stroke::new(1.0, palette.border_strong) };
    ui.painter().rect(rect, CHIP_HEIGHT / 2.0, fill, stroke, egui::StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    focus::ring(ui, &response, rect);
    if response.clicked() {
        *is_on = !*is_on;
        return true;
    }
    false
}

/// While the panel is active and no text field has focus: ↑↓ or N/P step (each step shows the
/// thread), R opens the selected thread's reply box, Shift+R resolves or reopens it, Esc gives the
/// keys back to the diff.
fn keyboard(ui: &Ui, threads: &mut ThreadsUi, visible: &[Entry<'_>], events: &mut Vec<PanelEvent>) {
    // Threads on their way are not there to step to yet.
    let visible: Vec<&Entry<'_>> = visible.iter().filter(|entry| !entry.is_pending).collect();
    if threads.panel.keys == Keys::Diff || ui.ctx().text_edit_focused() {
        return;
    }
    // Once the threads are there to pick from.
    if threads.panel.keys == Keys::PanelPicksFirst && !visible.is_empty() {
        threads.panel.keys = Keys::Panel;
        if let Some(first) = first_to_read(threads, &visible) {
            select(threads, first, events);
            threads.panel.scroll_to_selected = true;
        }
    }
    // Only the bare keys (and Shift+R): Alt+N and the like stay with the diff.
    let (down, up, reply, resolve, release) = ui.input_mut(|input| {
        let is_bare = input.modifiers.is_none();
        let mut bare = |key| is_bare && input.consume_key(Modifiers::NONE, key);
        let down = bare(Key::ArrowDown) || bare(Key::N);
        let up = bare(Key::ArrowUp) || bare(Key::P);
        let reply = bare(Key::R);
        let release = bare(Key::Escape);
        let resolve =
            input.modifiers == Modifiers::SHIFT && input.consume_key(Modifiers::SHIFT, Key::R);
        // The reply box takes the keyboard this frame: the key's letter is not its first text.
        if reply || resolve {
            input.events.retain(
                |event| !matches!(event, egui::Event::Text(text) if text.eq_ignore_ascii_case("r")),
            );
        }
        (down, up, reply, resolve, release)
    });
    if release {
        threads.panel.release_keys();
        return;
    }
    let current = threads
        .panel
        .selected
        .as_ref()
        .and_then(|id| visible.iter().position(|entry| &entry.thread.id == id));
    if let Some(thread) = current.map(|index| visible[index].thread) {
        if reply && threads.context.can_write && thread.can_reply {
            threads.set_collapsed(&thread.id, false);
            threads.open_reply(&thread.id);
        }
        if resolve && threads.context.can_write && thread.can_resolve {
            let status = match thread.status {
                ThreadStatus::Active => ThreadStatus::Resolved,
                ThreadStatus::Resolved => ThreadStatus::Active,
            };
            let thread_id = thread.id.clone();
            events.push(PanelEvent::Thread(ThreadEvent::SetStatus { thread_id, status }));
        }
    }
    if visible.is_empty() {
        return;
    }
    let next = match (down, up, current) {
        (true, _, Some(index)) => Some((index + 1).min(visible.len() - 1)),
        (_, true, Some(index)) => Some(index.saturating_sub(1)),
        (true, _, None) => Some(0),
        (_, true, None) => Some(visible.len() - 1),
        _ => None,
    };
    if let Some(index) = next.filter(|index| Some(*index) != current) {
        select(threads, visible[index].thread, events);
        threads.panel.scroll_to_selected = true;
    }
}

/// The thread the panel starts from when Alt+T opens it: the first unread, else the first active.
fn first_to_read<'a>(threads: &ThreadsUi, visible: &[&Entry<'a>]) -> Option<&'a ReviewThread> {
    let unread = visible.iter().find(|entry| threads.is_unread(entry.thread));
    let active = || visible.iter().find(|entry| entry.thread.status == ThreadStatus::Active);
    unread.or_else(active).map(|entry| entry.thread)
}

/// Selects a thread and shows it: in its file, or (a general thread) opened in the panel.
fn select(threads: &mut ThreadsUi, thread: &ReviewThread, events: &mut Vec<PanelEvent>) {
    threads.panel.selected = Some(thread.id.clone());
    if let Some(anchor) = &thread.anchor {
        threads.set_collapsed(&thread.id, false);
        events.push(PanelEvent::Reveal { path: anchor.path.clone(), thread_id: thread.id.clone() });
    }
}

fn list(
    ui: &mut Ui,
    threads: &mut ThreadsUi,
    visible: &[Entry<'_>],
    inputs: &mut PanelInputs<'_>,
    events: &mut Vec<PanelEvent>,
) {
    if visible.is_empty() {
        empty_list(ui, threads, inputs, events);
        return;
    }
    // Only the rows near the view are laid out; the selected one also when it scrolls into view.
    let mut heights = std::mem::take(&mut threads.panel.heights);
    let mut group_heights = std::mem::take(&mut threads.panel.group_heights);
    egui::ScrollArea::vertical().id_salt("threads_panel_list").auto_shrink(false).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut index = 0;
        while index < visible.len() {
            let group = visible[index].group;
            let count = visible[index..].iter().take_while(|entry| entry.group == group).count();
            lazy_list::block(ui, &mut group_heights, group, false, |ui| {
                group_header(ui, group, count);
            });
            for entry in &visible[index..index + count] {
                let thread = entry.thread;
                let is_selected = threads.panel.selected.as_ref() == Some(&thread.id);
                let must_show = is_selected && threads.panel.scroll_to_selected;
                lazy_list::block(ui, &mut heights, &thread.id, must_show, |ui| {
                    thread_row(ui, threads, thread, entry.is_pending, events);
                });
            }
            index += count;
        }
        ui.add_space(style::SPACE_S);
    });
    threads.panel.heights = heights;
    threads.panel.group_heights = group_heights;
}

const NO_MATCH: &str = "No thread matches these filters.";
const SHOW_ALL: &str = "Show all threads";
const NO_COMMENTS: &str = "No comments yet. Select lines in the diff to start a thread, or leave a \
                           general comment below.";

/// What the list says when it lists nothing: no thread passes the filters (with the way back to
/// all of them), or the pull request has none yet.
fn empty_list(
    ui: &mut Ui,
    threads: &ThreadsUi,
    inputs: &mut PanelInputs<'_>,
    events: &mut Vec<PanelEvent>,
) {
    let palette = theme::palette(ui.ctx());
    egui::Frame::new().inner_margin(Margin::same(MARGIN)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 8.0;
        let has_any =
            inputs.threads.iter().any(|thread| is_listed(thread, ThreadFilters::ALL, threads));
        if has_any {
            ui.label(RichText::new(NO_MATCH).color(palette.text_secondary));
            if ui.link(SHOW_ALL).clicked() {
                *inputs.filters = ThreadFilters::ALL;
                events.push(PanelEvent::FiltersChanged);
            }
        } else {
            ui.label(RichText::new(NO_COMMENTS).color(palette.text_secondary));
        }
    });
}

fn group_header(ui: &mut Ui, group: &str, count: usize) {
    let palette = theme::palette(ui.ctx());
    egui::Frame::new()
        .inner_margin(Margin { left: MARGIN, right: MARGIN, top: 18, bottom: 8 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let font = theme::fonts::semibold_font(style::SMALL_SIZE);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(count.to_string())
                            .font(font.clone())
                            .color(palette.text_secondary),
                    );
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(group).font(font).color(palette.text_secondary),
                            )
                            .truncate(),
                        )
                        .on_hover_text(group);
                    });
                });
            });
        });
}

fn footer(
    ui: &mut Ui,
    threads: &mut ThreadsUi,
    visible: &[Entry<'_>],
    events: &mut Vec<PanelEvent>,
) {
    let palette = theme::palette(ui.ctx());
    ui.set_width(ui.available_width());
    // Posting hands the general comment on to the list: the composer folds back into the footer
    // at once, and opens again with the text if the post fails.
    let is_posting_general = threads.post_state(GENERAL_KEY).is_some_and(PostState::is_pending);
    if threads.panel.is_writing_general && !is_posting_general {
        general_composer(ui, threads, events);
        return;
    }
    let visible: Vec<&Entry<'_>> = visible.iter().filter(|entry| !entry.is_pending).collect();
    ui.horizontal(|ui| {
        if threads.context.can_write
            && buttons::secondary_icon(ui, icons::CHAT_CIRCLE_TEXT, "General comment").clicked()
        {
            threads.panel.is_writing_general = true;
            threads.focus = Some(GENERAL_KEY.to_owned());
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let current = threads
                .panel
                .selected
                .as_ref()
                .and_then(|id| visible.iter().position(|entry| &entry.thread.id == id));
            let start = current.map_or(0, |index| index + 1);
            let next = (0..visible.len())
                .map(|offset| (start + offset) % visible.len())
                .find(|index| threads.is_unread(visible[*index].thread));
            match next {
                Some(index) => {
                    if buttons::subtle(ui, icons::ARROW_DOWN, "Next unread").clicked() {
                        select(threads, visible[index].thread, events);
                        threads.panel.scroll_to_selected = true;
                    }
                }
                None => {
                    ui.label(
                        RichText::new("All read")
                            .size(style::SMALL_SIZE)
                            .color(palette.text_secondary),
                    );
                }
            }
        });
    });
}

/// The pull request level comment, drafted under [`GENERAL_KEY`] like the overview's composer.
fn general_composer(ui: &mut Ui, threads: &mut ThreadsUi, events: &mut Vec<PanelEvent>) {
    let state = threads.post_state(GENERAL_KEY).cloned();
    let grab_focus = threads.focus.as_deref() == Some(GENERAL_KEY);
    if grab_focus {
        threads.focus = None;
    }
    let has_text = threads.has_draft(GENERAL_KEY);
    let options = markdown_editor::Options {
        hint: "Comment on the whole pull request…",
        grab_focus,
        min_rows: 3,
        platform: Some(threads.context.platform),
        footer_hint: Some(if has_text {
            "Draft saved"
        } else {
            concat!(shortcut!(command Enter), " to comment")
        }),
        leading_buttons: true,
        ..markdown_editor::Options::default()
    };
    let id = egui::Id::unique("threads_panel_general");
    let text = threads.text_mut(GENERAL_KEY);
    let output = markdown_editor::show(ui, id, text, &options);
    threads.keep_draft(GENERAL_KEY, None);
    if matches!(output, Some(Outcome::Submitted { .. })) {
        events.push(PanelEvent::PostGeneral);
    } else if output == Some(Outcome::Cancelled) {
        threads.discard_draft(GENERAL_KEY);
        threads.panel.is_writing_general = false;
    }
    if let Some(PostState::Failed { message, needs_token }) = &state {
        ui.add_space(style::SPACE_S);
        threads.failure_actions(ui, GENERAL_KEY, message, *needs_token);
    }
}
