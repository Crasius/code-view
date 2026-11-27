//! A calm strip above every screen while a host rate-limits a connection: who, how long until the
//! queued requests go out on their own, and the ways forward (retry now, read the platform's
//! limits, and add a token to an anonymous connection).

use std::time::Duration;

use egui::{Align, Frame, Layout, Margin, RichText, Stroke, Ui};
use review_core::http::Api;

use cv_base::theme::{self, Palette, icons, style};
use cv_runtime::intents;
use cv_services::Services;
use cv_services::connection::traffic::{RateLimited, rate_limit_docs, retry_now};
use cv_widgets::{buttons, glyph, provider_icon};

/// Most notices shown at once; the soonest resumes come first.
const MAX_SHOWN: usize = 2;

pub fn show(ui: &mut Ui, services: &Services) {
    let limited = services.rate_limited();
    if limited.is_empty() {
        return;
    }
    // The countdowns' next change, also of a notice not shown yet: it ends, and the next moves up.
    if let Some(next) = limited.iter().map(|notice| until_next_tick(notice.resumes_in)).min() {
        ui.ctx().request_repaint_after(next);
    }
    let palette = theme::palette(ui.ctx());
    for notice in limited.iter().take(MAX_SHOWN) {
        strip(ui, notice, &palette);
    }
}

/// One notice: the clock, what waits and for how long, and the ways forward.
fn strip(ui: &mut Ui, notice: &RateLimited, palette: &Palette) {
    let platform = notice.connection.kind.label();
    let (headline, detail) = texts(notice, platform);
    Frame::new()
        .fill(palette.surface_subtle)
        .stroke(Stroke::new(1.0, palette.border))
        .inner_margin(Margin::symmetric(16, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                glyph::decorative(
                    ui,
                    RichText::new(icons::CLOCK).size(18.0).color(palette.accent_text),
                );
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(
                        RichText::new(headline)
                            .font(theme::fonts::semibold_font(style::BODY_SIZE))
                            .color(palette.text),
                    );
                    ui.horizontal(|ui| {
                        let connection = &notice.connection;
                        provider_icon::show(
                            ui,
                            connection.kind,
                            connection.base_url(),
                            icons::SIZE_SMALL,
                        );
                        ui.label(
                            RichText::new(detail)
                                .size(style::SMALL_SIZE)
                                .color(palette.text_secondary),
                        );
                    });
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    actions(ui, notice, platform);
                });
            });
        });
}

/// The headline and the detail line of `notice`.
fn texts(notice: &RateLimited, platform: &str) -> (String, String) {
    match notice.api {
        Api::Rest => (
            format!(
                "{platform} is rate-limiting this connection — retrying in {}",
                countdown(notice.resumes_in)
            ),
            format!(
                "{}: requests wait and go out on their own when {platform} accepts them again.",
                notice.connection.name
            ),
        ),
        // GraphQL is metered apart from REST; only what it adds (a pipeline graph's job
        // dependencies) waits or is left out meanwhile.
        Api::GraphQl => (
            format!(
                "{platform} is rate-limiting this connection's GraphQL requests — retrying in {}",
                countdown(notice.resumes_in)
            ),
            format!(
                "{}: everything else loads as usual; details read through GraphQL wait or are left out until then.",
                notice.connection.name
            ),
        ),
    }
}

/// The platform's limits, retry now, and for an anonymous connection: add a token.
fn actions(ui: &mut Ui, notice: &RateLimited, platform: &str) {
    let docs = rate_limit_docs(&notice.connection);
    let docs_label = format!("Open {platform} rate limit docs");
    if buttons::secondary_icon(ui, icons::ARROW_SQUARE_OUT, &docs_label)
        .on_hover_text(docs)
        .clicked()
    {
        ui.ctx().open_url(egui::OpenUrl::new_tab(docs));
    }
    if buttons::secondary_icon(ui, icons::ARROW_CLOCKWISE, "Retry now").clicked() {
        retry_now(&notice.origin);
    }
    // Anonymous limits are the tightest; a token is the lasting way out.
    if notice.connection.token().is_none()
        && buttons::secondary_icon(ui, icons::KEY, "Add a token")
            .on_hover_text(format!("{platform} allows signed-in connections far more requests"))
            .clicked()
    {
        intents::request_fix(ui.ctx(), &notice.connection.id);
    }
}

/// `42 s`, `3 min 5 s`, `40 min`: whole seconds, rounded up so it never shows 0 while waiting.
pub fn countdown(remaining: Duration) -> String {
    let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
    match seconds {
        0..=89 => format!("{} s", seconds.max(1)),
        90..=599 if !seconds.is_multiple_of(60) => {
            format!("{} min {} s", seconds / 60, seconds % 60)
        }
        _ => format!("{} min", seconds.div_ceil(60)),
    }
}

/// How long [`countdown`] of `remaining` keeps its text: the time to wake the page that shows it.
/// A pause lasts up to an hour and mostly counts in minutes, so a tick every second would draw
/// sixty frames for each change.
pub fn until_next_tick(remaining: Duration) -> Duration {
    let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
    // The whole seconds left once the text changes: one less below ten minutes, where it shows
    // seconds or changes to them; above, where the next minute starts or the seconds show again.
    let changes_at = match seconds {
        0..=599 => seconds.saturating_sub(1),
        _ => ((seconds.div_ceil(60) - 1) * 60).max(599),
    };
    remaining.saturating_sub(Duration::from_secs(changes_at))
}

#[cfg(test)]
mod tests {
    use super::{countdown, until_next_tick};
    use std::time::Duration;

    #[test]
    fn counts_down_in_readable_units() {
        assert_eq!(countdown(Duration::from_millis(41_200)), "42 s");
        assert_eq!(countdown(Duration::from_millis(10)), "1 s");
        assert_eq!(countdown(Duration::from_secs(185)), "3 min 5 s");
        assert_eq!(countdown(Duration::from_secs(120)), "2 min");
        assert_eq!(countdown(Duration::from_mins(40)), "40 min");
    }

    /// The page wakes when the text changes, not before: once a minute while it counts minutes.
    #[test]
    fn wakes_when_the_text_changes() {
        for (remaining, wake) in [
            (Duration::from_millis(2_390_400), Duration::from_millis(50_400)),
            (Duration::from_mins(11), Duration::from_mins(1)),
            (Duration::from_secs(600), Duration::from_secs(1)),
            (Duration::from_millis(185_500), Duration::from_millis(500)),
            (Duration::from_secs(120), Duration::from_secs(1)),
            (Duration::from_millis(41_200), Duration::from_millis(200)),
            (Duration::from_millis(400), Duration::from_millis(400)),
        ] {
            assert_eq!(until_next_tick(remaining), wake, "{remaining:?}");
            let text = countdown(remaining);
            let at_wake = remaining.saturating_sub(wake);
            let before = at_wake + Duration::from_millis(1);
            assert_eq!(countdown(before), text, "{remaining:?}: unchanged until the wake");
            if !at_wake.is_zero() {
                assert_ne!(countdown(at_wake), text, "{remaining:?}: changed at it");
            }
        }
    }
}
