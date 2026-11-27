//! Browse repositories of a connection, open their pull request list and star them for the
//! overview's Favorites card.

use cv_base::theme::{self, icons, motion, style};
use cv_runtime::intents;
use cv_runtime::load::Live;
use cv_runtime::route::{Navigator, Route};
use cv_services::Services;
use cv_widgets::refresh_status::{self, RefreshStatus};
use cv_widgets::states::{self, Action};
use cv_widgets::{buttons, elide, glyph, provider_icon, row, skeleton, star};
use egui::{Align2, Id, Pos2, Rect, RichText, Sense, Stroke, TextEdit, pos2, vec2};
use review_core::{Connection, RepositoryInfo};
use review_store::Favorites;

use crate::pull_request_list;

const ROW_HEIGHT: f32 = 56.0;
/// The star button's square, left of the row's caret.
const STAR_BUTTON: f32 = buttons::SUBTLE_HEIGHT;
const SKELETON_ROWS: usize = 8;

#[derive(Default)]
pub struct RepositoriesPage {
    connection_id: Option<String>,
    search: String,
    /// The term last submitted with Search or Enter; typing alone sends no request.
    submitted: String,
    searched: Option<Searched>,
    repositories: Live<Vec<RepositoryInfo>>,
    /// Page cache key of the shown answer.
    cache_key: String,
}

/// What the shown answer was asked for.
#[derive(Clone, PartialEq, Eq)]
struct Searched {
    connection_id: String,
    search: String,
    /// The connection as it lists ([`pull_request_list::cache_stamp`]): an edited connection asks
    /// again instead of showing what it listed before.
    stamp: String,
}

impl Searched {
    fn cache_key(&self) -> String {
        format!("repositories|{}|{}", self.stamp, self.search)
    }
}

impl RepositoriesPage {
    /// Shows the repositories of `connection_id` (the connection's breadcrumb).
    pub fn select_connection(&mut self, connection_id: &str) {
        self.connection_id = Some(connection_id.to_owned());
    }

    /// The page is shown again: the list stays and refreshes in the background when stale.
    /// Another connection than the shown one loads on the next frame instead.
    pub fn revisit(&mut self, services: &mut Services) {
        let is_shown = self
            .searched
            .as_ref()
            .is_some_and(|shown| self.connection_id.as_ref() == Some(&shown.connection_id));
        if is_shown && services.cache.is_stale(&self.cache_key, services.now()) {
            self.start(services);
        }
    }

