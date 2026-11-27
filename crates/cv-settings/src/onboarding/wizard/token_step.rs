//! Step 3: sign in with a CLI login or a token created from a pre-filled link, then paste it.

use std::time::Instant;

use cv_base::theme::{self, icons};
use cv_services::Services;
use cv_widgets::{badges, buttons, checkbox, glyph, states};
use egui::{RichText, TextEdit, Ui};
use review_core::setup::TokenRecipe;

use super::{Check, Fix, Step, Wizard, failed_callout, footer};
use crate::onboarding::cli::CliLogin;
use crate::onboarding::parts::{self, Tone};

/// Lines a setting's reason up with its name, past the checkbox.
const SETTING_INDENT: i8 = 22;

pub fn show(wizard: &mut Wizard, ui: &mut Ui, services: &mut Services) {
    let Ok(host) = wizard.resolved() else {
        wizard.step = Step::Host;
        return;
    };
    let host_label = host.display();
    let fix_url = wizard.fix_url(&host);
    parts::headline(
        ui,
        &format!("Sign in to {host_label}"),
        "Code View needs a token to read and review pull requests for you.",
    );
    if problem_or_stored(wizard, ui, &host_label, &fix_url) {
        wizard.recheck(services);
    }

    let cli_login = wizard.cli_login();
    if let Some(login) = &cli_login {
        cli_option(wizard, ui, services, login);
        parts::section(ui, "Or create a token");
    } else {
        parts::section(ui, "Create a token");
    }
    recipes(wizard, ui, cli_login.as_ref(), &host_label);

    parts::section(ui, "Paste the token");
    // Before the token is pasted: saving it will need the store this message is about.
    if wizard.vault_message(ui, false).wants_recheck {
        wizard.vault = super::VaultCheck::Idle;
    }
    token_field(wizard, ui, services.paths.token_vault().summary());
    check_state(wizard, ui, services, &host_label, &fix_url);

    let mut goes_back = false;
    footer(ui, Some(&mut goes_back), |ui| {
        let has_token = wizard.token().is_some();
        let is_busy = matches!(wizard.check, Check::Running { .. });
        let check = ui.add_enabled_ui(has_token && !is_busy, |ui| {
            buttons::secondary_icon(ui, icons::SHIELD_CHECK, "Check token")
        });
        if check.inner.clicked() {
            wizard.recheck(services);
        }
        if wizard.report().is_some()
            && buttons::secondary_icon(ui, icons::LIST_CHECKS, "Show access").clicked()
        {
            wizard.step = Step::Access;
        }
    });
    if goes_back {
        wizard.step = Step::Host;
    }
}

/// The last check's problem with its fixes, else a note that a token is stored; true when the
/// user asks to check the stored token again.
fn problem_or_stored(wizard: &Wizard, ui: &mut Ui, host_label: &str, fix_url: &str) -> bool {
    let has_stored_token =
        wizard.existing.as_ref().is_some_and(|connection| connection.token().is_some())
            && wizard.token_input.trim().is_empty()
            && wizard.cli_token.is_none();
    let mut wants_recheck = false;
    if let Some(problem) = &wizard.problem {
        parts::callout(ui, Tone::Error, &problem.title, &problem.fix, |ui| {
            for action in &problem.actions {
                parts::open_button(ui, true, &action.label, &action.url);
            }
            parts::open_button(
                ui,
                problem.actions.is_empty(),
                &format!("Create token on {host_label}"),
                fix_url,
            );
            if has_stored_token
                && buttons::secondary_icon(ui, icons::ARROW_CLOCKWISE, "Check stored token again")
                    .clicked()
            {
                wants_recheck = true;
            }
        });
    } else if has_stored_token {
        parts::callout(
            ui,
            Tone::Info,
            "A token is stored for this connection",
            "Check it again, or replace it below.",
            |ui| {
                wants_recheck =
                    buttons::secondary_icon(ui, icons::ARROW_CLOCKWISE, "Check stored token")
                        .clicked();
            },
        );
    }
    wants_recheck
}

