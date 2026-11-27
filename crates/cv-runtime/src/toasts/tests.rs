//! Each kind of way out on an error toast: the button shows, and clicking it does its one thing
//! (asks for the retry, opens the token fix, the web page or the folder, copies) and dismisses
//! the toast. Opening a folder is stubbed in test builds ([`super::opened_folder`]). Expiry runs
//! on simulated input time with the fades off.
//! `cargo test -p cv-runtime toast`

use std::path::PathBuf;

use cv_base::theme::{self, motion};
use cv_testkit::Clicks;
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::ProviderKind;
use review_store::ThemePreference;

use super::{
    SHOW_FOR, SHOW_WITH_ACTIONS_FOR, ToastAction, Toasts, WaysOut, opened_folder, take_retry,
};
use crate::intents;

const FAILURE: &str = "Could not save the file";

fn harness_of(toasts: Toasts) -> Harness<'static, Toasts> {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(900.0, 500.0))
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::activate(ctx, ThemePreference::Light);
            motion::set_reduced(ctx, true);
        })
        .build_ui_state(|ui, toasts: &mut Toasts| toasts.show(ui.ctx()), toasts);
    harness.run();
    harness
}

fn harness(ways_out: WaysOut) -> Harness<'static, Toasts> {
    let mut toasts = Toasts::default();
    toasts.error(FAILURE, ways_out);
    harness_of(toasts)
}

/// Runs frames from input time `seconds` on.
fn run_at(harness: &mut Harness<'static, Toasts>, seconds: f64) {
    harness.input_mut().time = Some(seconds);
    harness.run();
}

/// Clicks the toast's `label` and checks the toast left with it.
fn click(harness: &mut Harness<'static, Toasts>, label: &str) {
    harness.click_at(harness.get_by_role_and_label(Role::Button, label).rect());
    harness.run();
    assert!(harness.query_by_label(FAILURE).is_none(), "the click dismissed the toast");
}

fn output_commands(harness: &Harness<'static, Toasts>) -> Vec<egui::OutputCommand> {
    harness.output().platform_output.commands.clone()
}

#[test]
fn error_stays_until_dismissed_and_replaces_the_same_error() {
    let mut harness = harness(ToastAction::retry(egui::Id::unique("save")).into());
    harness.state_mut().error(FAILURE, ToastAction::retry(egui::Id::unique("save")));
    harness.run();
    assert_eq!(harness.query_all_by_label(FAILURE).count(), 1, "the same error shows once");
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Dismiss").rect());
    harness.run();
    assert!(harness.query_by_label(FAILURE).is_none());
}

#[test]
fn toasts_expire_by_severity_and_actions() {
    let mut toasts = Toasts::default();
    toasts.success("Saved");
    toasts.info("Moved on", vec![ToastAction::copy("Copy path", "C:/x")]);
    toasts.error(FAILURE, ToastAction::retry(egui::Id::unique("save")));
    let mut harness = harness_of(toasts);
    let shown_at = harness.ctx.input(|input| input.time);
    let after = |lifetime: std::time::Duration| shown_at + lifetime.as_secs_f64() + 0.1;
    let cases = [
        (shown_at, vec!["Saved", "Moved on", FAILURE]),
        (after(SHOW_FOR), vec!["Moved on", FAILURE]),
        (after(SHOW_WITH_ACTIONS_FOR), vec![FAILURE]),
        (after(SHOW_WITH_ACTIONS_FOR) + 600.0, vec![FAILURE]),
    ];
    for (seconds, expected) in cases {
        run_at(&mut harness, seconds);
        assert_eq!(harness.state().texts(), expected, "the toasts left at {seconds} s");
    }
}

#[test]
fn a_toast_with_buttons_waits_while_the_pointer_is_on_it() {
    let mut toasts = Toasts::default();
    toasts.info("Moved on", vec![ToastAction::copy("Copy path", "C:/x")]);
    let mut harness = harness_of(toasts);
    let shown_at = harness.ctx.input(|input| input.time);
    harness.event(egui::Event::PointerMoved(harness.get_by_label("Moved on").rect().center()));
    let half = SHOW_WITH_ACTIONS_FOR.as_secs_f64() / 2.0 + 0.1;
    for step in 1..=3 {
        run_at(&mut harness, shown_at + half * f64::from(step));
        assert_eq!(harness.state().texts(), ["Moved on"], "held after {step} half lifetimes");
    }
    harness.event(egui::Event::PointerGone);
    run_at(&mut harness, shown_at + half * 3.0 + SHOW_WITH_ACTIONS_FOR.as_secs_f64() + 0.1);
    assert!(harness.state().texts().is_empty(), "it starts over once the pointer left");
}

#[test]
fn retry_asks_its_owner_once() {
    let id = egui::Id::unique("download");
    let mut harness = harness(ToastAction::retry(id).into());
    click(&mut harness, "Retry");
    assert!(
        !take_retry(&harness.ctx, egui::Id::unique("other")),
        "another owner's id stays unasked"
    );
    assert!(take_retry(&harness.ctx, id), "the owner sees the request");
    assert!(!take_retry(&harness.ctx, id), "and runs it once");
}

#[test]
fn fix_token_opens_the_connection_setup() {
    let mut harness = harness(ToastAction::fix_token("gh").into());
    click(&mut harness, "Fix token");
    assert_eq!(intents::take_fix_request(&harness.ctx).as_deref(), Some("gh"));
}

#[test]
fn open_on_platform_opens_the_page() {
    let url = "https://github.com/acme/app/pull/7";
    let mut harness = harness(
        WaysOut::new(ToastAction::open_on_platform(ProviderKind::GitHub, url))
            .and(ToastAction::open_in_browser("https://example.com/pr/7")),
    );
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Open on GitHub").rect());
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
fn open_folder_shows_the_folder() {
    let folder = PathBuf::from("C:/cache/viewed");
    let mut harness = harness(ToastAction::open_folder(folder.clone()).into());
    click(&mut harness, "Open folder");
    assert_eq!(opened_folder(&harness.ctx), Some(folder));
}

#[test]
fn copy_puts_the_text_on_the_clipboard() {
    let mut harness = harness(ToastAction::copy("Copy command", "claude --resume 42").into());
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Copy command").rect());
    harness.step();
    let copied = output_commands(&harness).into_iter().find_map(|command| match command {
        egui::OutputCommand::CopyText(text) => Some(text),
        _ => None,
    });
    assert_eq!(copied.as_deref(), Some("claude --resume 42"));
}
