//! Settings › Local checkouts: the clones Check out uses (found by remote URL in the search
//! folders, or pinned by hand), the search folders, where new worktrees go, what happens to a
//! worktree once its pull request is merged or closed elsewhere, what opens by default, the terminal, and the
//! coding agents as the Assistant section finds them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cv_base::platform;
use cv_base::theme::{self, icons, style};
use cv_services::Services;
use cv_services::agents::Detection;
use cv_services::checkouts::{self, FolderPurpose};
use cv_widgets::{buttons, tabs};
use egui::{RichText, Ui};
use review_core::AgentKind;
use review_git::checkout::worktree_path;
use review_store::checkouts::{
    CheckoutKind, CheckoutSettings, DEFAULT_WORKTREE_PATTERN, OpenByDefault, OpenWith,
    TerminalChoice, WhenFinished,
};

use super::rows::RowId;

/// The control of Local checkouts row `id`; true when its link to the Assistant section was
/// clicked.
pub fn control(ui: &mut Ui, id: RowId, services: &mut Services) -> bool {
    let mut changed = false;
    let mut opens_assistant = false;
    match id {
        RowId::CheckoutClones => changed = clones(ui, services),
        RowId::CheckoutSearch => changed = search(ui, services),
        RowId::CheckoutPattern => changed = pattern(ui, services),
        RowId::CheckoutRemoval => changed = when_finished(ui, services),
        RowId::CheckoutOpen => {
            let options = [
                (OpenByDefault::Ask, "Ask each time"),
                (OpenByDefault::Always(OpenWith::Nothing), "Nothing"),
                (OpenByDefault::Always(OpenWith::Terminal), "Terminal"),
                (OpenByDefault::Always(OpenWith::Agent(AgentKind::Claude)), "Claude Code"),
                (OpenByDefault::Always(OpenWith::Agent(AgentKind::Codex)), "Codex"),
            ];
            changed =
                tabs::segmented(ui, &mut services.settings.checkouts.open_by_default, &options);
        }
        RowId::CheckoutTerminal => {
            let options: Vec<(TerminalChoice, &str)> = platform::TERMINALS.to_vec();
            changed = tabs::segmented(ui, &mut services.settings.checkouts.terminal, &options);
        }
        RowId::CheckoutAgents => opens_assistant = agents(ui, services),
        _ => {}
    }
    if changed {
        services.save_settings();
    }
    opens_assistant
}

/// A repository Check out knows, and its clone.
struct KnownClone {
    name: String,
    path: Option<PathBuf>,
    is_pinned: bool,
}

/// The repositories looked up this run, those with a checkout and those with a pinned clone, by
/// their settings key.
fn known_clones(services: &Services) -> BTreeMap<String, KnownClone> {
    let mut rows = BTreeMap::new();
    for (key, known) in services.checkouts.repositories() {
        let row =
            KnownClone { name: known.name.clone(), path: known.found.clone(), is_pinned: false };
        rows.insert(key.clone(), row);
    }
    for record in services.checkouts.records().all() {
        let key = CheckoutSettings::repository_key(&record.connection_id, &record.repository_key);
        let row = rows.entry(key).or_insert_with(|| KnownClone {
            name: record.repository_key.clone(),
            path: None,
            is_pinned: false,
        });
        if row.path.is_none() && record.kind != CheckoutKind::NewClone {
            row.path = Some(record.clone.clone());
        }
    }
    for (key, path) in &services.settings.checkouts.pinned_clones {
        let name = key.split_once('|').map_or(key.as_str(), |(_, name)| name).to_owned();
        let row = rows.entry(key.clone()).or_insert_with(|| KnownClone {
            name,
            path: None,
            is_pinned: false,
        });
        row.path = Some(path.clone());
        row.is_pinned = true;
    }
    rows
}

/// One row per repository Check out knows: its clone (pinned, found or not found), how many
/// worktrees the app made of it, and Change… or Pick… to pin one.
fn clones(ui: &mut Ui, services: &mut Services) -> bool {
    let palette = theme::palette(ui.ctx());
    let rows = known_clones(services);
    let mut changed = false;
    for key in rows.keys() {
        if let Some(folder) = services.checkouts.take_picked(&FolderPurpose::PinClone(key.clone()))
        {
            services.settings.checkouts.pinned_clones.insert(key.clone(), folder);
            changed = true;
        }
    }
    if rows.is_empty() {
        ui.label(
            RichText::new("Check out a pull request once, and its repository shows here.")
                .color(palette.text_secondary),
        );
        return changed;
    }
    egui::Grid::new("checkout_clones")
        .num_columns(4)
        .spacing([style::SPACE_L, style::SPACE_S])
        .show(ui, |ui| {
            for heading in ["Repository", "Clone", "Worktrees", ""] {
                ui.label(
                    RichText::new(heading).size(style::SMALL_SIZE).color(palette.text_secondary),
                );
            }
            ui.end_row();
            for (key, row) in &rows {
                clone_row(ui, key, row, services);
                ui.end_row();
            }
        });
    changed
}

