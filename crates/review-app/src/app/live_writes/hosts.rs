//! What the live write walkthroughs set up outside the app, through the owner's logged-in CLIs
//! (`gh`, `glab`, `az`) and plain `git` with the owner's credential helper: a base and a topic
//! branch of the run's own, the test pull request between them, a commit pushed while the app has
//! it open, and the cleanup. Every call goes to one of the repositories the owner released for
//! these tests ([`Released`]); the CLIs keep their own sign-in, nothing here reads a token.
//!
//! Which repositories those are is the owner's to say and stays out of the tracked source: they
//! come from [`TARGETS_FILE`] in the owner's config folder, beside the connections file the
//! walkthroughs already read, one entry per platform in the shape of [`TARGETS_FORMAT`]. Only the
//! platform being run needs its entry; a missing file, entry or field fails the walkthrough with
//! the file's path and that shape. There is no default: a walkthrough never writes to a
//! repository nobody named.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use review_core::ProviderKind;
use review_store::AppPaths;
use serde_json::Value;

/// The file in the owner's config folder that names the released repositories.
pub const TARGETS_FILE: &str = "live-writes.json";
/// The shape of [`TARGETS_FILE`]. `repository` is the app's repository key; Azure DevOps takes the
/// organization, the project and the repository from it for `az`.
const TARGETS_FORMAT: &str = r#"{
  "azure":  { "connection_id": "ado-1", "repository": "<organization>/<project>/<repository>",
              "clone_url": "https://dev.azure.com/<organization>/<project>/_git/<repository>",
              "default_branch": "main" },
  "github": { "connection_id": "gh-1", "repository": "<owner>/<repository>",
              "clone_url": "https://github.com/<owner>/<repository>.git", "default_branch": "main" },
  "gitlab": { "connection_id": "gl-1", "repository": "<group>/<project>",
              "clone_url": "https://<gitlab host>/<group>/<project>.git", "default_branch": "main" }
}"#;

/// Every branch the walkthroughs create starts with this; cleanup deletes only those.
const BRANCH_PREFIX: &str = "codeview-live/";
/// The file the topic branch adds, which the walkthrough comments on.
pub const FILE: &str = "codeview-live.txt";
const FILE_TEXT: &str =
    "# Code View live write test\n\nFirst line to comment on.\nSecond line.\nThird line.\n";
const CI_WAIT: Duration = Duration::from_secs(600);

/// A repository the owner released for live writes, with the saved connection that reaches it,
/// as [`TARGETS_FILE`] names it.
pub struct Released {
    pub connection_id: String,
    pub kind: ProviderKind,
    /// The host of the clone URL, which the connection's base URL must share.
    pub host: String,
    pub repository_key: String,
    clone_url: String,
    default_branch: String,
    /// The pipeline file the topic branch adds, whose one job fails so that it can be rerun.
    ci_file: Option<(&'static str, &'static str)>,
}

