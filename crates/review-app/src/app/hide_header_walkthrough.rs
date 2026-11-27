//! The pull request's header slides away while the content scrolls down and comes back only from
//! the top, end to end against the mocked GitHub host: the header moves with the scroll and the
//! tab bar becomes the top edge; scrolling up scrolls a long diff first and brings the header back
//! once the diff is at its top; the pointer on its way to a tab, resting on it and clicking it
//! leaves it hidden, so the tab stays where the pointer aimed; the pointer resting past the tabs,
//! in the strip above them, brings it back, and it stays until the next scroll down; another tab
//! or the keyboard focus on the merge button bring it back;
//! an open menu holds it; reduced motion lands a glide at once.
//!
//! Per tab: on an Always shown tab nothing moves while another tab still hides it; a Start
//! minimized tab hides it on opening the pull request and on entering the tab, a scroll up past the
//! top leaves it hidden, the peek strip brings it back and the next scroll down tucks it away; an
//! older settings file with the switch off keeps it shown everywhere. Asserts geometry only.
//!
//! Settings › Appearance › Pull request header: a card per tab with its three choices, a choice and
//! "Set all tabs to" apply and are saved, the miniature plays a choice while the pointer or the
//! keyboard focus is on it and stops when it leaves, and holds still under reduced motion (read
//! through its caption).
//! `cargo test -p review-app hide_header`

use egui::accesskit::Toggled;
use egui::{
    Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, Rect, Role,
    TouchPhase, Vec2, pos2,
};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_store::{AppSettings, HeaderMode, HeaderModes};

use super::ReviewApp;
use super::action_e2e::{app, open_pull_request};
use super::github_mock::GitHubHost;
use super::tree_side_walkthrough::{long_file, open_long_file};
use cv_runtime::route::Route;
use cv_settings::onboarding::CliTools;
use cv_testkit::Clicks;
use cv_testkit::ScratchDir;

const MERGE_OPTIONS: &str = "Merge options";
const LONG_DIFF: &str = "Diff of src/long.rs";

fn start(name: &str) -> (Harness<'static, ReviewApp>, GitHubHost, ScratchDir) {
    start_with(name, HeaderModes::default())
}

/// The pull request open on its Overview, the header behaving per `modes`.
fn start_with(
    name: &str,
    modes: HeaderModes,
) -> (Harness<'static, ReviewApp>, GitHubHost, ScratchDir) {
    let scratch = ScratchDir::new(name);
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    harness.state_mut().services.settings.header_modes = modes;
    open_pull_request(&mut harness);
    pump(&mut harness, 4);
    (harness, host, scratch)
}

fn pump(harness: &mut Harness<'_, ReviewApp>, frames: usize) {
    for _ in 0..frames {
        harness.step();
    }
}

/// The header's number link, which copies the pull request's address.
fn number(harness: &Harness<'_, ReviewApp>) -> Rect {
    harness
        .get_by(|node| node.label().is_some_and(|label| label.starts_with("Copy link to #")))
        .rect()
}

fn tab_bar(harness: &Harness<'_, ReviewApp>) -> Rect {
    harness.get_by_role_and_label(Role::Tab, "Overview").rect()
}

/// Clicks the tab whose label starts with `name` and lets the header settle.
fn switch_to(harness: &mut Harness<'_, ReviewApp>, name: &str) {
    harness.click_at(
        harness
            .get_by(|node| {
                node.role() == Role::Tab
                    && node.label().is_some_and(|label| label.starts_with(name))
            })
            .rect(),
    );
    pump(harness, 4);
}

/// The tab whose label starts with `name` (its count follows) and whether it is selected.
fn tab(harness: &Harness<'_, ReviewApp>, name: &str) -> (Rect, bool) {
    let node = harness.get_by(|node| {
        node.role() == Role::Tab && node.label().is_some_and(|label| label.starts_with(name))
    });
    (node.rect(), node.accesskit_node().toggled() == Some(Toggled::True))
}

/// The pointer moves from `from` to `to` in a few steps of a frame each, every step longer than
/// the peek's dwell, and rests at `to`.
fn move_to(harness: &mut Harness<'_, ReviewApp>, from: Pos2, to: Pos2) {
    for step in 1..=4 {
        harness.hover_at(from.lerp(to, step as f32 / 4.0));
        harness.step();
    }
    pump(harness, 3);
}

