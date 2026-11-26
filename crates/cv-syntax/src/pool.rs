//! A fixed number of named background threads taking jobs from one queue, urgent ones first.
//! The threads start with the first job and live as long as the app.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, Once, PoisonError};

type Job = Box<dyn FnOnce() + Send>;

/// Where a job joins the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// Before the jobs waiting: the file on screen.
    Urgent,
    /// Behind the jobs waiting: work ahead of the user.
    Queued,
}

pub struct Pool {
    name: &'static str,
    threads: usize,
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
    start: Once,
}

impl Pool {
    pub const fn new(name: &'static str, threads: usize) -> Self {
        Self {
            name,
            threads,
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
            start: Once::new(),
        }
    }

    /// Queues `job` behind the jobs waiting, or before them when [`Priority::Urgent`].
    pub fn submit(&'static self, priority: Priority, job: impl FnOnce() + Send + 'static) {
        self.start.call_once(|| {
            for index in 0..self.threads {
                // A thread that cannot start leaves the queue to the others. With none, the jobs
                // never run and every diff stays uncolored, which is how it shows before its
                // colors arrive; nothing waits on a job without a timeout.
                let _colors_are_optional = std::thread::Builder::new()
                    .name(format!("{}-{index}", self.name))
                    .spawn(move || self.work());
            }
        });
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        match priority {
            Priority::Urgent => queue.push_front(Box::new(job)),
            Priority::Queued => queue.push_back(Box::new(job)),
        }
        drop(queue);
        self.wake.notify_one();
    }

    fn work(&self) {
        loop {
            let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
            let job = loop {
                if let Some(job) = queue.pop_front() {
                    break job;
                }
                queue = self.wake.wait(queue).unwrap_or_else(PoisonError::into_inner);
            };
            drop(queue);
            job();
        }
    }
}