impl Released {
    /// The released repository of `kind` from [`TARGETS_FILE`]; fails the walkthrough with the
    /// file's path and [`TARGETS_FORMAT`] when the file, the entry or a field is missing.
    pub fn load(kind: ProviderKind) -> Self {
        let file = AppPaths::from_environment().config_dir().join(TARGETS_FILE);
        let section = match kind {
            ProviderKind::AzureDevOps => "azure",
            ProviderKind::GitHub => "github",
            ProviderKind::GitLab => "gitlab",
        };
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|error| missing(&file, &format!("it is not readable: {error}")));
        let targets: Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| missing(&file, &format!("it is no JSON: {error}")));
        let field = |name: &str| -> String {
            match targets[section][name].as_str() {
                Some(value) if !value.is_empty() => value.to_owned(),
                _ => missing(&file, &format!("\"{section}\".\"{name}\" is missing")),
            }
        };
        let clone_url = field("clone_url");
        // Azure DevOps hands out clone URLs with the organization as user name.
        let host = clone_url.split("://").nth(1).and_then(|rest| rest.split('/').next());
        let host = host.and_then(|host| host.rsplit('@').next()).unwrap_or_default().to_owned();
        if host.is_empty() {
            missing(&file, &format!("\"{section}\".\"clone_url\" {clone_url} has no host"));
        }
        let repository_key = field("repository");
        if kind == ProviderKind::AzureDevOps && repository_key.split('/').count() != 3 {
            missing(
                &file,
                &format!("\"azure\".\"repository\" {repository_key} is not three parts"),
            );
        }
        Self {
            connection_id: field("connection_id"),
            kind,
            host,
            repository_key,
            clone_url,
            default_branch: field("default_branch"),
            ci_file: ci_file(kind),
        }
    }

    /// Refuses a saved connection that is not the released one: another kind or another host.
    pub fn assert_connection(&self, stored: &Value) {
        assert_eq!(stored["id"].as_str(), Some(self.connection_id.as_str()));
        let kind: ProviderKind =
            serde_json::from_value(stored["kind"].clone()).expect("the connection's kind");
        assert_eq!(
            kind,
            self.kind,
            "{} is not a {} connection",
            self.connection_id,
            self.kind.label()
        );
        let base_url = stored["base_url"].as_str().unwrap_or_default();
        let host = base_url.split("://").nth(1).unwrap_or_default().split('/').next();
        assert_eq!(host, Some(self.host.as_str()), "{} points at {base_url}", self.connection_id);
    }

    pub fn has_pipeline(&self) -> bool {
        self.ci_file.is_some()
    }

    /// The organization, the project and the repository of an Azure DevOps repository key;
    /// [`Released::load`] made sure it has all three.
    fn azure_parts(&self) -> [&str; 3] {
        let mut parts = self.repository_key.splitn(3, '/');
        [(); 3].map(|()| parts.next().unwrap_or_default())
    }

    /// `az` with the released Azure DevOps project; on Windows it is a batch file. Callers pick
    /// the fields they read with `--query`: the full answer carries display names that its Python
    /// writes in the console's code page, which is no JSON.
    fn az(&self, args: &[&str]) -> Result<Value, String> {
        let program = if cfg!(windows) { "az.cmd" } else { "az" };
        let [organization, project, _] = self.azure_parts();
        let organization = format!("https://{}/{organization}", self.host);
        let scope = ["--organization", &organization, "--project", project, "--output", "json"];
        // `az repos pr show/update` take no project.
        let scope = if args.get(2).is_some_and(|verb| *verb == "create") {
            &scope[..]
        } else {
            &[scope[0], scope[1], scope[4], scope[5]][..]
        };
        json(&run(Command::new(program).args(args).args(scope))?)
    }
}

/// The pipeline file the topic branch adds on `kind`, whose one job fails so that it can be rerun.
fn ci_file(kind: ProviderKind) -> Option<(&'static str, &'static str)> {
    match kind {
        // Azure Repos runs pipelines on pull requests only through a branch policy; the release
        // covers no policy change.
        ProviderKind::AzureDevOps => None,
        ProviderKind::GitHub => Some((
            ".github/workflows/codeview-live.yml",
            "name: Code View live test\non: pull_request\njobs:\n  fails:\n    runs-on: ubuntu-latest\n    steps:\n      - run: exit 1\n",
        )),
        ProviderKind::GitLab => Some((
            ".gitlab-ci.yml",
            "fails:\n  script:\n    - exit 1\n  rules:\n    - if: $CI_PIPELINE_SOURCE == \"merge_request_event\"\n",
        )),
    }
}

/// Fails the walkthrough because [`TARGETS_FILE`] at `file` cannot say what it needs, with the
/// shape it expects.
fn missing(file: &Path, what: &str) -> ! {
    panic!(
        "{}: {what}. Name the repositories released for live writes there, as\n{TARGETS_FORMAT}",
        file.display()
    )
}

/// Whether the platform still has the test pull request open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Open,
    Merged,
    Closed,
}

/// The run's pull request from its topic branch into its base branch, both new. Dropping it
/// closes the pull request if it is still open and deletes both branches.
pub struct TestPullRequest<'r> {
    released: &'r Released,
    checkout: PathBuf,
    base: String,
    topic: String,
    pub number: u64,
}

impl<'r> TestPullRequest<'r> {
    /// Branches off the default branch twice, adds [`FILE`] (and the failing pipeline) on the
    /// topic branch and opens the pull request.
    pub fn open(released: &'r Released, scratch: &Path) -> Self {
        let checkout = scratch.join("checkout");
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_secs();
        let base = format!("{BRANCH_PREFIX}{stamp}-base");
        let topic = format!("{BRANCH_PREFIX}{stamp}-topic");
        let parent = checkout.parent().expect("scratch").to_string_lossy().into_owned();
        must(run(Command::new("git").current_dir(&parent).args([
            "clone",
            "--depth",
            "1",
            "--single-branch",
            "--branch",
            released.default_branch.as_str(),
            released.clone_url.as_str(),
            "checkout",
        ])));
        let mut pull_request = Self { released, checkout, base, topic, number: 0 };
        pull_request.git(&["push", "origin", &format!("HEAD:refs/heads/{}", pull_request.base)]);
        pull_request.git(&["checkout", "-b", &pull_request.topic.clone()]);
        pull_request.write(FILE, FILE_TEXT);
        let mut added = vec![FILE];
        if let Some((path, text)) = released.ci_file {
            pull_request.write(path, text);
            added.push(path);
        }
        pull_request.git(&[&["add", "--"], added.as_slice()].concat());
        pull_request.git(&["commit", "-m", "Add the Code View live test file"]);
        pull_request.git(&["push", "origin", &pull_request.topic.clone()]);
        pull_request.number = pull_request.create();
        println!("opened {} #{}", released.repository_key, pull_request.number);
        pull_request
    }