/// The recommended way to create a token, and the alternative behind a toggle.
fn recipes(wizard: &mut Wizard, ui: &mut Ui, cli_login: Option<&CliLogin>, host_label: &str) {
    let Some(guide) = wizard.guide() else {
        return;
    };
    // With a CLI sign-in or a fix on offer, that stays the one primary action.
    let is_primary = cli_login.is_none() && wizard.problem.is_none();
    recipe(
        wizard,
        ui,
        &guide.recommended,
        Emphasis { is_recommended: true, is_primary },
        host_label,
    );
    let Some(alternative) = &guide.alternative else {
        return;
    };
    let toggle = if wizard.shows_alternative { icons::CARET_DOWN } else { icons::CARET_RIGHT };
    let text = format!("Prefer a {}?", alternative.title.to_lowercase());
    if buttons::subtle(ui, toggle, &text).clicked() {
        wizard.shows_alternative = !wizard.shows_alternative;
    }
    if wizard.shows_alternative {
        ui.add_space(4.0);
        ui.indent("alternative_recipe", |ui| {
            let emphasis = Emphasis { is_recommended: false, is_primary: false };
            recipe(wizard, ui, alternative, emphasis, host_label);
        });
    }
}

fn cli_option(wizard: &mut Wizard, ui: &mut Ui, services: &mut Services, login: &CliLogin) {
    let tool = login.tool_name();
    let is_running = wizard.cli_fetch.is_some();
    let is_used = wizard.cli_token.as_ref().is_some_and(|(used, _)| used == login);
    parts::callout(
        ui,
        Tone::Info,
        &format!("Use your {tool} sign-in"),
        login.explanation(),
        |ui| {
            if is_running {
                states::loading(ui, &format!("Asking the {tool}…"));
            } else if is_used {
                let note = format!("Using your {tool} sign-in");
                let label = ui.label(
                    RichText::new(format!("{}  {note}", icons::CHECK))
                        .color(theme::palette(ui.ctx()).success),
                );
                glyph::speak_as(&label, &note);
            } else if buttons::primary_icon(ui, icons::KEY, &format!("Use {tool} sign-in"))
                .clicked()
            {
                wizard.start_cli(services, login.clone());
            }
        },
    );
    if let Some((failed, message)) = wizard.cli_error.clone()
        && failed == *login
    {
        let command = login.sign_in_command();
        parts::callout(
            ui,
            Tone::Warning,
            message.as_str(),
            &format!("Sign in once in a terminal: {command}"),
            |ui| {
                if buttons::secondary_icon(ui, icons::COPY, "Copy command").clicked() {
                    ui.ctx().copy_text(command.clone());
                }
                if buttons::secondary_icon(ui, icons::ARROW_CLOCKWISE, "Try again").clicked() {
                    wizard.start_cli(services, login.clone());
                }
            },
        );
    }
}

#[derive(Clone, Copy)]
struct Emphasis {
    is_recommended: bool,
    is_primary: bool,
}

