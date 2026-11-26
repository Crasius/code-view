//! Settings → Connections: one row per connection. A click expands its facts in place (platform,
//! host, what it shows, how it signs in, where the token rests, its access) with Check again, Edit
//! and Remove; a connection with an access problem shows "Fix token" on its row. Adding, editing
//! and checking run through the onboarding wizard, which the page opens for the returned action.
//! A removal that failed offers Retry, which removes the same connection once more.

use cv_base::theme::{self, icons, motion, style};
use cv_runtime::toasts::{self, ToastAction};
use cv_services::Services;
use cv_widgets::{buttons, focus, glyph, provider_icon, row};
use egui::{Align, Align2, Color32, RichText, Sense, Ui, vec2};
use review_core::{AccessProblem, Connection};

use super::rows;
use crate::onboarding;

const FACT_LABEL_WIDTH: f32 = 128.0;
/// Where a failure toast's Retry asks for the failed removal once more.
const RETRY_REMOVAL_ID: &str = "connections.remove.retry";
/// Where a removal leaves the removed connection for the app ([`take_removed`]).
const REMOVED_ID: &str = "connections.removed";

/// The connection removed in this frame, at most once. The app closes its pull request pages and
/// only then lets retention delete its data, so no last save of a page (drafts, read marks) lands
/// after the deletion and brings its folder back.
pub fn take_removed(ctx: &egui::Context) -> Option<String> {
    ctx.data_mut(|data| data.remove_temp::<String>(egui::Id::unique(REMOVED_ID)))
}

pub enum ListAction {
    Add,
    Edit(Connection),
    Check(Connection),
}

/// Which connection is open, and which one asks before it is removed.
#[derive(Default)]
pub struct ConnectionList {
    expanded: Option<String>,
    confirming_removal: Option<String>,
    /// The connection whose removal failed last, for Retry.
    failed_removal: Option<String>,
}

impl ConnectionList {
    /// Opens `connection_id`'s facts, as when the user clicked its row.
    pub fn expand(&mut self, connection_id: &str) {
        self.expanded = Some(connection_id.to_owned());
    }

    pub fn show(&mut self, ui: &mut Ui, services: &mut Services) -> Option<ListAction> {
        let mut action = None;
        let mut removed = None;
        if toasts::take_retry(ui.ctx(), egui::Id::unique(RETRY_REMOVAL_ID)) {
            removed = self.failed_removal.take();
        }
        rows::card(ui, |ui| {
            for (index, connection) in services.connections.all().iter().enumerate() {
                if index > 0 {
                    rows::divider(ui);
                }
                let problem = services.providers.issues().get(&connection.id);
                let is_open = self.expanded.as_deref() == Some(connection.id.as_str());
                if summary_row(ui, connection, problem.as_ref(), is_open).clicked() {
                    self.expanded = if is_open { None } else { Some(connection.id.clone()) };
                    self.confirming_removal = None;
                }
                if is_open {
                    let vault = services.paths.token_vault().summary();
                    facts(ui, connection, problem.as_ref(), vault);
                    match self.actions(ui, connection) {
                        Some(RowAction::Remove) => removed = Some(connection.id.clone()),
                        Some(RowAction::List(picked)) => action = Some(picked),
                        None => {}
                    }
                }
            }
        });
        if let Some(connection_id) = removed {
            if !remove(ui.ctx(), services, &connection_id) {
                self.failed_removal = Some(connection_id);
            }
            self.expanded = None;
            self.confirming_removal = None;
        }
        action
    }

    /// Check again, Edit and Remove, or the question Remove asks first.
    fn actions(&mut self, ui: &mut Ui, connection: &Connection) -> Option<RowAction> {
        let mut action = None;
        let is_confirming = self.confirming_removal.as_deref() == Some(connection.id.as_str());
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: indent(),
                right: style::SPACE_L as i8,
                top: 0,
                bottom: style::SPACE_M as i8,
            })
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if is_confirming {
                        let question = format!(
                            "Remove {}? Its mirrors, logs, viewed marks and drafts go with it.",
                            connection.name
                        );
                        ui.label(RichText::new(question).color(theme::palette(ui.ctx()).text));
                        let is_escaped = ui.input(|input| input.key_pressed(egui::Key::Escape));
                        if buttons::secondary(ui, "Cancel").on_hover_text("Esc").clicked()
                            || is_escaped
                        {
                            self.confirming_removal = None;
                        }
                        if buttons::danger(ui, icons::TRASH, "Remove connection").clicked() {
                            action = Some(RowAction::Remove);
                        }
                        return;
                    }
                    if connection.token().is_some()
                        && buttons::secondary_icon(ui, icons::SHIELD_CHECK, "Check again").clicked()
                    {
                        action = Some(RowAction::List(ListAction::Check(connection.clone())));
                    }
                    if buttons::secondary_icon(ui, icons::PENCIL_SIMPLE, "Edit")
                        .on_hover_text("Edit host or token")
                        .clicked()
                    {
                        action = Some(RowAction::List(ListAction::Edit(connection.clone())));
                    }
                    if buttons::subtle(ui, icons::TRASH, "Remove…").clicked() {
                        self.confirming_removal = Some(connection.id.clone());
                    }
                });
            });
        action
    }
}