fn clone_row(ui: &mut Ui, key: &str, row: &KnownClone, services: &mut Services) {
    let palette = theme::palette(ui.ctx());
    ui.label(&row.name);
    match &row.path {
        Some(path) => {
            let shown = path.display();
            let text = if row.is_pinned { format!("{shown} (pinned)") } else { shown.to_string() };
            ui.label(RichText::new(text).font(theme::fonts::code_font(style::SMALL_SIZE)));
        }
        None => {
            ui.label(RichText::new("Not found").color(palette.text_secondary));
        }
    }
    let worktrees =
        row.path.as_deref().map_or(0, |path| services.checkouts.records().worktrees_of(path));
    ui.label(worktrees.to_string());
    let label = if row.path.is_some() { "Change…" } else { "Pick…" };
    if buttons::text_link(ui, label, "Pin the folder of this repository's clone").clicked() {
        let purpose = FolderPurpose::PinClone(key.to_owned());
        services.checkouts.ask_folder(
            purpose,
            "Your clone of the repository",
            &services.background,
        );
    }
}

/// The search folders, each with a remove button, and "Add folder…". Until one is added or removed
/// the defaults that exist on this computer stand.
fn search(ui: &mut Ui, services: &mut Services) -> bool {
    let palette = theme::palette(ui.ctx());
    let mut changed = false;
    let defaults = services.checkouts.default_folders(&services.background).cloned();
    if let Some(folder) = services.checkouts.take_picked(&FolderPurpose::SearchFolder) {
        let folders = services
            .settings
            .checkouts
            .search_folders
            .get_or_insert_with(|| defaults.clone().unwrap_or_default());
        if !folders.contains(&folder) {
            folders.push(folder);
        }
        changed = true;
    }
    let is_default = services.settings.checkouts.search_folders.is_none();
    let folders = match &services.settings.checkouts.search_folders {
        Some(folders) => folders.clone(),
        None => defaults.clone().unwrap_or_default(),
    };
    if is_default {
        let text = if defaults.is_none() {
            "Looking for the usual folders…"
        } else if folders.is_empty() {
            "None of the usual folders (source, src, dev, code, projects, repos) exist here. Add the folder your clones are in."
        } else {
            "The usual folders that exist on this computer."
        };
        ui.label(RichText::new(text).size(style::SMALL_SIZE).color(palette.text_secondary));
    }
    let mut removed = None;
    for folder in &folders {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(folder.display().to_string())
                    .font(theme::fonts::code_font(style::SMALL_SIZE)),
            );
            let tooltip = format!("Stop searching {}", folder.display());
            if buttons::icon(ui, icons::X, &tooltip).clicked() {
                removed = Some(folder.clone());
            }
        });
    }
    if let Some(removed) = removed {
        let mut kept = folders.clone();
        kept.retain(|folder| folder != &removed);
        services.settings.checkouts.search_folders = Some(kept);
        changed = true;
    }
    let is_asking = services.checkouts.is_asking_folder();
    let add = ui
        .add_enabled_ui(!is_asking, |ui| buttons::secondary_icon(ui, icons::PLUS, "Add folder…"))
        .inner;
    if add.clicked() {
        services.checkouts.ask_folder(
            FolderPurpose::SearchFolder,
            "Folder to search for clones",
            &services.background,
        );
    }
    changed
}

/// What happens to a worktree once its pull request is merged or closed elsewhere.
fn when_finished(ui: &mut Ui, services: &mut Services) -> bool {
    let options = [
        (WhenFinished::RemoveAutomatically, "Remove worktree automatically"),
        (WhenFinished::Ask, "Ask me first"),
        (WhenFinished::Keep, "Keep worktree"),
    ];
    let value = &mut services.settings.checkouts.when_finished;
    let selected = options.iter().find(|(option, _)| option == value).map_or("", |(_, name)| *name);
    let mut changed = false;
    egui::ComboBox::from_id_salt("checkout_when_finished")
        .width(320.0)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for (option, name) in options {
                changed |= ui.selectable_value(value, option, name).changed();
            }
        });
    changed
}

/// The pattern as text, with the folder it gives for an example pull request.
fn pattern(ui: &mut Ui, services: &mut Services) -> bool {
    let palette = theme::palette(ui.ctx());
    let edit = egui::TextEdit::singleline(&mut services.settings.checkouts.worktree_pattern)
        .font(theme::fonts::code_font(style::BODY_SIZE))
        .desired_width(ui.available_width().min(480.0));
    let response = ui.add(edit);
    let example_clone: &Path =
        Path::new(if cfg!(windows) { r"C:\src\login-app" } else { "/home/you/src/login-app" });
    let example = worktree_path(
        &services.settings.checkouts.worktree_pattern,
        example_clone,
        12,
        "feature/login-cache",
    );
    ui.label(
        RichText::new(format!("Example: {}", example.display()))
            .size(style::SMALL_SIZE)
            .color(palette.text_secondary),
    );
    if services.settings.checkouts.worktree_pattern.trim().is_empty() && response.lost_focus() {
        DEFAULT_WORKTREE_PATTERN.clone_into(&mut services.settings.checkouts.worktree_pattern);
        return true;
    }
    response.changed()
}

/// Each agent's CLI as the Assistant section found it; true when its link was clicked.
fn agents(ui: &mut Ui, services: &mut Services) -> bool {
    let palette = theme::palette(ui.ctx());
    for kind in [AgentKind::Claude, AgentKind::Codex] {
        let name = checkouts::agent_name(kind);
        let text = match services.agents.detection(kind, &services.background) {
            Detection::Installed(cli) => format!("{name} {}", cli.version),
            Detection::Missing(_) => format!("{name}: not installed"),
            Detection::Checking => format!("{name}: looking…"),
        };
        ui.label(RichText::new(text).color(palette.text_secondary));
    }
    buttons::text_link(ui, "Set up in Settings › Assistant", "Open the Assistant section").clicked()
}
