//! Settings → Appearance → Frame rate limit, offline: one GitHub connection answered by a scripted
//! provider. Checks the default of 45, that a choice applies at once, is saved and moves nothing
//! around it.
//! `cargo test -p review-app frame_rate`

use egui::{Rect, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{Connection, ProviderKind};
use review_store::{AppPaths, AppSettings, ConnectionStore, FrameRateLimit};

use super::ReviewApp;
use super::provider_icons_walkthrough::scripted_provider;
use cv_runtime::route::Route;
use cv_settings::onboarding::CliTools;
use cv_testkit::Clicks;

const ROW: &str = "Frame rate limit";
/// The row below the control: it must not move when the choice changes.
const NEXT_ROW: &str = "Pane sizes";

#[test]
fn frame_rate_limit_defaults_to_45_applies_live_and_is_saved() {
    let paths = isolated_paths();
    let mut store = ConnectionStore::open(&paths).expect("empty store");
    store
        .upsert(Connection {
            id: "gh".to_owned(),
            name: "Contoso GitHub".to_owned(),
            kind: ProviderKind::GitHub,
            base_url: "https://github.com".to_owned(),
            organizations: Vec::new(),
            token: Some("scripted".to_owned()),
        })
        .expect("store the connection");
    let app_paths = paths.clone();
    let builder = Harness::builder().with_size([1280.0, 1000.0]).with_step_dt(1.0 / 60.0);
    let mut harness = super::startup::start(builder, move |creation| {
        ReviewApp::new(creation, app_paths, scripted_provider)
    });
    harness.state_mut().settings.cli_tools = Some(CliTools::default());
    harness.state_mut().navigator.go(Route::Settings);
    settle(&mut harness);
    harness.click_at(harness.get_by_role_and_label(Role::Tab, "Appearance").rect());
    wait_for(&mut harness, "the frame rate row", |harness| harness.query_by_label(ROW).is_some());
    // The row sits below the fold of Appearance.
    harness.get_by_label(NEXT_ROW).scroll_to_me();
    settle(&mut harness);

    // Users without a saved limit get 45, and the control shows it.
    assert_eq!(harness.state().services.settings.frame_rate, FrameRateLimit::DEFAULT);
    assert!(is_selected(&harness, "45"), "45 is selected by default");
    let every_option =
        FrameRateLimit::PRESETS.iter().map(u16::to_string).chain(["Match display".to_owned()]);
    for option in every_option {
        assert!(
            harness.query_by_role_and_label(Role::RadioButton, &option).is_some(),
            "{option} is offered"
        );
    }
    let next_row = rect_of(&harness, NEXT_ROW);
    let control = segments_rect(&harness);

    // A click applies at once (the pacer reads the setting each frame) and is saved.
    for (label, limit) in [
        ("144", FrameRateLimit::Fps(144)),
        ("Match display", FrameRateLimit::MatchDisplay),
        ("30", FrameRateLimit::Fps(30)),
    ] {
        harness.click_at(harness.get_by_role_and_label(Role::RadioButton, label).rect());
        settle(&mut harness);
        assert_eq!(harness.state().services.settings.frame_rate, limit, "{label} applies live");
        assert_eq!(saved(&paths).frame_rate, limit, "{label} is saved");
        assert!(is_selected(&harness, label), "{label} shows as selected");
        assert_eq!(rect_of(&harness, NEXT_ROW), next_row, "choosing {label} moves nothing below");
        assert_eq!(segments_rect(&harness), control, "choosing {label} keeps the control's size");
    }
    harness.get_by_role_and_label(Role::RadioButton, "Match display").hover();
    for _ in 0..60 {
        harness.step();
    }
    assert!(
        harness.query_by_label_contains("as the monitor refreshes").is_some(),
        "the option explains itself"
    );

    // Back to the default, as the next start would load it.
    harness.click_at(harness.get_by_role_and_label(Role::RadioButton, "45").rect());
    settle(&mut harness);
    assert_eq!(saved(&paths).frame_rate, FrameRateLimit::DEFAULT);
    drop(harness);
    cv_testkit::remove_dir(paths.config_dir().parent().expect("scratch root"));
}

fn is_selected(harness: &Harness<'_, ReviewApp>, label: &str) -> bool {
    harness.get_by_role_and_label(Role::RadioButton, label).accesskit_node().toggled()
        == Some(egui::accesskit::Toggled::True)
}

fn rect_of(harness: &Harness<'_, ReviewApp>, label: &str) -> Rect {
    harness.get_by_label(label).rect()
}

/// The segmented control, from its first segment to its last.
fn segments_rect(harness: &Harness<'_, ReviewApp>) -> Rect {
    rect_of(harness, "30")
        .union(harness.get_by_role_and_label(Role::RadioButton, "Match display").rect())
}

fn saved(paths: &AppPaths) -> AppSettings {
    AppSettings::load(&paths.settings_file()).expect("settings file")
}

fn isolated_paths() -> AppPaths {
    let root = std::env::temp_dir().join(format!("review-frame-rate-{}", std::process::id()));
    cv_testkit::remove_dir(&root);
    AppPaths::new(root.join("config"), root.join("cache"))
}

fn wait_for(
    harness: &mut Harness<'_, ReviewApp>,
    what: &str,
    is_done: impl Fn(&Harness<'_, ReviewApp>) -> bool,
) {
    super::walkthrough::wait_for(harness, what, |harness| is_done(harness).then_some(()), |_| {});
    settle(harness);
}

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    for _ in 0..30 {
        harness.step();
    }
}
