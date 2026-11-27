//! App-wide services the views share through [`Services`]: providers, git mirrors, checkouts,
//! coding agents, the user's writes, line counts and persistence.

#![forbid(unsafe_code)]
// RS-LINT-02: the size column's bar turns a line count into a fraction of its width
// (diff_stats::bar_fraction).
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "egui pixel math converts between f32 points and integer counts"
)]

pub mod agent_names;
pub mod agents;
#[cfg(any(test, feature = "test-support"))]
mod agents_fake;
pub mod checkouts;
pub mod connection;
pub mod diff_stats;
pub mod linked;
pub mod memory_journal;
pub mod merge_policies;
pub mod mirrors;
pub mod self_review;
pub mod writes;

use std::sync::Arc;

use review_core::{Connection, Provider};
use review_store::retention::{Retention, Schedule};
use review_store::{AppPaths, AppSettings, ConnectionStore};

use cv_media::image_clipboard::ImageClipboard;
use cv_media::images::ImageStore;
use cv_media::{mermaid, video};
use cv_runtime::background::Background;
use cv_runtime::cache::cold::ColdTier;
use cv_runtime::cache::page_cache::PageCache;
use cv_runtime::refresh::RefreshScheduler;
use cv_runtime::toasts::{ToastAction, Toasts};

use self::agents::AgentDirectory;
use self::checkouts::Checkouts;
use self::connection::providers::ProviderRegistry;
use self::connection::traffic::{self, ConnectionRate, RateLimited};
use self::diff_stats::DiffStats;
use self::linked::LinkedCache;
use self::merge_policies::MergePolicies;
use self::mirrors::MirrorRegistry;

pub struct Services {
    pub background: Background,
    pub paths: AppPaths,
    pub connections: ConnectionStore,
    pub settings: AppSettings,
    pub providers: ProviderRegistry,
    pub mirrors: Arc<MirrorRegistry>,
    /// Keeps the cache folder small: scheduled sweeps, Settings → Storage, removed connections.
    pub retention: Retention,
    pub toasts: Toasts,
    /// Every automatic refresh asks here; nothing fires while the window is inactive.
    pub refresh: RefreshScheduler,
    /// Last answers of lists and pull request parts, shown at once on a revisit.
    pub cache: PageCache,
    /// The disk tier behind the memory caches: what is expensive to get again after a restart.
    pub cold: ColdTier,
    /// Title and state of linked pull requests, issues and work items, shared by all pages.
    pub linked: LinkedCache,
    /// Merge strategies each target branch allows, loaded where a merge is offered.
    pub merge_policies: MergePolicies,
    /// Fails a test whose services live past the watchdog limit instead of letting it hang.
    #[cfg(any(test, feature = "test-support"))]
    _watchdog: cv_testkit::Watchdog,
    /// Lines and files of listed pull requests, counted for the list rows' size column.
    pub diff_stats: DiffStats,
    /// Images embedded in descriptions, comments and rendered files, loaded as they scroll into view.
    pub images: Arc<ImageStore>,
    /// "Copy image": puts images on the clipboard off the UI thread and toasts the outcome.
    pub image_clipboard: ImageClipboard,
    /// The coding agent CLIs the pull request assistant drives, looked for once per app run.
    pub agents: AgentDirectory,
    /// The user's writes that succeeded, which every view of their pull request catches up with.
    pub writes: writes::Writes,
    /// Pull requests checked out locally, the checkouts that run and their upkeep.
    pub checkouts: Checkouts,
}

impl Services {
    pub fn new(
        ctx: egui::Context,
        paths: AppPaths,
        factory: impl Fn(Connection, reqwest::Client) -> Arc<dyn Provider> + Send + Sync + 'static,
    ) -> Self {
        Self::with_providers(ctx, paths, ProviderRegistry::new(factory))
    }

    fn with_providers(ctx: egui::Context, paths: AppPaths, providers: ProviderRegistry) -> Self {
        #[cfg(any(test, feature = "test-support"))]
        let watchdog = cv_testkit::Watchdog::start();
        #[cfg(any(test, feature = "test-support"))]
        cv_testkit::assert_hermetic(&paths);
        let cold = ColdTier::new(&paths);
        let mut toasts = Toasts::default();
        let (connections, are_connections_read) = match ConnectionStore::open(&paths) {
            Ok(connections) => (connections, true),
            Err(error) => {
                let folder = ToastAction::open_folder(paths.config_dir());
                toasts.error(format!("Could not read saved connections: {error}"), folder);
                let recovered = ConnectionStore::load(
                    paths.config_dir().join("connections.recovered.json"),
                    paths.token_vault(),
                )
                .expect("an absent recovery file always loads");
                (recovered, false)
            }
        };
        let mut settings = load_settings(&paths, &mut toasts);
        // Without the saved connections every id would look removed: their favorites, merge
        // choices and filters stay for the start that reads the file again.
        if are_connections_read {
            settings.retain_connections(|id| connections.find(id).is_some());
        }
        let (checkouts, checkouts_error) = Checkouts::open(&paths);
        if let Some(message) = checkouts_error {
            toasts.error(message, ToastAction::open_folder(paths.config_dir()));
        }
        let mirrors = Arc::new(MirrorRegistry::new(paths.mirrors_dir()));
        let repaint = ctx.clone();
        let retention = Retention::start(
            paths.clone(),
            mirrors.clone(),
            Arc::new(move || repaint.request_repaint()),
            Schedule::APP,
        );
        let background = Background::new(ctx);
        let images = Arc::new(ImageStore::new(background.spawner()));
        let image_clipboard = ImageClipboard::new(background.spawner());
        Self {
            background,
            images,
            image_clipboard,
            paths,
            connections,
            settings,
            providers,
            mirrors,
            retention,
            toasts,
            refresh: RefreshScheduler::default(),
            cache: PageCache::with_cold(cold.clone()),
            cold,
            linked: LinkedCache::default(),
            merge_policies: MergePolicies::default(),
            #[cfg(any(test, feature = "test-support"))]
            _watchdog: watchdog,
            diff_stats: DiffStats::default(),
            agents: AgentDirectory::default(),
            writes: writes::Writes::default(),
            checkouts,
        }
    }

