//! The app built the way the whole-app tests build it never reads the machine's reduced-motion
//! setting: only the binary's `main` does (`motion::install`, repo-lint RL016). A walkthrough
//! that asserts a fade or a pulse would otherwise fail on a machine with Reduce Motion on, as
//! four did on GitHub's macOS runners.

use cv_base::theme::motion;
use egui_kittest::Harness;

use super::walkthrough::app_with;
use cv_testkit::ScratchDir;

/// Whether the app built with `preset` (or nothing) set before it exists animates.
fn is_reduced_in_app(name: &str, preset: Option<bool>) -> bool {
    let scratch = ScratchDir::new(name);
    let mut builder = Harness::builder().with_size([960.0, 600.0]);
    if let Some(is_reduced) = preset {
        builder = builder.with_context_setup(move |ctx| motion::set_reduced(ctx, is_reduced));
    }
    let harness = app_with(builder, scratch.app_paths(), cv_providers::create_provider);
    motion::is_reduced(&harness.ctx)
}

#[test]
fn the_app_built_by_a_test_keeps_the_motion_it_was_given() {
    // Unset, as in every walkthrough: it animates, whatever the machine says.
    assert!(!is_reduced_in_app("motion-unset", None), "an unset setting animates");
    // Set before the app exists: the app keeps it instead of reading the machine's, which on any
    // one machine differs from one of the two.
    assert!(!is_reduced_in_app("motion-on", Some(false)), "motion stays on");
    assert!(is_reduced_in_app("motion-reduced", Some(true)), "reduced motion stays reduced");
}