/// What a connection's open row asks for.
enum RowAction {
    Remove,
    List(ListAction),
}

fn indent() -> i8 {
    (style::SPACE_L + icons::SIZE_LARGE + style::SPACE_M) as i8
}

/// The connection's mark, name and one line about it; a click opens or closes its facts. The
/// row's click area is allocated first, so "Fix token" on it takes its own clicks.
fn summary_row(
    ui: &mut Ui,
    connection: &Connection,
    problem: Option<&AccessProblem>,
    is_open: bool,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), style::ROW_TWO_LINE), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, true, is_open, &connection.name)
    });
    let hover = motion::mix(Color32::TRANSPARENT, palette.hover, row::hover_amount(ui, &response));
    let tint = if is_open { palette.selection } else { hover };
    if tint != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 0, tint);
    }
    let inside = rect.shrink2(vec2(style::SPACE_L, 0.0));
    let mut content = ui.new_child(
        egui::UiBuilder::new().max_rect(inside).layout(egui::Layout::left_to_right(Align::Center)),
    );
    row::with_trailing(
        &mut content,
        Align::Center,
        |ui| {
            let caret = if is_open { icons::CARET_UP } else { icons::CARET_DOWN };
            let (slot, _) = ui.allocate_exact_size(vec2(icons::SIZE, icons::SIZE), Sense::hover());
            ui.painter().text(
                slot.center(),
                Align2::CENTER_CENTER,
                caret,
                icons::rest_font(&palette, icons::SIZE_SMALL),
                palette.text_secondary,
            );
            if problem.is_some() {
                onboarding::fix_token_button(ui, &connection.id);
            }
        },
        |ui| {
            let (mark, _) =
                ui.allocate_exact_size(vec2(icons::SIZE_LARGE, icons::SIZE_LARGE), Sense::hover());
            provider_icon::paint(
                ui.painter(),
                &palette,
                mark.center(),
                icons::SIZE_LARGE,
                connection.kind,
            );
            ui.add_space(style::SPACE_M - ui.spacing().item_spacing.x);
            row::centered_lines(ui, |ui| {
                ui.spacing_mut().item_spacing.y = style::SPACE_LINE;
                let name = RichText::new(&connection.name)
                    .font(theme::fonts::semibold_font(style::BODY_SIZE));
                ui.add(egui::Label::new(name).truncate());
                let small =
                    |text: &str, color| RichText::new(text).size(style::SMALL_SIZE).color(color);
                if let Some(problem) = problem {
                    let icon = small(icons::WARNING, palette.error);
                    glyph::truncated(ui, icon, small(&problem.title, palette.error));
                } else {
                    let line = small(&one_line(connection), palette.text_secondary);
                    ui.add(egui::Label::new(line).truncate());
                }
            });
        },
    );
    focus::ring(ui, &response, rect);
    response
}

/// "Access token on gitlab.com", or that it has no token.
fn one_line(connection: &Connection) -> String {
    format!("{} on {}", sign_in(connection), host(connection))
}

/// How the connection signs in: the Azure CLI's sign-in, an access token, or not at all.
fn sign_in(connection: &Connection) -> &'static str {
    if onboarding::uses_azure_cli(connection) {
        "Azure CLI sign-in"
    } else if connection.token().is_some() {
        "Access token"
    } else {
        "No token, public content only"
    }
}

fn host(connection: &Connection) -> &str {
    connection.base_url().split_once("://").map_or(connection.base_url(), |(_, rest)| rest)
}