    pub fn connection(&self, connection_id: &str) -> Option<&Connection> {
        self.connections.find(connection_id)
    }

    /// The provider for a stored connection, or `None` when it was removed meanwhile.
    pub fn provider(&mut self, connection_id: &str) -> Option<Arc<dyn Provider>> {
        let connection = self.connections.find(connection_id)?.clone();
        Some(self.providers.get(&connection))
    }

    /// After the stored connection `connection_id` changed (a fixed token, a renewed sign-in): its
    /// provider takes the new settings everywhere it is held, and images that failed for lack of
    /// access load once more when they are next seen.
    pub fn connection_saved(&mut self, connection_id: &str) {
        let Some(connection) = self.connections.find(connection_id).cloned() else {
            return;
        };
        self.providers.update(&connection);
        self.images.retry_private(connection_id);
    }

    /// The frame's bookkeeping after the pages drew: finished checkouts and their updates land
    /// (and open what they were asked to), and what nobody drew lately is let go.
    pub fn end_frame(&mut self, ctx: &egui::Context) {
        checkouts::poll(self);
        self.release_unseen(ctx);
    }

    /// Lets go of the image and diagram textures and the video players nobody drew lately.
    fn release_unseen(&self, ctx: &egui::Context) {
        self.images.release_unseen(ctx.input(|input| input.time));
        mermaid::DiagramStore::release_unseen(ctx);
        video::player::Players::release_unseen(ctx);
    }

    /// egui's input time, the clock of the page cache and the refresh scheduler.
    pub fn now(&self) -> f64 {
        self.background.ctx().input(|input| input.time)
    }

    pub fn save_settings(&mut self) {
        if let Err(error) = self.settings.save(&self.paths.settings_file()) {
            let folder = ToastAction::open_folder(self.paths.config_dir());
            self.toasts.error(format!("Could not save settings: {error}"), folder);
        }
    }

    /// Requests per minute (and totals per endpoint) for every stored connection that sent any.
    pub fn request_rates(&self) -> Vec<ConnectionRate> {
        traffic::request_rates(&self.connections)
    }

    /// Stored connections that are paused by a rate limit right now, soonest resume first.
    pub fn rate_limited(&self) -> Vec<RateLimited> {
        traffic::rate_limited(&self.connections)
    }
}

/// The saved settings, or the defaults when the file cannot be read. Then the file is moved aside
/// first, so the next save does not overwrite what the user set, and a toast says where it went.
fn load_settings(paths: &AppPaths, toasts: &mut Toasts) -> AppSettings {
    let file = paths.settings_file();
    let error = match AppSettings::load(&file) {
        Ok(settings) => return settings,
        Err(error) => error,
    };
    let kept = paths.config_dir().join("settings.unreadable.json");
    let text = match std::fs::rename(&file, &kept) {
        Ok(()) => format!(
            "Could not read the settings ({error}), so they start from the defaults. The old file \
             is kept as {}.",
            kept.display()
        ),
        Err(move_error) => format!(
            "Could not read the settings ({error}), so they start from the defaults. The old \
             file could not be kept aside ({move_error}): copy {} before changing a setting.",
            file.display()
        ),
    };
    toasts.error(text, ToastAction::open_folder(paths.config_dir()));
    AppSettings::default()
}

#[cfg(test)]
mod tests {
    use review_store::{AppPaths, AppSettings};

    use cv_testkit::ScratchDir;

    use super::Services;

    fn paths(dir: &ScratchDir) -> AppPaths {
        AppPaths::new(dir.path().join("config"), dir.path().join("cache"))
    }

    #[test]
    fn unreadable_settings_are_kept_aside_and_reported() {
        let dir = ScratchDir::new("services-unreadable-settings");
        let paths = paths(&dir);
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.settings_file(), "{\"theme\": \"Dark\", ").unwrap();
        let mut services =
            Services::new(egui::Context::default(), paths.clone(), cv_providers::create_provider);
        let errors = services.toasts.error_texts();
        assert!(
            errors.iter().any(|text| text.starts_with("Could not read the settings")),
            "the reset is reported: {errors:?}"
        );
        services.save_settings();
        let kept = std::fs::read_to_string(paths.config_dir().join("settings.unreadable.json"));
        assert_eq!(kept.unwrap(), "{\"theme\": \"Dark\", ", "a save leaves the old file alone");
    }

    #[test]
    fn unreadable_connections_keep_the_per_connection_settings() {
        let dir = ScratchDir::new("services-unreadable-connections");
        let paths = paths(&dir);
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let mut settings = AppSettings::default();
        settings.collapsed_connections.insert("gh".to_owned());
        settings.save(&paths.settings_file()).unwrap();
        std::fs::write(paths.connections_file(), "not json").unwrap();
        let mut services =
            Services::new(egui::Context::default(), paths.clone(), cv_providers::create_provider);
        assert!(services.settings.collapsed_connections.contains("gh"), "kept in memory");
        services.save_settings();
        let saved = AppSettings::load(&paths.settings_file()).unwrap();
        assert!(saved.collapsed_connections.contains("gh"), "kept on disk");
    }
}
