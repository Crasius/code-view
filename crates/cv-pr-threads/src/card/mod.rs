//! One conversation as a card: a header with the status menu, line range, people and link; the
//! comments separated by hairlines; posts on their way; and the reply box. Resolved (or
//! collapsed) threads shrink to one line. Used inline in the diff and in the overview, where the
//! code the thread is about sits under the header ([`code_context`]).
//!
//! A comment on its way is drawn with the very rows the posted one will have (same widgets, same
//! padding, the reply box under it), only inert and marked "Posting…": when the platform's copy
//! takes its place, nothing around it moves.

pub(super) mod code_context;
mod collapsed;
mod comment;
mod own_comment;
mod reply;
#[cfg(test)]
mod walkthrough;

use cv_base::theme::{self, Palette, icons, motion, style};
use cv_widgets::{buttons, focus, row};
use egui::{
    Align, Color32, CornerRadius, Layout, Margin, Rect, RichText, Sense, Stroke, Ui, pos2, vec2,
};
use review_core::provider::CommentActions;
use review_core::{ReviewThread, ThreadComment, ThreadStatus};

use self::collapsed::collapsed;
pub use self::collapsed::excerpt;
use self::comment::{comment_row, comments};
pub use self::reply::{FailureAction, failure_row};
use self::reply::{ReplyBox, reply_area};
use super::posts::PostState;
use super::suggestion;
use super::{ThreadEvent, ThreadsUi, drafts, links};

/// Width of the status bar along the card's left edge.
const BAR_WIDTH: f32 = 3.0;
const AVATAR_SIZE: f32 = 28.0;
/// Space between an avatar and the text beside it.
const AVATAR_GAP: f32 = 12.0;
const PILL_HEIGHT: f32 = 26.0;
const COLLAPSED_HEIGHT: f32 = 40.0;
const NEW_DOT: f32 = 8.0;
/// How strongly a comment on its way is shown until the platform has it.
const SENDING_OPACITY: f32 = 0.7;
/// Where the time of a comment on its way goes until the platform has it.
pub const POSTING: &str = "Posting…";

pub struct CardOptions {
    /// Show the file and line as a link to the code, and the code itself under the header: the
    /// card is away from the diff (the overview).
    pub show_location: bool,
}

/// The card of `thread`; returns what the user asked for.
pub fn show(
    ui: &mut Ui,
    thread: &ReviewThread,
    threads: &mut ThreadsUi,
    options: &CardOptions,
) -> Option<ThreadEvent> {
    if threads.edits.hides(thread) {
        // Its last comment is deleted: the platform drops the thread, and the next load with it.
        return None;
    }
    let top = ui.cursor().top();
    let is_collapsed = threads.is_collapsed(thread);
    let event = if is_collapsed {
        collapsed(ui, thread, threads)
    } else {
        card(ui, thread, threads, options, Phase::Loaded)
    };
    // Laid out is not seen: a list lays out cards just beyond the view too. A collapsed card shows
    // none of the replies, so its news stays news.
    let rect = Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=ui.cursor().top());
    if !is_collapsed && ui.is_rect_visible(rect) {
        threads.reading.note(thread);
    }
    event
}

/// The card of a new thread on its way (see [`ThreadsUi::pending_thread`]): laid out as the
/// posted thread's card will be, inert, its comment marked "Posting…" until the platform has it.
pub fn show_pending(
    ui: &mut Ui,
    thread: &ReviewThread,
    threads: &mut ThreadsUi,
    options: &CardOptions,
    is_sending: bool,
) {
    let phase = if is_sending { Phase::Sending } else { Phase::Sent };
    card(ui, thread, threads, options, phase);
}

/// Where a card's thread is: loaded from the platform, or a new one on its way.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Loaded,
    /// On its way; the platform does not have it yet.
    Sending,
    /// The platform has it; shown until the reloaded threads carry it.
    Sent,
}

impl Phase {
    fn is_pending(self) -> bool {
        self != Phase::Loaded
    }
}

