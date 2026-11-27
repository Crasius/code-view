//! Settings: connections to Azure DevOps, GitHub and GitLab hosts, appearance, merging, the
//! assistant, links (the clipboard and the browser extension), storage, network activity and the
//! keyboard shortcuts.
//!
//! A sidebar lists the sections and searches them; the page shows one section, or the rows that
//! match the search grouped by section. Every setting is one row of [`rows`], drawn the same way
//! in both views. Only sections whose settings change how something looks get a preview column:
//! Appearance a sample diff, Merging the merge button. Without room for it the preview moves above
//! the rows. Adding, editing and fixing a connection runs through the onboarding wizard, which
//! takes the whole page while it is open.
//!
//! Keyboard and mouse is the one list of shortcuts. `?` outside a text field, and the rail's
//! keyboard button ([`intents::request_shortcuts`]), open it scrolled to the shortcuts of the page the user
//! came from ([`SettingsPage::open_shortcuts`]); there is no overlay of its own.

mod appearance;
mod assistant;
mod browser_extension;
mod category;
mod connections;
mod local_checkouts;
mod merging;
mod network;
mod preview_diff;
#[cfg(any(test, feature = "test-support"))]
pub use preview_diff::slack as sample_slack;
mod preview_merge;
mod rows;
mod shortcuts;
mod sidebar;
mod storage;

use cv_base::theme::{self, icons, style};
use cv_runtime::intents::{self, SettingsSection};
use cv_runtime::route::{Navigator, Route};
use cv_services::Services;
use cv_widgets::states::{self, Action};
use cv_widgets::{buttons, scroll};
use egui::{Key, KeyboardShortcut, Modifiers, RichText, Stroke, Ui, UiBuilder};
use review_core::ProviderKind;
use review_store::PullRequestTab;

use self::category::Category;
use self::connections::{ConnectionList, ListAction};
use self::rows::{Placement, Row, RowId};
use self::shortcuts::ShortcutGroup;
use self::sidebar::Navigation;
use crate::onboarding::parts::Tone;
use crate::onboarding::{self, CliRenewal, CliTools, EditStart, Wizard, WizardOutcome};

/// Opens Settings from anywhere: Ctrl+Comma, and Command-Comma, the system's own, on macOS.
const OPEN_KEY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Comma);
/// Focuses the search while Settings is open.
const FIND_KEY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);
/// Opens Keyboard and mouse outside text fields. Matched as typed text, so it works on every
/// keyboard layout (Shift+/ on a US one, Shift+ß on a German one).
const SHORTCUTS_TEXT: &str = "?";
/// The content column's widest; text lines past it get hard to read.
const CONTENT_MAX: f32 = 820.0;
/// The content column beside a preview: room for the widest control, five merge types in a row.
const CONTENT_WITH_PREVIEW: f32 = 640.0;
/// Narrower than this, the preview moves above the rows.
const CONTENT_MIN: f32 = 600.0;
const PREVIEW_MIN: f32 = 320.0;
/// The preview column's padding on each side.
const PREVIEW_MARGIN: i8 = 28;
const PAGE_PADDING: egui::Margin = egui::Margin::symmetric(32, 24);

#[derive(Default)]
pub struct SettingsPage {
    wizard: Option<Wizard>,
    /// Filled on first use by `tools()`.
    cli_tools: Option<CliTools>,
    renewal: CliRenewal,
    just_saved: Option<String>,
    /// The section shown stays for the session; the first visit opens Connections.
    navigation: Navigation,
    connections: ConnectionList,
    storage: storage::StorageSection,
    browser_extension: browser_extension::BrowserExtension,
    /// The platform the merge button preview shows.
    merge_preview: Option<ProviderKind>,
    /// The shortcut group Keyboard and mouse scrolls to once it is drawn.
    scroll_to_group: Option<ShortcutGroup>,
}

/// The section of the sidebar a [`intents::request_settings_section`] opens.
fn category_of(section: SettingsSection) -> Category {
    match section {
        SettingsSection::Assistant => Category::Assistant,
        SettingsSection::LocalCheckouts => Category::LocalCheckouts,
    }
}

