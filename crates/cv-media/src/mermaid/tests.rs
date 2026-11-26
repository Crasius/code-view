use std::time::{Duration, Instant};

use cv_base::theme::Palette;
use egui::{Context, vec2};

use super::{
    DiagramRequest, DiagramState, DiagramStore, DiagramTheme, MAX_TEXTURE_EDGE, scale_key,
    texture_scale,
};

const FLOWCHART: &str = "flowchart LR\n  A[Start] --> B[Done]";

/// Asks until the diagram is no longer being drawn.
fn settled(
    store: &DiagramStore,
    ctx: &Context,
    request: &DiagramRequest<'_>,
    pixel_scale: Option<f32>,
) -> DiagramState {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let state = store.state(ctx, request, pixel_scale);
        // Without a pixel scale a drawn diagram stays "drawing", with its size.
        let is_done = match &state {
            DiagramState::Drawing { size } => pixel_scale.is_none() && size.is_some(),
            DiagramState::Ready { .. } | DiagramState::Failed(_) => true,
        };
        if is_done || Instant::now() > deadline {
            return state;
        }
        cv_testkit::let_threads_run(Duration::from_millis(5));
    }
}

fn next_pass(ctx: &Context) {
    // No painter takes the textures: their uploads are dropped.
    ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
}

#[test]
fn draws_off_the_ui_thread_and_rasterizes_at_the_pixel_scale() {
    let ctx = Context::default();
    let store = DiagramStore::new(super::MEMORY_LIMIT);
    let theme = DiagramTheme::from_palette(&Palette::LIGHT);
    let request = DiagramRequest { source: FLOWCHART, theme: &theme };
    assert!(
        matches!(store.state(&ctx, &request, Some(2.0)), DiagramState::Drawing { size: None }),
        "the first ask only queues it"
    );
    let DiagramState::Ready { texture, size } = settled(&store, &ctx, &request, Some(2.0)) else {
        panic!("the flowchart draws")
    };
    assert_eq!(texture.size(), [(size.x * 2.0).ceil() as usize, (size.y * 2.0).ceil() as usize]);

    // Another zoom rasterizes again; meanwhile the old texture stands in.
    assert!(matches!(store.state(&ctx, &request, Some(1.0)), DiagramState::Ready { .. }));
    let deadline = Instant::now() + Duration::from_secs(30);
    let smaller = loop {
        if let DiagramState::Ready { texture, .. } = store.state(&ctx, &request, Some(1.0))
            && texture.size()[0] < (size.x * 2.0) as usize
        {
            break texture;
        }
        assert!(Instant::now() < deadline, "rasterized again at the new scale");
        cv_testkit::let_threads_run(Duration::from_millis(5));
    };
    assert_eq!(smaller.size()[0], size.x.ceil() as usize);
}

#[test]
fn a_theme_switch_draws_again_in_the_same_place() {
    let ctx = Context::default();
    let store = DiagramStore::new(super::MEMORY_LIMIT);
    let light = DiagramTheme::from_palette(&Palette::LIGHT);
    let dark = DiagramTheme::from_palette(&Palette::DARK);
    let DiagramState::Drawing { size: Some(size) } =
        settled(&store, &ctx, &DiagramRequest { source: FLOWCHART, theme: &light }, None)
    else {
        panic!("drawn, not rasterized without a pixel scale")
    };
    match store.state(&ctx, &DiagramRequest { source: FLOWCHART, theme: &dark }, Some(1.0)) {
        DiagramState::Drawing { size: Some(held) } => {
            assert_eq!(held, size, "the known size holds the place");
        }
        _ => panic!("the dark version is drawn anew"),
    }
}

#[test]
fn errors_and_oversized_sources_fail_with_their_cause() {
    let ctx = Context::default();
    let store = DiagramStore::new(super::MEMORY_LIMIT);
    let theme = DiagramTheme::from_palette(&Palette::LIGHT);
    let DiagramState::Failed(failure) = settled(
        &store,
        &ctx,
        &DiagramRequest { source: "nonsenseChart\n  a", theme: &theme },
        Some(1.0),
    ) else {
        panic!("an unknown type fails")
    };
    assert_eq!(failure.line, Some(1));
    let huge = "A-->B\n".repeat(20_000);
    let DiagramState::Failed(failure) =
        store.state(&ctx, &DiagramRequest { source: &huge, theme: &theme }, Some(1.0))
    else {
        panic!("refused at once")
    };
    assert!(
        failure.reason.contains("too large") || failure.reason.contains("lines"),
        "{failure:?}"
    );
    assert!(store.lock().jobs.is_empty(), "never sent to the renderer");
}

#[test]
fn textures_not_drawn_lately_go_when_memory_runs_out() {
    let ctx = Context::default();
    // Room for about one small texture.
    let store = DiagramStore::new(64 * 1024);
    let theme = DiagramTheme::from_palette(&Palette::LIGHT);
    let first = DiagramRequest { source: FLOWCHART, theme: &theme };
    let second = DiagramRequest { source: "graph TD; X-->Y", theme: &theme };
    assert!(matches!(settled(&store, &ctx, &first, Some(1.0)), DiagramState::Ready { .. }));
    next_pass(&ctx);
    assert!(matches!(settled(&store, &ctx, &second, Some(1.0)), DiagramState::Ready { .. }));
    next_pass(&ctx);
    store.state(&ctx, &second, Some(1.0));
    let inner = store.lock();
    assert!(inner.entries[&first.key()].texture.is_none(), "the older texture was evicted");
    assert!(inner.entries[&second.key()].texture.is_some(), "the one drawn this pass stays");
}

#[test]
fn textures_stay_within_their_limits() {
    assert_eq!(texture_scale(vec2(400.0, 300.0), 2.0), 2.0);
    let huge = vec2(10_000.0, 500.0);
    assert!(huge.x * texture_scale(huge, 2.0) <= MAX_TEXTURE_EDGE + 0.5);
    let square = vec2(3_000.0, 3_000.0);
    assert!(
        square.x * square.y * texture_scale(square, 2.0).powi(2) <= super::MAX_TEXTURE_PIXELS + 1.0
    );
    assert_eq!(scale_key(1.004), scale_key(1.0), "tiny differences rasterize nothing again");
}