/// The open connection's facts, a label column and a value column.
fn facts(ui: &mut Ui, connection: &Connection, problem: Option<&AccessProblem>, vault: &str) {
    let scope = if connection.organizations.is_empty() {
        "Everything the token can see".to_owned()
    } else {
        connection.organizations.join(", ")
    };
    let storage = if connection.token().is_some() { vault } else { "Nothing stored" };
    let access = problem.map_or_else(
        || "No problem since the app started".to_owned(),
        |problem| format!("{} {}", problem.title, problem.fix),
    );
    let rows: [(&str, &str); 6] = [
        ("Platform", connection.kind.label()),
        ("Host", host(connection)),
        ("Shows", &scope),
        ("Signed in with", sign_in(connection)),
        ("Token storage", storage),
        ("Access", &access),
    ];
    let palette = theme::palette(ui.ctx());
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: indent(),
            right: style::SPACE_L as i8,
            top: 0,
            bottom: style::SPACE_S as i8,
        })
        .show(ui, |ui| {
            egui::Grid::new(("connection_facts", &connection.id))
                .num_columns(2)
                .min_col_width(FACT_LABEL_WIDTH)
                .spacing([style::SPACE_M, style::SPACE_XS])
                .show(ui, |ui| {
                    for (label, value) in rows {
                        ui.label(RichText::new(label).color(palette.text_secondary));
                        let color = if label == "Access" && problem.is_some() {
                            palette.error
                        } else {
                            palette.text
                        };
                        ui.add(egui::Label::new(RichText::new(value).color(color)).wrap());
                        ui.end_row();
                    }
                });
        });
}

/// Removes the connection with everything kept for it, and says how that went; false when it
/// failed.
fn remove(ctx: &egui::Context, services: &mut Services, connection_id: &str) -> bool {
    match services.connections.remove(connection_id) {
        Ok(()) => {
            services.providers.remove(connection_id);
            // Its mirrors, logs, viewed marks, drafts and read marks go once its pages are closed.
            ctx.data_mut(|data| {
                data.insert_temp(egui::Id::unique(REMOVED_ID), connection_id.to_owned());
            });
            // Its favorites, collapsed card and saved filters go with it.
            let connections = &services.connections;
            services.settings.retain_connections(|id| connections.find(id).is_some());
            services.save_settings();
            services.toasts.success("Connection removed.");
            true
        }
        Err(error) => {
            let retry = ToastAction::retry(egui::Id::unique(RETRY_REMOVAL_ID));
            services.toasts.error(format!("Could not remove the connection: {error}"), retry);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use review_core::{Connection, ProviderKind};
    use review_store::ThemePreference;

    use super::{sign_in, summary_row};
    use cv_base::theme;
    use cv_widgets::row;

    fn connection(kind: ProviderKind, token: Option<&str>) -> Connection {
        Connection {
            id: "id".to_owned(),
            name: "name".to_owned(),
            kind,
            base_url: kind.default_base_url().to_owned(),
            organizations: Vec::new(),
            token: token.map(str::to_owned),
        }
    }

    #[test]
    fn names_the_azure_cli_sign_in_apart_from_access_tokens() {
        let cli = connection(ProviderKind::AzureDevOps, Some("eyJhead.payload.signature"));
        assert_eq!(sign_in(&cli), "Azure CLI sign-in");
        let token = connection(ProviderKind::AzureDevOps, Some("personal-access-token"));
        assert_eq!(sign_in(&token), "Access token");
        let public = connection(ProviderKind::GitHub, None);
        assert_eq!(sign_in(&public), "No token, public content only");
    }

    /// The name and the line below it sit centered in the row, as the mark and the caret do. The
    /// block used to start flush under the row's top border.
    #[test]
    fn centers_the_name_and_its_line_in_the_row() {
        let shown = connection(ProviderKind::GitHub, Some("personal-access-token"));
        let mut harness = Harness::builder()
            .with_size([480.0, 120.0])
            .with_context_setup(|ctx| {
                theme::fonts::install(ctx);
                theme::activate(ctx, ThemePreference::Light);
            })
            .build_ui(move |ui| {
                summary_row(ui, &shown, None, false);
            });
        harness.run();
        let row = harness.get_by_role_and_label(Role::Button, "name").rect();
        let name = harness.get_by_role_and_label(Role::Label, "name").rect();
        let line = harness.get_by_label_contains("Access token on").rect();
        row::assert_centered_in(row, name.union(line));
    }
}
