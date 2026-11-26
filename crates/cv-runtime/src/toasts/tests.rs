//! Each kind of way out on an error toast: the button shows, and clicking it does its one thing
//! (asks for the retry, opens the token fix, the web page or the folder, copies) and dismisses
//! the toast. Opening a folder is stubbed in test builds ([`super::opened_folder`]).
//! `cargo test -p cv-runtime toast`

use std::path::PathBuf;

use cv_base::theme;
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::ProviderKind;
use review_store::ThemePreference;

use super::{ToastAction, Toasts, WaysOut, opened_folder, take_retry};
use crate::intents;

const FAILURE: &str = "Could not save the file";

fn harness(ways_out: WaysOut) -> Harness<'static, Toasts> {
    let mut toasts = Toasts::default();
    toasts.error(FAILURE, ways_out);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(900.0, 500.0))
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::activate(ctx, ThemePreference::Light);
        })
        .build_ui_state(|ui, toasts: &mut Toasts| toasts.show(ui.ctx()), toasts);
    harness.run_steps(2);
    harness
}

/// Clicks the toast's `label` and checks the toast left with it.
fn click(harness: &mut Harness<'static, Toasts>, label: &str) {
    harness.get_by_role_and_label(Role::Button, label).click();
    harness.run_steps(2);
    assert!(harness.query_by_label(FAILURE).is_none(), "the click dismissed the toast");
}

fn output_commands(harness: &Harness<'static, Toasts>) -> Vec<egui::OutputCommand> {
    harness.output().platform_output.commands.clone()
}

#[test]
fn toast_error_stays_until_dismissed_and_replaces_the_same_error() {
    let mut harness = harness(ToastAction::retry(egui::Id::unique("save")).into());
    harness.state_mut().error(FAILURE, ToastAction::retry(egui::Id::unique("save")));
    harness.run_steps(2);
    assert_eq!(harness.query_all_by_label(FAILURE).count(), 1, "the same error shows once");
    assert_eq!(harness.state().toasts[0].lifetime(), None, "an error waits to be dismissed");
    harness.get_by_role_and_label(Role::Button, "Dismiss").click();
    harness.run_steps(2);
    assert!(harness.query_by_label(FAILURE).is_none());
}

#[test]
fn toast_retry_asks_its_owner_once() {
    let id = egui::Id::unique("download");
    let mut harness = harness(ToastAction::retry(id).into());
    click(&mut harness, "Retry");
    assert!(take_retry(&harness.ctx, id), "the owner sees the request");
    assert!(!take_retry(&harness.ctx, id), "and runs it once");
}

#[test]
fn toast_fix_token_opens_the_connection_setup() {
    let mut harness = harness(ToastAction::fix_token("gh").into());
    click(&mut harness, "Fix token");
    assert_eq!(intents::take_fix_request(&harness.ctx).as_deref(), Some("gh"));
}

#[test]
fn toast_open_on_platform_opens_the_page() {
    let url = "https://github.com/acme/app/pull/7";
    let mut harness = harness(
        WaysOut::new(ToastAction::open_on_platform(ProviderKind::GitHub, url))
            .and(ToastAction::open_in_browser("https://example.com/pr/7")),
    );
    harness.get_by_role_and_label(Role::Button, "Open on GitHub").click();
    harness.step();
    let opened: Vec<String> = output_commands(&harness)
        .into_iter()
        .filter_map(|command| match command {
            egui::OutputCommand::OpenUrl(open) => Some(open.url),
            _ => None,
        })
        .collect();
    assert_eq!(opened, [url]);
}

#[test]
fn toast_open_folder_shows_the_folder() {
    let folder = PathBuf::from("C:/cache/viewed");
    let mut harness = harness(ToastAction::open_folder(folder.clone()).into());
    click(&mut harness, "Open folder");
    assert_eq!(opened_folder(&harness.ctx), Some(folder));
}

#[test]
fn toast_copy_puts_the_text_on_the_clipboard() {
    let mut harness = harness(ToastAction::copy("Copy command", "claude --resume 42").into());
    harness.get_by_role_and_label(Role::Button, "Copy command").click();
    harness.step();
    let copied = output_commands(&harness).into_iter().find_map(|command| match command {
        egui::OutputCommand::CopyText(text) => Some(text),
        _ => None,
    });
    assert_eq!(copied.as_deref(), Some("claude --resume 42"));
}