fn card(
    ui: &mut Ui,
    thread: &ReviewThread,
    threads: &mut ThreadsUi,
    options: &CardOptions,
    phase: Phase,
) -> Option<ThreadEvent> {
    let palette = theme::palette(ui.ctx());
    let mut event = None;
    let new = if phase.is_pending() { None } else { threads.new_comments(thread) };
    let response = style::card_frame(&palette).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        if phase.is_pending() {
            inert(ui, phase == Phase::Sending);
        }
        header(ui, thread, threads, options, &mut event);
        if options.show_location
            && !phase.is_pending()
            && let Some(code_event) = code_context::show(ui, thread, threads)
        {
            event = Some(code_event);
        }
        if let Some(new) = &new {
            hairline(ui, &palette);
            new_banner(ui, thread, threads, new);
        }
        if thread.is_outdated {
            hairline(ui, &palette);
            if outdated_note(ui, thread, threads) {
                event = Some(ThreadEvent::ShowOrigin { thread_id: thread.id.clone() });
            }
        }
        if let Some(comment_event) = comments(ui, thread, threads, new.as_ref(), phase) {
            event = Some(comment_event);
        }
        if phase.is_pending() {
            if thread.can_reply || thread.can_resolve {
                hairline(ui, &palette);
                reply_area(ui, thread, threads, ReplyBox::Inert);
            }
            return;
        }
        let key = drafts::reply_key(&thread.id);
        let reply_state = threads.post_state(&key).cloned();
        if let Some(reply) = threads.pending_reply(&thread.id) {
            // The reply on its way, as the posted one will show: a comment row, then the reply box.
            let is_sending = reply_state == Some(PostState::Sending);
            hairline(ui, &palette);
            ui.scope(|ui| {
                inert(ui, is_sending);
                let note = is_sending.then_some(POSTING);
                comment_row(ui, thread, &reply, threads, false, note, CommentActions::NONE);
            });
            hairline(ui, &palette);
            ui.scope(|ui| {
                inert(ui, false);
                reply_area(ui, thread, threads, ReplyBox::Inert);
            });
        } else if thread.can_reply || thread.can_resolve {
            hairline(ui, &palette);
            if let Some(reply) = reply_area(ui, thread, threads, ReplyBox::Live) {
                event = Some(reply);
            }
        }
    });
    let bar = if thread.is_outdated { palette.warning } else { palette.accent };
    paint_bar(ui, response.response.rect, bar);
    event
}

/// Takes clicks and focus away from what follows in `ui`, which stays readable: dimmed while
/// `is_sending`, else at full strength.
fn inert(ui: &mut Ui, is_sending: bool) {
    let opacity = ui.opacity();
    ui.disable();
    ui.set_opacity(if is_sending { opacity * SENDING_OPACITY } else { opacity });
}

/// The colored edge that tells active (accent) and outdated (warning) cards apart at a glance.
fn paint_bar(ui: &Ui, card: Rect, color: Color32) {
    let radius = style::RADIUS_CARD;
    let bar = Rect::from_min_size(card.min, vec2(BAR_WIDTH, card.height()));
    ui.painter().rect_filled(bar, CornerRadius { nw: radius, sw: radius, ne: 0, se: 0 }, color);
}

/// The thread on the pull request's page, once the page and the platform are known.
fn thread_url(threads: &ThreadsUi, thread: &ReviewThread) -> Option<String> {
    let page = threads.context.web_url.as_deref()?;
    links::thread_url(threads.edits.provider()?, page, thread)
}

fn hairline(ui: &mut Ui, palette: &Palette) {
    let y = ui.cursor().top();
    ui.painter().hline(ui.max_rect().x_range(), y + 0.5, Stroke::new(1.0, palette.border));
    ui.add_space(1.0);
}

fn section<R>(ui: &mut Ui, margin: Margin, content: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(margin)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui)
        })
        .inner
}

fn header(
    ui: &mut Ui,
    thread: &ReviewThread,
    threads: &mut ThreadsUi,
    options: &CardOptions,
    event: &mut Option<ThreadEvent>,
) {
    let palette = theme::palette(ui.ctx());
    let url = thread_url(threads, thread);
    section(ui, Margin { left: 14, right: 8, top: 7, bottom: 7 }, |ui| {
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), buttons::SUBTLE_HEIGHT),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if let Some(status) = status_menu(ui, thread, threads) {
                    *event = Some(ThreadEvent::SetStatus { thread_id: thread.id.clone(), status });
                }
                if thread.is_outdated {
                    outdated_pill(ui, &palette);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if buttons::icon(ui, icons::CARET_UP, "Collapse thread").clicked() {
                        threads.set_collapsed(&thread.id, true);
                    }
                    if let Some(url) = &url
                        && buttons::icon(ui, icons::LINK, "Copy link to thread").clicked()
                    {
                        ui.ctx().copy_text(url.clone());
                    }
                    ui.add_space(style::SPACE_S);
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        // The icon buttons' 2 points would run the path into its meta.
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if location_and_meta(ui, thread, options) {
                            *event = Some(ThreadEvent::Reveal { thread_id: thread.id.clone() });
                        }
                    });
                });
            },
        );
    });
}

