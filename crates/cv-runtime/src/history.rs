//! Back and forward like a browser: where the user was, one step at a time in either direction.
//!
//! A place is the route plus, on a pull request, its tab, so a tab switch is a step of its own.
//! The app records every change of place it sees; going back moves the current place onto the
//! forward stack and the other way round, and a new step clears the forward stack. Both stacks are
//! bounded; the oldest entries fall off. A trackpad swipe asks for a step too ([`swipe`]).
//!
//! Each entry keeps the breadcrumb trail its place showed ([`Visit`]), so a step restores it;
//! only the place decides whether two entries are the same step.

pub mod swipe;

use super::route::{PullRequestTab, Route};
use super::trail::Trail;

/// Entries kept on each stack.
const LIMIT: usize = 50;

/// Where the user was: the page and, on a pull request, the tab it showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub route: Route,
    pub tab: Option<PullRequestTab>,
}

/// A place with the trail it showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Visit {
    pub place: Place,
    pub trail: Trail,
}

/// A step through the history, asked for by Alt+Left/Right (⌘[ and ⌘], ⌘← and ⌘→ on macOS), the
/// mouse's side buttons, a trackpad swipe or the bar's arrows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Back,
    Forward,
}

#[derive(Default)]
pub struct History {
    /// Oldest first; the last entry is where back leads.
    back: Vec<Visit>,
    /// Oldest first; the last entry is where forward leads.
    forward: Vec<Visit>,
}

impl History {
    /// The user went from `left` to `entered` on a new way: `left` becomes the back step and
    /// forward is gone. Staying in place records nothing, and `left` is not stacked twice.
    pub fn record(&mut self, left: Visit, entered: &Place) {
        if left.place == *entered {
            return;
        }
        self.forward.clear();
        if self.back.last().map(|last| &last.place) != Some(&left.place) {
            push_bounded(&mut self.back, left);
        }
    }

    /// Where a step from `current` leads, with `current` moved onto the opposite stack; `None`
    /// when there is nothing that way.
    pub fn step(&mut self, step: Step, current: Visit) -> Option<Visit> {
        let (from, to) = match step {
            Step::Back => (&mut self.back, &mut self.forward),
            Step::Forward => (&mut self.forward, &mut self.back),
        };
        // An entry equal to the current place would be a step that goes nowhere.
        while from.last().is_some_and(|last| last.place == current.place) {
            from.pop();
        }
        let target = from.pop()?;
        push_bounded(to, current);
        Some(target)
    }

    /// Drops the entries of `connection_id` from both stacks: its pages are gone with it. The
    /// trails of the others lose its levels.
    pub fn forget_connection(&mut self, connection_id: &str) {
        for stack in [&mut self.back, &mut self.forward] {
            stack.retain(|visit| visit.place.route.connection_id() != Some(connection_id));
            for visit in stack.iter_mut() {
                visit.trail.forget_connection(connection_id);
            }
        }
    }

    /// The pages back leads to, oldest first.
    #[cfg(any(test, feature = "test-support"))]
    pub fn back_routes(&self) -> Vec<&Route> {
        self.back.iter().map(|visit| &visit.place.route).collect()
    }

    /// Where a step would lead, for the bar's arrows and their tooltips.
    pub fn target(&self, step: Step) -> Option<&Place> {
        let stack = match step {
            Step::Back => &self.back,
            Step::Forward => &self.forward,
        };
        stack.last().map(|visit| &visit.place)
    }
}

