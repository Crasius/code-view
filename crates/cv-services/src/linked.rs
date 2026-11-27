//! Title and state of linked pull requests, issues and work items, shared by every page.
//!
//! Nothing here runs on its own: a screen asks for the items it shows ([`LinkedCache::request`]),
//! one batched request per connection goes out, and the answers stay for [`FRESH_FOR`]. Asking
//! again within that time costs nothing; after it, the next ask refetches. Failures stay until
//! the user retries, so a broken host is not hammered.

use std::collections::HashMap;
use std::sync::Arc;

use review_core::{ItemRef, LinkedItem, Provider, ProviderError};

use cv_runtime::background::{Background, TASK_LOST, Task, TaskPoll};
use cv_runtime::load::Load;

/// How long an answer counts as current.
pub(crate) const FRESH_FOR: f64 = 10.0 * 60.0;

/// What the cache knows about one item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status<'a> {
    /// Never asked.
    Unknown,
    Loading,
    Found(&'a LinkedItem),
    /// The platform does not know it, or this connection may not see it.
    Missing,
    Failed(&'a str),
}

enum Entry {
    Loading,
    Found(LinkedItem, f64),
    Missing(f64),
    Failed(String),
}

struct Batch {
    connection_id: String,
    references: Vec<ItemRef>,
    task: Task<Result<Vec<LinkedItem>, ProviderError>>,
}

#[derive(Default)]
pub struct LinkedCache {
    entries: HashMap<(String, ItemRef), Entry>,
    batches: Vec<Batch>,
    /// Items the platform links to a pull request itself (Azure DevOps work items), per pull request.
    native: HashMap<String, Load<Vec<ItemRef>>>,
    /// Bumped whenever an answer arrived, so views can rebuild what they derive from it.
    version: u64,
}

impl LinkedCache {
    pub fn status(&self, connection_id: &str, item: &ItemRef) -> Status<'_> {
        match self.entries.get(&(connection_id.to_owned(), item.clone())) {
            None => Status::Unknown,
            Some(Entry::Loading) => Status::Loading,
            Some(Entry::Found(item, _)) => Status::Found(item),
            Some(Entry::Missing(_)) => Status::Missing,
            Some(Entry::Failed(message)) => Status::Failed(message),
        }
    }

    /// Every item found so far, for link tooltips.
    pub fn found(&self) -> impl Iterator<Item = (&str, &LinkedItem)> {
        self.entries.iter().filter_map(|((connection_id, _), entry)| match entry {
            Entry::Found(item, _) => Some((connection_id.as_str(), item)),
            _ => None,
        })
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Starts one request for the `references` of one connection that are unknown or older than
    /// [`FRESH_FOR`]. With `retry`, failed ones go out again too. Returns whether a request started.
    pub fn request(
        &mut self,
        background: &Background,
        provider: Arc<dyn Provider>,
        connection_id: &str,
        references: impl IntoIterator<Item = ItemRef>,
        now: f64,
        retry: bool,
    ) -> bool {
        let mut wanted: Vec<ItemRef> = references
            .into_iter()
            .filter(|item| match self.entries.get(&(connection_id.to_owned(), item.clone())) {
                None => true,
                Some(Entry::Loading) => false,
                Some(Entry::Found(_, at) | Entry::Missing(at)) => now - at > FRESH_FOR,
                Some(Entry::Failed(_)) => retry,
            })
            .collect();
        wanted.sort();
        wanted.dedup();
        if wanted.is_empty() {
            return false;
        }
        for item in &wanted {
            self.entries.insert((connection_id.to_owned(), item.clone()), Entry::Loading);
        }
        let asked = wanted.clone();
        let task = background.spawn(async move { provider.linked_items(&asked).await });
        self.batches.push(Batch {
            connection_id: connection_id.to_owned(),
            references: wanted,
            task,
        });
        true
    }

    /// The platform's own links of the pull request `key` (one request, the first time it is asked).
    pub fn native(
        &mut self,
        background: &Background,
        provider: Arc<dyn Provider>,
        key: &str,
        repo: &str,
        number: u64,
    ) -> &Load<Vec<ItemRef>> {
        self.native.entry(key.to_owned()).or_insert_with(|| {
            let repo = repo.to_owned();
            let mut load = Load::Idle;
            load.start(background, async move { provider.linked_references(&repo, number).await });
            load
        })
    }

    /// Asks for the platform's own links of `key` again (after a failure).
    pub fn forget_native(&mut self, key: &str) {
        self.native.remove(key);
    }

    /// Moves finished requests into the cache. Returns true when anything arrived.
    pub fn poll(&mut self, now: f64) -> bool {
        let mut changed = false;
        for load in self.native.values_mut() {
            changed |= load.poll();
        }
        let mut index = 0;
        while index < self.batches.len() {
            let outcome = match self.batches[index].task.poll() {
                TaskPoll::Pending => {
                    index += 1;
                    continue;
                }
                TaskPoll::Done(result) => result.map_err(|error| error.to_string()),
                TaskPoll::Lost => Err(TASK_LOST.to_owned()),
            };
            let batch = self.batches.swap_remove(index);
            self.store(batch, outcome, now);
            changed = true;
        }
        if changed {
            self.version += 1;
        }
        changed
    }

    fn store(&mut self, batch: Batch, outcome: Result<Vec<LinkedItem>, String>, now: f64) {
        let connection_id = batch.connection_id;
        match outcome {
            Ok(items) => {
                for reference in batch.references {
                    self.entries.insert((connection_id.clone(), reference), Entry::Missing(now));
                }
                for item in items {
                    let key = (connection_id.clone(), item.reference.clone());
                    self.entries.insert(key, Entry::Found(item, now));
                }
            }
            Err(message) => {
                for reference in batch.references {
                    self.entries
                        .insert((connection_id.clone(), reference), Entry::Failed(message.clone()));
                }
            }
        }
    }
}
