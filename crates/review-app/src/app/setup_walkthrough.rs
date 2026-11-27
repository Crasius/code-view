//! Walks the connection wizard and the "Fix token" path like a user, against a scripted provider:
//! no network, no real tokens, no command-line tools ([`super::scripted_gitlab`]). Runs with
//! `cargo test`.

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{Connection, ProviderKind};
use review_store::{ConnectionStore, LINUX_HELP_URL, TokenVault, VaultProblem};

use super::ReviewApp;
use super::scripted_gitlab::{COMPLETE_TOKEN, LIMITED_TOKEN, app_on_scripted_gitlab};
use super::walkthrough::{press, saved_paths, settle_background, wait_until};
use cv_testkit::{Clicks, ScratchDir, samples};

#[test]
fn wizard_first_run_to_saved_connection() {
    let scratch = ScratchDir::new("setup-first-run");
    let paths = scratch.app_paths();
    let mut harness = app_on_scripted_gitlab(paths.clone());

    to_gitlab_token_step(&mut harness);

    wait_until(&mut harness, "the token step", |harness| {
        harness.query_by_role(Role::PasswordInput).is_some()
    });
    type_token(&mut harness, LIMITED_TOKEN);
    wait_until(&mut harness, "the access report", |harness| {
        harness.query_by_label_contains("Some features won't work").is_some()
    });
    assert!(harness.query_by_label_contains("Needs Merge Request: Read.").is_some());

    harness.click_at(harness.get_by_label("Paste a new token").rect());
    wait_until(&mut harness, "the token step again", |harness| {
        harness.query_by_role(Role::PasswordInput).is_some()
    });
    complete_token_to_groups(&mut harness);
    harness.click_at(harness.get_by_label_contains("Only selected groups").rect());
    wait_until(&mut harness, "discovered groups", |harness| {
        harness.query_by_label("TTRPG").is_some()
    });
    harness.click_at(harness.get_by_label("TTRPG").rect());
    harness.run_steps(4);

    harness.click_at(harness.get_by_label("Save connection").rect());
    wait_until(&mut harness, "the saved confirmation", |harness| {
        harness.query_by_label_contains("is connected").is_some()
    });

    let store = ConnectionStore::open(&paths).expect("saved store");
    let saved = store.all().first().expect("one connection");
    assert_eq!(saved.kind, ProviderKind::GitLab);
    assert_eq!(saved.base_url, "https://gitlab.com");
    assert_eq!(saved.organizations, ["ttrpg"]);
    assert_eq!(saved.name, "ada on gitlab.com");
    assert_eq!(saved.token(), Some(COMPLETE_TOKEN));
}

/// A save that fails (here: a file where the config folder belongs) keeps the wizard on its step
/// with the cause, the file it tried, and "Try again"; nothing is a silent no-op.
#[test]
fn save_failure_stays_on_the_step_with_its_cause() {
    let scratch = ScratchDir::new("setup-save-failure");
    let paths = scratch.app_paths();
    let mut harness = app_on_scripted_gitlab(paths.clone());
    to_gitlab_token_step(&mut harness);
    wait_until(&mut harness, "the token step", |harness| {
        harness.query_by_role(Role::PasswordInput).is_some()
    });
    complete_token_to_groups(&mut harness);

    cv_testkit::remove_dir(paths.config_dir());
    std::fs::create_dir_all(paths.config_dir().parent().expect("the config folder's parent"))
        .expect("the temp root");
    std::fs::write(paths.config_dir(), b"not a folder").expect("block the config folder");
    harness.click_at(harness.get_by_label("Save connection").rect());
    wait_until(&mut harness, "the save error", |harness| {
        harness.query_by_label_contains("The connection could not be saved").is_some()
    });
    assert!(
        harness.query_by_label_contains("connections.json").is_some(),
        "the message names the file"
    );
    assert!(harness.query_by_label_contains("is connected").is_none());

    std::fs::remove_file(paths.config_dir()).expect("unblock the config folder");
    press(&mut harness, "Try again");
    wait_until(&mut harness, "the saved confirmation", |harness| {
        harness.query_by_label_contains("is connected").is_some()
    });
    let store = ConnectionStore::open(&paths).expect("saved store");
    assert_eq!(store.all().first().and_then(Connection::token), Some(COMPLETE_TOKEN));
}

/// A token store that cannot keep tokens (a Linux session without a keyring) shows its fix on the
/// token step and again before the save, which then fails with that fix and offers to save the
/// connection without its token.
#[test]
fn missing_keyring_offers_a_tokenless_save() {
    let problem: &'static VaultProblem = Box::leak(Box::new(VaultProblem {
        title: "No keyring answers on this desktop session".to_owned(),
        fix: "Install GNOME Keyring, start it, then check again.".to_owned(),
        commands: vec!["gnome-keyring-daemon --start --components=secrets".to_owned()],
        help_url: LINUX_HELP_URL,
    }));
    let scratch = ScratchDir::new("setup-missing-keyring");
    let paths = scratch.app_paths().with_token_vault(TokenVault::Refusing(problem));
    let mut harness = app_on_scripted_gitlab(paths.clone());
    to_gitlab_token_step(&mut harness);
    wait_until(&mut harness, "the keyring message", |harness| {
        harness.query_by_label_contains(&problem.title).is_some()
    });
    assert!(harness.query_by_label("Copy commands").is_some());
    assert!(
        harness.query_by_label("Save without the token").is_none(),
        "nothing to save on the token step yet"
    );

    complete_token_to_groups(&mut harness);
    assert!(
        harness.query_by_label_contains(&problem.title).is_some(),
        "the fix shows before the save"
    );

    press(&mut harness, "Save connection");
    settle_background(&mut harness);
    assert!(
        harness.query_by_label_contains("is connected").is_none(),
        "the refused token keeps the wizard on its step"
    );
    assert!(
        harness.query_by_label_contains("The connection could not be saved").is_none(),
        "the keyring's fix, not a bare error"
    );

    press(&mut harness, "Save without the token");
    wait_until(&mut harness, "the saved confirmation", |harness| {
        harness.query_by_label_contains("is connected").is_some()
    });
    let store = ConnectionStore::open(&paths).expect("saved store");
    let saved = store.all().first().expect("one connection");
    assert_eq!(saved.token(), None);
}

