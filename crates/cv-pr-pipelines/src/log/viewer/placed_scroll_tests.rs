//! An offset the viewer places past the end its scroll area knew from the last frame, in the frame
//! in which the lines grew to reach it: a running job's log opened where a shorter log showed goes
//! to its end and stays there, it does not spring back towards the old end and coast on.

use cv_shell::focus_mode::{self, Surface};
use egui_kittest::Harness;

use super::test_harness::{State, harness_of, log_of};

/// `lines` numbered lines.
fn numbered(lines: usize) -> String {
    let lines: Vec<String> = (1..=lines).map(|line| format!("line {line}")).collect();
    lines.join("\n") + "\n"
}

/// The line at the top of the view.
fn top_line(harness: &Harness<'_, State>) -> usize {
    focus_mode::top_line(&harness.ctx, Surface::Log).expect("the lines showed").0
}

#[test]
fn a_running_jobs_log_opens_at_its_end_after_a_shorter_one() {
    let mut harness = harness_of(log_of(&numbered(120), None));
    harness.run();
    assert_eq!(top_line(&harness), 0, "the finished log shows from its top");

    // A running job's longer log takes its place, as its download would.
    harness.state_mut().log = log_of(&numbered(600), None);
    let viewer = &mut harness.state_mut().viewer;
    viewer.window = None;
    viewer.fetch.is_live = true;
    viewer.fetch.to_end = true;
    harness.step();
    let end = top_line(&harness);
    assert!(end > 500, "the view went to the end: line {end} at the top");
    for step in 0..12 {
        harness.step();
        assert_eq!(top_line(&harness), end, "step {step} after the log opened at its end");
    }
}
