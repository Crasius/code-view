use std::future::Future;

use review_core::ProviderError;
use tokio::sync::mpsc;

use super::background::{Background, Task, TaskPoll};

/// The state of one asynchronous value a view shows.
#[derive(Default)]
pub enum Load<T> {
    #[default]
    Idle,
    Loading(Task<Result<T, ProviderError>>),
    Ready(T),
    Failed(String),
}

impl<T: Send + 'static> Load<T> {
    pub fn start<F>(&mut self, background: &Background, future: F)
    where
        F: Future<Output = Result<T, ProviderError>> + Send + 'static,
    {
        *self = Load::Loading(background.spawn(future));
    }

    /// Moves a finished task into `Ready`/`Failed`. Returns true when the state changed this frame.
    pub fn poll(&mut self) -> bool {
        let Load::Loading(task) = self else {
            return false;
        };
        let next = match task.poll() {
            TaskPoll::Pending => return false,
            TaskPoll::Done(Ok(value)) => Load::Ready(value),
            TaskPoll::Done(Err(error)) => Load::Failed(error.to_string()),
            TaskPoll::Lost => Load::Failed("The background task stopped unexpectedly.".into()),
        };
        *self = next;
        true
    }
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Load::Loading(_))
    }

    pub fn is_idle(&self) -> bool {
        matches!(self, Load::Idle)
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Load::Failed(message) => Some(message),
            _ => None,
        }
    }
}

/// Stale-while-revalidate: a value that stays on screen while a newer copy loads. Views show
/// skeletons only before the first value arrived (or was seeded from the page cache); later loads
/// swap the new value in once it is there. Shares `ready`/`error`/`is_loading` with [`Load`].
///
/// A load started with [`Self::start_streaming`] also hands over values on the way to its
/// result: each replaces the shown one while the load still runs.
pub struct Live<T> {
    current: Option<T>,
    next: Load<T>,
    /// The values on the way from a load started with [`Self::start_streaming`].
    partial: Option<mpsc::UnboundedReceiver<T>>,
    error: Option<String>,
    /// Bumped with every value that arrived, so pollers can tell a fresh answer from the old one.
    version: u64,
}

impl<T> Default for Live<T> {
    fn default() -> Self {
        Self { current: None, next: Load::Idle, partial: None, error: None, version: 0 }
    }
}

impl<T: Send + 'static> Live<T> {
    pub fn start<F>(&mut self, background: &Background, future: F)
    where
        F: Future<Output = Result<T, ProviderError>> + Send + 'static,
    {
        self.partial = None;
        self.next.start(background, future);
    }

    /// [`Self::start`] for a load that hands over values on the way to its result through the
    /// [`Publisher`] `load` gets.
    pub fn start_streaming<F>(
        &mut self,
        background: &Background,
        load: impl FnOnce(Publisher<T>) -> F,
    ) where
        F: Future<Output = Result<T, ProviderError>> + Send + 'static,
    {
        let (sender, receiver) = mpsc::unbounded_channel();
        let future = load(Publisher { sender, ctx: background.ctx() });
        self.next.start(background, future);
        self.partial = Some(receiver);
    }

    /// Returns true when a new value arrived this frame: the result, or the latest value on the
    /// way to it.
    pub fn poll(&mut self) -> bool {
        if !self.next.poll() {
            return self.poll_partial();
        }
        self.partial = None;
        match std::mem::take(&mut self.next) {
            Load::Ready(value) => {
                self.current = Some(value);
                self.error = None;
                self.version += 1;
                true
            }
            Load::Failed(message) => {
                self.error = Some(message);
                false
            }
            Load::Idle | Load::Loading(_) => false,
        }
    }

    /// Shows the latest value the running load handed over since the last frame.
    fn poll_partial(&mut self) -> bool {
        let Some(receiver) = &mut self.partial else {
            return false;
        };
        let mut latest = None;
        while let Ok(value) = receiver.try_recv() {
            latest = Some(value);
        }
        let Some(value) = latest else {
            return false;
        };
        self.current = Some(value);
        self.error = None;
        self.version += 1;
        true
    }
}

/// Hands a running load's values on the way to its result to its [`Live`]
/// ([`Live::start_streaming`]), each with one repaint.
pub struct Publisher<T> {
    sender: mpsc::UnboundedSender<T>,
    ctx: egui::Context,
}

impl<T> Publisher<T> {
    pub fn publish(&self, value: T) {
        // Fails only when the Live dropped the load (another one replaced it); then nobody shows
        // the value any more.
        if self.sender.send(value).is_ok() {
            self.ctx.request_repaint();
        }
    }
}

impl<T> Live<T> {
    /// A remembered value (page cache) shown until the first answer replaces it.
    pub fn seeded(value: Option<T>) -> Self {
        Self { current: value, ..Self::default() }
    }

    pub fn ready(&self) -> Option<&T> {
        self.current.as_ref()
    }

    /// The failure to show in place of the value; `None` while a value is on screen.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref().filter(|_| self.current.is_none())
    }

    /// A refresh failed while the older value stayed on screen.
    pub fn refresh_error(&self) -> Option<&str> {
        self.error.as_deref().filter(|_| self.current.is_some())
    }

    pub fn is_loading(&self) -> bool {
        self.next.is_loading()
    }

    /// A newer copy is loading behind the value on screen.
    pub fn is_refreshing(&self) -> bool {
        self.current.is_some() && self.next.is_loading()
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Drops the value, the load and the error. The version stays: versions only grow, so a
    /// watcher that saw one before the clear never takes a later value for the one it saw.
    pub fn clear(&mut self) {
        *self = Self { version: self.version, ..Self::default() };
    }

    /// A value that arrived another way (a live update read it ahead) replaces the shown one.
    pub fn set(&mut self, value: T) {
        self.current = Some(value);
        self.error = None;
        self.version += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::Live;
    use crate::background::Background;
    use crate::refresh::LiveWatch;

    /// Polls until a value arrives, as the frames would.
    fn poll_until_new(live: &mut Live<u32>) {
        let started = Instant::now();
        while !live.poll() {
            assert!(started.elapsed() < Duration::from_secs(10), "no value arrived");
            cv_testkit::let_threads_run(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_value_on_the_way_shows_until_the_result_replaces_it() {
        let background = Background::new(egui::Context::default());
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let mut live = Live::default();
        live.start_streaming(&background, move |publisher| async move {
            publisher.publish(1);
            let _released_or_test_gone = released.await;
            Ok(3)
        });
        poll_until_new(&mut live);
        assert_eq!((live.ready(), live.is_loading(), live.version()), (Some(&1), true, 1));
        release.send(()).expect("the load waits");
        poll_until_new(&mut live);
        assert_eq!((live.ready(), live.is_loading(), live.version()), (Some(&3), false, 2));
    }

    /// A watcher that saw a version before the clear still sees the values after it, however
    /// many arrive between two of its frames.
    #[test]
    fn versions_keep_growing_across_a_clear() {
        let mut live = Live::default();
        let mut watch = LiveWatch::default();
        live.set(1);
        live.set(2);
        assert_eq!(watch.update(&live), Some(true));

        live.clear();
        live.set(3);
        live.set(4);

        assert_eq!(live.version(), 4, "the clear kept the version");
        assert_eq!(watch.update(&live), Some(true), "the watcher saw the new value");
    }
}