/// One way to create a token: the link, what it pre-fills, and the settings to choose.
fn recipe(
    wizard: &mut Wizard,
    ui: &mut Ui,
    recipe: &TokenRecipe,
    emphasis: Emphasis,
    host_label: &str,
) {
    let palette = theme::palette(ui.ctx());
    let is_recommended = emphasis.is_recommended;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&recipe.title).font(theme::fonts::semibold_font(theme::style::BODY_SIZE)),
        );
        if is_recommended {
            badges::pill(ui, "Recommended", palette.selection, palette.accent_text);
        }
    });
    ui.label(RichText::new(&recipe.summary).color(palette.text_secondary));
    ui.add_space(6.0);
    let button = if recipe.is_prefilled {
        format!("Create token on {host_label}")
    } else {
        format!("Open token page on {host_label}")
    };
    ui.horizontal_wrapped(|ui| {
        parts::open_button(ui, emphasis.is_primary, &button, &recipe.create_url);
        let hint = if recipe.is_prefilled {
            "Name and permissions are filled in. Choose an expiration, then create it."
        } else {
            "The page can't be pre-filled. Tick these as you go:"
        };
        parts::small(ui, hint);
    });
    ui.add_space(6.0);
    if is_recommended && !recipe.is_prefilled {
        if wizard.ticked.len() != recipe.settings.len() {
            wizard.ticked = vec![false; recipe.settings.len()];
        }
        for (setting, ticked) in recipe.settings.iter().zip(wizard.ticked.iter_mut()) {
            setting_row(ui, &setting.reason, |ui| {
                checkbox::checkbox(
                    ui,
                    ticked,
                    RichText::new(&setting.name)
                        .font(theme::fonts::semibold_font(theme::style::BODY_SIZE)),
                );
            });
        }
    } else {
        for setting in &recipe.settings {
            setting_row(ui, &setting.reason, |ui| {
                ui.horizontal(|ui| {
                    glyph::decorative(ui, RichText::new(icons::CHECK).color(palette.success));
                    ui.label(
                        RichText::new(&setting.name)
                            .font(theme::fonts::semibold_font(theme::style::BODY_SIZE)),
                    );
                });
            });
        }
    }
    for caveat in &recipe.caveats {
        parts::small_note(ui, caveat);
    }
    ui.add_space(8.0);
}

/// A setting's name (with its checkbox or check mark) and, right under it, why the app needs it.
fn setting_row(ui: &mut Ui, reason: &str, add_name: impl FnOnce(&mut Ui)) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        add_name(ui);
        egui::Frame::new()
            .inner_margin(egui::Margin { left: SETTING_INDENT, ..egui::Margin::ZERO })
            .show(ui, |ui| {
                parts::small(ui, reason);
            });
    });
    ui.add_space(6.0);
}

/// The token input, then `vault_summary`: where the token will rest.
fn token_field(wizard: &mut Wizard, ui: &mut Ui, vault_summary: &str) {
    if let Some((login, _)) = &wizard.cli_token {
        let words = format!(
            "Using your {} sign-in. Paste a token instead to replace it.",
            login.tool_name()
        );
        let text = format!("{}  {words}", icons::CHECK_CIRCLE);
        let label = ui.label(RichText::new(text).color(theme::palette(ui.ctx()).success));
        glyph::speak_as(&label, &words);
    }
    ui.horizontal(|ui| {
        let width = ui.available_width() - buttons::SUBTLE_HEIGHT - ui.spacing().item_spacing.x;
        let hint =
            if wizard.existing.as_ref().is_some_and(|connection| connection.token().is_some()) {
                "Paste a new token to replace the stored one"
            } else {
                "Paste the token here; it is checked right away"
            };
        let response = ui.add(
            TextEdit::singleline(&mut wizard.token_input)
                .id_salt("onboarding_token")
                .password(!wizard.reveals_token)
                .hint_text(hint)
                .desired_width(width),
        );
        if response.changed() {
            wizard.cli_token = None;
            wizard.problem = None;
            wizard.token_edited_at = Some(Instant::now());
        }
        // A fixed-size toggle: the field keeps its width whichever way the token is shown.
        if buttons::icon_toggle(
            ui,
            icons::EYE,
            icons::EYE_SLASH,
            wizard.reveals_token,
            "Show token",
        )
        .clicked()
        {
            wizard.reveals_token = !wizard.reveals_token;
        }
    });
    parts::small(ui, vault_summary);
    ui.add_space(8.0);
}

fn check_state(
    wizard: &mut Wizard,
    ui: &mut Ui,
    services: &mut Services,
    host_label: &str,
    fix_url: &str,
) {
    let mut wants_retry = false;
    match &wizard.check {
        Check::Running { .. } => {
            states::loading(ui, &format!("Checking the token with {host_label}…"));
        }
        Check::Failed { failure, .. } => {
            wants_retry = failed_callout(ui, failure, fix_url, false) == Some(Fix::Recheck);
        }
        Check::Done { .. } | Check::Idle => {}
    }
    if wants_retry {
        wizard.recheck(services);
    }
}
