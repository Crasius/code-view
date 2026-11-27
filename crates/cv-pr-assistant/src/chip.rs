//! The agent and model chip: one button that names the conversation's agent and model and opens
//! the choices, with the switch of the [`super::mini`] chat. It sits in the title line of the card,
//! the panel and the mini chat, left of their buttons. It shows its label when the label fits the
//! room the title leaves, else only its icon, with the label as its tooltip.

use cv_base::theme::{self, icons, style};
use cv_services::Services;
use cv_services::agents::Detection;
use cv_widgets::buttons;
use egui::{Response, RichText, Ui};
use review_agent::{AgentKind, ModelChoice};

use super::{Action, Assistant};

/// The chip menu's switch of the mini chat (Settings → Assistant has it too).
pub const MINI_CHAT_TOGGLE: &str = "Keep the answer beside the code";
/// The chip's tooltip after its label.
const HINT: &str = "Agent and model of this conversation";

/// The chip's label: agent and model, with the model "Fastest" or the CLI default stands for once
/// it is known.
pub fn label(assistant: &Assistant, services: &mut Services) -> String {
    let resolved = fastest_model(assistant, services);
    let model = match &assistant.chat.model {
        ModelChoice::Fastest => match &resolved {
            Some(model) => format!("Fastest · {model}"),
            None => "Fastest".to_owned(),
        },
        ModelChoice::CliDefault => assistant
            .model_in_use
            .clone()
            .map_or_else(|| "CLI default".to_owned(), |model| format!("CLI default · {model}")),
        ModelChoice::Named(id) => id.clone(),
    };
    format!("{} · {model}", assistant.chat.agent.label())
}

/// The chip in a right-to-left title line, left of the buttons already there: its label when it
/// fits the room left, else its icon. Its response opens the [`menu`].
pub fn in_title(ui: &mut Ui, label: &str) -> Response {
    ui.add_space(style::SPACE_M);
    let fits = buttons::icon_button_width(ui, label) <= ui.available_width();
    if fits {
        buttons::subtle(ui, icons::ROBOT, label).on_hover_text(HINT)
    } else {
        buttons::icon(ui, icons::ROBOT, &format!("{label}. {HINT}"))
    }
}

/// The choices of the chip drawn as `response`, while its menu is open.
pub fn menu(response: &Response, assistant: &Assistant, services: &mut Services) -> Option<Action> {
    let mut action = None;
    egui::Popup::menu(response).show(|ui| {
        ui.set_min_width(260.0);
        action = choices(ui, assistant, services);
    });
    action
}

fn choices(ui: &mut Ui, assistant: &Assistant, services: &mut Services) -> Option<Action> {
    let palette = theme::palette(ui.ctx());
    let heading = |ui: &mut Ui, text: &str| {
        ui.label(RichText::new(text).size(style::SMALL_SIZE).color(palette.text_secondary));
    };
    heading(ui, "Agent");
    let mut action = agents(ui, assistant, services);
    ui.separator();
    heading(ui, "Model, from the next question");
    action = action.or(models(ui, assistant, services));
    ui.separator();
    heading(ui, "Citations");
    let settings = &mut services.settings.assistant;
    let changed = switch(ui, &mut settings.mini_chat, MINI_CHAT_TOGGLE)
        .on_hover_text("A citation opens the file with the answer floating beside it")
        .clicked();
    if changed {
        services.save_settings();
    }
    action
}

/// A menu item that turns `value` over when clicked, checked while it is on.
fn switch(ui: &mut Ui, value: &mut bool, text: &str) -> Response {
    let mark = if *value { icons::CHECK } else { "" };
    let item = buttons::menu_item(ui, mark, text, None, None);
    if item.clicked() {
        *value = !*value;
    }
    item
}

fn agents(ui: &mut Ui, assistant: &Assistant, services: &mut Services) -> Option<Action> {
    let mut action = None;
    for kind in AgentKind::ALL {
        let is_installed = matches!(
            services.agents.detection(kind, &services.background),
            Detection::Installed(_)
        );
        let is_current = kind == assistant.chat.agent;
        let mark = if is_current { icons::CHECK } else { "" };
        let hint = (!is_installed).then_some("Not installed");
        let item = ui.add_enabled_ui(is_installed || is_current, |ui| {
            buttons::menu_item(ui, mark, kind.label(), hint, None)
        });
        if item.inner.clicked() && !is_current {
            action = Some(Action::SwitchAgent(kind));
        }
    }
    action
}

fn models(ui: &mut Ui, assistant: &Assistant, services: &mut Services) -> Option<Action> {
    let fastest = fastest_model(assistant, services);
    let mut action = None;
    ui.add_enabled_ui(!assistant.is_busy(), |ui| {
        let mut options = vec![
            (
                ModelChoice::Fastest,
                format!("Fastest{}", fastest.map_or(String::new(), |m| format!(" · {m}"))),
            ),
            (ModelChoice::CliDefault, "CLI default".to_owned()),
        ];
        let kind = assistant.chat.agent;
        if let Some(Ok(models)) = services.agents.models(kind, &services.background) {
            options.extend(
                models.iter().map(|model| {
                    (ModelChoice::Named(model.id.clone()), model.display_name.clone())
                }),
            );
        }
        for (choice, label) in options {
            let is_current = choice == assistant.chat.model;
            let mark = if is_current { icons::CHECK } else { "" };
            if buttons::menu_item(ui, mark, &label, None, None).clicked() && !is_current {
                action = Some(Action::SwitchModel(choice));
            }
        }
    });
    action
}

/// The model "Fastest" stands for with this agent's CLI, once its models are known.
fn fastest_model(assistant: &Assistant, services: &mut Services) -> Option<String> {
    let kind = assistant.chat.agent;
    let Some(Ok(models)) = services.agents.models(kind, &services.background) else {
        return None;
    };
    review_agent::resolve_model(kind, &ModelChoice::Fastest, models).model
}
