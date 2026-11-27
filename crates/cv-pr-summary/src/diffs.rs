//! Which cards get their diff, and when: the diffs that arrived go into their cards, the cards on
//! screen and one screen around it are wanted, and their diffs (or a rendered card's versions)
//! start loading as the budget allows; loaded cards near the view get their syntax colors.

use std::sync::Arc;

use review_git::{ChangedFile, FileDiff};

use super::bodies::CardBody;
use super::card::{self, Body};
use super::rendered;
use super::{DiffSource, FolderSummary, MAX_BUILDS_PER_FRAME, SummaryInputs};
use cv_syntax::highlight::LineSpans;

/// Takes in the diffs that arrived: kept for the file view, and shown in their cards.
pub fn finish_loads(
    summary: &mut FolderSummary,
    inputs: &SummaryInputs<'_>,
    source: &mut dyn DiffSource,
) {
    for (path, file, diff) in summary.loads.finish() {
        let mut spans = None;
        if let Some(changed) = inputs.files.get(file).filter(|changed| changed.path == path) {
            source.store(changed, diff.clone());
            spans = source.spans(changed);
        }
        add_body(summary, inputs, path, diff, spans);
    }
}

fn add_body(
    summary: &mut FolderSummary,
    inputs: &SummaryInputs<'_>,
    path: String,
    diff: Arc<FileDiff>,
    spans: Option<Arc<LineSpans>>,
) {
    let mut body = CardBody::new(diff, summary.context(), summary.forced.contains(&path));
    body.spans = spans;
    let shapes = body.shapes();
    let shape = shapes.pick(summary.split);
    if summary.shapes.insert(path.clone(), shapes) != Some(shapes) {
        if let Some(slot) = summary
            .slots
            .iter_mut()
            .find(|slot| inputs.files.get(slot.file).is_some_and(|file| file.path == path))
        {
            slot.shape = shape;
        }
        summary.index_dirty = true;
    }
    let (slots, wanted, files) = (&summary.slots, summary.loads.wanted(), inputs.files);
    let keep = |candidate: &str| {
        wanted.iter().any(|slot| {
            slots
                .get(*slot)
                .and_then(|slot| files.get(slot.file))
                .is_some_and(|file| file.path == candidate)
        })
    };
    summary.bodies.insert(path, body, summary.frame, &keep);
}

/// The expanded cards on screen and one screen around it, nearest first (see [`Loads::plan`]).
pub fn plan_wanted(summary: &mut FolderSummary, inputs: &SummaryInputs<'_>) {
    let (slots, collapsed, files) = (&summary.slots, &summary.collapsed, inputs.files);
    let is_expanded = |card: usize| !collapsed.contains(&files[slots[card].file].path);
    summary.loads.plan(&summary.index, summary.viewport, is_expanded);
}

/// Starts the wanted loads the budget allows: from the cache at once, from the mirror a few at
/// a time, and nothing new while the view flies by or the window is in the background.
pub fn start_loads(
    summary: &mut FolderSummary,
    ctx: &egui::Context,
    inputs: &SummaryInputs<'_>,
    source: &mut dyn DiffSource,
    now: f64,
    is_active: bool,
) {
    if !inputs.can_load || !is_active || !summary.loads.is_settled(now) {
        return;
    }
    let mut builds = 0;
    for position in 0..summary.loads.wanted().len() {
        if !summary.loads.has_room() || builds >= MAX_BUILDS_PER_FRAME {
            break;
        }
        let slot = summary.slots[summary.loads.wanted()[position]];
        let file = &inputs.files[slot.file];
        if inputs.renders(file) {
            start_rendered(summary, inputs, file, source);
            continue;
        }
        if !needs_load(summary, file) {
            continue;
        }
        if let Some(diff) = source.cached(file) {
            builds += 1;
            let spans = source.spans(file);
            add_body(summary, inputs, file.path.clone(), diff, spans);
            continue;
        }
        let Some(load) = source.start(file) else {
            break;
        };
        summary.loads.push(file.path.clone(), slot.file, load);
    }
    if builds > 0 {
        ctx.request_repaint();
    }
}

/// Starts loading the versions of the rendered card of `file` when it has none yet and fewer
/// than [`rendered::MAX_RUNNING`] are on their way.
fn start_rendered(
    summary: &mut FolderSummary,
    inputs: &SummaryInputs<'_>,
    file: &ChangedFile,
    source: &mut dyn DiffSource,
) {
    if !summary.rendered.needs_load(&file.path)
        || summary.rendered.running() >= rendered::MAX_RUNNING
    {
        return;
    }
    let Some(load) = source.start_rendered(file) else {
        return;
    };
    let (slots, wanted, files) = (&summary.slots, summary.loads.wanted(), inputs.files);
    let keep =
        |candidate: &str| wanted.iter().any(|&card| files[slots[card].file].path == candidate);
    summary.rendered.start(file.path.clone(), load, summary.frame, &keep);
}

/// The card shows code that is not loaded, failed or loading yet.
pub fn needs_load(summary: &FolderSummary, file: &ChangedFile) -> bool {
    let shows_code = matches!(
        card::placeholder_body(file, summary.forced.contains(&file.path), None),
        Body::Pending
    );
    shows_code && !summary.bodies.contains(&file.path) && !summary.loads.is_tried(&file.path)
}

/// Queues the syntax colors of the loaded cards on screen and one screen around it that have
/// none yet, nearest first, so a card scrolled into view is colored already.
pub fn color_wanted(summary: &mut FolderSummary, ctx: &egui::Context, inputs: &SummaryInputs<'_>) {
    for &card in summary.loads.wanted() {
        let path = &inputs.files[summary.slots[card].file].path;
        if !summary.collapsed.contains(path)
            && let Some(body) = summary.bodies.get_mut(path)
        {
            summary.loads.color(ctx, path, body);
        }
    }
}
