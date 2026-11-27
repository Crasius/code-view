//! The body of a run, as on Azure DevOps' run page: a "Stages | Jobs" pivot; Stages shows the
//! stage graph across the full width, every card listing its jobs, Jobs a flat job list. Knows
//! nothing about sessions or services, so it renders from plain data (and in tests).

use std::collections::HashSet;

use cv_base::theme;
use cv_widgets::tabs;
use egui::Ui;
use review_core::{PipelineJob, ProviderKind, RerunAction};

use super::attempts::Known;
use super::graph::stage_card::{CardAction, CardContext};
use super::graph::stage_graph::{self, GraphInput};
use super::job_tree::{self, LogTarget};
use super::jobs_list;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pivot {
    #[default]
    Stages,
    Jobs,
}

const PIVOTS: [(Pivot, &str); 2] = [(Pivot::Stages, "Stages"), (Pivot::Jobs, "Jobs")];

pub enum RunViewAction {
    /// A job, step or stage was clicked: its log, or its state while it has none.
    Log(LogTarget),
    Rerun(RerunAction),
    AddToken,
    /// An earlier attempt picked from a badge: the retried stage or job, the attempt's number.
    Attempt {
        node_id: String,
        number: u32,
    },
}

#[derive(Clone, Copy)]
pub struct RunViewInput<'a> {
    pub run_id: &'a str,
    pub stages: &'a [PipelineJob],
    pub platform: ProviderKind,
    pub has_token: bool,
    /// A rerun is being started.
    pub is_busy: bool,
    /// Earlier attempts loaded so far.
    pub known: Known<'a>,
}

#[derive(Default)]
pub struct RunView {
    pivot: Pivot,
    expanded: HashSet<String>,
    /// The run whose initial expansion was applied.
    expanded_for: Option<String>,
    highlighted: Option<String>,
    scroll_to: Option<String>,
}

impl RunView {
    /// Back to the Stages pivot, for a run opened anew.
    pub fn show_stages(&mut self) {
        self.pivot = Pivot::Stages;
    }

    #[cfg(test)]
    pub fn expand(&mut self, stage_id: &str) {
        self.expanded.insert(stage_id.to_owned());
    }

    pub fn show(&mut self, ui: &mut Ui, input: RunViewInput<'_>) -> Option<RunViewAction> {
        if self.expanded_for.as_deref() != Some(input.run_id) {
            self.expanded = stage_graph::initially_expanded(input.stages);
            self.expanded_for = Some(input.run_id.to_owned());
            self.highlighted = None;
        }
        let palette = theme::palette(ui.ctx());
        let labels: Vec<String> = PIVOTS.iter().map(|(_, label)| (*label).to_owned()).collect();
        let selected = PIVOTS.iter().position(|(pivot, _)| *pivot == self.pivot).unwrap_or(0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if let Some(index) = tabs::pivot(ui, &labels, selected) {
                self.pivot = PIVOTS[index].0;
            }
        });
        let top = ui.cursor().top();
        ui.painter().hline(ui.max_rect().x_range(), top, egui::Stroke::new(1.0, palette.border));
        ui.add_space(1.0);
        match self.pivot {
            Pivot::Stages => self.stages(ui, input),
            Pivot::Jobs => jobs_list::show(
                ui,
                input.stages,
                input.platform,
                input.has_token,
                input.is_busy,
                input.known,
            ),
        }
    }

    fn stages(&mut self, ui: &mut Ui, input: RunViewInput<'_>) -> Option<RunViewAction> {
        let palette = theme::palette(ui.ctx());
        let stages = input.stages;
        let context = CardContext {
            platform: input.platform,
            has_token: input.has_token,
            is_busy: input.is_busy,
            stages,
            known: input.known,
        };
        let scroll_to = self.scroll_to.take();
        ui.painter().rect_filled(ui.available_rect_before_wrap(), 0, palette.ground);
        let graph_input = GraphInput {
            stages,
            expanded: &self.expanded,
            highlighted: self.highlighted.as_deref(),
            scroll_to: scroll_to.as_deref(),
            context,
        };
        let (index, card) = stage_graph::show(ui, graph_input)?;
        let stage = &stages[index];
        self.highlighted = Some(stage.id.clone());
        match card {
            CardAction::Toggle => {
                if !self.expanded.remove(&stage.id) {
                    self.expanded.insert(stage.id.clone());
                }
                None
            }
            CardAction::OpenStage => {
                Some(RunViewAction::Log(job_tree::click_target(stage, None, None)))
            }
            CardAction::Job(target) => Some(RunViewAction::Log(target)),
            CardAction::Rerun(rerun) => Some(RunViewAction::Rerun(rerun)),
            CardAction::AddToken => Some(RunViewAction::AddToken),
            CardAction::Attempt { node_id, number } => {
                Some(RunViewAction::Attempt { node_id, number })
            }
        }
    }
}
