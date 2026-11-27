//! Platform marks across the app, offline: one Azure DevOps, one GitHub and one self-managed
//! GitLab connection answered by a scripted provider. Checks that every mark carries the platform
//! and host as its accessible name: on the overview, each connection's breadcrumb and list, and
//! a pull request header, whose mark links to the pull request.
//! `cargo test -p review-app provider_icons`

use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use review_core::{Connection, ProviderKind};

use super::ReviewApp;
use super::scripted_platforms::{HOSTS, origin, platforms, title};
use super::walkthrough::{
    app_with, files_tab, has, opened_urls, press, saved_paths, settle_background, shows,
    shows_folder_view, wait_until,
};
use cv_runtime::route::{PullRequestKey, Route};
use cv_testkit::{Clicks, ScratchDir, samples};

#[test]
fn provider_icons_name_the_platform_everywhere() {
    let scratch = ScratchDir::new("provider-icons");
    // GitHub's pull requests come from a local repository, so its Files tab has a file.
    let origin = origin(&scratch);
    let connections: Vec<Connection> = HOSTS
        .iter()
        .map(|host| Connection {
            name: host.name.to_owned(),
            organizations: if host.kind == ProviderKind::AzureDevOps {
                vec!["contoso".to_owned()]
            } else {
                Vec::new()
            },
            token: Some("scripted".to_owned()),
            ..samples::connection(host.id, host.kind, host.base_url)
        })
        .collect();
    let builder = Harness::builder().with_size([1280.0, 1080.0]).with_step_dt(1.0 / 60.0);
    let mut harness =
        app_with(builder, saved_paths(&scratch, &connections), platforms(Some(origin)));

    wait_until(&mut harness, "every card", |harness| {
        HOSTS.iter().all(|host| has(harness, &title(host.kind, 1)))
    });
    for host in &HOSTS {
        // Card header plus one per row: never a mark without its platform and host.
        assert!(
            harness.query_all_by_role_and_label(Role::Image, host.mark).count() >= 3,
            "{} on the card and its rows",
            host.mark
        );
    }
    harness.state_mut().navigator.go(Route::MyPullRequests);
    settle_background(&mut harness);
    for host in &HOSTS {
        // As from outside: the trail is the structure, with the connection's crumb.
        harness.state_mut().navigator.jump(Route::RepositoryPullRequests {
            connection_id: host.id.to_owned(),
            repository_key: host.repository.to_owned(),
        });
        wait_until(&mut harness, "the repository's rows", |harness| {
            has(harness, &title(host.kind, 1))
        });
        assert!(
            harness.query_by_role_and_label(Role::Link, host.name).is_some(),
            "{} crumb",
            host.name
        );
        if host.kind == ProviderKind::AzureDevOps {
            crumb_tooltip(&mut harness, host.name, host.mark);
        }
    }
    for host in &HOSTS {
        let key = PullRequestKey {
            connection_id: host.id.to_owned(),
            repository_key: host.repository.to_owned(),
            number: 1,
        };
        harness.state_mut().navigator.go(Route::PullRequest(key));
        // The page keeps the tab it showed last; its header names it on every tab.
        let shown = title(host.kind, 1);
        wait_until(&mut harness, "the pull request header", |harness| {
            harness.query_by_role_and_label(Role::Tab, "Overview").is_some() && has(harness, &shown)
        });
    }
    // GitHub's page was opened last: its platform mark links to the pull request.
    platform_link(&mut harness);
}

/// Hovering the connection's crumb opens a tooltip that names the platform and host too.
fn crumb_tooltip(harness: &mut Harness<'_, ReviewApp>, connection_name: &str, mark: &str) {
    let marks = harness.query_all_by_label(mark).count();
    harness.get_by_role_and_label(Role::Link, connection_name).hover();
    wait_until(harness, "the crumb's tooltip", |harness| {
        has(harness, "This connection's repositories")
    });
    assert!(harness.query_all_by_label(mark).count() > marks, "the crumb's tooltip names {mark}");
    harness.event(egui::Event::PointerGone);
    wait_until(harness, "the crumb's tooltip closed", |harness| {
        !has(harness, "This connection's repositories")
    });
}