/// A press and release of the primary button at `at`, where the pointer rests.
fn click_at(harness: &mut Harness<'_, ReviewApp>, at: Pos2) {
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    pump(harness, 3);
}

/// Somewhere in the Overview's content, below the tab bar.
fn content_point(harness: &Harness<'_, ReviewApp>) -> Pos2 {
    tab_bar(harness).left_bottom() + Vec2::new(200.0, 200.0)
}

/// The middle of the long diff in the Files tab.
fn diff_point(harness: &Harness<'_, ReviewApp>) -> Pos2 {
    harness.get_by_label(LONG_DIFF).rect().center()
}

/// One trackpad gesture at `at` that moves the content by `delta` (negative scrolls down),
/// handed to egui as it is; the frame of the move is stepped, the end comes with the next one.
fn scroll_at(harness: &mut Harness<'_, ReviewApp>, at: Pos2, delta: f32) {
    harness.hover_at(at);
    harness.step();
    let wheel = |delta: f32, phase: TouchPhase| Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: Vec2::new(0.0, delta),
        phase,
        modifiers: Modifiers::NONE,
        source: MouseWheelSource::Trackpad,
    };
    harness.event(wheel(0.0, TouchPhase::Start));
    harness.event(wheel(delta, TouchPhase::Move));
    harness.step();
    harness.event(wheel(0.0, TouchPhase::End));
}

/// [`scroll_at`] over the Overview's content.
fn scroll(harness: &mut Harness<'_, ReviewApp>, delta: f32) {
    scroll_at(harness, content_point(harness), delta);
}

/// [`scroll_at`] over the long diff.
fn scroll_diff(harness: &mut Harness<'_, ReviewApp>, delta: f32) {
    scroll_at(harness, diff_point(harness), delta);
}

/// The header is shown whole: the number sits where it started, the tab bar below it.
fn assert_shown(harness: &Harness<'_, ReviewApp>, shown: (Rect, Rect), why: &str) {
    let (number_at, tabs_at) = shown;
    assert!((number(harness).top() - number_at.top()).abs() < 0.5, "{why}: {:?}", number(harness));
    assert!((tab_bar(harness).top() - tabs_at.top()).abs() < 0.5, "{why}: {:?}", tab_bar(harness));
}

/// The header slid out above the tab bar, which moved up by the header's height.
fn assert_hidden(harness: &Harness<'_, ReviewApp>, shown: (Rect, Rect), why: &str) {
    let (_, tabs_at) = shown;
    let (number_now, tabs_now) = (number(harness), tab_bar(harness));
    assert!(number_now.bottom() <= tabs_now.top() + 0.5, "{why}: the number is above the tabs");
    let height = tabs_at.top() - tabs_now.top();
    assert!(
        height > 40.0,
        "{why}: the tab bar moved up by the header: {tabs_now:?} vs {tabs_at:?}"
    );
    assert!(tabs_now.top() < tabs_at.top() - height + 0.5, "{why}: the tab bar is the top edge");
}

#[test]
fn hide_header_follows_the_scroll_and_comes_back() {
    let (mut harness, _host, _scratch) = start("hide-header");
    let modes = harness.state().services.settings.header_modes;
    assert_eq!(modes, HeaderModes::all(HeaderMode::HideOnScroll), "on every tab by default");
    let shown = (number(&harness), tab_bar(&harness));

    // The header moves with the scroll: 30 points down slide it up by 30, the tab bar with it.
    scroll(&mut harness, -30.0);
    let moved = shown.0.top() - number(&harness).top();
    assert!((moved - 30.0).abs() < 0.5, "the header moved with the scroll: {moved}");
    assert!((shown.1.top() - tab_bar(&harness).top() - 30.0).abs() < 0.5, "the tabs follow");
    // Once the scrolling stops it glides on, out of view.
    pump(&mut harness, 3);
    assert_hidden(&harness, shown, "after scrolling down");

    // Another tab shows it at once.
    harness.click_at(harness.get_by_role_and_label(Role::Tab, "Pipelines").rect());
    pump(&mut harness, 2);
    assert_shown(&harness, shown, "on another tab");
    harness.click_at(harness.get_by_role_and_label(Role::Tab, "Overview").rect());
    pump(&mut harness, 2);

    // The keyboard focus on the merge button brings it back.
    scroll(&mut harness, -400.0);
    pump(&mut harness, 3);
    assert_hidden(&harness, shown, "before the focus moves");
    harness.get_by_label(MERGE_OPTIONS).focus();
    pump(&mut harness, 3);
    assert!(harness.get_by_label(MERGE_OPTIONS).is_focused(), "the merge options have the focus");
    assert_shown(&harness, shown, "with the focus on the merge button");

    // An open menu of the header holds it where it is.
    harness.click_at(harness.get_by_label("More actions").rect());
    pump(&mut harness, 2);
    scroll(&mut harness, -400.0);
    pump(&mut harness, 3);
    assert_shown(&harness, shown, "while its menu is open");
}

