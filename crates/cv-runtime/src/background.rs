use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::oneshot;

/// A small tokio runtime for network and git work. Finished tasks wake the UI with one repaint;
/// nothing polls in between, so an idle app does no work at all.
///
/// Dropping it cancels what still runs, except the tasks started with [`Spawner::spawn_kept`]
/// (the last write of a closing page): the drop waits up to [`EXIT_WAIT`] for those.
pub struct Background {
    runtime: tokio::runtime::Runtime,
    ctx: egui::Context,
    running: Arc<AtomicUsize>,
    kept: Kept,
}

const WORKER_THREADS: usize = 2;

/// How long the app's exit waits for the kept tasks; our own budget, a write takes milliseconds.
pub const EXIT_WAIT: Duration = Duration::from_secs(2);

/// The kept tasks not ended yet, and the signal each one gives when it ends.
type Kept = Arc<(Mutex<usize>, Condvar)>;

impl Background {
    pub fn new(ctx: egui::Context) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKER_THREADS)
            .thread_name("review-io")
            .enable_all()
            .build()
            .expect("tokio runtime must start");
        Self { runtime, ctx, running: Arc::default(), kept: Kept::default() }
    }

    pub fn spawn<T, F>(&self, future: F) -> Task<T>
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let running = Running::start(&self.running, &self.ctx);
        self.runtime.spawn(async move {
            let _running = running;
            let value = future.await;
            // Fails only when the task's owner (a page that closed) dropped its receiver; then
            // nobody waits for the value any more.
            let _owner_gone = sender.send(value);
        });
        Task { receiver }
    }

    /// For tasks that report progress and need to wake the UI before they finish.
    pub fn ctx(&self) -> egui::Context {
        self.ctx.clone()
    }

    /// Starts tasks from where the [`Background`] is out of reach (widgets drawing images): they
    /// run on this runtime, count as [`Self::running`] and wake the UI when done.
    pub fn spawner(&self) -> Spawner {
        Spawner {
            handle: self.runtime.handle().clone(),
            ctx: self.ctx.clone(),
            running: self.running.clone(),
            kept: self.kept.clone(),
        }
    }

    /// Tasks started and not finished yet (network and git work in flight).
    pub fn running(&self) -> usize {
        self.running.load(Ordering::SeqCst)
    }
}

/// Fire-and-forget access to the [`Background`] runtime; the task reports through shared state.
#[derive(Clone)]
pub struct Spawner {
    handle: tokio::runtime::Handle,
    ctx: egui::Context,
    running: Arc<AtomicUsize>,
    kept: Kept,
}

impl Spawner {
    pub fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let running = Running::start(&self.running, &self.ctx);
        self.handle.spawn(async move {
            let _running = running;
            future.await;
        });
    }

    /// Like [`Self::spawn`], for work that must end even when the app exits right after: the
    /// [`Background`]'s drop waits for it.
    pub fn spawn_kept<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let open = Open::start(&self.kept);
        self.spawn(async move {
            let _open = open;
            future.await;
        });
    }
}

/// One task in [`Background::running`]. Counts it out when dropped, so a task that panics (tests
/// and debug builds unwind) or is cancelled leaves the count like one that ended.
struct Running {
    count: Arc<AtomicUsize>,
    ctx: egui::Context,
}

impl Running {
    fn start(count: &Arc<AtomicUsize>, ctx: &egui::Context) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self { count: count.clone(), ctx: ctx.clone() }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::SeqCst);
        self.ctx.request_repaint();
    }
}

/// One kept task not ended yet. Counts it out and wakes the exit wait when dropped, also when the
/// task panicked, so the exit never waits for a task that is gone.
struct Open(Kept);

impl Open {
    fn start(kept: &Kept) -> Self {
        *kept.0.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        Self(kept.clone())
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        let (open, ended) = &*self.0;
        *open.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
        ended.notify_all();
    }
}

impl Drop for Background {
    /// The app exits: the kept tasks end first, up to [`EXIT_WAIT`] in all.
    fn drop(&mut self) {
        let (open, ended) = &*self.kept;
        let open = open.lock().unwrap_or_else(PoisonError::into_inner);
        // The runtime still runs here: it is dropped after this returns.
        let _past_the_wait = ended
            .wait_timeout_while(open, EXIT_WAIT, |open| *open > 0)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

pub struct Task<T> {
    receiver: oneshot::Receiver<T>,
}

pub enum TaskPoll<T> {
    Pending,
    Done(T),
    /// The task panicked or was dropped.
    Lost,
}

/// The message a view shows for a [`TaskPoll::Lost`] task.
pub const TASK_LOST: &str = "The background task stopped unexpectedly.";

impl<T> Task<T> {
    /// A task that already finished with `value`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn ready(value: T) -> Self {
        let (sender, receiver) = oneshot::channel();
        // The receiver is alive right here, so the send cannot fail.
        let _receiver_alive = sender.send(value);
        Self { receiver }
    }

    pub fn poll(&mut self) -> TaskPoll<T> {
        match self.receiver.try_recv() {
            Ok(value) => TaskPoll::Done(value),
            Err(oneshot::error::TryRecvError::Empty) => TaskPoll::Pending,
            Err(oneshot::error::TryRecvError::Closed) => TaskPoll::Lost,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use super::{Background, EXIT_WAIT, Task, TaskPoll};

    #[test]
    fn a_ready_task_is_done_at_the_first_poll() {
        assert!(matches!(Task::ready(7).poll(), TaskPoll::Done(7)));
    }

    /// A task that panics counts out like one that ends, whichever way it started; a kept one
    /// no longer holds the app's exit for the whole [`EXIT_WAIT`].
    #[test]
    fn a_panicking_task_leaves_the_running_count() {
        let background = Background::new(egui::Context::default());
        let mut task = background.spawn::<(), _>(async { panic!("the task fails") });
        let spawner = background.spawner();
        spawner.spawn(async { panic!("the task fails") });
        spawner.spawn_kept(async { panic!("the task fails") });

        let started = Instant::now();
        while background.running() > 0 {
            assert!(started.elapsed() < Duration::from_secs(5), "the panicked tasks still count");
            cv_testkit::let_threads_run(Duration::from_millis(5));
        }
        assert!(matches!(task.poll(), TaskPoll::Lost), "the owner sees the task lost");

        let dropping = Instant::now();
        drop(background);
        assert!(dropping.elapsed() < EXIT_WAIT, "the exit waited for a kept task that panicked");
    }

    /// A kept task that has not even started when the app exits still runs to its end.
    #[test]
    fn dropping_the_runtime_waits_for_a_kept_task() {
        let background = Background::new(egui::Context::default());
        let ended = Arc::new(AtomicBool::new(false));
        let done = ended.clone();
        background.spawner().spawn_kept(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            done.store(true, Ordering::SeqCst);
        });

        drop(background);

        assert!(ended.load(Ordering::SeqCst), "the kept task ended before the drop returned");
    }
}