/// The shortcut group of the page at `route`; on a pull request, of its `tab`.
fn shortcuts_group(route: &Route, tab: Option<PullRequestTab>) -> ShortcutGroup {
    match (route, tab) {
        (Route::MyPullRequests | Route::RepositoryPullRequests { .. }, _) => ShortcutGroup::List,
        (Route::PullRequest(_), Some(PullRequestTab::Files)) => ShortcutGroup::Files,
        (Route::PullRequest(_), Some(PullRequestTab::Pipelines)) => ShortcutGroup::Pipelines,
        (Route::PullRequest(_), _) => ShortcutGroup::PullRequest,
        (Route::Repositories | Route::Connection { .. } | Route::Settings, _) => ShortcutGroup::App,
    }
}

impl SettingsPage {
    /// Runs every frame on every route: Ctrl+Comma, "Fix token" requests and Azure CLI sign-in
    /// renewal. Returns true when the Settings route should open.
    pub fn update(&mut self, ctx: &egui::Context, services: &mut Services) -> bool {
        let tools = self.tools();
        self.renewal.update(ctx, services, tools);
        if ctx.input_mut(|input| input.consume_shortcut(&OPEN_KEY)) {
            return true;
        }
        if let Some(section) = intents::take_settings_section(ctx) {
            self.navigation.open(category_of(section));
            return true;
        }
        if let Some(request) = intents::take_add_request(ctx) {
            self.just_saved = None;
            self.navigation.open(Category::Connections);
            self.wizard = Some(Wizard::for_cloud(request.kind, request.organization, tools));
            return true;
        }
        let Some(connection_id) = intents::take_fix_request(ctx) else {
            return false;
        };
        let Some(connection) = services.connection(&connection_id).cloned() else {
            return false;
        };
        let problem = services.providers.issues().get(&connection_id);
        self.navigation.open(Category::Connections);
        self.connections.expand(&connection_id);
        self.wizard = Some(Wizard::edit(&connection, tools, EditStart::Fix(problem)));
        true
    }