    fn start(&mut self, services: &mut Services) {
        let Some(Searched { connection_id, search, .. }) = self.searched.clone() else {
            return;
        };
        let Some(provider) = services.provider(&connection_id) else {
            return;
        };
        self.repositories.start(&services.background, async move {
            let filter = (!search.is_empty()).then_some(search);
            provider.list_repositories(filter.as_deref()).await
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, services: &mut Services, navigator: &mut Navigator) {
        if self.repositories.poll()
            && let Some(repositories) = self.repositories.ready()
        {
            services.cache.put(&self.cache_key, repositories.clone(), services.now());
        }
        let palette = theme::palette(ui.ctx());
        let connections = services.connections.all();
        let Some(first) = connections.first() else {
            let picked = states::message_with_actions(
                ui,
                icons::PLUGS,
                "No connections",
                "Connect Azure DevOps, GitHub or GitLab to browse its repositories.",
                &[Action::primary(icons::PLUS, "Add a connection")],
            );
            if picked.is_some() {
                navigator.go(Route::Settings);
            }
            return;
        };
        let connection_id = match &self.connection_id {
            Some(id) if connections.iter().any(|connection| &connection.id == id) => id.clone(),
            _ => first.id.clone(),
        };
        self.connection_id = Some(connection_id.clone());
        let wants_search = self.header(ui, connections, &connection_id, navigator);
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.min_rect().bottom(),
            Stroke::new(1.0, palette.border),
        );
        self.sync_query(services, &connection_id, wants_search);
        egui::Frame::new().inner_margin(egui::Margin::same(style::PAGE_MARGIN_X)).show(ui, |ui| {
            self.body(ui, services, navigator, &connection_id);
        });
    }

    /// The title, the connection picker, the search box and the refresh note; true when a search
    /// (or a retry) was asked for.
    fn header(
        &mut self,
        ui: &mut egui::Ui,
        connections: &[Connection],
        connection_id: &str,
        navigator: &mut Navigator,
    ) -> bool {
        let palette = theme::palette(ui.ctx());
        let mut wants_search = false;
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(egui::Margin::symmetric(style::PAGE_MARGIN_X, 16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Repositories").heading());
                ui.add_space(style::SPACE_M);
                ui.horizontal(|ui| {
                    // A picked connection is a place of its own: its breadcrumb and Alt+Left lead back.
                    if let Some(picked) = connection_picker(ui, connections, connection_id) {
                        self.connection_id = Some(picked.clone());
                        navigator.go(Route::Connection { connection_id: picked });
                    }
                    wants_search = self.search_box(ui);
                    // Reserved whether or not it shows, so the note never moves the layout.
                    let status = RefreshStatus::of(
                        self.repositories.is_refreshing(),
                        self.repositories.refresh_error(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        wants_search |= refresh_status::show(ui, status);
                    });
                });
            });
        wants_search
    }

    /// The search field and its button; true when the term was submitted.
    fn search_box(&mut self, ui: &mut egui::Ui) -> bool {
        let search = ui.add(
            TextEdit::singleline(&mut self.search)
                .hint_text(format!("{}  Search repositories", icons::MAGNIFYING_GLASS))
                .desired_width(320.0),
        );
        glyph::hint_as(&search, "Search repositories");
        let pressed_enter =
            search.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let is_submitted = buttons::secondary_icon(ui, icons::MAGNIFYING_GLASS, "Search")
            .on_hover_text("Enter")
            .clicked()
            || pressed_enter;
        if is_submitted {
            self.search.trim().clone_into(&mut self.submitted);
        }
        is_submitted
    }

    /// Another connection or search shows its remembered answer at once, refreshed when stale;
    /// `wants_search` asks again for the same one.
    fn sync_query(&mut self, services: &mut Services, connection_id: &str, wants_search: bool) {
        let Some(connection) = services.connection(connection_id) else {
            return;
        };
        let query = Searched {
            connection_id: connection_id.to_owned(),
            search: self.submitted.clone(),
            stamp: pull_request_list::cache_stamp(connection),
        };
        if self.searched.as_ref() != Some(&query) {
            self.cache_key = query.cache_key();
            self.repositories = Live::seeded(services.cache.get(&self.cache_key));
            self.searched = Some(query);
            if self.repositories.ready().is_none()
                || services.cache.is_stale(&self.cache_key, services.now())
            {
                self.start(services);
            }
        } else if wants_search && !self.repositories.is_loading() {
            self.start(services);
        }
    }

    /// The repositories, their skeleton while loading, the failure or the empty state.
    fn body(
        &mut self,
        ui: &mut egui::Ui,
        services: &mut Services,
        navigator: &mut Navigator,
        connection_id: &str,
    ) {
        let palette = theme::palette(ui.ctx());
        let is_ready = self.repositories.ready().is_some() || self.repositories.error().is_some();
        let opacity =
            motion::reveal(ui.ctx(), Id::unique(("repositories", connection_id)), is_ready);
        let repositories = match (self.repositories.ready(), self.repositories.error()) {
            (Some(repositories), _) => repositories,
            (None, Some(message)) => {
                let actions = [
                    Action::primary(icons::ARROW_CLOCKWISE, "Retry"),
                    Action::secondary(icons::GEAR, "Check connection"),
                ];
                match states::error_with_actions(ui, message, &actions) {
                    Some(0) => self.start(services),
                    Some(_) => intents::request_fix(ui.ctx(), connection_id),
                    None => {}
                }
                return;
            }
            (None, None) => {
                style::card_frame(&palette).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for seed in 0..SKELETON_ROWS {
                        let (rect, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), ROW_HEIGHT),
                            Sense::hover(),
                        );
                        skeleton::list_row(ui, rect, seed);
                    }
                });
                return;
            }
        };
        if repositories.is_empty() {
            self.empty(ui, connection_id);
            return;
        }
        let favorites = &services.settings.favorites;
        let mut starred = None;
        style::card_frame(&palette).show(ui, |ui| {
            ui.multiply_opacity(opacity);
            let rows = Rows { repositories, favorites, connection_id };
            starred = rows.show(ui, navigator);
        });
        if let Some(repository_key) = starred {
            pull_request_list::toggle_favorite(services, connection_id, &repository_key);
        }
    }

    /// No repository: clear the search, or check the connection when there is none.
    fn empty(&mut self, ui: &mut egui::Ui, connection_id: &str) {
        let has_search = !self.submitted.is_empty();
        let (detail, action) = if has_search {
            ("No repository matches this search.", Action::primary(icons::X, "Clear search"))
        } else {
            (
                "This connection lists no repositories. Check its organizations or token.",
                Action::primary(icons::GEAR, "Check connection"),
            )
        };
        let picked = states::message_with_actions(
            ui,
            icons::BOOKS,
            "No repositories found",
            detail,
            &[action],
        );
        if picked.is_none() {
            return;
        }
        if has_search {
            self.search.clear();
            self.submitted.clear();
        } else {
            intents::request_fix(ui.ctx(), connection_id);
        }
    }
}