#[test]
fn hide_header_comes_back_from_the_top_of_the_files_or_above_the_tabs() {
    let scratch = ScratchDir::new("hide-header-files");
    let host = GitHubHost::with_pull_request_files(scratch.path(), &[long_file()]);
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    open_long_file(&mut harness);
    pump(&mut harness, 4);
    let shown = (number(&harness), tab_bar(&harness));

    scroll_diff(&mut harness, -400.0);
    pump(&mut harness, 3);
    assert_hidden(&harness, shown, "after scrolling the diff down");

    // A scroll up scrolls the diff, which can still go up: the header stays out of view.
    scroll_diff(&mut harness, 30.0);
    pump(&mut harness, 3);
    assert_hidden(&harness, shown, "while the diff can still scroll up");

    // Up to the diff's top: the scroll that reaches it is the diff's; the one after it, the
    // header's. The trackpad pulls the diff past its top; it springs back before the next one.
    scroll_diff(&mut harness, 2000.0);
    pump(&mut harness, 40);
    assert_hidden(&harness, shown, "the diff took the scroll to its top");
    scroll_diff(&mut harness, 30.0);
    pump(&mut harness, 3);
    assert_shown(&harness, shown, "scrolled on past the diff's top");

    // The pointer on its way from the diff to a tab, resting on it and clicking it, leaves the
    // header hidden: the tab stays where the pointer aimed.
    scroll_diff(&mut harness, -400.0);
    pump(&mut harness, 3);
    assert_hidden(&harness, shown, "scrolled down again");
    let (files, _) = tab(&harness, "Files");
    let from = diff_point(&harness);
    move_to(&mut harness, from, files.center());
    assert_hidden(&harness, shown, "with the pointer resting on a tab");
    click_at(&mut harness, files.center());
    assert_hidden(&harness, shown, "after a click on the tab");
    assert_eq!(tab(&harness, "Files"), (files, true), "the tab stayed under the pointer");

    // Past the tabs, in the strip above them, the resting pointer brings it back, and it stays
    // when the pointer leaves.
    let above = files.center_top() - Vec2::new(0.0, 4.0);
    move_to(&mut harness, files.center(), above);
    assert_shown(&harness, shown, "with the pointer above the tab bar");
    harness.hover_at(diff_point(&harness));
    pump(&mut harness, 3);
    assert_shown(&harness, shown, "after the pointer left the strip");

    // The next scroll down hides it again; a click on another tab lands on it and switches.
    scroll_diff(&mut harness, -400.0);
    pump(&mut harness, 3);
    assert_hidden(&harness, shown, "scrolled down after the peek");
    let (commits, _) = tab(&harness, "Commits");
    let from = diff_point(&harness);
    move_to(&mut harness, from, commits.center());
    assert_hidden(&harness, shown, "with the pointer resting on another tab");
    assert_eq!(tab(&harness, "Commits").0, commits, "the tab stayed under the pointer");
    click_at(&mut harness, commits.center());
    assert!(tab(&harness, "Commits").1, "the click switched to the tab it aimed at");
}

#[test]
fn hide_header_glides_at_once_under_reduced_motion() {
    let (mut harness, _host, _scratch) = start("hide-header-reduced");
    let shown = (number(&harness), tab_bar(&harness));
    cv_base::theme::motion::set_reduced(&harness.ctx, true);

    scroll(&mut harness, -30.0);
    harness.step();
    assert_hidden(&harness, shown, "in the frame the scrolling stopped");
}

