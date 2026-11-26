//! The popover's choices: where the branch goes (a new worktree, recommended, or the clone itself),
//! what opens after it, and whether to remember both for the repository. A clone with changes or a
//! branch checked out elsewhere turns the form into the card that explains it.

use std::path::Path;

use egui::{Margin, RichText, Sense, Stroke, Ui, pos2, vec2};
use review_git::checkout::{Survey, same_folder};
use review_store::checkouts::{CheckoutPlace, OpenWith};

use super::{Act, Choice, Section, View, cards, settings_link, then_name, worktree_folder};
use cv_base::theme::{self, icons, style};
use cv_services::checkouts::{FolderPurpose, plural};
use cv_widgets::{badges, buttons, checkbox, focus, row};

const RADIO: f32 = 16.0;

/// What the form shows about the clone.
pub(super) struct Place<'a> {
    pub survey: &'a Survey,
    /// "Change folder…" picked this folder for the worktree.
    pub folder: Option<&'a Path>,
    /// The mirror has the pull request's commits, so a checkout can start.
    pub has_mirror: bool,
}

pub(super) fn show(
    ui: &mut Ui,
    view: &View<'_>,
    choice: &mut Choice,
    place: &Place<'_>,
) -> Option<Act> {
    let survey = place.survey;
    if let Some(path) = &survey.checked_out
        && !(choice.place == CheckoutPlace::Clone && same_folder(path, &survey.clone))
    {
        return cards::checked_out(ui, path);
    }
    if choice.place == CheckoutPlace::Clone && !survey.state.is_clean() {
        return cards::dirty(ui, &survey.clone, &survey.state.tracked_changes());
    }
    let mut act = None;
    heading(ui, view);
    caption(ui, "Where");
    let worktree = worktree_folder(
        view.settings,
        &survey.clone,
        view.key.number,
        &view.detail.summary.source_branch,
        place.folder,
    );
    let new_worktree = Choosable {
        title: "New worktree",
        badge: Some("recommended"),
        text: "Its own folder beside your clone. Your current work stays untouched.",
        detail: worktree.display().to_string(),
    };
    if option(ui, choice.place == CheckoutPlace::Worktree, &new_worktree).clicked() {
        choice.place = CheckoutPlace::Worktree;
    }
    if choice.place == CheckoutPlace::Worktree {
        ui.horizontal(|ui| {
            ui.add_space(RADIO + style::SPACE_M + 10.0);
            if buttons::text_link(ui, "Change folder…", "Pick the folder the worktree goes into")
                .clicked()
            {
                let purpose = FolderPurpose::WorktreeFolder(view.key.clone());
                act = Some(Act::AskFolder(purpose, "Folder for the new worktree"));
            }
        });
    }
    let clone_name = folder_name(&survey.clone);
    let state = match (&survey.state.branch, survey.state.tracked_changes().len()) {
        (Some(branch), 0) => format!("on {branch}, clean"),
        (Some(branch), changed) => {
            format!("on {branch}, {changed} changed {}", plural(changed, "file"))
        }
        (None, 0) => "detached, clean".to_owned(),
        (None, changed) => format!("detached, {changed} changed {}", plural(changed, "file")),
    };
    let your_clone = Choosable {
        title: "Your clone",
        badge: None,
        text: &format!("Switches {clone_name} to the branch. Needs a clean working tree."),
        detail: format!("{} · {state}", survey.clone.display()),
    };
    if option(ui, choice.place == CheckoutPlace::Clone, &your_clone).clicked() {
        choice.place = CheckoutPlace::Clone;
    }
    ui.add_space(style::SPACE_XS);
    caption(ui, "Then open");
    act = act.or(then_open(ui, view, choice));
    checkbox::checkbox(ui, &mut choice.remember, "Remember for this repository");
    ui.add_space(style::SPACE_XS);
    act.or(commands(ui, view, *choice, place.has_mirror))
}

/// Before the lookup is back: what it looks for, or why it failed with a way on.
pub(super) fn looking(ui: &mut Ui, view: &View<'_>, error: Option<&str>) -> Option<Act> {
    heading(ui, view);
    let palette = theme::palette(ui.ctx());
    let Some(error) = error else {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(icons::SIZE - 2.0).color(palette.text_secondary));
            ui.label(
                RichText::new(format!("Looking for your clone of {}…", view.repository))
                    .color(palette.text_secondary),
            );
        });
        return None;
    };
    ui.label(RichText::new(format!("Could not look at your clone: {error}")).color(palette.error));
    let mut act = None;
    ui.horizontal(|ui| {
        if buttons::secondary_icon(ui, icons::ARROW_CLOCKWISE, "Try again").clicked() {
            act = Some(Act::Dismiss);
        }
        act =
            act.take().or(settings_link(ui, "Settings › Local checkouts", Section::LocalCheckouts));
    });
    act
}

fn heading(ui: &mut Ui, view: &View<'_>) {
    let palette = theme::palette(ui.ctx());
    ui.label(
        RichText::new(format!("Check out #{} locally", view.key.number))
            .font(theme::fonts::semibold_font(style::SECTION_SIZE)),
    );
    ui.label(
        RichText::new("The commits are already in Code View's mirror, so this only takes seconds.")
            .color(palette.text_secondary),
    );
    ui.add_space(style::SPACE_XS);
}

fn caption(ui: &mut Ui, text: &str) {
    let palette = theme::palette(ui.ctx());
    ui.label(
        RichText::new(text)
            .font(theme::fonts::semibold_font(style::SMALL_SIZE))
            .color(palette.text_secondary),
    );
}

