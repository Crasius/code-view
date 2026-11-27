//! The `needs` graph of a workflow file, and which workflow job an Actions job belongs to.
//!
//! The jobs API reports no dependencies; the workflow file does. Jobs are matched to their
//! workflow job by name: GitHub names a job after its `name:` (expressions evaluated) or its key,
//! appends ` (matrix values)` for matrix jobs without expressions in the name, and
//! ` / inner job` for jobs of a called reusable workflow.

use yaml_rust2::{Yaml, YamlLoader};

const EXPRESSION_START: &str = "${{";
const EXPRESSION_END: &str = "}}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkflowJob {
    pub key: String,
    /// `name:` as written, or the key.
    pub name_template: String,
    /// Keys of the jobs this one needs.
    pub needs: Vec<String>,
}

impl WorkflowJob {
    /// The name GitHub shows for the whole job (all matrix legs), as close as it can be known.
    pub(crate) fn display_name(&self) -> &str {
        if self.name_template.contains(EXPRESSION_START) { &self.key } else { &self.name_template }
    }

    /// How well `job_name` (from the jobs API) fits this workflow job; `None` when it doesn't.
    fn match_score(&self, job_name: &str) -> Option<usize> {
        let outer_name = job_name.split(" / ").next().unwrap_or(job_name);
        [job_name, outer_name]
            .into_iter()
            .filter_map(|candidate| template_score(&self.name_template, candidate))
            .max()
    }
}

/// Parsed `jobs:` of a workflow file, in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkflowGraph {
    pub jobs: Vec<WorkflowJob>,
}

impl WorkflowGraph {
    /// `None` when the file is not YAML or has no `jobs:` mapping.
    pub(crate) fn parse(source: &str) -> Option<Self> {
        let documents = YamlLoader::load_from_str(source).ok()?;
        let jobs = documents.first()?["jobs"].as_hash()?;
        let jobs = jobs
            .iter()
            .filter_map(|(key, job)| {
                let key = key.as_str()?.to_owned();
                let name_template = job["name"].as_str().map_or_else(|| key.clone(), str::to_owned);
                let needs = match &job["needs"] {
                    Yaml::String(needed) => vec![needed.clone()],
                    Yaml::Array(needed) => {
                        needed.iter().filter_map(Yaml::as_str).map(str::to_owned).collect()
                    }
                    _ => Vec::new(),
                };
                Some(WorkflowJob { key, name_template, needs })
            })
            .collect();
        Some(Self { jobs })
    }

    /// The workflow job an Actions job named `job_name` ran for: the best name match.
    pub(crate) fn job_for(&self, job_name: &str) -> Option<&WorkflowJob> {
        self.jobs
            .iter()
            .filter_map(|job| Some((job.match_score(job_name)?, job)))
            .max_by_key(|(score, _)| *score)
            .map(|(_, job)| job)
    }
}

/// Exact names beat matrix and reusable-workflow suffixes, which beat names built from
/// expressions; among those, more matched literal text wins.
fn template_score(template: &str, name: &str) -> Option<usize> {
    if !template.contains(EXPRESSION_START) {
        if name == template {
            return Some(3_000_000 + template.len());
        }
        let has_suffix = name
            .strip_prefix(template)
            .is_some_and(|rest| rest.starts_with(" (") || rest.starts_with(" / "));
        return has_suffix.then_some(2_000_000 + template.len());
    }
    let pieces = literal_pieces(template);
    let first = pieces.first()?;
    let last = pieces.last()?;
    let mut rest = name.strip_prefix(first.as_str())?.strip_suffix(last.as_str())?;
    // The pieces between the first and the last, in order.
    for piece in pieces.iter().take(pieces.len() - 1).skip(1) {
        (_, rest) = rest.split_once(piece.as_str())?;
    }
    Some(1_000_000 + pieces.iter().map(String::len).sum::<usize>())
}

/// The literal text around `${{ … }}` expressions; always starts and ends with a (maybe empty) piece.
fn literal_pieces(template: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut rest = template;
    while let Some((piece, expression)) = rest.split_once(EXPRESSION_START) {
        pieces.push(piece.to_owned());
        rest = expression.split_once(EXPRESSION_END).map_or("", |(_, after)| after);
    }
    pieces.push(rest.to_owned());
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKFLOW: &str = r"
name: CI
on: [pull_request]
jobs:
  lint:
    runs-on: ubuntu-latest
  build:
    name: Build
    needs: lint
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest]
  test:
    name: Test on ${{ matrix.os }}
    needs: [build, lint]
  release:
    uses: ./.github/workflows/release.yml
    needs: test
";

    #[test]
    fn parses_jobs_and_needs_in_file_order() {
        let graph = WorkflowGraph::parse(WORKFLOW).expect("workflow");
        let summary: Vec<(&str, &str, Vec<&str>)> = graph
            .jobs
            .iter()
            .map(|job| {
                (
                    job.key.as_str(),
                    job.display_name(),
                    job.needs.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("lint", "lint", vec![]),
                ("build", "Build", vec!["lint"]),
                ("test", "test", vec!["build", "lint"]),
                ("release", "release", vec!["test"]),
            ]
        );
        assert_eq!(WorkflowGraph::parse("not: [valid"), None);
        assert_eq!(WorkflowGraph::parse("name: no jobs"), None);
    }

    #[test]
    fn matches_job_names_from_the_jobs_api() {
        let graph = WorkflowGraph::parse(WORKFLOW).expect("workflow");
        let key = |name: &str| graph.job_for(name).map(|job| job.key.as_str());
        assert_eq!(key("lint"), Some("lint"));
        assert_eq!(key("Build (ubuntu-latest)"), Some("build"));
        assert_eq!(key("Test on windows-latest"), Some("test"));
        assert_eq!(key("release / publish"), Some("release"));
        assert_eq!(key("deploy"), None);
    }

    #[test]
    fn names_with_several_expressions_match_their_literal_text_in_order() {
        let template = "Build ${{ matrix.os }} on node ${{ matrix.node }}!";
        assert_eq!(literal_pieces(template), ["Build ", " on node ", "!"]);
        let score = |name: &str| template_score(template, name);
        assert!(score("Build ubuntu on node 20!").is_some());
        assert!(score("Build windows on node 22!").is_some());
        assert_eq!(score("Build ubuntu node 20!"), None, "a middle piece is missing");
        assert_eq!(score("Build ubuntu on node 20"), None, "the last piece is missing");
        assert_eq!(score("Build!"), None, "the first piece is missing");
        assert_eq!(
            template_score("ab${{ x }}ba", "aba"),
            None,
            "the name starts with the first piece and ends with the last, but they overlap"
        );
        assert_eq!(literal_pieces("${{ unclosed"), ["", ""]);
    }
}