#[test]
fn hide_header_never_moves_on_an_always_shown_tab() {
    let modes = HeaderModes { overview: HeaderMode::AlwaysShown, ..HeaderModes::default() };
    let (mut harness, _host, _scratch) = start_with("hide-header-always", modes);
    let shown = (number(&harness), tab_bar(&harness));

    scroll(&mut harness, -300.0);
    assert_shown(&harness, shown, "while scrolling");
    pump(&mut harness, 3);
    assert_shown(&harness, shown, "after scrolling");

    // The Commits tab, Hide on scroll, still lets it slide away.
    switch_to(&mut harness, "Commits");
    scroll(&mut harness, -300.0);
    pump(&mut harness, 3);
    assert_hidden(&harness, shown, "on a Hide on scroll tab");
}

#[test]
fn hide_header_starts_minimized_on_its_tab() {
    let modes = HeaderModes { overview: HeaderMode::StartMinimized, ..HeaderModes::default() };
    let (mut harness, _host, _scratch) = start_with("hide-header-minimized", modes);
    assert!(number(&harness).bottom() <= tab_bar(&harness).top() + 0.5, "hidden on opening");

    // Another tab shows it whole; coming back hides it again.
    switch_to(&mut harness, "Commits");
    let shown = (number(&harness), tab_bar(&harness));
    switch_to(&mut harness, "Overview");
    assert_hidden(&harness, shown, "on entering the tab");

    // However far a scroll up goes past the top, it stays hidden.
    scroll(&mut harness, 400.0);
    pump(&mut harness, 3);
    assert_hidden(&harness, shown, "after scrolling up at the top");

    // The pointer resting past the tabs lets it peek; the next scroll down tucks it away.
    let above = tab_bar(&harness).center_top() - Vec2::new(0.0, 4.0);
    let from = content_point(&harness);
    move_to(&mut harness, from, above);
    assert_shown(&harness, shown, "with the pointer above the tab bar");
    scroll(&mut harness, -300.0);
    pump(&mut harness, 3);
    assert_hidden(&harness, shown, "scrolled down after the peek");
}

#[test]
fn hide_header_switch_of_older_settings_keeps_the_header_shown() {
    let scratch = ScratchDir::new("hide-header-older");
    let config = scratch.path().join("config");
    std::fs::create_dir_all(&config).expect("the config folder");
    let older = r#"{"hide_header_on_scroll":false}"#;
    std::fs::write(config.join("settings.json"), older).expect("an older settings file");
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    let modes = harness.state().services.settings.header_modes;
    assert_eq!(modes, HeaderModes::all(HeaderMode::AlwaysShown), "the switch off carries over");
    open_pull_request(&mut harness);
    pump(&mut harness, 4);
    let shown = (number(&harness), tab_bar(&harness));

    scroll(&mut harness, -300.0);
    pump(&mut harness, 3);
    assert_shown(&harness, shown, "after scrolling");
}

/// The caption of a miniature that holds still.
const IDLE: &str = "Hover a choice to watch it";

/// Settings › Appearance with the header's cards in view.
fn header_settings(name: &str) -> (Harness<'static, ReviewApp>, GitHubHost, ScratchDir) {
    let scratch = ScratchDir::new(name);
    let host = GitHubHost::with_pull_request(scratch.path());
    let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
    harness.state_mut().settings.cli_tools = Some(CliTools::default());
    harness.state_mut().navigator.go(Route::Settings);
    pump(&mut harness, 4);
    harness.click_at(harness.get_by_role_and_label(Role::Tab, "Appearance").rect());
    pump(&mut harness, 4);
    (harness, host, scratch)
}

/// The radio button of `choice`, scrolled into view.
fn choice<'h>(harness: &'h mut Harness<'_, ReviewApp>, choice: &'h str) -> egui_kittest::Node<'h> {
    harness.get_by_role_and_label(Role::RadioButton, choice).scroll_to_me();
    pump(harness, 2);
    harness.get_by_role_and_label(Role::RadioButton, choice)
}

fn is_selected(harness: &Harness<'_, ReviewApp>, choice: &str) -> bool {
    let node = harness.get_by_role_and_label(Role::RadioButton, choice);
    node.accesskit_node().toggled() == Some(Toggled::True)
}

fn idle_cards(harness: &Harness<'_, ReviewApp>) -> usize {
    harness.query_all_by_label(IDLE).count()
}