/// The thread's file and line as a link (where `options` shows it), then its meta; true when the
/// link was clicked.
fn location_and_meta(ui: &mut Ui, thread: &ReviewThread, options: &CardOptions) -> bool {
    let palette = theme::palette(ui.ctx());
    let meta = meta_text(thread, options.show_location);
    let font = egui::FontId::proportional(style::SMALL_SIZE);
    let mut is_clicked = false;
    if options.show_location
        && let Some(anchor) = &thread.anchor
    {
        let line =
            anchor.new_line.or(anchor.old_line).map(|line| format!(":{line}")).unwrap_or_default();
        let location = format!("{}{line}", anchor.path);
        // A long path is cut (the meta after it keeps its room) and reads in full on hover.
        let meta_width = ui.fonts_mut(|fonts| {
            fonts.layout_no_wrap(meta.clone(), font.clone(), palette.text_secondary).size().x
        });
        let link = ui
            .scope(|ui| {
                let room = ui.available_width() - meta_width - ui.spacing().item_spacing.x;
                ui.set_max_width(room.max(0.0));
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                ui.link(RichText::new(&location).font(font.clone()))
            })
            .inner;
        is_clicked = link.on_hover_text(format!("Show in the file: {location}")).clicked();
    }
    ui.add(
        egui::Label::new(RichText::new(meta).font(font).color(palette.text_secondary)).truncate(),
    );
    is_clicked
}

/// "Lines 84–87 · 2 comments · 2 people"; the overview leaves the lines to the location link.
fn meta_text(thread: &ReviewThread, has_location: bool) -> String {
    let mut parts = Vec::with_capacity(3);
    if !has_location && let Some(anchor) = &thread.anchor {
        parts.push(suggestion::lines_title(anchor));
    }
    let comments = thread.comments.len();
    parts.push(if comments == 1 { "1 comment".to_owned() } else { format!("{comments} comments") });
    let mut people: Vec<&str> =
        thread.comments.iter().map(|comment| comment.author.id.as_str()).collect();
    people.sort_unstable();
    people.dedup();
    if people.len() > 1 {
        parts.push(format!("{} people", people.len()));
    }
    parts.join(" · ")
}

/// The status as a pill. Where the user may change it, the pill opens a menu with the states
/// every platform supports: Active and Resolved.
fn status_menu(ui: &mut Ui, thread: &ReviewThread, threads: &ThreadsUi) -> Option<ThreadStatus> {
    let palette = theme::palette(ui.ctx());
    let (label, fill, text) = status_look(thread.status, &palette);
    let can_change = threads.context.can_write && thread.can_resolve;
    let font = theme::fonts::semibold_font(style::SMALL_SIZE);
    let galley = ui.painter().layout_no_wrap(format!("●  {label}"), font, text);
    let caret = if can_change { icons::SIZE_SMALL + 4.0 } else { 0.0 };
    let size = vec2(galley.size().x + 20.0 + caret, PILL_HEIGHT);
    let sense = if can_change { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let name = format!("Status: {label}");
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            if can_change { egui::Role::ComboBox } else { egui::Role::Label },
            true,
            &name,
        )
    });
    let hover = if can_change { row::hover_amount(ui, &response) } else { 0.0 };
    ui.painter().rect_filled(
        rect,
        PILL_HEIGHT / 2.0,
        motion::mix(fill, palette.hover, hover * 0.4),
    );
    let text_pos = pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(text_pos, galley, text);
    if can_change {
        let caret_center =
            pos2(rect.right() - 10.0 - icons::SIZE_SMALL / 2.0 + 2.0, rect.center().y);
        ui.painter().text(
            caret_center,
            egui::Align2::CENTER_CENTER,
            icons::CARET_DOWN,
            icons::rest_font(&palette, icons::SIZE_SMALL),
            text,
        );
    }
    focus::ring(ui, &response, rect);
    if !can_change {
        return None;
    }
    let mut picked = None;
    egui::Popup::menu(&response).show(|ui| {
        ui.set_min_width(180.0);
        for status in [ThreadStatus::Active, ThreadStatus::Resolved] {
            let (label, _, _) = status_look(status, &palette);
            let icon = if status == thread.status { icons::CHECK } else { "" };
            if buttons::menu_item(ui, icon, label, None, None).clicked() && status != thread.status
            {
                picked = Some(status);
            }
        }
    });
    picked
}

