//! Settings → Assistant: whether the assistant is shown at all, which agent CLI a new conversation
//! starts with, and the model per agent. Each agent shows whether its CLI is installed (with its
//! version) or links its install guide; the models come from the CLI itself, asked once per app
//! run. A changed model applies to new conversations and to the next question of an open one that
//! uses the default. Whether a followed citation keeps the answer beside the code in a mini chat.
//! While the assistant is off the other rows read as inactive and ask no CLI.

use cv_base::theme::{self, icons, style};
use cv_services::Services;
use cv_services::agent_names::{cli_name, install_url};
use cv_services::agents::Detection;
use cv_widgets::tabs::{self, Segment};
use cv_widgets::{buttons, glyph};
use egui::{RichText, Ui};
use review_agent::{AgentError, AgentKind, ModelChoice};

use super::rows::{self, AssistantRow, Row, RowId};

/// Whether row `id` can be used: every Assistant row but the switch waits for the assistant to be
/// on; rows of other sections always can.
pub fn is_active(id: RowId, services: &Services) -> bool {
    !matches!(id, RowId::Assistant(row) if row != AssistantRow::Shown)
        || services.settings.assistant.enabled
}

/// The control of Assistant row `id`; a switch takes its name from `row`.
pub fn control(ui: &mut Ui, id: AssistantRow, row: &Row, services: &mut Services) {
    let changed = match id {
        AssistantRow::Shown => rows::switch(ui, &mut services.settings.assistant.enabled, row),
        AssistantRow::Agent => agent(ui, services),
        AssistantRow::ClaudeModel => model(ui, AgentKind::Claude, services),
        AssistantRow::CodexModel => model(ui, AgentKind::Codex, services),
        AssistantRow::MiniChat => rows::switch(ui, &mut services.settings.assistant.mini_chat, row),
    };
    if changed {
        services.save_settings();
    }
}

fn agent(ui: &mut Ui, services: &mut Services) -> bool {
    let options =
        AgentKind::ALL.map(|kind| Segment::new(kind, cli_name(kind)).with_hint(kind.label()));
    let changed = tabs::segments(ui, &mut services.settings.assistant.agent, &options);
    if !services.settings.assistant.enabled {
        return changed;
    }
    for kind in AgentKind::ALL {
        ui.add_space(style::SPACE_S);
        status(ui, kind, services);
    }
    changed
}

/// "Claude Code 2.1.3"; "Not installed" with the install guide and "Check again"; or why an
/// installed CLI did not answer `--version`, with "Check again".
fn status(ui: &mut Ui, kind: AgentKind, services: &mut Services) {
    let palette = theme::palette(ui.ctx());
    let detection = services.agents.detection(kind, &services.background);
    let (icon, color, text) = match detection {
        Detection::Checking => (icons::CIRCLE_NOTCH, palette.text_secondary, "Looking…".to_owned()),
        Detection::Installed(cli) => {
            (icons::CHECK_CIRCLE, palette.success, format!("Installed, {}", cli.version))
        }
        Detection::Missing(error) => (icons::WARNING, palette.warning, missing_text(error)),
    };
    let is_not_installed = matches!(detection, Detection::Missing(AgentError::NotInstalled(_)));
    let is_failed = matches!(detection, Detection::Missing(_));
    let mut check_again = false;
    ui.horizontal_wrapped(|ui| {
        glyph::decorative(ui, RichText::new(icon).color(color));
        ui.label(RichText::new(cli_name(kind)).strong());
        ui.label(RichText::new(text).color(palette.text_secondary));
        if is_not_installed {
            let guide = format!("Install {}", cli_name(kind));
            if buttons::text_link(ui, &guide, install_url(kind)).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(install_url(kind)));
            }
        }
        if is_failed {
            let hint = if is_not_installed { "After installing it" } else { "Ask the CLI again" };
            check_again = buttons::text_link(ui, "Check again", hint).clicked();
        }
    });
    if check_again {
        services.agents.check_again(kind);
    }
}

