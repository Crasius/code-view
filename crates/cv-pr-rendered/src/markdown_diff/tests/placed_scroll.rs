//! An offset the view places past the end its scroll area knew from the last frame, in the frame
//! in which the rows grew to reach it: the view goes there and stays, it does not spring back
//! towards the old end and coast on.

use egui::Key;
use review_core::DiffSide;
use review_git::FULL_FILE_CONTEXT;
use review_store::DiffLayout;

use super::{Scene, harness, line_in, same, sample, thread_at, tops};

/// At the end of the notes, comments switched on put their cards above the view: the view moves
/// down by their height in the frame they appear, past the end of the frame before, and the last
/// paragraph stays where it was on screen in every frame after.
#[test]
fn comments_switched_on_at_the_end_keep_the_last_block_in_place() {
    let (old, new) = sample();
    let mut scene = Scene::new(&old, &new, DiffLayout::SideBySide, FULL_FILE_CONTEXT);
    scene.doc.threads = [1, 2, 3]
        .into_iter()
        .map(|index| {
            let body = format!("A comment on paragraph {index}.");
            thread_at(&format!("p{index}"), DiffSide::New, line_in(&new, &same(index)), &body)
        })
        .collect();
    scene.doc.show_threads = false;
    scene.doc.past_end = false;
    let mut harness = harness(scene, [1000.0, 500.0]);
    harness.run();
    harness.key_press(Key::End);
    harness.run();
    let last = same(39);
    let before = tops(&harness, &last);
    assert!(before.first().is_some_and(|top| *top > 0.0), "the last paragraph is in view");

    harness.state_mut().doc.show_threads = true;
    for step in 0..12 {
        harness.step();
        let after = tops(&harness, &last);
        assert!(
            after
                .first()
                .zip(before.first())
                .is_some_and(|(after, before)| (after - before).abs() < 0.5),
            "step {step} after comments showed: the last paragraph moved from {before:?} to {after:?}"
        );
    }
}
