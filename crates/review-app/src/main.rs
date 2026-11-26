#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Code View: a native, low-footprint pull request review app for Azure DevOps, GitHub and GitLab.
//! The binary holds the window's top level (`app`) and the title bar; the pages, widgets and
//! services live in the `cv-*` crates (the layer table in `.agents/rules/rust.md`, Workspace).
#![forbid(unsafe_code)]
// RS-LINT-02: egui lays out in f32 points while rows, columns and byte offsets are usize, so pixel
// math converts between them on nearly every line. The modules that parse external text (CI
// logs, git output, URLs, Markdown) opt back in with the RS-DONT-04 #![warn(...)].
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "egui pixel math converts between f32 points and usize indices"
)]

mod app;
mod title_bar;

use cv_base::platform;
use cv_shell::single_instance::{self, Claim};
use review_core::APP_NAME;

// `cargo build --workspace --all-targets` unifies the dev-only `test-support` feature into this
// binary; the release ship command (`cargo build --release -p review-app`) does not. The guard
// covers the shipped binary only: the bin's own release test harness (`cargo test --release`, the
// live performance checks) links the fakes through its dev-dependencies on purpose.
#[cfg(all(not(debug_assertions), not(test)))]
const _: () = assert!(
    !cv_base::TEST_SUPPORT,
    "CodeView was built with test fakes; build it with cargo build --release -p review-app"
);

// eframe replaces NSApplication's icon on startup, including the Dock and Command-Tab.
// Use the same high-resolution tile as the .icns instead of the transparent Windows mark.
#[cfg(target_os = "macos")]
const WINDOW_ICON_PNG: &[u8] = include_bytes!("../assets/icon/codeview-macos-512.png");

/// Window and taskbar icon, rendered from the same artwork as `assets/icon/codeview.ico`.
#[cfg(not(target_os = "macos"))]
const WINDOW_ICON_PNG: &[u8] = include_bytes!("../assets/icon/codeview-64.png");

fn main() -> eframe::Result<()> {
    // Started by the app itself to draw Mermaid diagrams (cv_media::mermaid::worker): no window.
    if cv_media::mermaid::serve_if_asked() {
        return Ok(());
    }
    // Started by the browser for the extension (docs/browser-extension.md): no window either.
    if cv_shell::browser::serve_if_asked(cv_providers::create_provider) {
        return Ok(());
    }
    platform::prepare_environment();
    // `CodeView <pull request URL>` opens that pull request, in the window already open if any.
    let link = std::env::args().nth(1);
    let paths = review_store::AppPaths::from_environment();
    let listener = match single_instance::claim(&paths, link.as_deref()) {
        Claim::HandedOver => return Ok(()),
        Claim::First(listener) => Some(listener),
        // The endpoint cannot be used (another user holds the name, the folder is not writable):
        // this window runs on its own, as before single instances.
        Claim::Alone(_reason) => None,
    };
    let options = eframe::NativeOptions {
        renderer: renderer(),
        multisampling: 0,
        depth_buffer: 0,
        stencil_buffer: 0,
        viewport: platform::viewport(
            egui::ViewportBuilder::default()
                .with_title(APP_NAME)
                .with_icon(window_icon())
                .with_inner_size([1440.0, 900.0])
                .with_min_inner_size([960.0, 600.0]),
        ),
        persist_window: false,
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: cv_shell::frame_stats::VSYNC,
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |creation| {
            if let Some(listener) = listener {
                listener.serve(creation.egui_ctx.clone());
            }
            // Checked like a link a later start hands over, before its page opens.
            if let Some(url) = &link {
                single_instance::queue_link(&creation.egui_ctx, url);
            }
            cv_services::memory_journal::start(
                paths.memory_journal_file(),
                creation.egui_ctx.clone(),
            );
            Ok(Box::new(app::ReviewApp::new(creation, paths, cv_providers::create_provider)))
        }),
    )
}

/// The window and taskbar icon; the PNG is compiled in, so a decode failure is a build mistake.
fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(WINDOW_ICON_PNG)
        .expect("the bundled window icon is a valid PNG")
}

/// glow (OpenGL). A build with the `wgpu` feature takes wgpu (Vulkan, Metal, DX12) when
/// `CODEVIEW_RENDERER=wgpu` is set: the way to tell a GL driver's font texture trouble from the
/// app's (docs/linux.md).
fn renderer() -> eframe::Renderer {
    #[cfg(feature = "wgpu")]
    if std::env::var("CODEVIEW_RENDERER").is_ok_and(|value| value.eq_ignore_ascii_case("wgpu")) {
        return eframe::Renderer::Wgpu;
    }
    eframe::Renderer::Glow
}
