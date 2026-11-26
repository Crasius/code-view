//! The pull request assistant's defaults: whether it is shown at all, which coding agent CLI
//! answers and with which model. The chat can switch agent and model per conversation; these are
//! what a new conversation starts with. The height of the card's conversation is kept here too,
//! and whether a followed citation keeps the answer beside the code in a mini chat, in which
//! corner of the Files tab.

use review_core::{AgentKind, ModelChoice};
use serde::{Deserialize, Serialize};

/// The assistant's defaults, per agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssistantSettings {
    /// Off, the app shows no assistant anywhere and starts no agent CLI; kept conversations stay.
    pub enabled: bool,
    /// The agent a new conversation starts with.
    pub agent: AgentKind,
    pub claude_model: ModelChoice,
    pub codex_model: ModelChoice,
    /// Height of the conversation in the Overview's assistant card, in points, once the user set
    /// one; `None` fills the room the side column shows.
    pub card_height: Option<f32>,
    /// On, a citation followed from an answer opens the Files tab with a mini chat floating in a
    /// corner of it: the answer, its references and a follow-up. Off, a citation only opens the
    /// file. On by default. Stored under its own key: files written while the default was off carry
    /// `mini_chat: false`, a value nobody chose, and that old key is ignored.
    #[serde(rename = "mini_chat_beside_code")]
    pub mini_chat: bool,
    /// The corner of the Files tab's content the mini chat sits in.
    pub mini_chat_corner: MiniChatCorner,
}

/// A corner of the Files tab's content; bottom right unless the user dragged the mini chat away.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MiniChatCorner {
    pub left: bool,
    pub top: bool,
}

impl Default for AssistantSettings {
    /// Shown: files written before the switch existed load with the assistant on.
    fn default() -> Self {
        Self {
            enabled: true,
            agent: AgentKind::default(),
            claude_model: ModelChoice::default(),
            codex_model: ModelChoice::default(),
            card_height: None,
            mini_chat: true,
            mini_chat_corner: MiniChatCorner::default(),
        }
    }
}

impl AssistantSettings {
    /// The default model choice for `agent`.
    pub fn model(&self, agent: AgentKind) -> &ModelChoice {
        match agent {
            AgentKind::Claude => &self.claude_model,
            AgentKind::Codex => &self.codex_model,
        }
    }

    pub fn model_mut(&mut self, agent: AgentKind) -> &mut ModelChoice {
        match agent {
            AgentKind::Claude => &mut self.claude_model,
            AgentKind::Codex => &mut self.codex_model,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AssistantSettings;

    /// Settings files written while the expanded panel had its own width still load.
    #[test]
    fn a_settings_file_with_the_old_panel_width_loads() {
        let json = r#"{"panel_width":640.0,"card_height":300.0}"#;
        let settings: AssistantSettings = serde_json::from_str(json).expect("loads");
        assert_eq!(settings.card_height, Some(300.0));
    }

    /// Files written before the assistant could be turned off load with it shown.
    #[test]
    fn a_settings_file_without_the_switch_loads_with_the_assistant_shown() {
        let settings: AssistantSettings =
            serde_json::from_str(r#"{"agent":"Codex"}"#).expect("loads");
        assert!(settings.enabled);
        let off: AssistantSettings = serde_json::from_str(r#"{"enabled":false}"#).expect("loads");
        assert!(!off.enabled);
    }

    /// Files without a choice load with the mini chat on, in the bottom-right corner; the old
    /// `mini_chat: false` was a default, not a choice, so it does not turn it off.
    #[test]
    fn a_settings_file_without_the_mini_chat_choice_loads_with_it_on() {
        for json in [r#"{"enabled":true}"#, r#"{"mini_chat":false}"#] {
            let settings: AssistantSettings = serde_json::from_str(json).expect("loads");
            assert!(settings.mini_chat, "{json}");
            let corner = settings.mini_chat_corner;
            assert!(!corner.left && !corner.top, "bottom right");
        }
    }

    /// An explicit choice under the current key is kept.
    #[test]
    fn a_stored_mini_chat_choice_is_kept() {
        let off: AssistantSettings =
            serde_json::from_str(r#"{"mini_chat_beside_code":false}"#).expect("loads");
        assert!(!off.mini_chat);
        let json = serde_json::to_string(&off).expect("saves");
        assert!(json.contains(r#""mini_chat_beside_code":false"#), "{json}");
    }

    /// Files written while the chip's place was an option still load; the field is ignored.
    #[test]
    fn a_settings_file_with_the_old_chip_place_loads() {
        let json = r#"{"enabled":true,"model_in_title":false}"#;
        let settings: AssistantSettings = serde_json::from_str(json).expect("loads");
        assert!(settings.enabled);
    }
}