fn push_bounded(stack: &mut Vec<Visit>, visit: Visit) {
    stack.push(visit);
    if stack.len() > LIMIT {
        stack.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::{History, LIMIT, Place, Step, Visit};
    use crate::route::{PullRequestKey, PullRequestTab, Route};
    use crate::trail::Trail;

    fn visit(place: Place) -> Visit {
        Visit { trail: Trail::structural(&place.route), place }
    }

    /// A step from `current` with its structural trail; the place it leads to.
    fn step(history: &mut History, step: Step, current: Place) -> Option<Place> {
        history.step(step, visit(current)).map(|visit| visit.place)
    }

    fn page(route: Route) -> Place {
        Place { route, tab: None }
    }

    fn pull_request(tab: PullRequestTab) -> Place {
        let key = PullRequestKey {
            connection_id: "gh".into(),
            repository_key: "acme/widgets".into(),
            number: 7,
        };
        Place { route: Route::PullRequest(key), tab: Some(tab) }
    }

    /// Records the way list → repositories → settings and returns the history with settings
    /// as the current place.
    fn walked() -> History {
        let mut history = History::default();
        history.record(visit(page(Route::MyPullRequests)), &page(Route::Repositories));
        history.record(visit(page(Route::Repositories)), &page(Route::Settings));
        history
    }

    /// A step restores the trail the place showed, which is no part of its sameness.
    #[test]
    fn steps_restore_the_trail_of_the_place() {
        let mut history = History::default();
        let list = page(Route::MyPullRequests);
        let pull_request = pull_request(PullRequestTab::Overview);
        let from_list = Trail::structural(&list.route).entered(&pull_request.route);
        history.record(visit(list.clone()), &pull_request);
        let opened = Visit { place: pull_request.clone(), trail: from_list };
        // Left the same place once more on another way: it is still one entry.
        history.record(opened.clone(), &list);
        history.record(visit(pull_request.clone()), &list);
        let back = history.step(Step::Back, visit(list.clone()));
        assert_eq!(back, Some(opened), "the pull request comes back below the list");
        assert_eq!(history.target(Step::Back), Some(&list));
    }

    #[test]
    fn back_and_forward_retrace_the_way() {
        let mut history = walked();
        assert_eq!(history.target(Step::Forward), None);
        let back = step(&mut history, Step::Back, page(Route::Settings));
        assert_eq!(back, Some(page(Route::Repositories)));
        let back = step(&mut history, Step::Back, page(Route::Repositories));
        assert_eq!(back, Some(page(Route::MyPullRequests)));
        assert_eq!(step(&mut history, Step::Back, page(Route::MyPullRequests)), None);
        let forward = step(&mut history, Step::Forward, page(Route::MyPullRequests));
        assert_eq!(forward, Some(page(Route::Repositories)));
        let forward = step(&mut history, Step::Forward, page(Route::Repositories));
        assert_eq!(forward, Some(page(Route::Settings)));
        assert_eq!(step(&mut history, Step::Forward, page(Route::Settings)), None);
        assert_eq!(history.target(Step::Back), Some(&page(Route::Repositories)));
    }

    #[test]
    fn a_new_step_clears_forward() {
        let mut history = walked();
        step(&mut history, Step::Back, page(Route::Settings));
        assert_eq!(history.target(Step::Forward), Some(&page(Route::Settings)));
        history.record(visit(page(Route::Repositories)), &page(Route::MyPullRequests));
        assert_eq!(history.target(Step::Forward), None);
        assert_eq!(history.target(Step::Back), Some(&page(Route::Repositories)));
    }

    #[test]
    fn tab_switches_are_steps() {
        let mut history = History::default();
        history.record(visit(page(Route::MyPullRequests)), &pull_request(PullRequestTab::Overview));
        history.record(
            visit(pull_request(PullRequestTab::Overview)),
            &pull_request(PullRequestTab::Files),
        );
        let back = step(&mut history, Step::Back, pull_request(PullRequestTab::Files));
        assert_eq!(back, Some(pull_request(PullRequestTab::Overview)));
        assert_eq!(history.target(Step::Forward), Some(&pull_request(PullRequestTab::Files)));
    }

    #[test]
    fn staying_or_repeating_a_place_records_it_once() {
        let mut history = History::default();
        history.record(visit(page(Route::Settings)), &page(Route::Settings));
        assert_eq!(history.target(Step::Back), None);
        history.record(visit(page(Route::MyPullRequests)), &page(Route::Settings));
        // Left the same place again (a revisit that recorded nothing in between).
        history.record(visit(page(Route::MyPullRequests)), &page(Route::Repositories));
        assert_eq!(
            step(&mut history, Step::Back, page(Route::Repositories)),
            Some(page(Route::MyPullRequests))
        );
        assert_eq!(step(&mut history, Step::Back, page(Route::MyPullRequests)), None);
    }

    #[test]
    fn a_step_skips_entries_equal_to_the_current_place() {
        let mut history = walked();
        // Back from the repositories page itself: its own entry leads nowhere.
        let back = step(&mut history, Step::Back, page(Route::Repositories));
        assert_eq!(back, Some(page(Route::MyPullRequests)));
    }

    #[test]
    fn a_removed_connection_leaves_both_stacks() {
        let mut history = History::default();
        let pull_request = || pull_request(PullRequestTab::Overview);
        let other = page(Route::Connection { connection_id: "gl".into() });
        // Walked pull request → other connection → settings → pull request → repositories, then
        // back twice: the pull request is on both stacks.
        history.record(visit(pull_request()), &other);
        history.record(visit(other.clone()), &page(Route::Settings));
        history.record(visit(page(Route::Settings)), &pull_request());
        history.record(visit(pull_request()), &page(Route::Repositories));
        step(&mut history, Step::Back, page(Route::Repositories));
        step(&mut history, Step::Back, pull_request());

        history.forget_connection("gh");
        assert_eq!(step(&mut history, Step::Back, page(Route::Settings)), Some(other.clone()));
        assert_eq!(step(&mut history, Step::Back, other.clone()), None, "the first entry went too");
        assert_eq!(step(&mut history, Step::Forward, other), Some(page(Route::Settings)));
        let forward = step(&mut history, Step::Forward, page(Route::Settings));
        assert_eq!(forward, Some(page(Route::Repositories)), "forward skips the pull request");
    }

    #[test]
    fn stacks_keep_the_newest_entries() {
        let mut history = History::default();
        let numbered =
            |number: usize| page(Route::Connection { connection_id: number.to_string() });
        for number in 0..LIMIT + 10 {
            history.record(visit(numbered(number)), &numbered(number + 1));
        }
        let mut current = numbered(LIMIT + 10);
        let mut steps = 0;
        while let Some(previous) = step(&mut history, Step::Back, current.clone()) {
            current = previous;
            steps += 1;
        }
        assert_eq!(steps, LIMIT);
        assert_eq!(current, numbered(10), "the oldest entries fell off");
    }
}
