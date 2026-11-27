//! The integrated Windows title bar as it works on a real window: icon, breadcrumbs, search box
//! and caption buttons, the maximized state and the open search results. No network, no tokens.

use egui::{Key, Modifiers, Role, ViewportId};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::ReviewApp;
use super::walkthrough::app_with;
use cv_runtime::route::Route;
use cv_testkit::ScratchDir;

const WIDTH: f32 = 1280.0;

fn harness(scratch: &ScratchDir) -> Harness<'static, ReviewApp> {
    let builder = Harness::builder().with_size([WIDTH, 360.0]).with_step_dt(1.0 / 60.0);
    let mut harness = app_with(builder, scratch.app_paths(), cv_providers::create_provider);
    let app = harness.state_mut();
    app.title_bar.integrate_for_test();
    app.navigator.go(Route::RepositoryPullRequests {
        connection_id: "Acme GitHub".to_owned(),
        repository_key: "acme/widgets".to_owned(),
    });
    settle(&mut harness);
    harness
}

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    for _ in 0..30 {
        harness.step();
    }
}

fn set_window(harness: &mut Harness<'_, ReviewApp>, is_focused: bool, is_maximized: bool) {
    let viewport = harness.input_mut().viewports.entry(ViewportId::ROOT).or_default();
    viewport.focused = Some(is_focused);
    viewport.maximized = Some(is_maximized);
}

#[test]
fn title_bar_caption_buttons_maximized_and_search() {
    let scratch = ScratchDir::new("title-bar");
    let mut harness = harness(&scratch);

    for button in ["Minimize", "Maximize", "Close"] {
        assert!(
            harness.query_by_role_and_label(Role::Button, button).is_some(),
            "{button} caption button"
        );
    }
    assert!(
        harness.query_by_role_and_label(Role::Link, review_core::APP_NAME).is_some(),
        "the breadcrumbs moved into the title bar"
    );
    assert!(harness.query_by_label("acme/widgets (current page)").is_some());

    set_window(&mut harness, false, true);
    settle(&mut harness);
    assert!(
        harness.query_by_role_and_label(Role::Button, "Restore").is_some(),
        "maximized shows restore"
    );
    set_window(&mut harness, true, false);

    // Ctrl+K focuses the search; with nothing loaded the text still leads to the full list.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::K);
    harness.event(egui::Event::Text("widgets".to_owned()));
    settle(&mut harness);
    let filter = "Show all pull requests";
    assert!(harness.query_by_label(filter).is_some(), "the search offers the full list");
    harness.key_press(Key::Enter);
    settle(&mut harness);
    assert_eq!(harness.state().route, Route::MyPullRequests, "Enter runs the highlighted result");
    assert!(harness.query_by_label(filter).is_none(), "the results close after a pick");
}