#[test]
fn token_pages_offer_cli_and_checklists() {
    let scratch = ScratchDir::new("setup-token-pages");
    let mut harness = app_on_scripted_gitlab(scratch.app_paths());

    wait_until(&mut harness, "the welcome screen", |harness| {
        harness.query_by_label_contains("GitHub.com or GitHub").is_some()
    });
    harness.click_at(harness.get_by_label_contains("GitHub.com or GitHub").rect());
    wait_until(&mut harness, "the host step", |harness| {
        harness.query_by_label("Continue").is_some()
    });
    harness.click_at(harness.get_by_label("Continue").rect());
    wait_until(&mut harness, "the GitHub CLI option", |harness| {
        harness.query_by_label("Use GitHub CLI sign-in").is_some()
    });
    assert!(harness.query_by_label_contains("Create token on github.com").is_some());

    press(&mut harness, BACK);
    wait_until(&mut harness, "the host step", |harness| harness.query_by_label(BACK).is_some());
    press(&mut harness, BACK);
    wait_until(&mut harness, "the platforms", |harness| {
        harness.query_by_label_contains("Azure DevOps Services or").is_some()
    });
    harness.click_at(harness.get_by_label_contains("Azure DevOps Services or").rect());
    wait_until(&mut harness, "the organization field", |harness| {
        harness.query_by_role(Role::TextInput).is_some()
    });
    harness.get_by_role(Role::TextInput).focus();
    harness.get_by_role(Role::TextInput).type_text("https://dev.azure.com/contoso/Web/_git/site");
    harness.run_steps(4);
    assert!(harness.query_by_label_contains("Connects to dev.azure.com/contoso").is_some());
    harness.click_at(harness.get_by_label("Continue").rect());
    wait_until(&mut harness, "the Azure checklist", |harness| {
        harness.query_by_label("Code: Read & write").is_some()
    });
    assert!(harness.query_by_label("Use Azure CLI sign-in").is_some());
}

#[test]
fn fix_token_from_a_denied_request() {
    let scratch = ScratchDir::new("setup-fix-token");
    let connection = Connection {
        name: "Tabletop".into(),
        organizations: vec!["ttrpg".into()],
        token: Some(LIMITED_TOKEN.into()),
        ..samples::connection("gl-1", ProviderKind::GitLab, "https://gitlab.com")
    };
    let mut harness = app_on_scripted_gitlab(saved_paths(&scratch, &[connection]));

    wait_until(&mut harness, "the access banner", |harness| {
        harness.query_by_label_contains("Tabletop: The fine-grained token").is_some()
    });
    harness.click_at(harness.get_by_label_contains("Fix token").rect());
    wait_until(&mut harness, "the fix screen", |harness| {
        harness.query_by_role(Role::PasswordInput).is_some()
    });
    assert!(
        harness
            .query_by_label_contains("The fine-grained token lacks Merge Request: Read.")
            .is_some()
    );
}

/// The wizard's Back button (its arrow icon is painted, not part of the name).
const BACK: &str = "Back";

fn type_token(harness: &mut Harness<'_, ReviewApp>, token: &str) {
    harness.get_by_role(Role::PasswordInput).focus();
    harness.step();
    harness.get_by_role(Role::PasswordInput).type_text(token);
    harness.step();
}

/// From the welcome screen through GitLab's host step to its token step.
fn to_gitlab_token_step(harness: &mut Harness<'_, ReviewApp>) {
    wait_until(harness, "the welcome screen", |harness| {
        harness.query_by_label_contains("GitLab.com or self-managed").is_some()
    });
    harness.click_at(harness.get_by_label_contains("GitLab.com or self-managed").rect());
    wait_until(harness, "the host step", |harness| harness.query_by_label("Continue").is_some());
    harness.click_at(harness.get_by_label("Continue").rect());
}

/// Types the complete token on the token step and goes on to the groups step.
fn complete_token_to_groups(harness: &mut Harness<'_, ReviewApp>) {
    type_token(harness, COMPLETE_TOKEN);
    wait_until(harness, "a complete report", |harness| {
        harness.query_by_label_contains("Everything works with this token").is_some()
    });
    harness.click_at(harness.get_by_label("Continue").rect());
    wait_until(harness, "the groups step", |harness| {
        harness.query_by_label("Save connection").is_some()
    });
}
