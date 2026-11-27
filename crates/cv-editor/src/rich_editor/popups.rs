//! The editor's popups, anchored under the caret: the mention list an `@` opens (Up and Down
//! choose, Enter or Tab take, Esc closes), the link popover (Ctrl+K, the toolbar's link button)
//! and the bubble a link at the caret shows (edit, open, remove).

use cv_base::theme::{self, icons, style};
use egui::{Context, Id, Key, Order, Rect, RichText, Ui, vec2};
use review_core::ProviderKind;

use super::input::{self, Command, CommandContext, link_at};
use super::model::{Link, Pos};
use super::{Popup, State};
use crate::markdown_editor::{Person, PullRequestContext};

/// Width of the link popover's field.
const LINK_FIELD_WIDTH: f32 = 280.0;
/// Longest query the mention list follows.
const MAX_QUERY: usize = 40;

/// Shows the open popup; returns a command it asks for.
pub(crate) fn show(
    ui: &Ui,
    id: Id,
    state: &mut State,
    caret_rect: Rect,
    context: &CommandContext<'_>,
) -> Option<Command> {
    let ctx = ui.ctx();
    let anchor = caret_rect.left_bottom() + vec2(0.0, style::SPACE_XS);
    match state.popup.clone() {
        Popup::Mention { start, selected } => {
            mention(ctx, id, state, context, start, selected, anchor);
            None
        }
        Popup::Link { url, is_new } => link_popover(ctx, id, state, url, is_new, anchor),
        Popup::None => link_bubble(
            ctx,
            id,
            state,
            anchor,
            ui.memory(|memory| memory.has_focus(super::text_id(id))),
        ),
    }
}

/// The text typed after the `@` at `start`, while the caret is still in it.
fn mention_query(state: &State, start: Pos) -> Option<String> {
    let caret = state.selection.head;
    if !state.selection.is_empty() || caret.block != start.block || caret.offset <= start.offset {
        return None;
    }
    let text: Vec<char> = state.doc.block(start.block).content.text().chars().collect();
    if text.get(start.offset) != Some(&'@') {
        return None;
    }
    let query: String = text.get(start.offset + 1..caret.offset)?.iter().collect();
    let is_word = !query.contains(char::is_whitespace) && query.chars().count() <= MAX_QUERY;
    is_word.then_some(query)
}

/// The key a mention atom stores for `person`: the login, the identity id on Azure DevOps.
fn mention_key(kind: ProviderKind, person: &Person) -> Option<String> {
    match kind {
        ProviderKind::AzureDevOps => (!person.id.is_empty()).then(|| person.id.clone()),
        ProviderKind::GitHub | ProviderKind::GitLab => person.login.clone(),
    }
}

fn mention(
    ctx: &Context,
    id: Id,
    state: &mut State,
    context: &CommandContext<'_>,
    start: Pos,
    selected: usize,
    anchor: egui::Pos2,
) {
    let (Some(query), Some(people)) =
        (mention_query(state, start), PullRequestContext::current(ctx))
    else {
        state.popup = Popup::None;
        return;
    };
    let matches: Vec<(String, Person)> = people
        .matching(&query)
        .into_iter()
        .filter_map(|person| mention_key(people.kind, person).map(|key| (key, person.clone())))
        .collect();
    let selected = selected.min(matches.len().saturating_sub(1));
    let (up, down, take, close) = ctx.input_mut(|input| {
        (
            input.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
            input.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
            input.consume_key(egui::Modifiers::NONE, Key::Enter)
                || input.consume_key(egui::Modifiers::NONE, Key::Tab),
            input.consume_key(egui::Modifiers::NONE, Key::Escape),
        )
    });
    if close {
        state.popup = Popup::None;
        return;
    }
    let count = matches.len().max(1);
    let selected = if up {
        (selected + count - 1) % count
    } else if down {
        (selected + 1) % count
    } else {
        selected
    };
    state.popup = Popup::Mention { start, selected };
    if take {
        match matches.get(selected) {
            Some((key, _)) => input::choose_mention(state, context, start, key.clone()),
            None => state.popup = Popup::None,
        }
        return;
    }
    let palette = theme::palette(ctx);
    let mut chosen = None;
    egui::Area::new(id.with("rich_mentions")).order(Order::Foreground).fixed_pos(anchor).show(
        ctx,
        |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(220.0);
                if matches.is_empty() {
                    ui.label(
                        RichText::new(format!("No one matches \u{201C}{query}\u{201D}"))
                            .color(palette.text_secondary),
                    );
                    return;
                }
                for (index, (key, person)) in matches.iter().enumerate() {
                    let login = person
                        .login
                        .as_deref()
                        .map(|login| format!("  @{login}"))
                        .unwrap_or_default();
                    let text = format!("{}{login}", person.display_name);
                    let entry = ui.add(
                        egui::Button::selectable(index == selected, text)
                            .min_size(vec2(ui.available_width(), 28.0)),
                    );
                    if entry.clicked() {
                        chosen = Some(key.clone());
                    }
                }
            });
        },
    );
    if let Some(key) = chosen {
        input::choose_mention(state, context, start, key);
        ctx.memory_mut(|memory| memory.request_focus(super::text_id(id)));
    }
}