fn status_look(status: ThreadStatus, palette: &Palette) -> (&'static str, Color32, Color32) {
    match status {
        ThreadStatus::Active => ("Active", palette.selection, palette.accent_text),
        ThreadStatus::Resolved => (
            "Resolved",
            palette.success.gamma_multiply(if palette.is_dark { 0.22 } else { 0.12 }),
            palette.success,
        ),
    }
}

fn outdated_pill(ui: &mut Ui, palette: &Palette) {
    let response =
        cv_widgets::badges::pill(ui, "Outdated", palette.warning_surface, palette.warning_text);
    response.on_hover_text("The code changed after this comment was written");
}

/// New comments in a thread: from other people, newer than what the user has seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewComments {
    /// Index of the first new comment.
    pub first: usize,
    pub count: usize,
    /// First names of their authors, each once, in order.
    pub authors: Vec<String>,
}

impl NewComments {
    /// The comments of `thread` that `is_new` tells; `None` when there are none.
    pub fn of(thread: &ReviewThread, is_new: impl Fn(&ThreadComment) -> bool) -> Option<Self> {
        let first = thread.comments.iter().position(&is_new)?;
        let mut authors: Vec<String> = Vec::new();
        let mut count = 0;
        for comment in thread.comments[first..].iter().filter(|comment| is_new(comment)) {
            count += 1;
            let name = first_name(&comment.author.display_name);
            if !authors.iter().any(|known| known == name) {
                authors.push(name.to_owned());
            }
        }
        Some(Self { first, count, authors })
    }

    /// The whole thread is new (its first comment is new).
    pub fn is_new_thread(&self) -> bool {
        self.first == 0
    }
}

fn first_name(display_name: &str) -> &str {
    display_name.split_whitespace().next().unwrap_or(display_name)
}

/// "● 2 new replies from Jonas, Mara · Jump to first new".
fn new_banner(ui: &mut Ui, thread: &ReviewThread, threads: &mut ThreadsUi, new: &NewComments) {
    let palette = theme::palette(ui.ctx());
    section(ui, Margin::symmetric(14, 10), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (clicked, ()) = row::with_trailing(
                ui,
                Align::Center,
                |ui| {
                    let label =
                        if new.is_new_thread() { "Mark as read" } else { "Jump to first new" };
                    ui.link(RichText::new(label).size(style::SMALL_SIZE + 1.0)).clicked()
                },
                |ui| {
                    let (dot, _) = ui.allocate_exact_size(vec2(NEW_DOT, NEW_DOT), Sense::hover());
                    ui.painter().circle_filled(dot.center(), NEW_DOT / 2.0, palette.accent);
                    let title = match (new.is_new_thread(), new.count) {
                        (true, _) => "New thread".to_owned(),
                        (false, 1) => "1 new reply".to_owned(),
                        (false, count) => format!("{count} new replies"),
                    };
                    ui.label(
                        RichText::new(title).font(theme::fonts::semibold_font(style::BODY_SIZE)),
                    );
                    let authors = RichText::new(format!("from {}", new.authors.join(", ")))
                        .size(style::SMALL_SIZE + 1.0)
                        .color(palette.text_secondary);
                    ui.add(egui::Label::new(authors).truncate());
                },
            );
            if clicked {
                if !new.is_new_thread() {
                    threads.jump_to_new = Some((thread.id.clone(), new.first));
                }
                threads.mark_read(thread);
            }
        });
    });
}

