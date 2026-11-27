//! Links inside one pull request: what its descriptions, comments and commits point at, how the
//! Markdown renderer shows those links, and where a click on one goes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cv_base::theme::icons;
use cv_markdown::{self as markdown, InApp, LinkResolver, ResolvedLink};
use cv_runtime::links::link_target::{self, Here, Target};
use cv_runtime::links::pull_request_link;
use cv_runtime::links::references::ticket_keys;
use cv_runtime::route::{Navigator, PullRequestKey, Route};
use cv_services::Services;
use cv_services::linked::Status;
use review_core::{Connection, ItemKind, ItemRef, LinkedItem, ProviderKind};

/// One row of the overview's Linked card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkedEntry {
    /// A pull request, issue or work item on a saved connection.
    Item(Target),
    /// A ticket in an external tracker (Jira-style key), opened in the browser.
    Ticket { key: String, url: String },
}

/// The texts of a pull request that may link to other items.
pub struct Sources<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub source_branch: &'a str,
    pub comments: Vec<&'a str>,
    pub commit_subjects: Vec<&'a str>,
    /// Items the platform links itself (Azure DevOps work items).
    pub native: &'a [ItemRef],
}

/// Everything the sources point at, in reading order (title, description, commits, comments,
/// then the platform's own links), each item once, the pull request itself left out.
pub fn collect(
    sources: &Sources<'_>,
    connections: &[Connection],
    here: &Here,
    number: u64,
) -> Vec<LinkedEntry> {
    let platform = here.kind;
    let mut targets: Vec<String> = markdown::plain_link_targets(sources.title, platform);
    targets.extend(markdown::link_targets(sources.description, Some(platform)));
    for subject in &sources.commit_subjects {
        targets.extend(markdown::plain_link_targets(subject, platform));
    }
    for comment in &sources.comments {
        targets.extend(markdown::link_targets(comment, Some(platform)));
    }

    let mut entries: Vec<LinkedEntry> = Vec::new();
    let mut positions: HashMap<ItemIdentity, usize> = HashMap::new();
    let mut tracker: Option<String> = None;
    let mut add_target = |target: Target, entries: &mut Vec<LinkedEntry>| {
        if is_self(&target, here, number) {
            return;
        }
        let key = identity(&target, connections);
        if let Some(&index) = positions.get(&key) {
            // A URL names kind and repository; it replaces a `#12` or `!12` that only guessed them.
            let is_better = |existing: &Target| {
                let names_kind = existing.item.kind == ItemKind::IssueOrPullRequest
                    && target.item.kind != ItemKind::IssueOrPullRequest;
                names_kind || (!existing.is_exact && target.is_exact)
            };
            if let LinkedEntry::Item(existing) = &entries[index]
                && is_better(existing)
            {
                entries[index] = LinkedEntry::Item(target);
            }
        } else {
            positions.insert(key, entries.len());
            entries.push(LinkedEntry::Item(target));
        }
    };
    for text in &targets {
        if let Some((key, prefix)) = link_target::ticket(text) {
            tracker.get_or_insert(prefix);
            push_ticket(&mut entries, key, text.clone());
            continue;
        }
        if let Some(target) = link_target::classify(text, connections, Some(here)) {
            add_target(target, &mut entries);
        }
    }
    for item in sources.native {
        let target = Target {
            connection_id: here.connection_id.clone(),
            item: item.clone(),
            is_exact: true,
        };
        add_target(target, &mut entries);
    }
    // Bare keys (`PAY-142` in a title or branch name) only link once a ticket URL named their tracker.
    if let Some(prefix) = tracker {
        let mut texts = vec![sources.title, sources.source_branch, sources.description];
        texts.extend(sources.commit_subjects.iter().copied());
        for text in texts {
            for key in ticket_keys(text) {
                push_ticket(&mut entries, key.to_owned(), format!("{prefix}{key}"));
            }
        }
    }
    entries
}

