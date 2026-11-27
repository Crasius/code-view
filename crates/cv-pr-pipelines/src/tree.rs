//! Lookups in a run's tree of stages, jobs and steps (and tasks), by node id or log id.

use review_core::{PipelineJob, RunStatus};

use super::job_tree::{self, LogTarget};

/// The node with `id` anywhere in the tree.
pub fn find<'a>(nodes: &'a [PipelineJob], id: &str) -> Option<&'a PipelineJob> {
    nodes.iter().find_map(|node| if node.id == id { Some(node) } else { find(&node.children, id) })
}

/// The node whose own log is `log_id`: the job (or task) a log belongs to, also when a step
/// without a log of its own shows it.
pub fn log_owner<'a>(nodes: &'a [PipelineJob], log_id: &str) -> Option<&'a PipelineJob> {
    nodes.iter().find_map(|node| {
        if node.log_id.as_deref() == Some(log_id) {
            Some(node)
        } else {
            log_owner(&node.children, log_id)
        }
    })
}

/// A stage's jobs; a stage without jobs of its own is its own job.
pub fn jobs_of(stage: &PipelineJob) -> &[PipelineJob] {
    if stage.children.is_empty() { std::slice::from_ref(stage) } else { &stage.children }
}

/// The job (a stage's child, or a stage without children) that holds the node with `id`.
pub fn owning_job<'a>(stages: &'a [PipelineJob], id: &str) -> Option<&'a PipelineJob> {
    stages.iter().find_map(|stage| {
        jobs_of(stage).iter().find(|job| find(std::slice::from_ref(*job), id).is_some())
    })
}

/// Puts `replacement` where the node with `id` is; false when there is none.
pub fn replace_node(nodes: &mut [PipelineJob], id: &str, replacement: PipelineJob) -> bool {
    let Some(node) = find_mut(nodes, id) else {
        return false;
    };
    *node = replacement;
    true
}

fn find_mut<'a>(nodes: &'a mut [PipelineJob], id: &str) -> Option<&'a mut PipelineJob> {
    for node in nodes {
        if node.id == id {
            return Some(node);
        }
        if let Some(found) = find_mut(&mut node.children, id) {
            return Some(found);
        }
    }
    None
}

/// The nearest retried node on the way to `id`: the item itself, its job or its stage.
pub fn retried_ancestor<'a>(nodes: &'a [PipelineJob], id: &str) -> Option<&'a PipelineJob> {
    let node = nodes.iter().find(|node| find(std::slice::from_ref(*node), id).is_some())?;
    let retried = (!node.previous_attempts.is_empty()).then_some(node);
    if node.id == id {
        return retried;
    }
    retried_ancestor(&node.children, id).or(retried)
}

/// The log "Logs" opens: the run's first failed item with a log (in a stage that still runs
/// too), else the first log of a failed stage, else its first log.
pub fn first_log(stages: &[PipelineJob]) -> Option<LogTarget> {
    stages
        .iter()
        .find_map(job_tree::failed_target)
        .or_else(|| {
            stages
                .iter()
                .filter(|stage| stage.status == RunStatus::Failed)
                .find_map(job_tree::initial_target)
        })
        .or_else(|| stages.iter().find_map(job_tree::initial_target))
}

#[cfg(test)]
mod tests {
    use review_core::{AttemptSource, PipelineAttempt, PipelineJob, RunStatus};

    use super::{find, first_log, log_owner, owning_job, replace_node, retried_ancestor};

    fn node(id: &str, children: Vec<PipelineJob>) -> PipelineJob {
        PipelineJob { id: id.to_owned(), name: id.to_owned(), children, ..PipelineJob::default() }
    }

    fn with_log(mut node: PipelineJob, log_id: &str) -> PipelineJob {
        node.log_id = Some(log_id.to_owned());
        node
    }

    fn retried(mut node: PipelineJob) -> PipelineJob {
        node.previous_attempts =
            vec![PipelineAttempt { number: 1, source: AttemptSource::Lazy(node.id.clone()) }];
        node
    }

    /// Build → (Linux → (checkout, test), Windows); Deploy without jobs.
    fn run() -> Vec<PipelineJob> {
        let linux = with_log(
            node("linux", vec![node("checkout", Vec::new()), node("test", Vec::new())]),
            "log-linux",
        );
        vec![node("build", vec![linux, node("windows", Vec::new())]), node("deploy", Vec::new())]
    }

    #[test]
    fn finds_nodes_and_the_job_that_holds_them() {
        let stages = run();
        assert_eq!(find(&stages, "test").map(|node| node.name.as_str()), Some("test"));
        assert!(find(&stages, "gone").is_none());
        let job = |id| owning_job(&stages, id).map(|job| job.id.as_str());
        assert_eq!(job("test"), Some("linux"), "a step belongs to its job");
        assert_eq!(job("linux"), Some("linux"));
        assert_eq!(job("deploy"), Some("deploy"), "a stage without jobs is its own job");
        assert_eq!(job("build"), None, "a stage with jobs is no job");
        assert_eq!(
            log_owner(&stages, "log-linux").map(|node| node.id.as_str()),
            Some("linux"),
            "the steps show their job's log"
        );
    }

    #[test]
    fn replaces_a_node_anywhere_in_the_tree() {
        let mut stages = run();
        assert!(replace_node(&mut stages, "test", node("test-attempt-1", Vec::new())));
        assert!(find(&stages, "test").is_none());
        assert!(find(&stages, "test-attempt-1").is_some());
        assert!(!replace_node(&mut stages, "gone", node("x", Vec::new())));
    }

    #[test]
    fn the_nearest_retried_node_wins() {
        let mut stages = run();
        assert!(retried_ancestor(&stages, "test").is_none(), "nothing was retried");
        stages[0] = retried(stages[0].clone());
        let nearest =
            |stages: &[PipelineJob], id| retried_ancestor(stages, id).map(|node| node.id.clone());
        assert_eq!(nearest(&stages, "test").as_deref(), Some("build"), "the stage was retried");
        let job = retried(find(&stages, "linux").cloned().expect("linux is in the run"));
        replace_node(&mut stages, "linux", job);
        assert_eq!(nearest(&stages, "test").as_deref(), Some("linux"), "the job is nearer");
        assert_eq!(nearest(&stages, "windows").as_deref(), Some("build"));
        assert_eq!(nearest(&stages, "deploy"), None);
    }

    #[test]
    fn logs_open_at_the_first_failure() {
        let mut stages = run();
        stages.push(with_log(node("lint", Vec::new()), "log-lint"));
        assert_eq!(first_log(&stages).map(|target| target.job_id), Some("linux".to_owned()));
        stages[2].status = RunStatus::Failed;
        assert_eq!(first_log(&stages).map(|target| target.job_id), Some("lint".to_owned()));
        assert_eq!(first_log(&[node("queued", Vec::new())]), None, "no log anywhere");
    }

    #[test]
    fn logs_open_at_a_failed_job_of_a_stage_that_still_runs() {
        let mut failed = with_log(node("unit", Vec::new()), "log-unit");
        failed.status = RunStatus::Failed;
        let mut test = node("test", vec![node("lint", Vec::new()), failed]);
        test.status = RunStatus::Running;
        let mut build = with_log(node("build", Vec::new()), "log-build");
        build.status = RunStatus::Succeeded;
        let stages = [build, test];
        assert_eq!(first_log(&stages).map(|target| target.job_id), Some("unit".to_owned()));
    }
}
