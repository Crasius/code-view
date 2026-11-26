//! One background thread colors whole files, one at a time. Compiling a grammar is the expensive
//! part (about 30 MB for TypeScript on the pure-Rust regex engine); the process
//! keeps the compiled sets (see [`grammar::compiled`]), so a job holds its set only while it runs.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};

use crate::grammar::{self, Grammar, WorkingSet};

type Job = Box<dyn FnOnce(&mut WarmSet) + Send>;

static JOBS: OnceLock<Sender<Job>> = OnceLock::new();

/// The compiled grammar set the running job colors with.
#[derive(Default)]
pub struct WarmSet {
    current: Option<Arc<WorkingSet>>,
}

impl WarmSet {
    pub fn for_grammar(&mut self, grammar: Grammar) -> &Arc<WorkingSet> {
        self.current.insert(grammar::compiled(grammar))
    }
}

/// Queues `job` behind the jobs already waiting. A job whose file is gone should return at once.
pub fn submit(job: impl FnOnce(&mut WarmSet) + Send + 'static) {
    let jobs = JOBS.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        // If the thread cannot start, the receiver is dropped with the closure and every job is
        // dropped unrun, which the file view reports as a failed load with a Retry.
        let _reported_as_failed_load =
            std::thread::Builder::new().name("syntax-colors".into()).spawn(move || run(&receiver));
        sender
    });
    // Fails only when the thread never started: the job is dropped unrun, as above.
    let _reported_as_failed_load = jobs.send(Box::new(job));
}

fn run(jobs: &Receiver<Job>) {
    while let Ok(job) = jobs.recv() {
        job(&mut WarmSet::default());
    }
}

/// Test driver: every repaint request on `ctx` (each delivered chunk and each finished job asks
/// for one) arrives on the returned channel, so a test waits for the workers instead of sleeping.
#[cfg(any(test, feature = "test-support"))]
pub fn observe_repaints(ctx: &egui::Context) -> Receiver<()> {
    let (sender, receiver) = mpsc::channel();
    ctx.set_repaint_observer(move |_| {
        // The test stopped listening once it had what it waited for.
        let _test_ended = sender.send(());
    });
    receiver
}
