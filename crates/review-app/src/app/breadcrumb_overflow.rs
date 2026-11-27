//! A breadcrumb trail far longer than the title bar, as long names and a page's own levels make
//! it on an Azure DevOps pull request: at 1440 and 960 points the crumbs stay clear of the search
//! box and the caption buttons, the first and last crumbs and the pull request stay shown, the
//! levels between fold into "…" whose list leads to each of them (pointer and keyboard), and
//! nothing moves from one frame to the next. No network, no tokens.

use egui::{Key, Rect, Role, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::ReviewApp;
use super::walkthrough::app_with;
use cv_base::platform;
use cv_runtime::route::{Landing, PullRequestKey, Route};
use cv_runtime::trail::Trail;
use cv_shell::breadcrumbs;
use cv_testkit::{Clicks, ScratchDir};
use cv_widgets::focus;

/// No connection by this id is saved: the pull request page shows its error and leaves the trail
/// to the test, the crumb shows the id. Navigation refuses such a pull request, so the test puts
/// the page in place itself.
const CONNECTION: &str = "Private/Fabrikam-Industries";
const REPOSITORY: &str = "Fabrikam-Industries/Fabrikam Logistics/fabrikam-logistics-service";
const PULL_REQUEST: &str = "Pull request #741";
const FOLDERS: [&str; 6] =
    ["src", "Backend", "Apps", "FabrikamLogistics.Creator", "v1", "Features"];
const FILE: &str = "OrderFabrikamShipmentHandler.cs";
const OVERFLOW: &str = "Show hidden levels";
const BAR_HEIGHT: f32 = 40.0;

fn pull_request() -> Route {
    Route::PullRequest(PullRequestKey {
        connection_id: CONNECTION.to_owned(),
        repository_key: REPOSITORY.to_owned(),
        number: 741,
    })
}

/// Every crumb's name, from the app's down to the open file.
fn full_trail() -> Vec<String> {
    let mut trail = vec![
        review_core::APP_NAME.to_owned(),
        CONNECTION.to_owned(),
        REPOSITORY.to_owned(),
        PULL_REQUEST.to_owned(),
    ];
    trail.extend(FOLDERS.iter().map(|folder| (*folder).to_owned()));
    trail.push(FILE.to_owned());
    trail
}

/// Opens the pull request page in place of navigation, which refuses it (no connection).
fn enter_pull_request(harness: &mut Harness<'_, ReviewApp>) {
    let Route::PullRequest(key) = pull_request() else { unreachable!("a pull request route") };
    let app = harness.state_mut();
    app.open_pull_request(key, Landing::Default);
    app.route = pull_request();
    app.trail = Trail::structural(&pull_request());
    settle(harness);
}

/// The app on `scratch`, without a connection, at the pull request in a window `width` wide.
fn harness(scratch: &ScratchDir, width: f32) -> Harness<'static, ReviewApp> {
    let builder = Harness::builder().with_size([width, 360.0]).with_step_dt(1.0 / 60.0);
    let mut harness = app_with(builder, scratch.app_paths(), cv_providers::create_provider);
    harness.state_mut().title_bar.integrate_for_test();
    enter_pull_request(&mut harness);
    let mut levels: Vec<(String, &'static str)> =
        FOLDERS.iter().map(|folder| ((*folder).to_owned(), "Files in this folder")).collect();
    levels.push((FILE.to_owned(), "The open file"));
    breadcrumbs::publish_trail(
        &harness.ctx,
        &pull_request(),
        "Summary of all changed files",
        levels,
    );
    settle(&mut harness);
    harness
}

fn settle(harness: &mut Harness<'_, ReviewApp>) {
    harness.run_steps(20);
}

/// The crumbs shown in the title bar, left to right: name and rect.
fn shown_crumbs(harness: &Harness<'_, ReviewApp>) -> Vec<(String, Rect)> {
    let mut crumbs: Vec<(String, Rect)> = harness
        .query_all_by(|node| matches!(node.role(), Role::Link | Role::Label | Role::Button))
        .filter(|node| node.rect().bottom() <= BAR_HEIGHT)
        .filter_map(|node| {
            // A plain text node (the current page) carries its text as the value.
            let label = node.accesskit_node().label().or_else(|| node.value())?;
            let name =
                label.strip_suffix(" (current page)").map_or_else(|| label.clone(), str::to_owned);
            let is_crumb = full_trail().contains(&name) || name == OVERFLOW;
            is_crumb.then(|| (name, node.rect()))
        })
        .collect();
    crumbs.sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
    crumbs
}

/// The caption buttons and the search box: what the trail must never run under.
fn right_side(harness: &Harness<'_, ReviewApp>) -> Vec<(String, Rect)> {
    let mut rects: Vec<(String, Rect)> = ["Minimize", "Maximize", "Close"]
        .into_iter()
        .map(|label| (label.to_owned(), harness.get_by_role_and_label(Role::Button, label).rect()))
        .collect();
    let regions = platform::published_caption().expect("the bar publishes its hit-test regions");
    let holes = regions.holes();
    assert!(
        holes.len() >= 2,
        "the trail and the search box are carved out of the drag area: {holes:?}"
    );
    let search = holes[1];
    rects.push((
        "search box".to_owned(),
        Rect::from_min_max(
            pos2(search.left as f32, search.top as f32),
            pos2(search.right as f32, search.bottom as f32),
        ),
    ));
    rects
}

fn hidden_rows(harness: &Harness<'_, ReviewApp>) -> Vec<String> {
    harness
        .query_all_by_role(Role::MenuItem)
        .filter_map(|node| node.accesskit_node().label())
        .collect()
}

/// The shown crumbs keep clear of the right side, the ends and the pull request stay, the rest
/// is folded; returns the folded names in trail order.
fn check_layout(harness: &mut Harness<'_, ReviewApp>, width: f32) -> Vec<String> {
    let crumbs = shown_crumbs(harness);
    let names: Vec<&str> = crumbs.iter().map(|(name, _)| name.as_str()).collect();
    for (side, side_rect) in right_side(harness) {
        for (name, rect) in &crumbs {
            assert!(
                !rect.intersects(side_rect),
                "{width}: crumb {name} {rect:?} runs under the {side} {side_rect:?}"
            );
            assert!(
                rect.right() <= side_rect.left(),
                "{width}: crumb {name} {rect:?} is not left of the {side} {side_rect:?}"
            );
        }
    }
    assert_eq!(names.first(), Some(&review_core::APP_NAME), "{width}: {names:?}");
    assert_eq!(names.last(), Some(&FILE), "{width}: {names:?}");
    assert!(names.contains(&PULL_REQUEST), "{width}: the pull request stays shown: {names:?}");
    assert_eq!(names.get(1), Some(&OVERFLOW), "{width}: \"…\" follows the first crumb: {names:?}");

    // Nothing moves while nothing changes.
    for _ in 0..3 {
        harness.step();
        assert_eq!(shown_crumbs(harness), crumbs, "{width}: the crumbs moved between frames");
    }
    full_trail().into_iter().filter(|name| !names.contains(&name.as_str())).collect()
}

fn open_overflow(harness: &mut Harness<'_, ReviewApp>) {
    harness.click_at(harness.get_by_role_and_label(Role::Button, OVERFLOW).rect());
    settle(harness);
}

#[test]
fn breadcrumbs_fold_into_an_overflow_list_and_stay_clear_of_the_caption() {
    for (width, tag) in [(1440.0, "1440"), (960.0, "960")] {
        let scratch = ScratchDir::new(&format!("breadcrumbs-{tag}"));
        let mut harness = harness(&scratch, width);
        let hidden = check_layout(&mut harness, width);
        assert!(
            hidden.contains(&CONNECTION.to_owned()) && hidden.contains(&REPOSITORY.to_owned()),
            "{width}: {hidden:?}"
        );

        settle(&mut harness);
        open_overflow(&mut harness);
        assert_eq!(
            hidden_rows(&harness),
            hidden,
            "{width}: the list names the folded crumbs in trail order"
        );
        harness.key_press(Key::Escape);
        settle(&mut harness);
        assert!(hidden_rows(&harness).is_empty(), "{width}: Esc closes the list");

        // A folded level leads exactly where its crumb would.
        open_overflow(&mut harness);
        harness.click_at(harness.get_by_role_and_label(Role::MenuItem, CONNECTION).rect());
        settle(&mut harness);
        assert_eq!(
            harness.state().route,
            Route::Connection { connection_id: CONNECTION.to_owned() }
        );
        assert!(hidden_rows(&harness).is_empty(), "{width}: the list closes after a pick");
        enter_pull_request(&mut harness);
        assert_eq!(
            check_layout(&mut harness, width),
            hidden,
            "{width}: back on the pull request, the same crumbs fold"
        );

        if let Some(folder) = FOLDERS.iter().find(|folder| hidden.contains(&(**folder).to_owned()))
        {
            open_overflow(&mut harness);
            harness.click_at(harness.get_by_role_and_label(Role::MenuItem, folder).rect());
            settle(&mut harness);
            let level = FOLDERS.iter().position(|other| other == folder).map(|index| index + 1);
            assert_eq!(
                breadcrumbs::take_trail_click(&harness.ctx),
                level,
                "{width}: a folded folder opens that folder"
            );
        }

        // Keyboard: Enter on "…" opens the list on its first row, the arrows move, Esc returns.
        harness.get_by_role_and_label(Role::Button, OVERFLOW).focus();
        harness.step();
        focus::use_keyboard(&harness.ctx);
        harness.key_press(Key::Enter);
        settle(&mut harness);
        let rows = hidden_rows(&harness);
        assert_eq!(rows, hidden, "{width}: Enter opens the list");
        assert!(
            harness.get_by_role_and_label(Role::MenuItem, &rows[0]).is_focused(),
            "{width}: the first row takes focus"
        );
        harness.key_press(Key::ArrowDown);
        settle(&mut harness);
        assert!(
            harness.get_by_role_and_label(Role::MenuItem, &rows[1]).is_focused(),
            "{width}: Down moves to the next row"
        );
        harness.key_press(Key::Escape);
        settle(&mut harness);
        assert_eq!(hidden_rows(&harness), Vec::<String>::new(), "{width}: Esc closes the menu");
        assert!(
            harness.get_by_role_and_label(Role::Button, OVERFLOW).is_focused(),
            "{width}: Esc returns to \"…\""
        );
    }
}

#[test]
fn the_files_trail_ends_at_the_pull_request_whose_crumb_leads_to_all_files() {
    let scratch = ScratchDir::new("files-crumb");
    let mut harness = harness(&scratch, 1440.0);
    // A file is open: the Files tab names no folders, only the way back to all files.
    breadcrumbs::publish_trail(
        &harness.ctx,
        &pull_request(),
        "Summary of all changed files",
        Vec::new(),
    );
    settle(&mut harness);
    let names: Vec<String> = shown_crumbs(&harness).into_iter().map(|(name, _)| name).collect();
    assert_eq!(names, [review_core::APP_NAME, CONNECTION, REPOSITORY, PULL_REQUEST]);
    harness.click_at(harness.get_by_role_and_label(Role::Link, PULL_REQUEST).rect());
    settle(&mut harness);
    assert_eq!(breadcrumbs::take_trail_click(&harness.ctx), Some(0), "the summary of all files");

    // At the top of the Files tab the pull request is the current page, not a link.
    breadcrumbs::publish_trail(&harness.ctx, &pull_request(), "", Vec::new());
    settle(&mut harness);
    assert!(harness.query_by_role_and_label(Role::Link, PULL_REQUEST).is_none());
}
