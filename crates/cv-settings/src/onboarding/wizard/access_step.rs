//! Step 4: who the token signs in as and a checklist of what it can do, each gap with its fix.

use cv_base::theme::{self, icons};
use cv_services::Services;
use cv_widgets::{avatar, buttons, states};
use egui::{Frame, Margin, RichText, Stroke, Ui};
use review_core::AccessReport;
use review_core::setup::Access;

use super::{Check, Failure, Step, Wizard, footer};
use crate::onboarding::parts::{self, Tone};

pub fn show(wizard: &mut Wizard, ui: &mut Ui, services: &mut Services) {
    let Ok(host) = wizard.resolved() else {
        wizard.go(Step::Host);
        return;
    };
    let host_label = host.display();
    let fix_url = wizard.guide().map(|guide| guide.recommended.create_url).unwrap_or_default();
    parts::headline(
        ui,
        "What this token can do",
        &format!("Checked with read-only requests. Nothing on {host_label} was changed."),
    );

    let fix = match (&wizard.check, wizard.report()) {
        (_, Some(report)) => {
            signed_in(ui, report);
            ui.add_space(16.0);
            let fix = report_callout(ui, report, &host_label, &fix_url);
            checklist(ui, report, &host_label, &fix_url);
            fix
        }
        (Check::Running { .. }, None) => {
            ui.add_space(8.0);
            states::loading(ui, &format!("Checking the token with {host_label}…"));
            None
        }
        (Check::Failed { failure, .. }, None) => failed_callout(ui, failure, &fix_url),
        (Check::Idle | Check::Done { .. }, None) => {
            let mut fix = None;
            parts::callout(ui, Tone::Info, "The token has not been checked yet", "", |ui| {
                if buttons::primary_icon(ui, icons::SHIELD_CHECK, "Check token").clicked() {
                    fix = Some(Fix::Recheck);
                }
            });
            fix
        }
    };

    let is_usable = wizard.report().is_some_and(review_core::AccessReport::is_usable);
    let mut goes_back = false;
    footer(ui, Some(&mut goes_back), |ui| {
        let next = ui.add_enabled_ui(is_usable, |ui| {
            buttons::primary_icon(ui, icons::ARROW_RIGHT, "Continue")
        });
        if next.inner.clicked() {
            wizard.go(Step::Scope);
        }
        if !is_usable && wizard.report().is_some() {
            parts::small(ui, "Reading pull requests is required to continue.");
        }
    });
    if goes_back || fix == Some(Fix::NewToken) {
        wizard.go(Step::Token);
    }
    match fix {
        Some(Fix::NewToken) => {
            wizard.token_input.clear();
            wizard.cli_token = None;
        }
        Some(Fix::Recheck) => wizard.recheck(services),
        None => {}
    }
}

/// What the user asked for to fix the token.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fix {
    /// Check the same token again.
    Recheck,
    /// Back to the token step with an empty field.
    NewToken,
}

/// The token's user with its login, token kind and expiry.
fn signed_in(ui: &mut Ui, report: &AccessReport) {
    let palette = theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        avatar::avatar(ui, &report.user, 40.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(
                RichText::new(format!("Signed in as {}", report.user.display_name))
                    .font(theme::fonts::semibold_font(theme::style::SECTION_SIZE)),
            );
            let mut facts = Vec::new();
            if let Some(login) = &report.user.login {
                facts.push(login.clone());
            }
            facts.push(report.token_kind.clone());
            if let Some(expires_at) = &report.expires_at {
                facts.push(format!("expires {expires_at}"));
            }
            ui.label(RichText::new(facts.join(", ")).color(palette.text_secondary));
        });
    });
}

/// The verdict over the checklist: what is missing with its fixes, what only a change could
/// prove, or that everything works.
fn report_callout(
    ui: &mut Ui,
    report: &AccessReport,
    host_label: &str,
    fix_url: &str,
) -> Option<Fix> {
    let mut fix = None;
    let has_unverified = report.checks.iter().any(|check| check.access == Access::Unverified);
    if report.has_missing() {
        parts::callout(
            ui,
            Tone::Warning,
            "Some features won't work with this token",
            "Create a token with the recommended settings and paste it here, or fix each item below.",
            |ui| {
                parts::open_button(ui, true, &format!("Create token on {host_label}"), fix_url);
                if buttons::secondary_icon(ui, icons::CLIPBOARD_TEXT, "Paste a new token").clicked()
                {
                    fix = Some(Fix::NewToken);
                }
                if buttons::secondary_icon(ui, icons::ARROW_CLOCKWISE, "Check again").clicked() {
                    fix = Some(Fix::Recheck);
                }
            },
        );
    } else if has_unverified {
        parts::callout(
            ui,
            Tone::Info,
            "Reading works. Confirm the write permissions",
            "Only a change could prove them, and the check never changes anything. Make sure the items with a \
             dashed circle are selected on the token page.",
            |ui| {
                parts::open_button(ui, false, "Open token page", fix_url);
                if buttons::secondary_icon(ui, icons::ARROW_CLOCKWISE, "Check again").clicked() {
                    fix = Some(Fix::Recheck);
                }
            },
        );
    } else {
        parts::notice(ui, Tone::Success, "Everything works with this token", "");
    }
    fix
}

/// Every check with its access and fix, then the report's notes.
fn checklist(ui: &mut Ui, report: &AccessReport, host_label: &str, fix_url: &str) {
    let palette = theme::palette(ui.ctx());
    Frame::new()
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(4)
        .inner_margin(Margin::symmetric(12, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (index, check) in report.checks.iter().enumerate() {
                if index > 0 {
                    parts::divider(ui);
                }
                parts::access_row(ui, check, fix_url, host_label);
            }
        });
    for note in &report.notes {
        ui.add_space(6.0);
        parts::small_note(ui, note);
    }
}

/// Why the check failed, with a new token, a paste and a retry.
fn failed_callout(ui: &mut Ui, failure: &Failure, fix_url: &str) -> Option<Fix> {
    let mut fix = None;
    let title = failure
        .problem
        .as_ref()
        .map_or("The token could not be checked", |problem| problem.title.as_str());
    parts::callout(ui, Tone::Error, title, &failure.message, |ui| {
        parts::open_button(ui, true, "Create a new token", fix_url);
        if buttons::secondary_icon(ui, icons::CLIPBOARD_TEXT, "Paste a new token").clicked() {
            fix = Some(Fix::NewToken);
        }
        if buttons::secondary_icon(ui, icons::ARROW_CLOCKWISE, "Try again").clicked() {
            fix = Some(Fix::Recheck);
        }
    });
    fix
}
