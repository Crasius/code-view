//! One-shot hand-overs between widgets that do not know each other: a lower crate posts a value
//! under an id, and the crate that answers takes it once on its next frame. The value waits in
//! `ctx.data`, so the poster never names the reader. Per-widget state that lives across frames
//! (animation times, scroll memory) is not a mailbox.

use std::any::Any;

use egui::util::id_type_map::RawKey;

/// Leaves `value` for the reader of `id` and asks for a frame.
pub fn post<T: Any + Clone + Send + Sync>(ctx: &egui::Context, id: egui::Id, value: T) {
    ctx.data_mut(|data| data.insert_temp(id, value));
    ctx.request_repaint();
}

/// Takes what was posted under `id`, once.
pub fn take<T: Any + Send + Sync>(ctx: &egui::Context, id: egui::Id) -> Option<T> {
    let posted = ctx.data_mut(|data| data.remove_temp_raw(RawKey::new::<T>(id)))?;
    posted.downcast::<T>().ok().map(|value| *value)
}

/// Takes it only when `is_mine` says so; else leaves it for another reader.
pub fn take_if<T: Any + Clone + Send + Sync>(
    ctx: &egui::Context,
    id: egui::Id,
    is_mine: impl FnOnce(&T) -> bool,
) -> Option<T> {
    ctx.data_mut(|data| {
        let posted = data.get_temp::<T>(id).filter(is_mine)?;
        data.remove::<T>(id);
        Some(posted)
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn id() -> egui::Id {
        egui::Id::unique("mailbox_test")
    }

    #[test]
    fn a_post_is_taken_once() {
        let ctx = egui::Context::default();
        post(&ctx, id(), String::from("open search"));
        assert_eq!(take::<String>(&ctx, id()).as_deref(), Some("open search"));
        assert_eq!(take::<String>(&ctx, id()), None);
    }

    #[test]
    fn a_post_of_another_type_is_not_taken() {
        let ctx = egui::Context::default();
        post(&ctx, id(), 7_u32);
        assert_eq!(take::<u64>(&ctx, id()), None);
        assert_eq!(take::<u32>(&ctx, id()), Some(7));
    }

    #[test]
    fn take_if_leaves_a_post_that_is_not_the_readers() {
        let ctx = egui::Context::default();
        post(&ctx, id(), 3_u32);
        assert_eq!(take_if::<u32>(&ctx, id(), |key| *key == 4), None);
        assert_eq!(take_if::<u32>(&ctx, id(), |key| *key == 3), Some(3));
        assert_eq!(take::<u32>(&ctx, id()), None);
    }

    #[test]
    fn a_post_asks_for_a_frame() {
        let ctx = egui::Context::default();
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&asked);
        ctx.set_request_repaint_callback(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        post(&ctx, id(), 1_u32);
        assert!(asked.load(Ordering::Relaxed) > 0);
    }
}
