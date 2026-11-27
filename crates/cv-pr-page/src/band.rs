//! The band above a tab's content: the header with the review actions, which slides away while
//! the content scrolls ([`super::header_reveal`]), and the tab bar, which stays. Ctrl+1…4 (⌘1…4
//! on macOS) select the tabs from any focus ([`tab_key`]); no text field takes Ctrl with a digit.

use egui::{Key, Modifiers};
use review_store::{HeaderMode, HeaderModes};

use super::checkout::CheckoutButton;
use super::header;
use super::header_reveal::{Frame, HeaderReveal};
use super::merge_button::MergeButton;
use cv_base::platform::shortcut;
use cv_pr_files::FilesTab;
use cv_pr_session::actions::{ActionRunner, Write};
use cv_pr_session::session::PullRequestSession;
use cv_pr_session::web_link::WebLink;
use cv_pr_threads::ThreadsUi;
use cv_runtime::intents;
use cv_runtime::route::PullRequestTab as Tab;
use cv_services::Services;
use cv_shell::focus_mode;
use cv_widgets::tabs;

const TABS: [Tab; 4] = [Tab::Overview, Tab::Files, Tab::Commits, Tab::Pipelines];
/// The key of each of [`TABS`], and its label in the tab's tooltip.
const TAB_KEYS: [(Key, &str); 4] = [
    (Key::Num1, shortcut!(command "1")),
    (Key::Num2, shortcut!(command "2")),
    (Key::Num3, shortcut!(command "3")),
    (Key::Num4, shortcut!(command "4")),
];

/// The tab Ctrl+1…4 (⌘1…4) asked for this frame.
pub(super) fn tab_key(ctx: &egui::Context) -> Option<Tab> {
    ctx.input_mut(|input| {
        TABS.iter()
            .zip(TAB_KEYS)
            .find_map(|(tab, (key, _))| input.consume_key(Modifiers::COMMAND, key).then_some(*tab))
    })
}

/// The page's parts the band reads or changes.
pub(super) struct Parts<'a> {
    pub session: &'a mut PullRequestSession,
    pub services: &'a mut Services,
    pub actions: &'a mut ActionRunner,
    pub threads: &'a ThreadsUi,
    /// The file it shows is offered on the platform by the header's menu.
    pub files: &'a FilesTab,
    pub merge: &'a mut Option<MergeButton>,
    pub checkout: &'a mut CheckoutButton,
}

/// The header, slid as far as `reveal` says, and the tab bar below it; true when the user
/// switched tabs.
pub(super) fn show(
    ui: &mut egui::Ui,
    parts: Parts<'_>,
    reveal: &mut HeaderReveal,
    tab: &mut Tab,
) -> bool {
    let Parts { session, services, actions, threads, files, merge, checkout } = parts;
    let frame = Frame {
        tab: *tab,
        mode: header_mode(services.settings.header_modes, *tab),
        is_in_use: merge.as_ref().is_some_and(MergeButton::is_engaged)
            || checkout.is_engaged(ui.ctx(), &session.key),
    };
    let file_link = if *tab == Tab::Files { files.shown_link() } else { None };
    reveal.show(ui, &frame, |ui| {
        let buttons = header::Buttons { merge: merge.as_mut(), checkout };
        header(ui, session, services, actions, file_link, buttons)
    });
    let is_switched = tab_bar(ui, session, services, tab, threads);
    reveal.mark_content(ui);
    is_switched
}

/// How the header behaves on `tab`: Settings › Appearance › Pull request header.
fn header_mode(modes: HeaderModes, tab: Tab) -> HeaderMode {
    match tab {
        Tab::Overview => modes.overview,
        Tab::Files => modes.files,
        Tab::Commits => modes.commits,
        Tab::Pipelines => modes.pipelines,
    }
}

