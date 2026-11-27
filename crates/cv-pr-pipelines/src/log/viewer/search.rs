//! The search over a downloaded log: the box, the matching lines found off the UI thread, and
//! the current match the view moves to.

use std::sync::Arc;

use cv_base::theme::icons;
use cv_runtime::background::Background;
use cv_runtime::load::Load;
use cv_shell::focus_mode::{self, strip};
use cv_widgets::find_bar::{self, FieldLook, Step};
use cv_widgets::glyph;
use cv_widgets::numbers::group_digits;
use egui::{Id, Ui};
use review_core::ProviderError;

use super::super::download::LoadedLog;

#[derive(Default)]
pub struct LogSearch {
    /// What the search box holds.
    text: String,
    /// The matching lines, ascending.
    matches: Load<Arc<Vec<usize>>>,
    /// What `matches` were found for.
    searched_for: String,
    /// Index into `matches`.
    current: usize,
    /// The search over a grown log runs again without moving to its first match.
    keeps_place: bool,
}

impl LogSearch {
    /// Forgets the matches of the log shown before; the box keeps its text.
    pub fn reset(&mut self) {
        self.matches = Load::Idle;
        self.searched_for.clear();
        self.current = 0;
        self.keeps_place = false;
    }

    /// The matching lines, once found.
    pub fn matches(&self) -> Option<&Arc<Vec<usize>>> {
        self.matches.ready()
    }

    /// The line of the current match.
    pub fn current_line(&self) -> Option<usize> {
        self.matches().and_then(|matches| matches.get(self.current).copied())
    }

    /// Takes in a finished search; returns its first match to scroll to. A search over a grown
    /// log keeps the current match where it still is.
    pub fn poll(&mut self) -> Option<usize> {
        if !self.matches.poll() {
            return None;
        }
        if std::mem::take(&mut self.keeps_place) {
            let count = self.matches().map_or(0, |matches| matches.len());
            self.current = self.current.min(count.saturating_sub(1));
            return None;
        }
        self.matches().and_then(|matches| matches.first().copied())
    }

    /// The log grew: the search runs again over it, in place.
    pub fn rerun(&mut self, log: Arc<LoadedLog>, background: &Background) {
        if self.searched_for.is_empty() || self.matches.is_loading() {
            return;
        }
        self.keeps_place = true;
        let needle = self.searched_for.clone();
        self.find(needle, log, background);
    }

    /// "3 / 12", "No matches", "Searching…" or "Search failed" beside the search box.
    pub fn status(&self) -> String {
        match &self.matches {
            Load::Loading(_) => "Searching…".to_owned(),
            Load::Ready(matches) if matches.is_empty() => "No matches".to_owned(),
            Load::Ready(matches) => {
                format!("{} / {}", self.current + 1, group_digits(matches.len()))
            }
            Load::Failed(_) => "Search failed".to_owned(),
            Load::Idle => String::new(),
        }
    }

    /// Why the search failed and how to try again, on hover over `status` (its [`Self::status`]).
    pub fn explain_failure(&self, status: egui::Response) {
        if let Load::Failed(message) = &self.matches {
            status.on_hover_text(format!("{message}\nPress Enter in the search box to try again."));
        }
    }

    /// The search box: Enter searches, Enter again (Shift+Enter) asks for the next (previous)
    /// match, which this returns. The toolbar and the focus strip share one id: the box keeps the
    /// keys and its cursor when focus mode moves it (one log is on screen at a time).
    pub fn field(
        &mut self,
        ui: &mut Ui,
        log: &Arc<LoadedLog>,
        background: &Background,
        width: f32,
    ) -> Option<Step> {
        let hint = format!("{}  Search log", icons::MAGNIFYING_GLASS);
        let mut look =
            FieldLook { id: Id::unique("pipeline_log_search"), hint: &hint, width, margin: None };
        // In the focus strip the box keeps to the height of the strip's buttons.
        let field = if focus_mode::is_shown(ui.ctx()) {
            look.margin = Some(egui::Margin::symmetric(6, 2));
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = strip::CONTROL_HEIGHT;
                find_bar::field(ui, &mut self.text, &look)
            })
            .inner
        } else {
            find_bar::field(ui, &mut self.text, &look)
        };
        glyph::hint_as(&field.response, "Search log");
        let step = field.submitted?;
        if self.should_search() {
            self.searched_for.clone_from(&self.text);
            self.current = 0;
            self.find(self.text.clone(), log.clone(), background);
            return None;
        }
        Some(step)
    }

    /// Enter searches for new text, and again for the same text after the search failed.
    fn should_search(&self) -> bool {
        self.text != self.searched_for || matches!(self.matches, Load::Failed(_))
    }

    /// Moves to the next or previous match, wrapping around; returns its line to scroll to.
    pub fn step(&mut self, step: Option<Step>) -> Option<usize> {
        let count = self.matches()?.len();
        self.current = find_bar::step_index(Some(self.current), count, step?)?;
        self.current_line()
    }

    fn find(&mut self, needle: String, log: Arc<LoadedLog>, background: &Background) {
        self.matches.start(background, async move {
            let matches = tokio::task::spawn_blocking(move || log.file.find(&needle))
                .await
                .map_err(|error| ProviderError::Local(format!("search stopped: {error}")))??;
            Ok(Arc::new(matches))
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cv_runtime::load::Load;
    use cv_widgets::find_bar::Step;

    use super::LogSearch;

    fn found(lines: &[usize], current: usize) -> LogSearch {
        LogSearch {
            matches: Load::Ready(Arc::new(lines.to_vec())),
            searched_for: "error".to_owned(),
            current,
            ..LogSearch::default()
        }
    }

    #[test]
    fn steps_wrap_around_the_matches() {
        let mut search = found(&[3, 40, 512], 2);
        assert_eq!(search.step(Some(Step::Next)), Some(3), "after the last comes the first");
        assert_eq!(search.step(Some(Step::Previous)), Some(512), "before the first, the last");
        assert_eq!(search.step(Some(Step::Previous)), Some(40));
        assert_eq!(search.status(), "2 / 3");
        assert_eq!(search.step(None), None, "no step, no scroll");
        assert_eq!(found(&[], 0).step(Some(Step::Next)), None, "nothing to move to");
    }

    #[test]
    fn says_how_the_search_stands() {
        assert_eq!(LogSearch::default().status(), "");
        assert_eq!(found(&[], 0).status(), "No matches");
        assert_eq!(found(&(0..1_200).collect::<Vec<_>>(), 0).status(), "1 / 1,200");
        let mut reset = found(&[3], 0);
        reset.reset();
        assert_eq!(reset.current_line(), None, "a new log forgets the old matches");
        let failed = LogSearch { matches: Load::Failed("disk".into()), ..LogSearch::default() };
        assert_eq!(failed.status(), "Search failed");
    }

    #[test]
    fn enter_retries_a_failed_search() {
        let mut search = found(&[3], 0);
        search.text = "error".to_owned();
        assert!(!search.should_search(), "Enter on the searched text steps through its matches");
        search.matches = Load::Failed("disk".into());
        assert!(search.should_search(), "Enter on the same text searches again after a failure");
        search.matches = Load::Idle;
        search.text = "warning".to_owned();
        assert!(search.should_search(), "new text searches");
    }
}