/// The connection's mark and a picker of every connection; returns the one picked this frame.
fn connection_picker(
    ui: &mut egui::Ui,
    connections: &[Connection],
    connection_id: &str,
) -> Option<String> {
    let selected = connections.iter().find(|connection| connection.id == connection_id);
    let selected_name = selected
        .map(|connection| format!("{}  {}", connection.name, connection.kind.label()))
        .unwrap_or_default();
    // The picked connection's platform mark leads the picker; the slot stays when none is picked.
    match selected {
        Some(connection) => {
            provider_icon::show(ui, connection.kind, connection.base_url(), icons::SIZE);
        }
        None => {
            ui.allocate_exact_size(egui::vec2(icons::SIZE, icons::SIZE), egui::Sense::hover());
        }
    }
    let mut picked = Some(connection_id.to_owned());
    egui::ComboBox::from_id_salt("repository_connection")
        .selected_text(selected_name)
        .width(280.0)
        .show_ui(ui, |ui| {
            for connection in connections {
                ui.horizontal(|ui| {
                    provider_icon::show(ui, connection.kind, connection.base_url(), icons::SIZE);
                    let label = format!("{}  {}", connection.name, connection.kind.label());
                    ui.selectable_value(&mut picked, Some(connection.id.clone()), label);
                });
            }
        });
    picked.filter(|picked| picked != connection_id)
}

/// The rows of a connection's repositories.
struct Rows<'a> {
    repositories: &'a [RepositoryInfo],
    favorites: &'a Favorites,
    connection_id: &'a str,
}

impl Rows<'_> {
    /// Shows the rows in view; a click opens a repository's pull requests. Returns the
    /// repository whose star was clicked.
    fn show(&self, ui: &mut egui::Ui, navigator: &mut Navigator) -> Option<String> {
        let mut starred = None;
        let count = self.repositories.len();
        egui::ScrollArea::vertical().auto_shrink([false, true]).show_rows(
            ui,
            ROW_HEIGHT,
            count,
            |ui, range| {
                for (index, repository) in self.repositories[range.clone()].iter().enumerate() {
                    let is_last = range.start + index == count - 1;
                    let is_favorite = self.favorites.contains(self.connection_id, &repository.key);
                    match repository_row(ui, repository, is_favorite, is_last) {
                        Some(RepositoryPick::Open) => {
                            navigator.go(Route::RepositoryPullRequests {
                                connection_id: self.connection_id.to_owned(),
                                repository_key: repository.key.clone(),
                            });
                        }
                        Some(RepositoryPick::ToggleFavorite) => {
                            starred = Some(repository.key.clone());
                        }
                        None => {}
                    }
                }
            },
        );
        starred
    }
}

/// What a repository row was asked for.
enum RepositoryPick {
    /// Its pull requests.
    Open,
    ToggleFavorite,
}