fn link_popover(
    ctx: &Context,
    id: Id,
    state: &mut State,
    mut url: String,
    is_new: bool,
    anchor: egui::Pos2,
) -> Option<Command> {
    let field_id = id.with("rich_link_field");
    let has_link = link_at(&state.doc, state.selection.head).is_some();
    let palette = theme::palette(ctx);
    let mut command = None;
    let mut is_closed = false;
    egui::Area::new(id.with("rich_link")).order(Order::Foreground).fixed_pos(anchor).show(
        ctx,
        |ui| {
            egui::Frame::popup(ui.style()).inner_margin(style::SPACE_S).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let field = egui::TextEdit::singleline(&mut url)
                        .id(field_id)
                        .hint_text("Paste or type a link\u{2026}")
                        .desired_width(LINK_FIELD_WIDTH);
                    let response = ui.add(field);
                    cv_widgets::glyph::speak_as(&response, "Link address");
                    if is_new {
                        response.request_focus();
                    }
                    let entered =
                        response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
                    let can_apply = input::is_url(url.trim())
                        || url.trim().starts_with('#')
                        || url.trim().starts_with('/');
                    let verb = if has_link { "Update link" } else { "Add link" };
                    let apply = ui.add_enabled(
                        can_apply,
                        egui::Button::new(RichText::new(verb).color(palette.on_brand))
                            .fill(palette.brand),
                    );
                    if (apply.clicked() || (entered && can_apply)) && !url.trim().is_empty() {
                        command = Some(Command::SetLink(Some(Link {
                            url: url.trim().to_owned(),
                            title: String::new(),
                        })));
                    }
                    if has_link {
                        let remove = ui.button(format!("{}  Remove", icons::LINK_BREAK));
                        cv_widgets::glyph::speak_as(&remove, "Remove link");
                        if remove.clicked() {
                            command = Some(Command::SetLink(None));
                        }
                    }
                    if ui.input(|input| input.key_pressed(Key::Escape))
                        || ui.button("Cancel").clicked()
                    {
                        is_closed = true;
                    }
                });
            });
        },
    );
    if command.is_some() || is_closed {
        state.popup = Popup::None;
        ctx.memory_mut(|memory| memory.request_focus(super::text_id(id)));
    } else {
        state.popup = Popup::Link { url, is_new: false };
    }
    command
}

/// The bubble of the link at the caret: its address, Edit, Open and Remove.
fn link_bubble(
    ctx: &Context,
    id: Id,
    state: &mut State,
    anchor: egui::Pos2,
    has_focus: bool,
) -> Option<Command> {
    let area_id = id.with("rich_link_bubble");
    let is_hovered = ctx
        .memory(|memory| memory.area_rect(area_id))
        .zip(ctx.pointer_hover_pos())
        .is_some_and(|(rect, pointer)| rect.contains(pointer));
    if !state.selection.is_empty() || !(has_focus || is_hovered) {
        return None;
    }
    let (_, link) = link_at(&state.doc, state.selection.head)?;
    let palette = theme::palette(ctx);
    let mut command = None;
    egui::Area::new(area_id).order(Order::Foreground).fixed_pos(anchor).show(ctx, |ui: &mut Ui| {
        egui::Frame::popup(ui.style()).inner_margin(style::SPACE_XS).show(ui, |ui| {
            ui.horizontal(|ui| {
                let shown: String = link.url.chars().take(48).collect();
                let shown =
                    if shown.len() < link.url.len() { format!("{shown}\u{2026}") } else { shown };
                ui.label(RichText::new(shown).color(palette.text_secondary))
                    .on_hover_text(&link.url);
                let edit = ui.button(icons::PENCIL_SIMPLE);
                cv_widgets::glyph::speak_as(&edit, "Edit link");
                if edit.on_hover_text("Edit link").clicked() {
                    command = Some(Command::EditLink);
                }
                let open = ui.button(icons::ARROW_SQUARE_OUT);
                cv_widgets::glyph::speak_as(&open, "Open link");
                if open.on_hover_text("Open link").clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(&link.url));
                }
                let remove = ui.button(icons::LINK_BREAK);
                cv_widgets::glyph::speak_as(&remove, "Remove link");
                if remove.on_hover_text("Remove link").clicked() {
                    command = Some(Command::SetLink(None));
                }
            });
        });
    });
    command
}