    /// `?` outside a text field, or [`intents::request_shortcuts`]: shows Keyboard and mouse, scrolled to
    /// the shortcuts of the page the user is on (`route`, on a pull request its `tab`). Returns
    /// true when the Settings route should open.
    pub fn open_shortcuts(
        &mut self,
        ctx: &egui::Context,
        route: &Route,
        tab: Option<PullRequestTab>,
    ) -> bool {
        let is_requested = intents::take_shortcuts_request(ctx);
        let is_typed = !ctx.text_edit_focused()
            && ctx.input(|input| {
                input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Text(text) if text == SHORTCUTS_TEXT))
            });
        if !is_requested && !is_typed {
            return false;
        }
        self.navigation.open(Category::Keyboard);
        self.scroll_to_group = Some(shortcuts_group(route, tab));
        true
    }

    /// A connection setup is open: leaving now would drop what was entered.
    pub fn is_editing(&self) -> bool {
        self.wizard.is_some()
    }

    /// The connection removed this frame, at most once. The app closes its pull request pages and
    /// only then lets retention delete its data, so no last save of a page (drafts, read marks)
    /// lands after the deletion and brings its folder back.
    pub fn take_removed_connection(&mut self) -> Option<String> {
        self.connections.take_removed()
    }

    /// The sign-in tools the setup offers: the ones on this system's `PATH`, or in test builds
    /// (this crate's tests, and the tests of crates that turn on its `test-support` feature) none,
    /// so no test depends on the machine's tools.
    fn tools(&mut self) -> CliTools {
        #[cfg(any(test, feature = "test-support"))]
        let found = CliTools::default;
        #[cfg(not(any(test, feature = "test-support")))]
        let found = CliTools::detect;
        *self.cli_tools.get_or_insert_with(found)
    }

    /// The sign-in tools a test's setup offers in place of none.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_cli_tools(&mut self, tools: CliTools) {
        self.cli_tools = Some(tools);
    }

    pub fn show(&mut self, ui: &mut Ui, services: &mut Services, navigator: &mut Navigator) {
        if self.wizard.is_none() && services.connections.all().is_empty() {
            self.wizard = Some(Wizard::first_run(self.tools()));
        }
        if let Some(wizard) = self.wizard.as_mut() {
            let outcome = scroll::page("settings", PAGE_PADDING)
                .show(ui, |ui| wizard.show(ui, services))
                .inner;
            match outcome {
                WizardOutcome::Open => {}
                WizardOutcome::Cancelled => self.wizard = None,
                WizardOutcome::Saved { name } => {
                    self.just_saved = Some(name);
                    self.wizard = None;
                }
            }
            return;
        }
        let shift = ui.input(|input| input.modifiers.shift);
        // Ctrl+Shift+F belongs to the frame statistics.
        if !shift && ui.input_mut(|input| input.consume_shortcut(&FIND_KEY)) {
            Navigation::focus_search(ui.ctx());
        }
        let categories: Vec<Category> =
            Category::available(!services.connections.all().is_empty()).collect();
        if !categories.contains(&self.navigation.category) {
            self.navigation.category = Category::Connections;
        }
        let rect = ui.available_rect_before_wrap();
        let (side, main) = rect.split_left_right_at_x(rect.left() + sidebar::WIDTH);
        let problems = services.providers.issues().all().len();
        let mut side_ui = ui.new_child(UiBuilder::new().max_rect(side));
        egui::ScrollArea::vertical().id_salt("settings_sidebar").show(&mut side_ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(style::SPACE_M as i8, style::SPACE_XL as i8))
                .show(ui, |ui| self.navigation.show(ui, &categories, problems));
        });
        let palette = theme::palette(ui.ctx());
        ui.painter().vline(side.right(), side.y_range(), Stroke::new(1.0, palette.border));
        let mut main_ui = ui.new_child(UiBuilder::new().max_rect(main));
        self.main(&mut main_ui, services, navigator, &categories);
        ui.advance_cursor_after_rect(rect);
    }

    /// The section or the search results, with the section's preview beside or above them.
    fn main(
        &mut self,
        ui: &mut Ui,
        services: &mut Services,
        navigator: &mut Navigator,
        categories: &[Category],
    ) {
        let category = self.navigation.category;
        let query = self.navigation.search().to_owned();
        let wants_preview = query.is_empty() && category.has_preview();
        let rect = ui.max_rect();
        // The sample diff gets the room its widest line needs in every pane; where that leaves the
        // rows too little, it moves above them.
        let preview_width = if category == Category::Appearance {
            (preview_diff::width(ui, services) + 2.0 * f32::from(PREVIEW_MARGIN)).max(PREVIEW_MIN)
        } else {
            PREVIEW_MIN
        };
        let is_beside = wants_preview && rect.width() - preview_width >= CONTENT_MIN;
        let content = if is_beside {
            let width = (rect.width() - preview_width).min(CONTENT_WITH_PREVIEW);
            let (content, preview) = rect.split_left_right_at_x(rect.left() + width);
            let mut preview_ui = ui.new_child(UiBuilder::new().max_rect(preview));
            self.preview_column(&mut preview_ui, services, category);
            content
        } else {
            rect
        };
        let mut content_ui = ui.new_child(UiBuilder::new().max_rect(content));
        let scroll_id =
            if query.is_empty() { category.scroll_id() } else { "settings_search_results" };
        scroll::page(scroll_id, PAGE_PADDING).show(&mut content_ui, |ui| {
            ui.set_max_width(ui.available_width().min(CONTENT_MAX));
            if query.is_empty() {
                self.section(ui, services, navigator, category, wants_preview && !is_beside);
            } else {
                self.results(ui, services, categories, &query);
            }
        });
    }

    fn preview_column(&mut self, ui: &mut Ui, services: &Services, category: Category) {
        let palette = theme::palette(ui.ctx());
        ui.painter().rect_filled(ui.max_rect(), 0, palette.surface_subtle);
        ui.painter().vline(
            ui.max_rect().left(),
            ui.max_rect().y_range(),
            Stroke::new(1.0, palette.border),
        );
        scroll::page("settings_preview", egui::Margin::same(PREVIEW_MARGIN)).show(ui, |ui| {
            self.preview(ui, services, category);
        });
    }

    fn preview(&mut self, ui: &mut Ui, services: &Services, category: Category) {
        let palette = theme::palette(ui.ctx());
        let title =
            if category == Category::Merging { "Merge button preview" } else { "Live sample" };
        ui.label(
            RichText::new(title)
                .font(theme::fonts::semibold_font(style::SMALL_SIZE))
                .color(palette.text_secondary),
        );
        ui.add_space(style::SPACE_S);
        match category {
            Category::Merging => preview_merge::show(ui, services, &mut self.merge_preview),
            Category::Appearance => preview_diff::show(ui, services),
            Category::Connections
            | Category::Assistant
            | Category::LocalCheckouts
            | Category::Links
            | Category::Storage
            | Category::Network
            | Category::Keyboard => {}
        }
    }

    /// One section: its title and intro, then its rows.
    fn section(
        &mut self,
        ui: &mut Ui,
        services: &mut Services,
        navigator: &mut Navigator,
        category: Category,
        has_preview_above: bool,
    ) {
        let palette = theme::palette(ui.ctx());
        if category == Category::Connections {
            self.connections_header(ui, services, navigator);
        } else {
            ui.label(
                RichText::new(category.title())
                    .font(theme::fonts::semibold_font(style::TITLE_SIZE)),
            );
            ui.add_space(style::SPACE_XS);
            ui.add(
                egui::Label::new(RichText::new(category.intro()).color(palette.text_secondary))
                    .wrap(),
            );
        }
        ui.add_space(style::SPACE_L);
        if has_preview_above {
            self.preview(ui, services, category);
            ui.add_space(style::SPACE_L);
        }
        let rows: Vec<Row> = rows::of(category).collect();
        self.rows(ui, services, &rows, "");
    }

    /// The Connections title with "Add connection", and the note after a connection was saved.
    fn connections_header(&mut self, ui: &mut Ui, services: &Services, navigator: &mut Navigator) {
        let palette = theme::palette(ui.ctx());
        let add = cv_widgets::row::with_trailing(
            ui,
            egui::Align::Min,
            |ui| buttons::primary_icon(ui, icons::PLUS, "Add connection").clicked(),
            |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(Category::Connections.title())
                            .font(theme::fonts::semibold_font(style::TITLE_SIZE)),
                    );
                    ui.add_space(style::SPACE_XS);
                    let intro = format!(
                        "{} {}",
                        Category::Connections.intro(),
                        services.paths.token_vault().summary()
                    );
                    ui.add(
                        egui::Label::new(RichText::new(intro).color(palette.text_secondary)).wrap(),
                    );
                });
            },
        )
        .0;
        if add {
            self.start(ListAction::Add);
        }
        let Some(name) = self.just_saved.clone() else {
            return;
        };
        ui.add_space(style::SPACE_M);
        let mut is_closed = false;
        onboarding::parts::callout(ui, Tone::Success, &format!("{name} is connected"), "", |ui| {
            if buttons::primary_icon(ui, icons::GIT_PULL_REQUEST, "Show my pull requests").clicked()
            {
                navigator.go(Route::MyPullRequests);
                is_closed = true;
            }
            if buttons::secondary_icon(ui, icons::PLUS, "Add another connection").clicked() {
                self.start(ListAction::Add);
                is_closed = true;
            }
        });
        if is_closed {
            self.just_saved = None;
        }
    }

    /// The search results: every matching row under its section's title, or the way back.
    fn results(
        &mut self,
        ui: &mut Ui,
        services: &mut Services,
        categories: &[Category],
        query: &str,
    ) {
        let palette = theme::palette(ui.ctx());
        let found: Vec<Row> = rows::all()
            .filter(|row| categories.contains(&row.category) && rows::matches(row, query))
            .collect();
        if found.is_empty() {
            let title = format!("No setting matches “{query}”");
            let actions = [Action::secondary(icons::X, "Clear search")];
            let detail = "Try another word, or browse the sections on the left.";
            if states::message_with_actions(ui, icons::MAGNIFYING_GLASS, &title, detail, &actions)
                .is_some()
            {
                self.navigation.query.clear();
            }
            return;
        }
        let count = match found.len() {
            1 => format!("1 setting matches “{query}”"),
            count => format!("{count} settings match “{query}”"),
        };
        ui.label(RichText::new(count).color(palette.text_secondary));
        for category in categories {
            let rows: Vec<Row> =
                found.iter().copied().filter(|row| row.category == *category).collect();
            if rows.is_empty() {
                continue;
            }
            ui.add_space(style::SPACE_L);
            ui.label(
                RichText::new(category.title())
                    .font(theme::fonts::semibold_font(style::SECTION_SIZE)),
            );
            ui.add_space(style::SPACE_S);
            self.rows(ui, services, &rows, query);
        }
    }

    /// Rows in cards: neighbouring plain rows share a card, a block stands alone, and in its own
    /// section each group of shortcuts starts a card under its name. In the search (`query` set)
    /// blocks show their label too, so the match is visible; a titled block shows its own.
    fn rows(&mut self, ui: &mut Ui, services: &mut Services, rows: &[Row], query: &str) {
        let mut at = 0;
        while let Some(first) = rows.get(at) {
            if at > 0 {
                ui.add_space(style::SPACE_L);
            }
            if first.placement.is_block() {
                if !query.is_empty() && first.placement == Placement::Block {
                    rows::header(ui, first, query);
                    ui.add_space(style::SPACE_S);
                }
                self.control(ui, first, services);
                at += 1;
                continue;
            }
            if query.is_empty()
                && let RowId::Shortcut(index) = first.id
                && let Some(group) = shortcuts::group_of(index)
            {
                let palette = theme::palette(ui.ctx());
                let heading = ui.label(
                    RichText::new(group.title())
                        .font(theme::fonts::semibold_font(style::BODY_SIZE))
                        .color(palette.text_secondary),
                );
                if self.scroll_to_group == Some(group) {
                    heading.scroll_to_me(Some(egui::Align::TOP));
                    self.scroll_to_group = None;
                }
                ui.add_space(style::SPACE_XS);
            }
            let run = rows
                .get(at..)
                .unwrap_or_default()
                .iter()
                .enumerate()
                .take_while(|(index, row)| {
                    !row.placement.is_block() && (*index == 0 || !starts_group(row, query))
                })
                .count();
            let run_rows = rows.get(at..at + run).unwrap_or_default();
            rows::card(ui, |ui| {
                for (index, row) in run_rows.iter().enumerate() {
                    if index > 0 {
                        onboarding::parts::divider(ui);
                    }
                    // While the assistant is off its other rows stay, read as inactive.
                    let is_active = assistant::is_active(row.id, services);
                    ui.add_enabled_ui(is_active, |ui| {
                        rows::show(ui, row, query, |ui| self.control(ui, row, services));
                    });
                }
            });
            at += run.max(1);
        }
    }

    /// The control of `row`, from the module of its section.
    fn control(&mut self, ui: &mut Ui, row: &Row, services: &mut Services) {
        match row.id {
            RowId::ConnectionList => {
                if let Some(action) = self.connections.show(ui, services) {
                    self.start(action);
                }
            }
            RowId::Appearance(id) => appearance::control(ui, id, row, services),
            RowId::Merging(id) => merging::control(ui, id, services, &mut self.merge_preview),
            RowId::Assistant(id) => assistant::control(ui, id, row, services),
            RowId::Checkout(id) => {
                if local_checkouts::control(ui, id, services) {
                    self.navigation.open(Category::Assistant);
                }
            }
            RowId::ClipboardLinks => {
                if rows::switch(ui, &mut services.settings.open_clipboard_links, row) {
                    services.save_settings();
                }
            }
            RowId::BrowserExtension => self.browser_extension.control(ui, services),
            RowId::StorageUsage => self.storage.usage(ui, services),
            RowId::ClearCache => self.storage.clear(ui, services),
            RowId::Requests => network::show(ui, services),
            RowId::Shortcut(index) => {
                let is_assistant_on = services.settings.assistant.enabled;
                if shortcuts::keys(ui, index, is_assistant_on) {
                    self.navigation.open(Category::Assistant);
                }
            }
        }
    }

    /// Opens the wizard for a connection list action.
    fn start(&mut self, action: ListAction) {
        let tools = self.tools();
        self.just_saved = None;
        self.wizard = Some(match action {
            ListAction::Add => Wizard::new(tools),
            ListAction::Edit(connection) => Wizard::edit(&connection, tools, EditStart::Token),
            ListAction::Check(connection) => Wizard::edit(&connection, tools, EditStart::Recheck),
        });
    }
}

/// A shortcut that opens a new group in its section's view.
fn starts_group(row: &Row, query: &str) -> bool {
    let RowId::Shortcut(index) = row.id else {
        return false;
    };
    query.is_empty() && shortcuts::group_of(index).is_some()
}

#[cfg(test)]
mod tests {
    use super::SettingsPage;
    use crate::onboarding::CliTools;

    #[test]
    fn a_test_build_offers_no_sign_in_tools_until_a_test_sets_them() {
        let mut page = SettingsPage::default();
        assert_eq!(page.tools(), CliTools::default());
        let both = CliTools { github: true, azure: true };
        page.set_cli_tools(both);
        assert_eq!(page.tools(), both);
    }
}
