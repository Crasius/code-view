//! The file of the thread row the pointer is on (the Threads panel, the Overview's timeline), for
//! the Files tab, which loads it ahead as it does a hovered row of its tree once the pointer rests
//! (the prefetch in `cv-pr-files`). A row names its file each frame it is hovered; the name holds
//! for that frame and the next, as the row may be drawn after the Files tab plans.

use std::sync::Arc;

use review_core::ReviewThread;
use review_git::ChangedFile;

use super::ThreadsUi;

/// A thread row's file, named at a pass of egui.
#[derive(Clone)]
struct Hovered {
    pass: u64,
    file: Arc<ChangedFile>,
}

fn id() -> egui::Id {
    egui::Id::unique("thread_row_hovered_file")
}

/// The pointer is on `thread`'s row or card in a list (the Threads panel, the Overview's
/// timeline) this frame.
pub fn note(ctx: &egui::Context, threads: &ThreadsUi, thread: &ReviewThread) {
    name(ctx, thread, |path| threads.code.changed_file(path));
}

/// Names `thread`'s file for this frame, when it is a changed file (`changed_file`) whose diff the
/// thread shows in, not the version an outdated thread was written on.
fn name<'a>(
    ctx: &egui::Context,
    thread: &ReviewThread,
    changed_file: impl FnOnce(&str) -> Option<&'a ChangedFile>,
) {
    if thread.is_outdated && thread.origin.is_some() {
        return;
    }
    let Some(file) = thread.anchor.as_ref().and_then(|anchor| changed_file(&anchor.path)) else {
        return;
    };
    let hovered = Hovered { pass: ctx.cumulative_pass_nr(), file: Arc::new(file.clone()) };
    ctx.data_mut(|data| data.insert_temp(id(), hovered));
}

/// The file of the thread row the pointer is on, named this frame or the one before.
pub fn file(ctx: &egui::Context) -> Option<ChangedFile> {
    let hovered: Hovered = ctx.data(|data| data.get_temp(id()))?;
    let is_fresh = hovered.pass + 1 >= ctx.cumulative_pass_nr();
    is_fresh.then(|| ChangedFile::clone(&hovered.file))
}

#[cfg(test)]
mod tests {
    use review_git::{ChangeKind, ChangedFile};

    use super::super::samples;

    #[test]
    fn a_hovered_thread_row_names_its_file_for_this_frame_and_the_next() {
        let ctx = egui::Context::default();
        let file = ChangedFile::sample("src/a.rs", ChangeKind::Modified);
        let thread = samples::discussion("t1", samples::lines("src/a.rs", 3, 4));
        let pass = |ctx: &egui::Context| {
            ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        };
        super::name(&ctx, &thread, |path| (path == file.path).then_some(&file));
        assert_eq!(super::file(&ctx).map(|named| named.path), Some("src/a.rs".to_owned()));
        pass(&ctx);
        assert!(super::file(&ctx).is_some(), "the row may be drawn after the Files tab plans");
        pass(&ctx);
        assert!(super::file(&ctx).is_none(), "the pointer left the row");

        let general = samples::resolved("t2", None);
        super::name(&ctx, &general, |_| Some(&file));
        assert!(super::file(&ctx).is_none(), "a general thread has no file");
        super::name(&ctx, &thread, |_| None);
        assert!(super::file(&ctx).is_none(), "a file outside the change is not loaded ahead");
    }
}