/// Nothing, the terminal and the agents as chips; an agent that is not installed is greyed, with
/// the way to set it up.
fn then_open(ui: &mut Ui, view: &View<'_>, choice: &mut Choice) -> Option<Act> {
    let mut options = vec![
        (OpenWith::Nothing, "Nothing".to_owned(), true),
        (OpenWith::Terminal, "Terminal".to_owned(), true),
    ];
    for (kind, is_installed) in view.agents {
        let name = cv_services::checkouts::agent_name(kind).to_owned();
        options.push((OpenWith::Agent(kind), name, is_installed));
    }
    ui.horizontal_wrapped(|ui| {
        for (with, name, is_installed) in &options {
            let is_selected = choice.then == *with;
            let response = ui.add_enabled_ui(*is_installed, |ui| chip(ui, name, is_selected)).inner;
            if response.clicked() {
                choice.then = *with;
            }
        }
    });
    let missing: Vec<&str> = options
        .iter()
        .filter(|(_, _, is_installed)| !is_installed)
        .map(|(_, name, _)| name.as_str())
        .collect();
    if missing.is_empty() {
        return None;
    }
    let palette = theme::palette(ui.ctx());
    let mut act = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("{} not installed.", missing.join(" and ")))
                .size(style::SMALL_SIZE)
                .color(palette.text_secondary),
        );
        act = settings_link(ui, "Set up in Settings › Assistant", Section::Assistant);
    });
    act
}

/// "Check out" (or "Check out and open Claude Code") and Cancel.
fn commands(ui: &mut Ui, view: &View<'_>, choice: Choice, has_mirror: bool) -> Option<Act> {
    let label = match then_name(choice.then, view.settings) {
        Some(name) => format!("Check out and open {name}"),
        None => "Check out".to_owned(),
    };
    if !has_mirror {
        let palette = theme::palette(ui.ctx());
        ui.label(
            RichText::new("Getting the pull request's commits into the mirror first…")
                .size(style::SMALL_SIZE)
                .color(palette.text_secondary),
        );
    }
    let mut act = None;
    ui.horizontal(|ui| {
        let start = ui.add_enabled_ui(has_mirror, |ui| buttons::primary(ui, &label)).inner;
        if start.clicked() {
            act = Some(Act::Start);
        }
        if buttons::secondary(ui, "Cancel").clicked() {
            act = Some(Act::Close);
        }
    });
    act
}

struct Choosable<'a> {
    title: &'a str,
    badge: Option<&'a str>,
    text: &'a str,
    /// A path, in the code font.
    detail: String,
}

/// One answer to "Where": a radio, its title (and badge), what it does and where it lands.
fn option(ui: &mut Ui, is_selected: bool, choosable: &Choosable<'_>) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let stroke = if is_selected { palette.accent } else { palette.border };
    let fill = if is_selected { palette.selection } else { palette.surface };
    let frame = egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(style::RADIUS_CARD)
        .inner_margin(Margin::same(10));
    let shown = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let (slot, _) = ui.allocate_exact_size(vec2(RADIO, RADIO), Sense::hover());
            let center = slot.center();
            ui.painter().circle_stroke(center, RADIO / 2.0 - 1.0, Stroke::new(1.5, stroke));
            if is_selected {
                ui.painter().circle_filled(center, RADIO / 4.0, palette.accent);
            }
            ui.add_space(style::SPACE_XS);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = style::SPACE_XS;
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(choosable.title)
                            .font(theme::fonts::semibold_font(style::BODY_SIZE)),
                    );
                    if let Some(badge) = choosable.badge {
                        badges::pill(ui, badge, palette.surface_subtle, palette.success);
                    }
                });
                ui.label(RichText::new(choosable.text).color(palette.text_secondary));
                ui.add(
                    egui::Label::new(
                        RichText::new(&choosable.detail)
                            .font(theme::fonts::code_font(style::SMALL_SIZE))
                            .color(palette.text_secondary),
                    )
                    .truncate(),
                )
                .on_hover_text(&choosable.detail);
            });
        });
    });
    let rect = shown.response.rect;
    let response =
        ui.interact(rect, ui.scope_id().with(("where", choosable.title)), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::RadioButton, true, is_selected, choosable.title)
    });
    if response.hovered() && !is_selected {
        ui.painter().rect_stroke(
            rect,
            style::RADIUS_CARD,
            Stroke::new(1.0, palette.border_strong),
            egui::StrokeKind::Inside,
        );
    }
    focus::ring(ui, &response, rect);
    response
}

/// A selectable chip of "Then open".
fn chip(ui: &mut Ui, text: &str, is_selected: bool) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let enabled = ui.is_enabled();
    let color = if !enabled {
        palette.text_disabled
    } else if is_selected {
        palette.accent_text
    } else {
        palette.text
    };
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(style::BODY_SIZE),
        color,
    );
    let size = vec2(galley.size().x + style::SPACE_M * 2.0, buttons::SUBTLE_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::RadioButton, enabled, is_selected, text)
    });
    let rest = if is_selected { palette.selection } else { palette.surface };
    let fill = if enabled { row::state_fill(ui, &response, rest) } else { palette.surface };
    let stroke = if is_selected { palette.accent } else { palette.border };
    ui.painter().rect(
        rect,
        style::RADIUS_CONTROL,
        fill,
        Stroke::new(1.0, stroke),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        pos2(rect.left() + style::SPACE_M, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    focus::ring(ui, &response, rect);
    response
}

pub(super) fn folder_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}
