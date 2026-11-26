//! The user's merge strategy choices: a favourite per platform, a favourite per repository that
//! overrides it, and the strategy last merged with per repository. The merge button picks among
//! these only what the platform still allows.

use std::collections::BTreeMap;

use review_core::{MergeStrategy, ProviderKind};
use serde::{Deserialize, Serialize};

/// Repository-keyed entries go per connection id, then per repository key.
type PerRepository = BTreeMap<String, BTreeMap<String, MergeStrategy>>;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MergePreferences {
    platform_favourites: BTreeMap<ProviderKind, MergeStrategy>,
    repository_favourites: PerRepository,
    last_used: PerRepository,
    /// Whether the last merge in a repository deleted its source branch.
    delete_source_branch: BTreeMap<String, BTreeMap<String, bool>>,
}

impl MergePreferences {
    pub fn platform_favourite(&self, kind: ProviderKind) -> Option<MergeStrategy> {
        self.platform_favourites.get(&kind).copied()
    }

    /// `None` clears it.
    pub fn set_platform_favourite(&mut self, kind: ProviderKind, strategy: Option<MergeStrategy>) {
        match strategy {
            Some(strategy) => self.platform_favourites.insert(kind, strategy),
            None => self.platform_favourites.remove(&kind),
        };
    }

    pub fn repository_favourite(
        &self,
        connection_id: &str,
        repository: &str,
    ) -> Option<MergeStrategy> {
        get(&self.repository_favourites, connection_id, repository)
    }

    /// `None` clears it.
    pub fn set_repository_favourite(
        &mut self,
        connection_id: &str,
        repository: &str,
        strategy: Option<MergeStrategy>,
    ) {
        set(&mut self.repository_favourites, connection_id, repository, strategy);
    }

    pub fn last_used(&self, connection_id: &str, repository: &str) -> Option<MergeStrategy> {
        get(&self.last_used, connection_id, repository)
    }

    pub fn remember_last_used(
        &mut self,
        connection_id: &str,
        repository: &str,
        strategy: MergeStrategy,
    ) {
        set(&mut self.last_used, connection_id, repository, Some(strategy));
    }

    /// The source branch choice of the last merge in this repository; `None` before the first.
    pub fn delete_source_branch(&self, connection_id: &str, repository: &str) -> Option<bool> {
        self.delete_source_branch.get(connection_id)?.get(repository).copied()
    }

    pub fn remember_delete_source_branch(
        &mut self,
        connection_id: &str,
        repository: &str,
        is_deleted: bool,
    ) {
        self.delete_source_branch
            .entry(connection_id.to_owned())
            .or_default()
            .insert(repository.to_owned(), is_deleted);
    }

    /// Repositories with a favourite, a last used strategy or a source branch choice.
    pub fn remembered_repositories(&self) -> usize {
        let mut keys: Vec<(&String, &String)> = Vec::new();
        for map in [&self.repository_favourites, &self.last_used] {
            for (connection_id, repositories) in map {
                keys.extend(repositories.keys().map(|repository| (connection_id, repository)));
            }
        }
        for (connection_id, repositories) in &self.delete_source_branch {
            keys.extend(repositories.keys().map(|repository| (connection_id, repository)));
        }
        keys.sort();
        keys.dedup();
        keys.len()
    }

    /// Forgets every repository's favourite, last used strategy and source branch choice;
    /// platform favourites stay.
    pub fn forget_repositories(&mut self) {
        self.repository_favourites.clear();
        self.last_used.clear();
        self.delete_source_branch.clear();
    }

    /// Drops what belongs to connections that no longer exist.
    pub fn retain_connections(&mut self, is_known: impl Fn(&str) -> bool) {
        self.repository_favourites.retain(|id, _| is_known(id));
        self.last_used.retain(|id, _| is_known(id));
        self.delete_source_branch.retain(|id, _| is_known(id));
    }
}

fn get(map: &PerRepository, connection_id: &str, repository: &str) -> Option<MergeStrategy> {
    map.get(connection_id)?.get(repository).copied()
}

fn set(
    map: &mut PerRepository,
    connection_id: &str,
    repository: &str,
    strategy: Option<MergeStrategy>,
) {
    match strategy {
        Some(strategy) => {
            map.entry(connection_id.to_owned())
                .or_default()
                .insert(repository.to_owned(), strategy);
        }
        None => {
            if let Some(repositories) = map.get_mut(connection_id) {
                repositories.remove(repository);
                if repositories.is_empty() {
                    map.remove(connection_id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json_and_forgets_removed_connections() {
        let mut preferences = MergePreferences::default();
        preferences.set_platform_favourite(ProviderKind::GitHub, Some(MergeStrategy::Squash));
        preferences.set_repository_favourite("ado", "o/p/web", Some(MergeStrategy::SemiLinear));
        preferences.remember_last_used("ado", "o/p/web", MergeStrategy::Rebase);
        preferences.remember_last_used("gone", "x/y", MergeStrategy::Merge);
        preferences.remember_delete_source_branch("ado", "o/p/web", false);
        preferences.remember_delete_source_branch("ado", "o/p/api", true);
        let json = serde_json::to_string(&preferences).unwrap();
        let mut loaded: MergePreferences = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, preferences);
        assert_eq!(loaded.platform_favourite(ProviderKind::GitHub), Some(MergeStrategy::Squash));
        assert_eq!(loaded.repository_favourite("ado", "o/p/web"), Some(MergeStrategy::SemiLinear));
        assert_eq!(loaded.last_used("ado", "o/p/web"), Some(MergeStrategy::Rebase));
        assert_eq!(loaded.delete_source_branch("ado", "o/p/web"), Some(false));
        assert_eq!(loaded.delete_source_branch("ado", "o/p/new"), None);
        assert_eq!(
            loaded.remembered_repositories(),
            3,
            "web with everything, x/y with last used, api with the branch choice"
        );
        loaded.retain_connections(|id| id == "ado");
        assert_eq!(loaded.last_used("gone", "x/y"), None);
        assert_eq!(loaded.remembered_repositories(), 2);
        loaded.forget_repositories();
        assert_eq!(
            (loaded.remembered_repositories(), loaded.delete_source_branch("ado", "o/p/web")),
            (0, None)
        );
        assert_eq!(
            loaded.platform_favourite(ProviderKind::GitHub),
            Some(MergeStrategy::Squash),
            "platform favourites stay"
        );
        loaded.set_repository_favourite("ado", "o/p/web", None);
        loaded.set_platform_favourite(ProviderKind::GitHub, None);
        assert_eq!(
            (
                loaded.repository_favourite("ado", "o/p/web"),
                loaded.platform_favourite(ProviderKind::GitHub)
            ),
            (None, None)
        );
    }
}
