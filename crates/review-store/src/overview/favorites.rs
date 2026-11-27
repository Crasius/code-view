//! Repositories the user starred, across every connection, for the overview's Favorites lane.

use serde::{Deserialize, Serialize};

/// One starred repository: the connection it is reached through and its repository key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FavoriteRepository {
    pub connection_id: String,
    pub repository_key: String,
}

/// The starred repositories, ordered by connection and key, whether the overview shows their lane,
/// and whether it shows it closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Favorites {
    repositories: Vec<FavoriteRepository>,
    /// Off, the overview has no Favorites card; the starred repositories' pull requests lead every
    /// card instead. On by default, also for files saved before the option existed.
    pub is_lane_shown: bool,
    pub is_collapsed: bool,
}

impl Default for Favorites {
    fn default() -> Self {
        Self { repositories: Vec::new(), is_lane_shown: true, is_collapsed: false }
    }
}

impl Favorites {
    pub fn repositories(&self) -> &[FavoriteRepository] {
        &self.repositories
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.repositories.is_empty()
    }

    pub fn contains(&self, connection_id: &str, repository_key: &str) -> bool {
        self.position(connection_id, repository_key).is_ok()
    }

    /// Stars the repository, or unstars it when it was starred. Returns whether it is starred now.
    pub fn toggle(&mut self, connection_id: &str, repository_key: &str) -> bool {
        match self.position(connection_id, repository_key) {
            Ok(index) => {
                self.repositories.remove(index);
                false
            }
            Err(index) => {
                let favorite = FavoriteRepository {
                    connection_id: connection_id.to_owned(),
                    repository_key: repository_key.to_owned(),
                };
                self.repositories.insert(index, favorite);
                true
            }
        }
    }

    /// Drops the favorites of connections that no longer exist.
    pub(crate) fn retain_connections(&mut self, is_known: impl Fn(&str) -> bool) {
        self.repositories.retain(|favorite| is_known(&favorite.connection_id));
    }

    /// Kept sorted and free of duplicates, also when a hand-edited file was not.
    fn normalize(&mut self) {
        self.repositories.sort();
        self.repositories.dedup();
    }

    fn position(&self, connection_id: &str, repository_key: &str) -> Result<usize, usize> {
        self.repositories.binary_search_by(|favorite| {
            (favorite.connection_id.as_str(), favorite.repository_key.as_str())
                .cmp(&(connection_id, repository_key))
        })
    }
}

/// Reads [`Favorites`] sorted and deduplicated, or the default where it does not parse.
pub(crate) fn deserialize<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Favorites, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    let mut favorites = Favorites::deserialize(value).unwrap_or_default();
    favorites.normalize();
    Ok(favorites)
}

#[cfg(test)]
mod tests {
    use super::Favorites;
    use crate::AppSettings;

    #[test]
    fn toggling_stars_and_unstars_in_order() {
        let mut favorites = Favorites::default();
        assert!(favorites.toggle("gh", "cli/cli"));
        assert!(favorites.toggle("ado", "Shop/web"));
        assert!(favorites.toggle("gh", "acme/app"));
        let keys: Vec<(&str, &str)> = favorites
            .repositories()
            .iter()
            .map(|favorite| (favorite.connection_id.as_str(), favorite.repository_key.as_str()))
            .collect();
        assert_eq!(keys, [("ado", "Shop/web"), ("gh", "acme/app"), ("gh", "cli/cli")]);
        assert!(favorites.contains("gh", "cli/cli"));
        assert!(!favorites.toggle("gh", "cli/cli"), "a second toggle unstars");
        assert!(!favorites.contains("gh", "cli/cli"));
        favorites.retain_connections(|id| id == "gh");
        assert_eq!(favorites.repositories().len(), 1);
    }

    #[test]
    fn a_favorites_object_without_the_lane_switch_shows_the_lane() {
        let older = r#"{"favorites":{"repositories":[{"connection_id":"gh","repository_key":"cli/cli"}],"is_collapsed":true}}"#;
        let settings: AppSettings = serde_json::from_str(older).expect("an older file");
        assert!(settings.favorites.is_lane_shown, "files from before the option show the lane");
        assert!(settings.favorites.is_collapsed && settings.favorites.contains("gh", "cli/cli"));
    }
}