fn push_ticket(entries: &mut Vec<LinkedEntry>, key: String, url: String) {
    let is_known = entries
        .iter()
        .any(|entry| matches!(entry, LinkedEntry::Ticket { key: known, .. } if *known == key));
    if !is_known {
        entries.push(LinkedEntry::Ticket { key, url });
    }
}

fn is_self(target: &Target, here: &Here, number: u64) -> bool {
    let is_numbered_like_pull_requests =
        matches!(target.item.kind, ItemKind::PullRequest | ItemKind::IssueOrPullRequest);
    let is_same_repository = target.item.repository_key == here.repository_key || !target.is_exact;
    target.connection_id == here.connection_id
        && is_numbered_like_pull_requests
        && is_same_repository
        && target.item.number == number
}

/// What two targets naming the same item share.
#[derive(Hash, PartialEq, Eq)]
struct ItemIdentity {
    connection_id: String,
    /// The repository, or the organization where numbers are unique across it.
    scope: String,
    number: u64,
    sequence: Sequence,
}

/// The numbering an item's number belongs to.
#[derive(Hash, PartialEq, Eq)]
enum Sequence {
    /// Numbered with the pull requests.
    PullRequests,
    /// Numbered apart from them.
    Separate,
}

/// GitHub numbers issues and pull requests in one sequence, GitLab and Azure DevOps keep separate
/// ones, and Azure DevOps pull request ids are unique per organization.
fn identity(target: &Target, connections: &[Connection]) -> ItemIdentity {
    let kind = connections
        .iter()
        .find(|connection| connection.id == target.connection_id)
        .map_or(ProviderKind::GitHub, |connection| connection.kind);
    let item = &target.item;
    let (scope, sequence) = match (kind, item.kind) {
        (ProviderKind::AzureDevOps, ItemKind::PullRequest) => {
            (organization(&item.repository_key), Sequence::PullRequests)
        }
        (ProviderKind::AzureDevOps, _) => (organization(&item.repository_key), Sequence::Separate),
        (ProviderKind::GitLab, ItemKind::Issue) => {
            (item.repository_key.clone(), Sequence::Separate)
        }
        _ => (item.repository_key.clone(), Sequence::PullRequests),
    };
    ItemIdentity {
        connection_id: target.connection_id.clone(),
        scope,
        number: item.number,
        sequence,
    }
}

fn organization(key: &str) -> String {
    key.split('/').next().unwrap_or_default().to_owned()
}

/// Where a click on a linked item goes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Decision {
    Navigate(PullRequestKey),
    Browser(String),
    /// Only the platform knows whether it is a pull request (and in which repository): ask first.
    Ask,
}

/// The click decision for `target` given what the cache knows. `fallback_url` is the page built
/// from the reference when the platform gave none.
fn decide(target: &Target, status: Status<'_>, fallback_url: &str) -> Decision {
    if let Status::Found(item) = status {
        return if item.kind == ItemKind::PullRequest {
            Decision::Navigate(PullRequestKey {
                connection_id: target.connection_id.clone(),
                repository_key: item.repository_key.clone(),
                number: item.reference.number,
            })
        } else {
            Decision::Browser(item.web_url.clone())
        };
    }
    if let Some(key) = target.pull_request_key() {
        return Decision::Navigate(key);
    }
    match (status, target.item.kind) {
        (
            Status::Unknown | Status::Loading,
            ItemKind::PullRequest | ItemKind::IssueOrPullRequest,
        ) => Decision::Ask,
        _ => Decision::Browser(fallback_url.to_owned()),
    }
}

/// The link context one pull request page publishes for its Markdown, plus the click that waits
/// for the platform's answer.
#[derive(Default)]
pub struct PageLinks {
    resolver: Option<(u64, usize, Arc<PullRequestLinks>)>,
    pending: Option<Target>,
}

