//! The overview's repository sections: whether its cards group their rows by repository, and which
//! sections the user closed.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// The card a section belongs to: a starred repository is listed twice, in the Favorites card and
/// in its connection's card, and each of its sections opens and closes on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SectionCard {
    Favorites,
    Connection,
}

/// One closed section: its card, and the repository and the connection it is reached through.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SectionKey {
    pub card: SectionCard,
    pub connection_id: String,
    pub repository_key: String,
}

/// Grouping on or off, and the closed sections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepositorySections {
    /// Off, each card lists its rows newest first with the repository in its own column. On by
    /// default, also for files saved before the option existed.
    pub is_grouped: bool,
    collapsed: BTreeSet<SectionKey>,
}

impl Default for RepositorySections {
    fn default() -> Self {
        Self { is_grouped: true, collapsed: BTreeSet::new() }
    }
}

impl RepositorySections {
    /// True when the user closed this section. Asked per section and frame, so it borrows; the
    /// closed sections are few, so a scan costs less than building an owned key.
    pub fn is_collapsed(
        &self,
        card: SectionCard,
        connection_id: &str,
        repository_key: &str,
    ) -> bool {
        self.collapsed.iter().any(|key| {
            key.card == card
                && key.connection_id == connection_id
                && key.repository_key == repository_key
        })
    }

    /// Closes an open section and opens a closed one. Returns whether it is closed now.
    pub fn toggle(&mut self, key: SectionKey) -> bool {
        if self.collapsed.remove(&key) {
            return false;
        }
        self.collapsed.insert(key);
        true
    }

    /// Drops the closed sections of connections that no longer exist.
    pub fn retain_connections(&mut self, is_known: impl Fn(&str) -> bool) {
        self.collapsed.retain(|key| is_known(&key.connection_id));
    }
}

#[cfg(test)]
mod tests {
    use super::{RepositorySections, SectionCard, SectionKey};

    fn key(card: SectionCard, connection_id: &str) -> SectionKey {
        SectionKey { card, connection_id: connection_id.into(), repository_key: "group/app".into() }
    }

    #[test]
    fn sections_close_per_card_and_go_with_their_connection() {
        let mut sections = RepositorySections::default();
        assert!(sections.toggle(key(SectionCard::Favorites, "gl")));
        assert!(sections.is_collapsed(SectionCard::Favorites, "gl", "group/app"));
        assert!(
            !sections.is_collapsed(SectionCard::Connection, "gl", "group/app"),
            "the same repository in its connection's card stays open"
        );
        assert!(sections.toggle(key(SectionCard::Connection, "gh")));
        assert!(!sections.toggle(key(SectionCard::Favorites, "gl")), "a second toggle opens");
        assert!(!sections.is_collapsed(SectionCard::Favorites, "gl", "group/app"));

        sections.retain_connections(|id| id == "gl");
        assert!(!sections.is_collapsed(SectionCard::Connection, "gh", "group/app"));
    }
}