    /// Pushes one more commit to the topic branch, as a colleague would while the review is open.
    pub fn push_commit(&self) {
        self.write(FILE, &format!("{FILE_TEXT}A line pushed after the review loaded.\n"));
        self.git(&["commit", "-am", "Push a commit after the review loaded"]);
        self.git(&["push", "origin", &self.topic]);
    }

    /// Waits until the first pipeline of the pull request failed, so the app offers its rerun.
    pub fn wait_for_failed_pipeline(&self) {
        let started = Instant::now();
        loop {
            let status = self.pipeline_status();
            println!("pipeline: {status:?} after {} s", started.elapsed().as_secs());
            if status.as_deref() == Some("failure") || status.as_deref() == Some("failed") {
                return;
            }
            assert!(started.elapsed() < CI_WAIT, "the pipeline did not fail in time: {status:?}");
            cv_testkit::let_threads_run(Duration::from_secs(10));
        }
    }

    pub fn state(&self) -> State {
        must(self.try_state())
    }

    fn try_state(&self) -> Result<State, String> {
        let (repo, number) = (self.released.repository_key.as_str(), self.number);
        Ok(match self.released.kind {
            ProviderKind::GitHub => {
                let pull = gh(&["api", &format!("repos/{repo}/pulls/{number}")])?;
                match (pull["state"].as_str(), pull["merged"].as_bool()) {
                    (Some("open"), _) => State::Open,
                    (_, Some(true)) => State::Merged,
                    _ => State::Closed,
                }
            }
            ProviderKind::GitLab => {
                match glab(&["api", &self.merge_request_path()])?["state"].as_str() {
                    Some("opened") => State::Open,
                    Some("merged") => State::Merged,
                    _ => State::Closed,
                }
            }
            ProviderKind::AzureDevOps => {
                let id = number.to_string();
                match self.released.az(&[
                    "repos",
                    "pr",
                    "show",
                    "--id",
                    &id,
                    "--query",
                    "{status:status}",
                ])?["status"]
                    .as_str()
                {
                    Some("active") => State::Open,
                    Some("completed") => State::Merged,
                    _ => State::Closed,
                }
            }
        })
    }

    /// Waits until the platform reports `state`; Azure DevOps completes a merge asynchronously.
    pub fn wait_for_state(&self, state: State) {
        let started = Instant::now();
        while self.state() != state {
            assert!(started.elapsed() < Duration::from_secs(120), "never became {state:?}");
            cv_testkit::let_threads_run(Duration::from_secs(3));
        }
    }

    fn create(&self) -> u64 {
        let released = self.released;
        let repo = released.repository_key.as_str();
        let title = format!("Code View live write test {}", self.topic);
        let body = "Opened by the Code View live write walkthrough; it merges or closes it.";
        let (head, base) = (self.topic.as_str(), self.base.as_str());
        let created = match released.kind {
            ProviderKind::GitHub => gh(&[
                "api",
                "-X",
                "POST",
                &format!("repos/{repo}/pulls"),
                "-f",
                &format!("title={title}"),
                "-f",
                &format!("head={head}"),
                "-f",
                &format!("base={base}"),
                "-f",
                &format!("body={body}"),
            ])
            .map(|created| created["number"].as_u64()),
            ProviderKind::GitLab => glab(&[
                "api",
                "-X",
                "POST",
                &format!("{}/merge_requests", self.project_path()),
                "-f",
                &format!("title={title}"),
                "-f",
                &format!("source_branch={head}"),
                "-f",
                &format!("target_branch={base}"),
                "-f",
                &format!("description={body}"),
            ])
            .map(|created| created["iid"].as_u64()),
            ProviderKind::AzureDevOps => released
                .az(&[
                    "repos",
                    "pr",
                    "create",
                    "--repository",
                    released.azure_parts()[2],
                    "--source-branch",
                    head,
                    "--target-branch",
                    base,
                    "--title",
                    &title,
                    "--description",
                    body,
                    "--query",
                    "{pullRequestId:pullRequestId}",
                ])
                .map(|created| created["pullRequestId"].as_u64()),
        };
        must(created).expect("the platform numbered the new pull request")
    }

