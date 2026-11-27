//! Mermaid fences in rendered Markdown, drawn by the real renderer (on a thread in tests): the
//! diagram arriving, the source toggle, the failure note and the lightbox.

use std::time::{Duration, Instant};

use cv_base::theme;
use cv_testkit::Clicks;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use review_core::ProviderKind;
use review_store::ThemePreference;

use super::RenderOptions;

const FLOWCHART: &str = "```mermaid\nflowchart LR\n  A[Start] --> B{Choice}\n  B --> C[Done]\n```";

struct Scene {
    text: String,
    /// Scrolls to this offset in the first frame.
    offset: Option<f32>,
    options: RenderOptions,
}

fn harness(text: String, offset: Option<f32>, size: [f32; 2]) -> Harness<'static, Scene> {
    harness_with(text, offset, size, RenderOptions::default())
}

fn harness_with(
    text: String,
    offset: Option<f32>,
    size: [f32; 2],
    options: RenderOptions,
) -> Harness<'static, Scene> {
    Harness::builder()
        .with_size(size)
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::activate(ctx, ThemePreference::Light);
        })
        .build_ui_state(
            |ui, scene: &mut Scene| {
                let mut area = egui::ScrollArea::vertical().id_salt("document").auto_shrink(false);
                if let Some(offset) = scene.offset.take() {
                    area = area.vertical_scroll_offset(offset);
                }
                area.show(ui, |ui| super::render(ui, "doc", &scene.text, scene.options));
                super::show_lightbox(ui.ctx());
            },
            Scene { text, offset, options },
        )
}

/// Runs frames until `done` holds (the renderer works on its own thread).
#[expect(
    clippy::disallowed_methods,
    reason = "the diagram is drawn on a background thread that offers the test no wake-up"
)]
fn run_until(
    harness: &mut Harness<'_, Scene>,
    what: &str,
    done: impl Fn(&Harness<'_, Scene>) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(harness) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
        harness.run_steps(1);
    }
    harness.run_steps(2);
}

fn has_diagram(harness: &Harness<'_, Scene>) -> bool {
    harness.query_by_role_and_label(Role::Image, "Mermaid diagram").is_some()
}

fn lines(count: usize) -> String {
    (0..count)
        .map(|index| format!("Line {index} of the document."))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[test]
fn a_mermaid_fence_shows_its_diagram_and_toggles_to_its_source() {
    let mut harness = harness(format!("{FLOWCHART}\n\nAfter the diagram."), None, [700.0, 600.0]);
    harness.run_steps(2);
    harness.get_by_role_and_label(Role::Image, "Drawing the Mermaid diagram");
    run_until(&mut harness, "the diagram", has_diagram);
    assert!(
        harness.query_by_label_contains("flowchart LR").is_none(),
        "the diagram, not its source"
    );

    harness.click_at(harness.get_by_role_and_label(Role::Button, "Show source").rect());
    harness.run_steps(3);
    harness.get_by_label_contains("A[Start] --> B{Choice}");
    assert!(!has_diagram(&harness));
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Show diagram").rect());
    harness.run_steps(3);
    assert!(has_diagram(&harness), "back to the diagram");
}

/// The Azure DevOps wiki's container syntax (its documented sample) takes the fence's path, in an
/// Azure DevOps description and elsewhere.
#[test]
fn an_azure_devops_container_shows_its_diagram_like_a_fence() {
    let text = "Intro.\n\n::: mermaid\nsequenceDiagram\n    Christie->>Josh: Hello Josh, how are you?\n    Josh-->>Christie: Great!\n:::\n\nAfter the diagram.".to_owned();
    for platform in [Some(ProviderKind::AzureDevOps), Some(ProviderKind::GitHub), None] {
        let mut harness = harness_with(
            text.clone(),
            None,
            [700.0, 600.0],
            RenderOptions { platform, ..RenderOptions::default() },
        );
        run_until(&mut harness, "the diagram", has_diagram);
        harness.get_by_label("After the diagram.");
        assert!(
            harness.query_by_label_contains(":::").is_none(),
            "{platform:?}: the markers are not text"
        );
        harness.click_at(harness.get_by_role_and_label(Role::Button, "Show source").rect());
        harness.run_steps(3);
        harness.get_by_label_contains("Christie->>Josh: Hello Josh, how are you?");
        assert!(
            harness.query_by_label_contains(":::").is_none(),
            "{platform:?}: the source is the diagram's, without its markers"
        );
    }
}

#[test]
fn a_source_that_cannot_be_drawn_shows_with_its_cause_and_actions() {
    let text = "```mermaid\nfooDiagram\n  a --> b\n```".to_owned();
    let mut harness = harness(text, None, [700.0, 400.0]);
    let note = "Diagram not drawn: Unsupported diagram type \u{201C}fooDiagram\u{201D}, line 1.";
    run_until(&mut harness, "the failure note", |harness| harness.query_by_label(note).is_some());
    harness.get_by_label_contains("a --> b");
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Copy source").rect());
    harness.run_steps(2);
    assert!(
        harness.query_by_role_and_label(Role::Button, "Retry").is_none(),
        "a parse error does not go away on its own"
    );
}

#[test]
fn a_diagram_arriving_above_the_view_keeps_the_text_in_place() {
    // The placeholder ends above the view, within the preloaded share: laid out, not seen.
    let text = format!("{FLOWCHART}\n\n{}", lines(40));
    let mut harness = harness(text, Some(200.0), [700.0, 300.0]);
    harness.run_steps(3);
    let line =
        |harness: &Harness<'_, Scene>| harness.get_by_label("Line 6 of the document.").rect().top();
    let before = line(&harness);
    run_until(&mut harness, "the diagram", has_diagram);
    assert!((line(&harness) - before).abs() < 0.5, "moved from {before} to {}", line(&harness));
}

#[test]
fn a_wide_diagram_shrinks_to_the_text_and_enlarges_in_the_lightbox() {
    let wide = "```mermaid\nflowchart LR\n  A[A long first step] --> B[A long second step] --> C[A long third step] --> D[A long fourth step] --> E[A long fifth step]\n```";
    let mut harness = harness(wide.to_owned(), None, [360.0, 500.0]);
    run_until(&mut harness, "the diagram", has_diagram);
    assert!(harness.get_by_role_and_label(Role::Image, "Mermaid diagram").rect().width() <= 360.0);
    harness.click_at(harness.get_by_role_and_label(Role::Button, "Enlarge").rect());
    harness.run_steps(3);
    harness.get_by_role_and_label(Role::Button, "Close (Esc)");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    assert!(harness.query_by_label("Close (Esc)").is_none());
}
