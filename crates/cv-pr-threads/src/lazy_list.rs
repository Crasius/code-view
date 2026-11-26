//! Long lists of blocks of different heights (the Threads panel's rows, the overview's cards)
//! that lay out only the blocks near the view. A block laid out once keeps its height; while it
//! is farther than [`AHEAD_SCREENS`] from the view it takes that much space and nothing else, so
//! a review with hundreds of threads costs what the few on screen cost. A block's height is
//! measured again whenever it comes near the view, which corrects a height that changed while it
//! was away (a new reply, another width).
//!
//! Every block is laid out on the first frame, once: its height is not known before.

use std::collections::HashMap;

use egui::{Rect, Ui, pos2, vec2};

/// How far beyond the view, in view heights, blocks are still laid out: a glide shows them
/// already laid out, and so does a step to the next thread.
const AHEAD_SCREENS: f32 = 0.5;

/// The heights of a list's blocks, by key, as last laid out.
#[derive(Default)]
pub struct Heights(HashMap<String, f32>);

/// Lays out `content` as the block `key` when it is near the view, or always with `must_show`
/// (it scrolls itself into view, or its height eases); otherwise only takes its known height.
/// Returns true when `content` ran.
pub fn block(
    ui: &mut Ui,
    heights: &mut Heights,
    key: &str,
    must_show: bool,
    content: impl FnOnce(&mut Ui),
) -> bool {
    let top = ui.cursor().top();
    if !must_show && let Some(&height) = heights.0.get(key) {
        let rect =
            Rect::from_min_size(pos2(ui.cursor().left(), top), vec2(ui.available_width(), height));
        let clip = ui.clip_rect();
        let near = clip.expand2(vec2(0.0, clip.height() * AHEAD_SCREENS));
        if !ui.is_visible() || !near.intersects(rect) {
            ui.add_space(height);
            return false;
        }
    }
    content(ui);
    let height = ui.cursor().top() - top;
    match heights.0.get_mut(key) {
        Some(known) => *known = height,
        None => {
            heights.0.insert(key.to_owned(), height);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{Heights, block};

    /// 200 blocks of 50 points in a 400-point view: the first frame lays all out, later frames
    /// only those near the view, and the list keeps its full height.
    #[test]
    fn only_blocks_near_the_view_are_laid_out_after_the_first_frame() {
        let ctx = egui::Context::default();
        let mut heights = Heights::default();
        let frame = |heights: &mut Heights| {
            let mut shown = Vec::new();
            let mut bottom = 0.0;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(300.0, 400.0),
                )),
                ..egui::RawInput::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let top = ui.cursor().top();
                for index in 0..200 {
                    let key = index.to_string();
                    if block(ui, heights, &key, index == 150, |ui| {
                        ui.allocate_exact_size(egui::vec2(100.0, 50.0), egui::Sense::hover());
                    }) {
                        shown.push(index);
                    }
                }
                bottom = ui.cursor().top() - top;
            });
            output.textures_delta.clear();
            (shown, bottom)
        };
        let (first, height) = frame(&mut heights);
        assert_eq!(first.len(), 200, "heights are not known yet");
        assert_eq!(height, 10_000.0);
        let (later, height) = frame(&mut heights);
        assert_eq!(height, 10_000.0, "skipped blocks keep their space");
        assert!(later.contains(&0) && later.contains(&11), "the view and half a view below");
        assert!(!later.contains(&13), "farther blocks are skipped");
        assert!(later.contains(&150), "a block that must show is laid out wherever it is");
    }
}
