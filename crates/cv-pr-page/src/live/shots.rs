//! The live-update cards over a pull request page: both looks overlay the page without moving it;
//! Reload and Later are real buttons and the pending card is announced as an alert. No network.

use egui::{Id, Rect, Role, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use review_core::{ProviderKind, ReviewThread, ThreadComment, ThreadStatus, UserIdentity};
use review_store::ThemePreference;
use time::OffsetDateTime;

use super::banner::{self, BannerAction, Content, Look};
use cv_base::theme::{self, motion, style};
use cv_pr_threads::{CardOptions, ThreadsUi};

struct Scene {
    card: Option<Content>,
    action: Option<BannerAction>,
    thread_rect: Option<Rect>,
    threads: ThreadsUi,
}

fn thread() -> ReviewThread {
    let comment = |id: &str, author: &str, body: &str, hours_ago: i64| ThreadComment {
        id: id.to_owned(),
        author: UserIdentity {
            id: author.to_owned(),
            display_name: author.to_owned(),
            login: None,
            avatar_url: None,
        },
        body: body.to_owned(),
        created_at: OffsetDateTime::now_utc() - time::Duration::hours(hours_ago),
        updated_at: None,
    };
    ReviewThread {
        id: "t1".to_owned(),
        status: ThreadStatus::Active,
        anchor: None,
        is_outdated: false,
        origin: None,
        is_system: false,
        can_reply: true,
        can_resolve: true,
        comments: vec![
            comment("c1", "Cy Rivera", "Should `greet` stay public? Other crates call it.", 3),
            comment("c2", "Dee Okafor", "Kept it as a deprecated alias in the latest push.", 1),
        ],
    }
}

/// The page was last read two hours ago: the reply that came in live since is new.
fn live_threads() -> ThreadsUi {
    let mut threads = ThreadsUi::for_tests(ProviderKind::GitHub);
    threads.visited_at((OffsetDateTime::now_utc() - time::Duration::hours(2)).unix_timestamp());
    threads
}

fn pending() -> Content {
    Content {
        look: Look::Pending,
        title: "New changes pushed".to_owned(),
        detail: "2 new commits by Dee Okafor, 1 new comment by Dee Okafor".to_owned(),
    }
}

fn updated() -> Content {
    Content {
        look: Look::Updated { can_show_changes: true },
        title: "Updated".to_owned(),
        detail: "2 new commits by Dee Okafor".to_owned(),
    }
}

fn harness() -> Harness<'static, Scene> {
    let builder = Harness::builder().with_size([980.0, 360.0]).with_context_setup(|ctx| {
        theme::fonts::install(ctx);
        theme::motion::install(ctx);
    });
    let scene = Scene { card: None, action: None, thread_rect: None, threads: live_threads() };
    builder.build_ui_state(
        |ui, scene: &mut Scene| {
            let ctx = ui.ctx().clone();
            let palette = theme::activate(&ctx, ThemePreference::Light);
            let page = ui.max_rect();
            ui.painter().rect_filled(page, 0, palette.ground);
            // The tab band the card sits under.
            let band = Rect::from_min_size(page.min, vec2(page.width(), 44.0));
            ui.painter().rect_filled(band, 0, palette.surface);
            for (index, tab) in ["Overview", "Files", "Commits", "Pipelines"].iter().enumerate() {
                let color = if index == 1 { palette.text } else { palette.text_secondary };
                let at = pos2(band.left() + 24.0 + index as f32 * 96.0, band.center().y);
                ui.painter().text(
                    at,
                    egui::Align2::LEFT_CENTER,
                    *tab,
                    egui::FontId::proportional(style::BODY_SIZE),
                    color,
                );
            }
            ui.painter().hline(
                band.x_range(),
                band.bottom(),
                egui::Stroke::new(1.0, palette.border),
            );
            let content = Rect::from_min_max(
                pos2(page.left() + 24.0, band.bottom() + 16.0),
                pos2(page.right() - 24.0, page.bottom()),
            );
            let thread = thread();
            let options = CardOptions { show_location: false };
            let card_rect = ui
                .scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                    cv_pr_threads::show_card(ui, &thread, &mut scene.threads, &options);
                    ui.min_rect()
                })
                .inner;
            scene.thread_rect = Some(card_rect);
            let id = Id::unique("live_card_shot");
            let shown = motion::toggle(&ctx, id.with("shown"), scene.card.is_some(), 0.18);
            if let Some(card) = &scene.card {
                let top_center = pos2(page.center().x, band.bottom() + style::SPACE_S);
                let result = banner::show(&ctx, id, top_center, page.width() - 32.0, card, shown);
                if let Some(action) = result.action {
                    scene.action = Some(action);
                }
            }
        },
        scene,
    )
}

#[test]
fn live_cards_overlay_the_page_without_moving_it() {
    let mut harness = harness();
    harness.run_steps(20);
    let before = harness.state().thread_rect;
    harness.state_mut().card = Some(pending());
    harness.run_steps(30);
    assert_eq!(
        harness.state().thread_rect,
        before,
        "the card overlays the page instead of pushing it down"
    );
    harness.state_mut().card = Some(updated());
    harness.run_steps(30);
    assert_eq!(harness.state().thread_rect, before, "so does the updated card");
}

#[test]
fn pending_card_announces_itself_with_reload_and_later() {
    let mut harness = harness();
    harness.state_mut().card = Some(pending());
    harness.run_steps(30);
    let announced = harness.get_by_role(Role::Status);
    assert_eq!(
        announced.accesskit_node().label().as_deref(),
        Some("New changes pushed: 2 new commits by Dee Okafor, 1 new comment by Dee Okafor"),
        "screen readers hear about the push, politely"
    );
    harness.get_by_role_and_label(Role::Button, "Later").click();
    harness.run_steps(2);
    assert_eq!(harness.state().action, Some(BannerAction::Later));
    harness.get_by_role_and_label(Role::Button, "Reload").click();
    harness.run_steps(2);
    assert_eq!(harness.state().action, Some(BannerAction::Reload));
    assert!(harness.query_by_label("New").is_some(), "the comment that came in live is marked");
}