/// The header with the review actions, `file_link` the file the Files tab shows, and what its
/// buttons asked for; true when one of them (or a shortcut of the header) ran, a refresh
/// excepted, so the header shows again.
fn header(
    ui: &mut egui::Ui,
    session: &mut PullRequestSession,
    services: &mut Services,
    actions: &mut ActionRunner,
    file_link: Option<&WebLink>,
    mut buttons: header::Buttons<'_>,
) -> bool {
    if let Some(merge) = buttons.merge.as_deref_mut()
        && let Some(reason) = actions.take_merge_failure()
    {
        merge.fail(reason);
    }
    let action = header::show(ui, session, actions, file_link, buttons, services);
    // A refresh (F5) leaves the header where it is: the content keeps its place on screen.
    let has_acted =
        action.as_ref().is_some_and(|action| !matches!(action, header::HeaderAction::Refresh));
    // A write that did not start (another one runs) is visible as the busy header.
    match action {
        Some(header::HeaderAction::Run(action)) => {
            Write::Lifecycle(action).start(session, services, actions);
        }
        Some(header::HeaderAction::Merge(options)) => {
            Write::Merge(options).start(session, services, actions);
        }
        Some(header::HeaderAction::Refresh) => session.refresh(services),
        Some(header::HeaderAction::OpenSettings) => {
            intents::request_fix(ui.ctx(), &session.key.connection_id);
        }
        None => {}
    }
    has_acted
}

/// The tab pivot with its counts, and on the Files tab the threads panel toggle and focus mode;
/// true when the user switched tabs.
fn tab_bar(
    ui: &mut egui::Ui,
    session: &PullRequestSession,
    services: &mut Services,
    tab: &mut Tab,
    threads: &ThreadsUi,
) -> bool {
    let labels = TABS.map(|tab| tab_label(tab, session));
    let tab_items: Vec<(&str, tabs::TabCount, &str)> = labels
        .iter()
        .zip(TAB_KEYS)
        .map(|((label, count), (_, shortcut))| (*label, count.clone(), shortcut))
        .collect();
    let selected = TABS.iter().position(|shown| shown == tab).unwrap_or(0);
    let palette = cv_base::theme::palette(ui.ctx());
    let mut is_switched = false;
    egui::Frame::new()
        .fill(palette.surface)
        .inner_margin(egui::Margin { left: 14, right: 16, top: 0, bottom: 0 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if let Some(index) = tabs::pivot_with_counts(ui, &tab_items, selected) {
                    *tab = TABS[index];
                    is_switched = true;
                }
                if *tab == Tab::Files {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        files_tab_tools(ui, session, services, threads);
                    });
                }
            });
        });
    ui.painter().hline(
        ui.max_rect().x_range(),
        ui.cursor().top(),
        egui::Stroke::new(1.0, palette.border),
    );
    is_switched
}

/// The Files tab's threads panel toggle and, once the files are known, focus mode.
fn files_tab_tools(
    ui: &mut egui::Ui,
    session: &PullRequestSession,
    services: &mut Services,
    threads: &ThreadsUi,
) {
    let is_open = services.settings.threads_panel.is_open;
    if cv_pr_threads::panel_toggle(ui, session, threads, is_open) {
        services.settings.threads_panel.is_open = !is_open;
        services.save_settings();
    }
    if session.files().is_some() {
        focus_mode::enter_button(ui);
    }
}

/// The tab's name and its count badge; counts that fail to load leave the badge out.
fn tab_label(tab: Tab, session: &PullRequestSession) -> (&'static str, tabs::TabCount) {
    let count = |count: Option<usize>, has_failed: bool| match (count, has_failed) {
        (Some(count), _) => tabs::TabCount::Ready(cv_widgets::numbers::group_digits(count)),
        (None, true) => tabs::TabCount::None,
        (None, false) => tabs::TabCount::Loading,
    };
    // Without the pull request or its git mirror, neither count will arrive.
    let is_blocked = session.detail.error().is_some() || session.mirror.error().is_some();
    let count = match tab {
        Tab::Overview | Tab::Pipelines => tabs::TabCount::None,
        Tab::Files => {
            let has_failed = is_blocked || session.files_fast.error().is_some();
            count(session.files().map(|files| files.len()), has_failed)
        }
        Tab::Commits => count(
            session.commits.ready().map(Vec::len),
            is_blocked || session.commits.error().is_some(),
        ),
    };
    (tab.name(), count)
}