/// Where an outdated thread's original code is: in the app when the platform said which version it
/// was written on, and on the platform's page. Returns true when "Show original version" was clicked.
fn outdated_note(ui: &mut Ui, thread: &ReviewThread, threads: &ThreadsUi) -> bool {
    let palette = theme::palette(ui.ctx());
    let line =
        thread.anchor.as_ref().map_or_else(|| "its line".to_owned(), suggestion::lines_label);
    let url = thread_url(threads, thread);
    let mut show_origin = false;
    section(ui, Margin::symmetric(14, 10), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let text = format!("Written on an earlier version: {line} changed since.");
            ui.label(
                RichText::new(text).size(style::SMALL_SIZE + 1.0).color(palette.text_secondary),
            );
            if thread.origin.is_some() {
                let link = ui.link(RichText::new(SHOW_ORIGIN).size(style::SMALL_SIZE + 1.0));
                show_origin = link.on_hover_text(ORIGIN_HINT).clicked();
            }
            if let Some(url) = url {
                let label = if thread.origin.is_some() {
                    format!("Open on {}", threads.context.platform.label())
                } else {
                    format!("See the original code on {}", threads.context.platform.label())
                };
                if ui.link(RichText::new(label).size(style::SMALL_SIZE + 1.0)).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            }
        });
    });
    show_origin
}

/// The action that opens the version an outdated thread was written on, wherever it is offered.
pub const SHOW_ORIGIN: &str = "Show original version";
const ORIGIN_HINT: &str =
    "The file as it was when the comment was written, with the comment at its line";

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use review_core::{ProviderKind, ThreadStatus};

    use super::super::samples::{self, comment, jonas, minutes_ago};
    use super::super::{CardOptions, ThreadsUi, links};
    use super::show;

    #[test]
    fn offers_the_thread_link_once_the_provider_is_known() {
        let thread = samples::thread(
            "issue-comment:5",
            ThreadStatus::Active,
            None,
            vec![comment("5", &jonas(), 3, "Ship it.")],
        );
        for has_provider in [false, true] {
            let mut threads = ThreadsUi::for_tests(ProviderKind::GitHub);
            threads.context.web_url = Some("https://code.example/acme/shop/pull/12".into());
            if has_provider {
                threads.edits.set_provider(&links::tests::github());
            }
            let shown = thread.clone();
            let mut harness = Harness::builder()
                .with_size([900.0, 600.0])
                .with_context_setup(cv_base::theme::fonts::install)
                .build_ui_state(
                    move |ui, threads: &mut ThreadsUi| {
                        cv_base::theme::activate(ui.ctx(), review_store::ThemePreference::Light);
                        show(ui, &shown, threads, &CardOptions { show_location: false });
                    },
                    threads,
                );
            harness.run_steps(2);
            let offered = harness.query_by_label("Copy link to thread").is_some();
            assert_eq!(offered, has_provider, "with a provider: {has_provider}");
        }
    }

    /// Frames at `times`, each started as the page starts it (read tracking first), then showing
    /// `thread`'s card or nothing.
    fn frames(threads: &mut ThreadsUi, thread: &review_core::ReviewThread, times: &[(f64, bool)]) {
        let ctx = egui::Context::default();
        cv_base::theme::fonts::install(&ctx);
        for &(time, is_shown) in times {
            let input = egui::RawInput { time: Some(time), ..egui::RawInput::default() };
            ctx.run_ui(input, |ui| {
                cv_base::theme::activate(ui.ctx(), review_store::ThemePreference::Light);
                threads.begin_frame_for_tests(ui.ctx(), std::slice::from_ref(thread), 1);
                if is_shown {
                    show(ui, thread, threads, &CardOptions { show_location: false });
                }
            })
            .drop_without_applying_deltas();
        }
    }

    #[test]
    fn a_collapsed_card_does_not_read_the_replies_it_hides() {
        let mut thread = samples::resolved("t-resolved", None);
        thread.comments.push(comment("t-resolved-4", &jonas(), 2, "Reopening: this broke CI."));
        let mut threads = ThreadsUi::for_tests(ProviderKind::GitHub);
        threads.set_me(samples::erika());
        threads.visited_at(minutes_ago(30).unix_timestamp());
        assert!(threads.is_collapsed(&thread) && threads.is_unread(&thread));
        frames(&mut threads, &thread, &[(0.0, true), (2.0, true), (3.0, false)]);
        assert!(threads.is_unread(&thread), "the new reply was never on screen");

        threads.set_collapsed(&thread.id, false);
        frames(&mut threads, &thread, &[(4.0, true), (6.0, true), (7.0, false), (8.0, false)]);
        assert!(!threads.is_unread(&thread), "the open card showed it long enough");
    }
}
