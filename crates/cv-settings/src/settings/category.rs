//! The Settings page's sections, in sidebar order, with their titles, icons and intros.

use cv_base::theme::icons;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Category {
    #[default]
    Connections,
    Appearance,
    Merging,
    Assistant,
    LocalCheckouts,
    Links,
    Storage,
    Network,
    Keyboard,
}

impl Category {
    pub(crate) const ALL: [Self; 9] = [
        Self::Connections,
        Self::Appearance,
        Self::Merging,
        Self::Assistant,
        Self::LocalCheckouts,
        Self::Links,
        Self::Storage,
        Self::Network,
        Self::Keyboard,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Connections => "Connections",
            Self::Appearance => "Appearance",
            Self::Merging => "Merging",
            Self::Assistant => "Assistant",
            Self::LocalCheckouts => "Local checkouts",
            Self::Links => "Links",
            Self::Storage => "Storage",
            Self::Network => "Network activity",
            Self::Keyboard => "Keyboard and mouse",
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::Connections => icons::PLUGS,
            Self::Appearance => icons::PALETTE,
            Self::Merging => icons::GIT_MERGE,
            Self::Assistant => icons::ROBOT,
            Self::LocalCheckouts => icons::GIT_BRANCH,
            Self::Links => icons::LINK,
            Self::Storage => icons::DATABASE,
            Self::Network => icons::PULSE,
            Self::Keyboard => icons::KEYBOARD,
        }
    }

    /// One sentence under the section's title; Connections writes its own (it names the token store).
    pub(crate) fn intro(self) -> &'static str {
        match self {
            Self::Connections => "One connection per account and host.",
            Self::Appearance => "The sample redraws with every choice.",
            Self::Merging => {
                "The merge button preselects your favourite when the target branch allows it. A repository's last used type comes first."
            }
            Self::Assistant => {
                "Questions about a pull request go to your own, signed-in Claude Code or Codex CLI. It reads a read-only checkout of the pull request; no host tokens are passed to it."
            }
            Self::LocalCheckouts => "Where Check out puts a pull request, and what opens after it.",
            Self::Links => "How pull request links reach the app.",
            Self::Storage => {
                "Downloads the app keeps on this computer. Merged and closed pull requests and old items are deleted automatically; clearing frees the space now."
            }
            Self::Network => {
                "Requests per connection since the app started. Counts and path templates only, never tokens or queries."
            }
            Self::Keyboard => "Every shortcut in one place. Type in the search to filter.",
        }
    }

    /// Id salt of the section's scroll area, so each keeps its own position.
    pub(crate) fn scroll_id(self) -> &'static str {
        match self {
            Self::Connections => "settings_connections",
            Self::Appearance => "settings_appearance",
            Self::Merging => "settings_merging",
            Self::Assistant => "settings_assistant",
            Self::LocalCheckouts => "settings_local_checkouts",
            Self::Links => "settings_links",
            Self::Storage => "settings_storage",
            Self::Network => "settings_network",
            Self::Keyboard => "settings_keyboard",
        }
    }

    /// Sections whose settings change how something looks get a preview beside them.
    pub(crate) fn has_preview(self) -> bool {
        matches!(self, Self::Appearance | Self::Merging)
    }

    /// The sections offered: Merging only once a connection exists, since it lists their platforms.
    pub(crate) fn available(has_connections: bool) -> impl Iterator<Item = Self> {
        Self::ALL.into_iter().filter(move |category| has_connections || *category != Self::Merging)
    }
}