/// Why a CLI is not usable. A timeout names what did not answer once, not as a nested reason.
fn missing_text(error: &AgentError) -> String {
    match error {
        AgentError::NotInstalled(_) => "Not installed".to_owned(),
        AgentError::Timeout(what) => format!("Found, but {what} did not answer in time"),
        error => format!("Found, but it did not answer: {error}"),
    }
}

/// Fastest, the CLI's own default, a model the CLI listed, or an id typed in.
fn model(ui: &mut Ui, kind: AgentKind, services: &mut Services) -> bool {
    let is_on = services.settings.assistant.enabled;
    let listed_models =
        if is_on { services.agents.models(kind, &services.background) } else { None };
    let models = match listed_models {
        Some(Ok(models)) => models.to_vec(),
        Some(Err(_)) | None => Vec::new(),
    };
    let listed: Vec<(String, String)> =
        models.iter().map(|model| (model.id.clone(), model.display_name.clone())).collect();
    let fastest = review_agent::resolve_model(kind, &ModelChoice::Fastest, &models)
        .model
        .map_or_else(|| "Fastest".to_owned(), |model| format!("Fastest · {model}"));
    let choice = services.settings.assistant.model_mut(kind);
    let is_custom =
        matches!(choice, ModelChoice::Named(id) if !listed.iter().any(|(listed, _)| listed == id));
    let selected = match &*choice {
        ModelChoice::Fastest => fastest.clone(),
        ModelChoice::CliDefault => "CLI default".to_owned(),
        ModelChoice::Named(id) => listed
            .iter()
            .find(|(listed, _)| listed == id)
            .map_or_else(|| "Custom model id".to_owned(), |(_, name)| name.clone()),
    };
    let mut changed = false;
    egui::ComboBox::from_id_salt(("assistant_model", kind.label()))
        .width(280.0)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            changed |= ui.selectable_value(choice, ModelChoice::Fastest, &fastest).changed();
            let default = ModelChoice::CliDefault;
            changed |= ui.selectable_value(choice, default, "CLI default").changed();
            for (id, name) in &listed {
                let named = ModelChoice::Named(id.clone());
                changed |= ui.selectable_value(choice, named, name).changed();
            }
            if ui.selectable_label(is_custom, "Custom model id").clicked() && !is_custom {
                *choice = ModelChoice::Named(String::new());
                changed = true;
            }
        });
    if let ModelChoice::Named(id) = choice
        && (is_custom || id.is_empty())
    {
        ui.add_space(style::SPACE_XS);
        let field = ui.add(
            egui::TextEdit::singleline(id)
                .id_salt(("assistant_model_id", kind.label()))
                .hint_text("Model id, as the CLI's --model takes it")
                .desired_width(280.0),
        );
        if field.lost_focus() {
            *choice = typed_model(id);
            changed = true;
        }
    }
    changed
}

/// The choice a typed model id stands for once the field is left: the id without the spaces a
/// paste brings (the CLI's `--model` would reject them), or Fastest for nothing typed.
fn typed_model(id: &str) -> ModelChoice {
    match id.trim() {
        "" => ModelChoice::Fastest,
        id => ModelChoice::Named(id.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use review_agent::{AgentError, AgentKind, ModelChoice};

    use super::{missing_text, typed_model};

    #[test]
    fn a_cli_that_does_not_answer_in_time_names_what_timed_out_once() {
        let text = missing_text(&AgentError::Timeout("--version"));
        assert_eq!(text, "Found, but --version did not answer in time");
        assert_eq!(missing_text(&AgentError::NotInstalled(AgentKind::Claude)), "Not installed");
    }

    #[test]
    fn a_typed_model_id_is_saved_without_surrounding_spaces() {
        assert_eq!(
            typed_model("  claude-opus-5-5 \n"),
            ModelChoice::Named("claude-opus-5-5".into())
        );
        assert_eq!(typed_model("gpt-6"), ModelChoice::Named("gpt-6".into()));
        assert_eq!(typed_model("   "), ModelChoice::Fastest, "nothing typed takes the fastest");
    }
}