/// One clickable repository with its favorite star (its own button, on top of the row).
fn repository_row(
    ui: &mut egui::Ui,
    repository: &RepositoryInfo,
    is_favorite: bool,
    is_last: bool,
) -> Option<RepositoryPick> {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &repository.key));
    row::card_background(
        ui,
        &response,
        rect.shrink(1.0),
        egui::Color32::TRANSPARENT,
        style::RADIUS_CONTROL,
    );
    let painter = ui.painter_at(rect);
    let icon_center = pos2(rect.left() + style::SPACE_L + 14.0, rect.center().y);
    painter.circle_filled(icon_center, 14.0, palette.selection);
    painter.text(
        icon_center,
        Align2::CENTER_CENTER,
        icons::BOOKS,
        egui::FontId::proportional(15.0),
        palette.accent,
    );
    let text_left = icon_center.x + 14.0 + style::SPACE_M;
    let caret_right = rect.right() - style::SPACE_L;
    let star_rect = Rect::from_center_size(
        pos2(caret_right - 14.0 - style::SPACE_S - STAR_BUTTON / 2.0, rect.center().y),
        vec2(STAR_BUTTON, STAR_BUTTON),
    );
    let description = repository.description.as_deref().unwrap_or("").trim();
    let title_y = if description.is_empty() { rect.center().y } else { rect.center().y - 9.0 };
    let text_width = (star_rect.left() - style::SPACE_S - text_left).max(1.0);
    let key_font = theme::fonts::semibold_font(style::BODY_SIZE);
    let at = pos2(text_left, title_y);
    let mut is_cut =
        paint_line(&painter, &repository.key, (key_font, palette.text), at, text_width);
    if !description.is_empty() {
        let font = egui::FontId::proportional(style::SMALL_SIZE);
        let at = pos2(text_left, rect.center().y + 10.0);
        is_cut |= paint_line(&painter, description, (font, palette.text_secondary), at, text_width);
    }
    // A cut key or description reads in full on hover.
    let response = if !is_cut {
        response
    } else if description.is_empty() {
        response.on_hover_text(&repository.key)
    } else {
        response.on_hover_text(format!("{}\n{description}", repository.key))
    };
    painter.text(
        pos2(caret_right, rect.center().y),
        Align2::RIGHT_CENTER,
        icons::CARET_RIGHT,
        egui::FontId::proportional(14.0),
        palette.text_disabled.gamma_multiply(row::hover_amount(ui, &response)),
    );
    if !is_last {
        painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, palette.border));
    }
    cv_widgets::focus::ring(ui, &response, rect);
    // Interacted after the row, so it sits on top: its click never opens the repository.
    let star =
        star::button_at(ui, star_rect, response.id.with("favorite"), is_favorite, &repository.key);
    if star.clicked() {
        Some(RepositoryPick::ToggleFavorite)
    } else {
        response.clicked().then_some(RepositoryPick::Open)
    }
}

/// `text` on one line from `at` (its left end and middle height), cut with an ellipsis at `width`;
/// true when it was cut.
fn paint_line(
    painter: &egui::Painter,
    text: &str,
    (font, color): (egui::FontId, egui::Color32),
    at: Pos2,
    width: f32,
) -> bool {
    let galley = elide::line(painter, text, font, color, width);
    let is_cut = galley.elided;
    painter.galley(pos2(at.x, at.y - galley.size().y / 2.0), galley, color);
    is_cut
}

#[cfg(test)]
mod tests {
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use review_core::RepositoryInfo;

    use super::repository_row;

    const KEY: &str = "platform-team/an-uncommonly-long-repository-name";
    const DESCRIPTION: &str = "Builds, tests and ships every service of the platform";

    /// The row of a repository with a long key and description, in a window `width` wide, hovered.
    fn hovered_row(width: f32) -> Harness<'static> {
        let repository = RepositoryInfo {
            key: KEY.into(),
            name: "an-uncommonly-long-repository-name".into(),
            description: Some(DESCRIPTION.into()),
            web_url: String::new(),
            default_branch: None,
        };
        let mut harness = Harness::builder()
            .with_size([width, 120.0])
            .with_context_setup(cv_base::theme::fonts::install)
            .build_ui(move |ui| {
                repository_row(ui, &repository, false, true);
            });
        harness.run();
        harness.hover_at(harness.get_by_role_and_label(Role::Link, KEY).rect().center());
        // Each kittest step moves its clock by its default `step_dt` of 0.25 s, so four steps pass
        // egui's default tooltip delay of 0.5 s with room to spare.
        harness.run_steps(4);
        harness
    }

    #[test]
    fn a_cut_repository_row_reads_in_full_on_hover() {
        let full = format!("{KEY}\n{DESCRIPTION}");
        assert!(hovered_row(280.0).query_by_label(&full).is_some(), "a narrow row is cut");
        assert!(hovered_row(1200.0).query_by_label(&full).is_none(), "a wide row shows it all");
    }
}