/// Rests the pointer off the cards, in the page's top left corner.
fn leave(harness: &mut Harness<'_, ReviewApp>) {
    harness.hover_at(pos2(2.0, 2.0));
    pump(harness, 2);
}

#[test]
fn hide_header_modes_are_set_per_tab_in_settings() {
    let (mut harness, _host, scratch) = header_settings("hide-header-settings");
    let saved = || {
        let file = scratch.path().join("config").join("settings.json");
        AppSettings::load(&file).expect("the saved settings").header_modes
    };
    for tab in ["Overview", "Files", "Commits", "Pipelines"] {
        for mode in ["Always shown", "Hide on scroll", "Start minimized"] {
            let choice = format!("{tab}: {mode}");
            assert_eq!(is_selected(&harness, &choice), mode == "Hide on scroll", "{choice}");
        }
    }
    assert_eq!(idle_cards(&harness), 4, "every miniature holds still");

    // A choice applies to its tab only and is saved.
    let rect = choice(&mut harness, "Files: Start minimized").rect();
    harness.click_at(rect);
    pump(&mut harness, 2);
    leave(&mut harness);
    let files = HeaderModes { files: HeaderMode::StartMinimized, ..HeaderModes::default() };
    assert_eq!(harness.state().services.settings.header_modes, files);
    assert_eq!(saved(), files, "saved");
    assert!(is_selected(&harness, "Files: Start minimized"));
    assert!(!is_selected(&harness, "Files: Hide on scroll"));

    // "Set all tabs to" sets every tab.
    let set_all = "Set all tabs to Always shown";
    harness.get_by_role_and_label(Role::Button, set_all).scroll_to_me();
    pump(&mut harness, 2);
    harness.click_at(harness.get_by_role_and_label(Role::Button, set_all).rect());
    pump(&mut harness, 2);
    leave(&mut harness);
    let always = HeaderModes::all(HeaderMode::AlwaysShown);
    assert_eq!(harness.state().services.settings.header_modes, always);
    assert_eq!(saved(), always, "saved");
    assert!(is_selected(&harness, "Pipelines: Always shown"));

    // The pointer on a choice plays its story on that card only, until it leaves.
    choice(&mut harness, "Commits: Start minimized").hover();
    harness.step();
    assert_eq!(idle_cards(&harness), 3, "the Commits card plays");
    assert!(harness.query_by_label("On another tab").is_some(), "from the story's start");
    // The story moves on to its next caption within its first steps (step length: the harness's).
    let opening = "Opening Commits: the header tucks away";
    let is_told = (0..8).any(|_| {
        harness.step();
        harness.query_by_label(opening).is_some()
    });
    assert!(is_told, "the story goes on");
    leave(&mut harness);
    assert_eq!(idle_cards(&harness), 4, "leaving the choice stops it");
    assert!(harness.query_by_label(opening).is_none());

    // The visible keyboard focus on a choice plays it too.
    harness.key_press(Key::Tab);
    choice(&mut harness, "Overview: Hide on scroll").focus();
    harness.step();
    assert_eq!(idle_cards(&harness), 3, "the focused choice plays");
    assert!(harness.query_by_label("At the top").is_some(), "its story starts");
    harness.get_by_role_and_label(Role::Button, set_all).focus();
    pump(&mut harness, 2);
    assert_eq!(idle_cards(&harness), 4, "the focus left the choices");

    // Under reduced motion a watched choice holds still with its main caption.
    cv_base::theme::motion::set_reduced(&harness.ctx, true);
    choice(&mut harness, "Commits: Start minimized").hover();
    harness.step();
    assert!(harness.query_by_label(opening).is_some(), "the main caption at once");
    pump(&mut harness, 12);
    assert!(harness.query_by_label(opening).is_some(), "and it stays");
    assert!(harness.query_by_label("On another tab").is_none(), "nothing plays");

    // The search finds the section, and its title shows once: the block draws its own.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::F);
    harness.step();
    harness.event(Event::Text("header".to_owned()));
    pump(&mut harness, 4);
    let results = harness.query_by_label_contains("match “header”");
    assert!(results.is_some(), "the search results show");
    let titles = harness.query_all_by_label("Pull request header").count();
    assert_eq!(titles, 1, "the search shows the title once");
}