/// The pull request header's platform mark: a link (Enter opens the pull request), with a hover
/// state, and a menu that adds the file the Files tab shows.
fn platform_link(harness: &mut Harness<'_, ReviewApp>) {
    const LINK: &str = "Open pull request on GitHub · github.com";
    const PAGE: &str = "https://github.com/contoso/widgets/pull/1";
    let key = PullRequestKey {
        connection_id: "gh".to_owned(),
        repository_key: "contoso/widgets".to_owned(),
        number: 1,
    };
    harness.state_mut().navigator.go(Route::PullRequest(key));
    wait_until(harness, "the GitHub pull request", |harness| {
        harness.query_by_role_and_label(Role::Link, LINK).is_some()
    });
    harness.get_by_role_and_label(Role::Link, LINK).focus();
    harness.step();
    assert!(
        harness.get_by_role_and_label(Role::Link, LINK).is_focused(),
        "the mark takes keyboard focus"
    );
    // One event per frame: the key-down frame is the one that opens the page.
    let enter = |pressed| egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    harness.event(enter(true));
    harness.step();
    assert_eq!(opened_urls(harness), [PAGE], "Enter on the focused mark opens the pull request");
    harness.event(enter(false));
    harness.step();
    // A click (on release) and a middle click open it too.
    harness.click_at(harness.get_by_role_and_label(Role::Link, LINK).rect());
    harness.step();
    assert_eq!(opened_urls(harness), [PAGE], "a click opens the pull request");
    let at = harness.get_by_role_and_label(Role::Link, LINK).rect().center();
    hooked_middle_click(harness, at);
    assert_eq!(opened_urls(harness), [PAGE], "a middle click opens the pull request");
    assert!(!harness.state().autoscroll.is_active(), "and starts no autoscroll");
    harness.event(egui::Event::PointerGone);
    settle_background(harness);
    harness.get_by_role_and_label(Role::Link, LINK).hover();
    wait_until(harness, "the mark's tooltip", |harness| has(harness, "Right-click for more links"));

    // On the Files tab the menu also offers the shown file.
    files_tab(harness);
    // The tab opens on the summary; the card's View opens the file.
    press(harness, "View README.md");
    wait_until(harness, "README.md's view", |harness| !shows_folder_view(harness));
    harness.secondary_click_at(harness.get_by_role_and_label(Role::Link, LINK).rect());
    wait_until(harness, "the mark's menu", |harness| shows(harness, "Copy link"));
    for item in ["Open pull request on GitHub", "Open this file on GitHub", "Copy link"] {
        assert!(harness.query_by_label(item).is_some(), "menu item {item}");
    }
    harness.click_at(harness.get_by_label("Open this file on GitHub").rect());
    harness.step();
    let opened = opened_urls(harness);
    assert!(
        opened.len() == 1
            && opened[0].starts_with("https://github.com/contoso/widgets/blob/")
            && opened[0].ends_with("/README.md"),
        "the menu opens the file at the head commit: {opened:?}"
    );
    // The file header carries no platform link; the mark's menu above is the one way out.
    assert!(
        harness.query_by_label("Open on GitHub").is_none(),
        "no platform button in the file header"
    );
}

/// A middle click at `at`, one frame each for press and release, whose raw input first runs
/// through the app's `raw_input_hook` as eframe does: autoscroll sees the press first.
fn hooked_middle_click(harness: &mut Harness<'_, ReviewApp>, at: egui::Pos2) {
    harness.hover_at(at);
    harness.step();
    for pressed in [true, false] {
        let ctx = harness.ctx.clone();
        let mut input = std::mem::take(harness.input_mut());
        input.events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Middle,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        eframe::App::raw_input_hook(harness.state_mut(), &ctx, &mut input);
        *harness.input_mut() = input;
        harness.step();
    }
}
