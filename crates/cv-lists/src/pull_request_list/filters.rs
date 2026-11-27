//! The list page's filters: the overview's role, state and repository pickers, and a repository
//! list's state tabs. They live in the settings (`ListFilters`), the same on every visit.

use cv_base::theme::style;
use cv_widgets::{buttons, tabs};
use review_core::{PullRequestScope, StateFilter};
use review_store::ListFilters;

use super::ListSource;
use super::toolbar::{choice, picker};

/// Between a repository's title and its favorite star.
pub const STAR_GAP: f32 = style::SPACE_XS;

/// The state tabs of a repository's list.
const REPOSITORY_TABS: [(StateFilter, &str); 4] = [
    (StateFilter::Open, "Active"),
    (StateFilter::Merged, "Merged"),
    (StateFilter::Closed, "Abandoned"),
    (StateFilter::All, "All"),
];

/// The overview's role filter: everything visible by default, or narrowed to the user's part.
const ROLE_SCOPES: [(PullRequestScope, &str); 3] = [
    (PullRequestScope::Visible, "All"),
    (PullRequestScope::CreatedByMe, "Created by me"),
    (PullRequestScope::ReviewRequested, "Assigned to me"),
];

const STATES: [(StateFilter, &str); 3] = [
    (StateFilter::Open, "Active"),
    (StateFilter::Merged, "Merged"),
    (StateFilter::Closed, "Abandoned"),
];

/// The overview's state picker.
const STATE_WIDTH: f32 = 120.0;

fn repository_tab_labels() -> Vec<String> {
    REPOSITORY_TABS.iter().map(|(_, label)| (*label).to_owned()).collect()
}

/// True when the overview's filters show less than their defaults (every role, active, all repositories).
pub fn narrows(filters: &ListFilters) -> bool {
    let defaults = ListFilters::default();
    overview_scope(filters.scope) != defaults.scope
        || filters.state != defaults.state
        || filters.repository.is_some()
}

/// The saved role, or "All" where it is none of the overview's roles.
pub fn overview_scope(saved: PullRequestScope) -> PullRequestScope {
    if ROLE_SCOPES.iter().any(|(scope, _)| *scope == saved) {
        saved
    } else {
        PullRequestScope::Visible
    }
}

/// The filters' width without the overview's repository picker: what [`show`] lays out besides it.
pub fn fixed_width(ui: &egui::Ui, source: &ListSource) -> f32 {
    match source {
        // The favorite star sits between the title and the tabs.
        ListSource::Repository { .. } => {
            STAR_GAP + buttons::icon_width() + tabs::pivot_width(ui, &repository_tab_labels())
        }
        ListSource::Mine => {
            let roles: Vec<&str> = ROLE_SCOPES.iter().map(|(_, label)| *label).collect();
            tabs::segmented_width(ui, &roles) + style::SPACE_M + STATE_WIDTH + style::SPACE_S
        }
    }
}

/// The role, state and repository pickers, the repository one `repository_width` wide and its
/// choices the `known` repositories (connection id, connection name, repository key); true
/// when the user changed one. The pickers cut a long choice with an ellipsis instead of growing.
pub fn show(
    ui: &mut egui::Ui,
    source: &ListSource,
    filters: &mut ListFilters,
    repository_width: f32,
    known: impl FnOnce() -> Vec<(String, String, String)>,
) -> bool {
    match source {
        ListSource::Repository { .. } => {
            let selected = REPOSITORY_TABS
                .iter()
                .position(|(state, _)| *state == filters.repository_state)
                .unwrap_or(0);
            let picked = tabs::pivot(ui, &repository_tab_labels(), selected)
                .filter(|index| *index != selected);
            if let Some(index) = picked {
                filters.repository_state = REPOSITORY_TABS[index].0;
            }
            picked.is_some()
        }
        ListSource::Mine => {
            let mut scope = overview_scope(filters.scope);
            let mut changed =
                tabs::segmented(ui, &mut scope, &ROLE_SCOPES) && scope != filters.scope;
            filters.scope = scope;
            ui.add_space(style::SPACE_M);
            let state_label = STATES
                .iter()
                .find(|(state, _)| *state == filters.state)
                .map_or("Active", |(_, label)| label);
            let state_name = format!("State: {state_label}");
            changed |=
                picker(ui, "pull_request_state", state_label, &state_name, STATE_WIDTH, |ui| {
                    STATES.iter().fold(false, |changed, (state, label)| {
                        ui.selectable_value(&mut filters.state, *state, *label).changed() || changed
                    })
                });
            ui.add_space(style::SPACE_S);
            let selected = filters
                .repository
                .as_ref()
                .map_or_else(|| "All repositories".to_owned(), |(_, key)| key.clone());
            let repository_name = format!("Repository: {selected}");
            changed |= picker(
                ui,
                "pull_request_repository",
                &selected,
                &repository_name,
                repository_width,
                |ui| {
                    let mut changed =
                        choice(ui, &mut filters.repository, None, "All repositories").changed();
                    // Built only while the list is open: the closed picker needs none of it.
                    for (connection_id, connection_name, repository_key) in known() {
                        let label = format!("{repository_key}  ·  {connection_name}");
                        changed |= choice(
                            ui,
                            &mut filters.repository,
                            Some((connection_id, repository_key)),
                            &label,
                        )
                        .changed();
                    }
                    changed
                },
            );
            changed
        }
    }
}
