//! Settings → Storage like a user, headless: the sizes arrive from the retention thread, "Clear
//! cache" asks first and keeps viewed marks, and removing a connection deletes what it left.

use egui::Role;
use egui_kittest::kittest::Queryable;
use review_core::{Connection, ProviderKind};
use review_store::{ConnectionStore, stable_hash};

use super::setup_walkthrough::{harness, isolated_paths, press, wait_for};
use cv_runtime::route::Route;

#[test]
fn storage_measures_clears_after_asking_and_forgets_removed_connections() {
    let paths = isolated_paths("storage");
    let mut store = ConnectionStore::open(&paths).expect("empty store");
    store
        .upsert(Connection {
            id: "gl-1".into(),
            name: "Tabletop".into(),
            kind: ProviderKind::GitLab,
            base_url: "https://gitlab.com".into(),
            organizations: Vec::new(),
            token: None,
        })
        .expect("store the connection");
    let log = paths.connection_logs_dir("gl-1").join("job.log");
    let viewed = paths.viewed_dir().join(stable_hash("gl-1")).join("pr.json");
    for (file, bytes) in [(&log, 64), (&viewed, 2)] {
        std::fs::create_dir_all(file.parent().expect("a folder")).expect("create the folder");
        std::fs::write(file, vec![b'x'; bytes]).expect("write the file");
    }

    let mut harness = harness(paths);
    harness.state_mut().navigator.go(Route::Settings);
    harness.step();
    harness.get_by_role_and_label(Role::Tab, "Storage").click();
    wait_for(&mut harness, "the measured log size", |harness| {
        harness.query_by_label("64 B of 1 GB").is_some()
    });
    harness.get_by_label("64 B of 1 GB").scroll_to_me();
    harness.step();

    press(&mut harness, "Clear cache");
    wait_for(&mut harness, "the confirmation", |harness| {
        harness.query_by_label_contains("Delete all mirrors, logs and saved pages?").is_some()
    });
    assert!(log.exists(), "nothing is deleted before the confirmation");
    press(&mut harness, "Clear cache");
    wait_for(&mut harness, "the cleared summary", |harness| {
        harness.query_by_label_contains("Freed 64 B").is_some()
    });
    assert!(!log.exists());
    assert!(viewed.exists(), "clearing the cache keeps viewed marks");

    harness.get_by_role_and_label(Role::Tab, "Connections").click();
    harness.step();
    wait_for(&mut harness, "the connection's row", |harness| {
        harness.query_by_role_and_label(Role::Button, "Tabletop").is_some()
    });
    harness.get_by_role_and_label(Role::Button, "Tabletop").click();
    wait_for(&mut harness, "the connection's facts", |harness| {
        harness.query_by_label("Remove…").is_some()
    });
    press(&mut harness, "Remove…");
    wait_for(&mut harness, "the removal question", |harness| {
        harness.query_by_label("Remove connection").is_some()
    });
    press(&mut harness, "Remove connection");
    wait_for(&mut harness, "the removed connection's marks to go", |_| !viewed.exists());
}