impl PageLinks {
    /// Makes this page's links resolvable by every Markdown text drawn this frame.
    pub fn publish(&mut self, ctx: &egui::Context, services: &Services, here: &Here) {
        let version = services.linked.version();
        let connection_count = services.connections.all().len();
        let is_stale = self.resolver.as_ref().is_none_or(|(built, count, links)| {
            *built != version || *count != connection_count || links.here != *here
        });
        if is_stale {
            let known = services
                .linked
                .found()
                .map(|(connection_id, item)| {
                    ((connection_id.to_owned(), item.reference.clone()), item.clone())
                })
                .collect();
            let links = PullRequestLinks {
                connections: services.connections.all().to_vec(),
                here: here.clone(),
                known,
            };
            self.resolver = Some((version, connection_count, Arc::new(links)));
        }
        if let Some((_, _, links)) = &self.resolver {
            markdown::publish_links(ctx, links.clone());
        }
    }

    /// Opens a link target from rendered Markdown in the app, asking the platform first when the
    /// text alone does not say whether it is a pull request.
    pub fn open_text(
        &mut self,
        text: &str,
        ctx: &egui::Context,
        services: &mut Services,
        navigator: &mut Navigator,
        here: &Here,
    ) {
        match link_target::classify(text, services.connections.all(), Some(here)) {
            Some(target) => self.open(target, false, ctx, services, navigator),
            None => ctx.open_url(egui::OpenUrl::new_tab(text)),
        }
    }