    /// GitHub's check conclusion or GitLab's pipeline status of the newest run, once there is one.
    fn pipeline_status(&self) -> Option<String> {
        let runs = match self.released.kind {
            ProviderKind::GitHub => {
                let path = format!(
                    "repos/{}/actions/runs?event=pull_request&branch={}",
                    self.released.repository_key, self.topic
                );
                must(gh(&["api", &path]))["workflow_runs"][0]["conclusion"].clone()
            }
            ProviderKind::GitLab => {
                let path = format!("{}/pipelines", self.merge_request_path());
                must(glab(&["api", &path]))[0]["status"].clone()
            }
            ProviderKind::AzureDevOps => Value::Null,
        };
        runs.as_str().map(str::to_owned)
    }

    fn close(&self) -> Result<Value, String> {
        let (repo, number) = (self.released.repository_key.as_str(), self.number);
        match self.released.kind {
            ProviderKind::GitHub => gh(&[
                "api",
                "-X",
                "PATCH",
                &format!("repos/{repo}/pulls/{number}"),
                "-f",
                "state=closed",
            ]),
            ProviderKind::GitLab => {
                glab(&["api", "-X", "PUT", &self.merge_request_path(), "-f", "state_event=close"])
            }
            ProviderKind::AzureDevOps => self.released.az(&[
                "repos",
                "pr",
                "update",
                "--id",
                &number.to_string(),
                "--status",
                "abandoned",
                "--query",
                "{status:status}",
            ]),
        }
    }

    fn project_path(&self) -> String {
        format!("projects/{}", urlencoding::encode(&self.released.repository_key))
    }

    fn merge_request_path(&self) -> String {
        format!("{}/merge_requests/{}", self.project_path(), self.number)
    }

    fn write(&self, path: &str, text: &str) {
        let file = self.checkout.join(path);
        std::fs::create_dir_all(file.parent().expect("a folder")).expect("the folder");
        std::fs::write(file, text).expect("the file");
    }

    fn git(&self, args: &[&str]) -> String {
        must(self.try_git(args))
    }

    fn try_git(&self, args: &[&str]) -> Result<String, String> {
        let checkout = self.checkout.to_string_lossy().into_owned();
        let output = run(Command::new("git").args(["-C", &checkout]).args(args))?;
        Ok(String::from_utf8_lossy(&output).trim().to_owned())
    }

    /// The run's branches that are still on the platform.
    fn remaining_branches(&self) -> Result<Vec<String>, String> {
        let pattern = format!("{BRANCH_PREFIX}*");
        let listed = self.try_git(&["ls-remote", "--heads", "origin", &pattern])?;
        Ok(listed
            .lines()
            .filter_map(|line| line.split_whitespace().nth(1)?.strip_prefix("refs/heads/"))
            .filter(|branch| *branch == self.base || *branch == self.topic)
            .map(str::to_owned)
            .collect())
    }

    /// Closes the pull request if it is still open and deletes the run's branches.
    fn clean_up(&self) -> Result<(), String> {
        if self.number != 0 && self.try_state()? == State::Open {
            self.close()?;
            println!("closed {} #{}", self.released.repository_key, self.number);
        }
        for branch in self.remaining_branches()? {
            self.try_git(&["push", "origin", "--delete", &branch])?;
            println!("deleted the branch {branch}");
        }
        Ok(())
    }
}

impl Drop for TestPullRequest<'_> {
    /// Runs after a failed step too, while the test thread unwinds: a panic here would abort the
    /// process, so a failed cleanup is reported for the owner to finish.
    fn drop(&mut self) {
        if let Err(error) = self.clean_up() {
            eprintln!(
                "cleanup of {} #{} (branches {} and {}) failed: {error}",
                self.released.repository_key, self.number, self.base, self.topic
            );
        }
    }
}

fn gh(args: &[&str]) -> Result<Value, String> {
    json(&run(Command::new("gh").args(args))?)
}

fn glab(args: &[&str]) -> Result<Value, String> {
    json(&run(Command::new("glab").args(args))?)
}

fn json(output: &[u8]) -> Result<Value, String> {
    serde_json::from_slice(output).map_err(|error| format!("the CLI answered no JSON: {error}"))
}

/// The answer of a setup step; a failed one fails the walkthrough.
fn must<T>(answer: Result<T, String>) -> T {
    answer.unwrap_or_else(|error| panic!("{error}"))
}

/// Runs a CLI without prompts and returns its standard output, or its error output (CLIs print
/// no token there).
#[expect(
    clippy::disallowed_methods,
    reason = "the live walkthroughs set up the platform from the test thread, which owns the wait"
)]
fn run(command: &mut Command) -> Result<Vec<u8>, String> {
    command.env("GIT_TERMINAL_PROMPT", "0").env("GH_PROMPT_DISABLED", "1");
    let output = command.output().map_err(|error| format!("{command:?} did not start: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{:?} failed: {}",
            command.get_args().collect::<Vec<_>>(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(output.stdout)
}
