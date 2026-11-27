//! The pull request page in a kittest harness, wired as the app wires it: a GitHub connection in
//! the store, the services on a scripted provider, the frame's surfaces, theme and focus mode
//! around `PullRequestPage::show`. The walkthroughs of this crate drive it; each brings its own
//! script and assertions.

use std::sync::Arc;

use egui_kittest::Harness;
use review_core::{Connection, Provider, ProviderKind};
use review_store::ConnectionStore;

use crate::PullRequestPage;
use cv_base::theme;
use cv_runtime::route::{Navigator, PullRequestKey};
use cv_services::Services;
use cv_shell::focus_mode;
use cv_testkit::{ScratchDir, samples};
use cv_widgets::surfaces;

/// The connection's API: every walkthrough's pull request is on GitHub.
const API: &str = "https://api.github.com";

/// The page with what it needs, as the app hands them to it. The services drop last: the page's
/// last writes run on their runtime.
pub(crate) struct Scene {
    pub(crate) page: PullRequestPage,
    pub(crate) navigator: Navigator,
    pub(crate) services: Services,
}

/// A harness that shows one pull request page. The scratch folder drops last, after the services
/// that write to it.
pub(crate) struct PageApp {
    pub(crate) harness: Harness<'static, Option<Scene>>,
    _scratch: ScratchDir,
}

impl PageApp {
    /// Opens pull request `key` on a connection named `id` (with `token`, or anonymous), each
    /// frame `frame` seconds long. `configure` sets the services up before the page opens;
    /// `provider` hands out the test's script for the connection.
    pub(crate) fn start(
        key: PullRequestKey,
        frame: f32,
        token: Option<&str>,
        configure: impl FnOnce(&mut Services),
        provider: impl Fn(Connection, reqwest::Client) -> Arc<dyn Provider> + Send + Sync + 'static,
    ) -> Self {
        let scratch = ScratchDir::new(&key.connection_id);
        let paths = scratch.app_paths();
        let mut store = ConnectionStore::open(&paths).expect("empty store");
        store
            .upsert(Connection {
                token: token.map(str::to_owned),
                ..samples::connection(&key.connection_id, ProviderKind::GitHub, API)
            })
            .expect("store the connection");
        drop(store);
        let mut harness = Harness::builder()
            .with_size([1500.0, 900.0])
            .with_step_dt(frame)
            .with_context_setup(theme::fonts::install)
            .build_ui_state(
                |ui, scene: &mut Option<Scene>| {
                    let Some(Scene { page, navigator, services }) = scene else {
                        return;
                    };
                    let ctx = ui.ctx().clone();
                    surfaces::begin_frame(&ctx);
                    theme::activate(&ctx, services.settings.theme);
                    focus_mode::begin_frame(&ctx, false);
                    page.show(ui, services, navigator);
                    focus_mode::end_frame(&ctx);
                },
                None,
            );
        let mut services = Services::new(harness.ctx.clone(), paths, provider);
        configure(&mut services);
        let page = PullRequestPage::new(key, &mut services);
        *harness.state_mut() = Some(Scene { page, navigator: Navigator::default(), services });
        Self { harness, _scratch: scratch }
    }

    pub(crate) fn scene(&self) -> &Scene {
        self.harness.state().as_ref().expect("the scene is set up")
    }

    pub(crate) fn scene_mut(&mut self) -> &mut Scene {
        self.harness.state_mut().as_mut().expect("the scene is set up")
    }

    pub(crate) fn page(&self) -> &PullRequestPage {
        &self.scene().page
    }

    pub(crate) fn page_mut(&mut self) -> &mut PullRequestPage {
        &mut self.scene_mut().page
    }

    /// The simulated time.
    pub(crate) fn now(&self) -> f64 {
        self.harness.ctx.input(|input| input.time)
    }

    /// One frame, then the real time the answers it asked for need, so the next frame has them.
    pub(crate) fn frame(&mut self) {
        self.harness.step();
        cv_testkit::poll_until("the answers a frame asked for", || {
            self.scene().services.background.running() == 0
        });
    }

    /// Whether the window has the focus, as the platform reports it.
    pub(crate) fn set_focus(&mut self, is_focused: bool) {
        let input = self.harness.input_mut();
        input.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(is_focused);
        input.focused = is_focused;
    }
}