    /// Opens `target` in the app when it is a pull request, else (or with `browser`) on the web.
    pub fn open(
        &mut self,
        target: Target,
        browser: bool,
        ctx: &egui::Context,
        services: &mut Services,
        navigator: &mut Navigator,
    ) {
        let fallback = fallback_url(&target, services.connections.all());
        let status = services.linked.status(&target.connection_id, &target.item);
        if browser {
            let url = match status {
                Status::Found(item) => item.web_url.clone(),
                _ => fallback,
            };
            ctx.open_url(egui::OpenUrl::new_tab(url));
            return;
        }
        match decide(&target, status, &fallback) {
            Decision::Navigate(key) => navigator.go(Route::PullRequest(key)),
            Decision::Browser(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            Decision::Ask => {
                if let Some(provider) = services.provider(&target.connection_id) {
                    let now = ctx.input(|input| input.time);
                    let item = target.item.clone();
                    services.linked.request(
                        &services.background,
                        provider,
                        &target.connection_id,
                        [item],
                        now,
                        true,
                    );
                }
                self.pending = Some(target);
            }
        }
    }

    /// Finishes a click that waited for the platform: the pull request opens in the app, anything
    /// else (or an unanswered question) in the browser.
    pub fn poll_pending(
        &mut self,
        ctx: &egui::Context,
        services: &Services,
        navigator: &mut Navigator,
    ) {
        let Some(target) = &self.pending else {
            return;
        };
        let fallback = fallback_url(target, services.connections.all());
        let status = services.linked.status(&target.connection_id, &target.item);
        let decision = match status {
            Status::Missing | Status::Failed(_) => Decision::Browser(fallback),
            other => decide(target, other, &fallback),
        };
        match decision {
            Decision::Ask => {}
            Decision::Navigate(key) => {
                self.pending = None;
                navigator.go(Route::PullRequest(key));
            }
            Decision::Browser(url) => {
                self.pending = None;
                ctx.open_url(egui::OpenUrl::new_tab(url));
            }
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// The web page of `target` built from the reference alone.
pub fn fallback_url(target: &Target, connections: &[Connection]) -> String {
    connections
        .iter()
        .find(|connection| connection.id == target.connection_id)
        .map(|connection| pull_request_link::item_web_url(connection, &target.item))
        .unwrap_or_default()
}

/// Resolves the links of one pull request's texts for the Markdown renderer.
struct PullRequestLinks {
    connections: Vec<Connection>,
    here: Here,
    known: HashMap<(String, ItemRef), LinkedItem>,
}

impl LinkResolver for PullRequestLinks {
    fn resolve(&self, text: &str) -> Option<ResolvedLink> {
        let Some(target) = link_target::classify(text, &self.connections, Some(&self.here)) else {
            let is_url = text.starts_with("https://") || text.starts_with("http://");
            return is_url.then(|| ResolvedLink {
                web_url: text.to_owned(),
                in_app: InApp::No,
                icon: None,
                title: None,
            });
        };
        let known = self.known.get(&(target.connection_id.clone(), target.item.clone()));
        let web_url = known
            .map_or_else(|| fallback_url(&target, &self.connections), |item| item.web_url.clone());
        let in_app = match (known, target.item.kind) {
            (Some(item), _) if item.kind == ItemKind::PullRequest => InApp::Yes,
            (None, ItemKind::PullRequest) => InApp::Yes,
            (None, ItemKind::IssueOrPullRequest) => InApp::Maybe,
            (Some(_), _) | (None, ItemKind::Issue | ItemKind::WorkItem) => InApp::No,
        };
        // Only what the text itself says decides the glyph, so it never pops in later.
        let icon = (target.item.kind == ItemKind::PullRequest).then_some(icons::GIT_PULL_REQUEST);
        let title = known.map(|item| format!("{} {}", target.label(Some(&self.here)), item.title));
        Some(ResolvedLink { web_url, in_app, icon, title })
    }
}

/// The connections whose items `entries` name, each with its items, for one batch per connection.
pub fn by_connection(entries: &[LinkedEntry]) -> Vec<(String, Vec<ItemRef>)> {
    let mut seen = HashSet::new();
    let mut groups: Vec<(String, Vec<ItemRef>)> = Vec::new();
    for entry in entries {
        let LinkedEntry::Item(target) = entry else {
            continue;
        };
        if !seen.insert((target.connection_id.clone(), target.item.clone())) {
            continue;
        }
        match groups.iter_mut().find(|(connection_id, _)| *connection_id == target.connection_id) {
            Some((_, items)) => items.push(target.item.clone()),
            None => groups.push((target.connection_id.clone(), vec![target.item.clone()])),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use cv_markdown::{InApp, LinkResolver};
    use cv_runtime::links::link_target::{Here, Target, classify};
    use cv_services::linked::Status;
    use cv_testkit::samples;
    use review_core::{Connection, ItemKind, ItemRef, LinkedItem, LinkedState, ProviderKind};

    use super::{Decision, LinkedEntry, PullRequestLinks, Sources, collect, decide};

    fn connections() -> Vec<Connection> {
        vec![
            samples::connection("gh", ProviderKind::GitHub, "https://github.com"),
            samples::connection("gl", ProviderKind::GitLab, "https://gitlab.example.com"),
            samples::connection("ado", ProviderKind::AzureDevOps, "https://dev.azure.com"),
        ]
    }

    fn github_here() -> Here {
        Here {
            connection_id: "gh".into(),
            kind: ProviderKind::GitHub,
            repository_key: "acme/app".into(),
        }
    }

    fn labels(entries: &[LinkedEntry], here: &Here) -> Vec<String> {
        entries
            .iter()
            .map(|entry| match entry {
                LinkedEntry::Item(target) => target.label(Some(here)),
                LinkedEntry::Ticket { key, .. } => key.clone(),
            })
            .collect()
    }

    #[test]
    fn collects_each_item_once_without_the_pull_request_itself() {
        let here = github_here();
        let sources = Sources {
            title: "PAY-7: Fix login (#12)",
            description: "Closes #3, see https://github.com/acme/app/pull/12 and https://github.com/other/lib/pull/9.\n\nTicket https://acme.atlassian.net/browse/PAY-142\n\n`#99` is code.",
            source_branch: "feature/PAY-7-login",
            comments: vec![
                "Also #3 and https://github.com/acme/app/pull/4",
                "https://example.com/not-linked",
            ],
            commit_subjects: vec!["Fix #5", "Merge !6"],
            native: &[],
        };
        let entries = collect(&sources, &connections(), &here, 12);
        assert_eq!(labels(&entries, &here), ["#3", "other/lib#9", "PAY-142", "#5", "#4", "PAY-7"]);
        let LinkedEntry::Item(four) = &entries[4] else { panic!("{entries:?}") };
        assert_eq!(four.item.kind, ItemKind::PullRequest, "the URL names the kind");
    }

    #[test]
    fn azure_work_items_and_pull_requests_by_id() {
        let here = Here {
            connection_id: "ado".into(),
            kind: ProviderKind::AzureDevOps,
            repository_key: "org/Proj/web".into(),
        };
        let native =
            [ItemRef { kind: ItemKind::WorkItem, repository_key: "org/Proj".into(), number: 31 }];
        let sources = Sources {
            title: "Fix #31",
            description: "Depends on !40 and https://dev.azure.com/org/Proj/_git/api/pullrequest/40",
            source_branch: "fix",
            comments: Vec::new(),
            commit_subjects: Vec::new(),
            native: &native,
        };
        let entries = collect(&sources, &connections(), &here, 39);
        assert_eq!(labels(&entries, &here), ["#31", "org/Proj/api!40"]);
        let LinkedEntry::Item(pull) = &entries[1] else { panic!() };
        assert!(pull.is_exact, "the URL with its repository replaced the guess");
    }

    fn found(kind: ItemKind, repository: &str, number: u64, reference: ItemRef) -> LinkedItem {
        LinkedItem {
            reference,
            kind,
            repository_key: repository.to_owned(),
            title: "Title".to_owned(),
            state: LinkedState::Open,
            state_label: "Active".to_owned(),
            checks: None,
            item_type: None,
            web_url: format!("https://github.com/{repository}/pull/{number}"),
        }
    }

    #[test]
    fn decisions_ask_only_when_the_text_cannot_tell() {
        let here = github_here();
        let reference = classify("#59", &connections(), Some(&here)).expect("reference");
        assert_eq!(decide(&reference, Status::Unknown, "u"), Decision::Ask);
        let item = found(ItemKind::PullRequest, "acme/app", 59, reference.item.clone());
        let Decision::Navigate(key) = decide(&reference, Status::Found(&item), "u") else {
            panic!()
        };
        assert_eq!((key.repository_key.as_str(), key.number), ("acme/app", 59));
        let issue = LinkedItem {
            kind: ItemKind::Issue,
            web_url: "https://github.com/acme/app/issues/59".into(),
            ..item
        };
        assert_eq!(
            decide(&reference, Status::Found(&issue), "u"),
            Decision::Browser(issue.web_url.clone())
        );
        assert_eq!(decide(&reference, Status::Missing, "u"), Decision::Browser("u".into()));

        let url = classify("https://github.com/acme/app/pull/3", &connections(), Some(&here))
            .expect("url");
        assert!(
            matches!(decide(&url, Status::Unknown, "u"), Decision::Navigate(_)),
            "a pull request URL opens at once"
        );
        let work_item = Target {
            connection_id: "ado".into(),
            item: ItemRef {
                kind: ItemKind::WorkItem,
                repository_key: "org/Proj".into(),
                number: 1,
            },
            is_exact: true,
        };
        assert_eq!(decide(&work_item, Status::Unknown, "w"), Decision::Browser("w".into()));
    }

    #[test]
    fn resolver_marks_in_app_links_and_keeps_plain_text_plain() {
        let here = github_here();
        let links =
            PullRequestLinks { connections: connections(), here, known: HashMap::default() };
        let pull = links.resolve("https://github.com/acme/app/pull/3").expect("pull request");
        assert_eq!((pull.in_app, pull.icon.is_some()), (InApp::Yes, true));
        let number = links.resolve("#7").expect("reference");
        assert_eq!(
            (number.in_app, number.icon, number.web_url.as_str()),
            (InApp::Maybe, None, "https://github.com/acme/app/issues/7")
        );
        let external = links.resolve("https://example.com").expect("url");
        assert_eq!(external.in_app, InApp::No);
        assert!(links.resolve("!7").is_none(), "GitHub has no ! references");
    }
}
