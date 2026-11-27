//! Settings → Network activity: what the app sent per connection, so users can check the request
//! budget themselves. One card per connection with the counters the HTTP layer already keeps and
//! its busiest endpoints. Counts and path templates only; no URLs with queries, headers or tokens.

use std::time::Duration;

use cv_base::theme::{self, icons, style};
use cv_services::Services;
use cv_services::connection::traffic::ConnectionRate;
use cv_widgets::provider_icon;
use egui::{Color32, RichText, Ui};

use super::rows;
use crate::onboarding::parts;

const TOP_ENDPOINTS: usize = 5;
const STAT_COLUMNS: usize = 4;

pub(crate) fn show(ui: &mut Ui, services: &Services) {
    let palette = theme::palette(ui.ctx());
    // Counts refresh while the section is shown; elsewhere it costs nothing.
    ui.ctx().request_repaint_after(Duration::from_secs(1));
    let rates = services.request_rates();
    if rates.is_empty() {
        ui.label(
            RichText::new("No requests yet. Counts appear here once the app loads pull requests.")
                .color(palette.text_secondary),
        );
    }
    for rate in &rates {
        card(ui, rate);
        ui.add_space(style::SPACE_M);
    }
    ui.label(
        RichText::new(format!("Background tasks running: {}", services.background.running()))
            .size(style::SMALL_SIZE)
            .color(palette.text_secondary),
    );
}

fn card(ui: &mut Ui, rate: &ConnectionRate) {
    rows::card(ui, |ui| {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(style::SPACE_L as i8, style::SPACE_M as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = style::SPACE_S;
                header(ui, rate);
                counters(ui, rate);
                if !rate.traffic.endpoints.is_empty() {
                    parts::divider(ui);
                    endpoints(ui, rate);
                }
            });
    });
}

/// The connection's mark and name, and its requests in the last minute.
fn header(ui: &mut Ui, rate: &ConnectionRate) {
    let palette = theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        cv_widgets::row::with_trailing(
            ui,
            egui::Align::Center,
            |ui| {
                ui.label(
                    RichText::new("per minute")
                        .size(style::SMALL_SIZE)
                        .color(palette.text_secondary),
                );
                ui.label(
                    RichText::new(rate.traffic.last_minute.to_string())
                        .font(theme::fonts::semibold_font(style::TITLE_SIZE)),
                );
            },
            |ui| {
                provider_icon::show(ui, rate.kind, &rate.base_url, icons::SIZE);
                let name = format!("{} ({})", rate.name, rate.kind.label());
                let name = RichText::new(name).font(theme::fonts::semibold_font(style::BODY_SIZE));
                ui.add(egui::Label::new(name).truncate());
            },
        );
    });
}

/// Sent, unchanged, shared and rate-limited side by side; screen readers get them as one sentence.
fn counters(ui: &mut Ui, rate: &ConnectionRate) {
    let traffic = &rate.traffic;
    let palette = theme::palette(ui.ctx());
    let limited = if traffic.rate_limited > 0 { palette.warning } else { palette.text };
    let stats = [
        (traffic.sent, "sent", palette.text),
        (traffic.not_modified, "unchanged (304)", palette.text),
        (traffic.shared, "shared with an identical request", palette.text),
        (traffic.rate_limited, "rate-limited", limited),
    ];
    let top = ui.cursor().top();
    ui.columns(STAT_COLUMNS, |columns| {
        for (column, (value, label, color)) in columns.iter_mut().zip(stats) {
            stat(column, value, label, color);
        }
    });
    let sentence = format!(
        "{} per minute now, {}",
        traffic.last_minute,
        stats.map(|(value, label, _)| format!("{value} {label}")).join(", ")
    );
    let row = ui.min_rect().with_min_y(top);
    let id = ui.scope_id().with(("network_counters", &rate.connection_id));
    ui.interact(row, id, egui::Sense::hover())
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &sentence));
}

/// The busiest endpoints as method and path template, with their counts.
fn endpoints(ui: &mut Ui, rate: &ConnectionRate) {
    let palette = theme::palette(ui.ctx());
    egui::Grid::new(("network_endpoints", &rate.connection_id))
        .num_columns(2)
        .spacing([style::SPACE_L, 2.0])
        .show(ui, |ui| {
            for endpoint in rate.traffic.endpoints.iter().take(TOP_ENDPOINTS) {
                let count = RichText::new(endpoint.count.to_string()).monospace();
                ui.label(count.color(palette.text_secondary));
                let path = format!("{} {}", endpoint.method, endpoint.path);
                let path = RichText::new(path).monospace().size(style::SMALL_SIZE);
                ui.add(egui::Label::new(path).truncate());
                ui.end_row();
            }
        });
}

/// A counter: its value in `color` above its name.
fn stat(ui: &mut Ui, value: u64, label: &str, color: Color32) {
    let palette = theme::palette(ui.ctx());
    ui.spacing_mut().item_spacing.y = 0.0;
    ui.label(
        RichText::new(value.to_string())
            .font(theme::fonts::semibold_font(style::BODY_SIZE))
            .color(color),
    );
    ui.add(
        egui::Label::new(
            RichText::new(label).size(style::SMALL_SIZE).color(palette.text_secondary),
        )
        .wrap(),
    );
}
