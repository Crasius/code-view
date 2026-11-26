//! Settings → Links → Browser extension like a user, offline: a registration for another copy of
//! Code View shows Outdated with Update and Unregister; Update registers this copy for Chrome and
//! Edge (keys and manifest, extension unpacked); Unregister takes it out and offers Register again;
//! "Open extension folder" shows the unpacked extension. The registry is a map in memory and every
//! file sits in a scratch folder. `cargo test -p cv-settings browser_extension_walkthrough`

use std::sync::Arc;
use std::time::{Duration, Instant};

use cv_runtime::route::Navigator;
use cv_services::Services;
use cv_shell::browser::registration::{KeyStore, MemoryKeys, Registry};
use cv_shell::browser::{self, HOST_NAME};
use cv_testkit::ScratchDir;
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{Connection, ProviderKind};
use review_store::{AppPaths, ConnectionStore};

use super::BrowserExtension;
use crate::settings::SettingsPage;
use crate::settings::category::Category;

const TIMEOUT: Duration = Duration::from_secs(30);
const CHROME_KEY: &str = r"Software\Google\Chrome\NativeMessagingHosts\de.crasius.codeview";
const EDGE_KEY: &str = r"Software\Microsoft\Edge\NativeMessagingHosts\de.crasius.codeview";

struct Shown {
    page: SettingsPage,
    services: Services,
    navigator: Navigator,
}

fn harness(paths: AppPaths, registry: Registry) -> Harness<'static, Shown> {
    let mut store = ConnectionStore::open(&paths).expect("empty store");
    store
        .upsert(Connection {
            id: "gh".to_owned(),
            name: "Acme".to_owned(),
            kind: ProviderKind::GitHub,
            base_url: "https://github.com".to_owned(),
            organizations: Vec::new(),
            token: None,
        })
        .expect("saved");
    let services = Services::new(egui::Context::default(), paths, cv_providers::create_provider);
    let mut page = SettingsPage {
        browser_extension: BrowserExtension::with_registry(registry),
        ..SettingsPage::default()
    };
    page.navigation.open(Category::Links);
    Harness::builder()
        .with_size([1280.0, 900.0])
        .with_context_setup(|ctx| {
            cv_base::theme::fonts::install(ctx);
            cv_base::theme::motion::set_reduced(ctx, false);
        })
        .build_ui_state(
            |ui, state: &mut Shown| state.page.show(ui, &mut state.services, &mut state.navigator),
            Shown { page, services, navigator: Navigator::default() },
        )
}

fn has(harness: &Harness<'_, Shown>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

fn has_button(harness: &Harness<'_, Shown>, label: &str) -> bool {
    harness.query_by_role_and_label(Role::Button, label).is_some()
}

fn wait_for(
    harness: &mut Harness<'_, Shown>,
    what: &str,
    is_done: impl Fn(&Harness<'_, Shown>) -> bool,
) {
    let started = Instant::now();
    loop {
        harness.step();
        if is_done(harness) {
            harness.run_steps(3);
            return;
        }
        assert!(started.elapsed() < TIMEOUT, "timed out waiting for {what}");
        cv_testkit::let_threads_run(Duration::from_millis(10));
    }
}

#[test]
fn browser_extension_walkthrough_updates_unregisters_and_opens_the_folder() {
    let scratch = ScratchDir::new("settings-browser-extension");
    let paths = AppPaths::new(scratch.path().join("config"), scratch.path().join("cache"));
    let keys = Arc::new(MemoryKeys::default());
    let manifest = paths.browser_dir().join(format!("{HOST_NAME}.json"));
    let registry = Registry::Keys { manifest: manifest.clone(), keys: keys.clone() };
    // Registered earlier by a copy of Code View that has moved since.
    registry
        .register(&scratch.path().join("old").join("CodeView.exe"))
        .expect("an old registration");
    let mut harness = harness(paths.clone(), registry);

    assert!(has(&harness, "Browser extension"), "the row in Links");
    wait_for(&mut harness, "the status", |harness| {
        has(harness, "Registered for another copy of Code View")
    });
    assert!(has_button(&harness, "Update") && has_button(&harness, "Unregister"));
    assert!(!has_button(&harness, "Register"));

    harness.get_by_role_and_label(Role::Button, "Update").click();
    wait_for(&mut harness, "the registration", |harness| {
        has(harness, "Registered for Chrome and Edge")
    });
    let exe = std::env::current_exe().expect("test binary");
    let text = std::fs::read_to_string(&manifest).expect("the manifest");
    let written: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(written["path"], exe.to_string_lossy().as_ref(), "the manifest starts this copy");
    for key in [CHROME_KEY, EDGE_KEY] {
        assert_eq!(
            keys.value(key).expect("key"),
            Some(manifest.to_string_lossy().into_owned()),
            "{key}"
        );
    }
    let unpacked = browser::extension_dir(&paths);
    assert!(
        unpacked.join("manifest.json").is_file(),
        "the extension is unpacked for Load unpacked"
    );
    assert!(has_button(&harness, "Unregister") && !has_button(&harness, "Update"));
    assert!(has(&harness, "Load unpacked"), "the one-line instruction");

    harness.get_by_role_and_label(Role::Button, "Unregister").click();
    wait_for(&mut harness, "the unregistration", |harness| has(harness, "Not registered"));
    assert!(has_button(&harness, "Register") && !has_button(&harness, "Unregister"));
    // The rows are as tall as their content: they once each took all the height left below them,
    // which pushed the buttons a screen apart.
    let rect = |label: &str| harness.get_by_role_and_label(Role::Button, label).rect();
    let (register, open) = (rect("Register"), rect("Open extension folder"));
    let title = harness.get_by_label("Browser extension").rect();
    assert!(
        register.top() - title.bottom() < 80.0,
        "status row under the title: {title:?} {register:?}"
    );
    assert!(
        open.top() - register.bottom() < 80.0,
        "folder row under the status: {register:?} {open:?}"
    );
    assert_eq!(keys.value(CHROME_KEY).expect("key"), None);
    assert_eq!(keys.value(EDGE_KEY).expect("key"), None);
    assert!(!manifest.exists());

    std::fs::remove_dir_all(&unpacked).expect("remove the unpacked extension");
    harness.get_by_role_and_label(Role::Button, "Open extension folder").click();
    wait_for(&mut harness, "the folder", |harness| {
        cv_runtime::toasts::opened_folder(&harness.ctx).is_some()
    });
    assert_eq!(cv_runtime::toasts::opened_folder(&harness.ctx), Some(unpacked.clone()));
    assert!(unpacked.join("manifest.json").is_file(), "opening unpacks it again");
    assert_eq!(harness.state().services.toasts.error_texts(), Vec::<&str>::new());
}
