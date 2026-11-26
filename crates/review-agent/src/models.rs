//! Which model and effort a session asks for. "Fastest" is the default, so quick questions answer
//! fast: the first model the CLI itself describes as fast, at low effort when the model takes it.
//! Model names are never hard-coded for Codex; for Claude the documented `haiku` alias stands in
//! when the CLI lists no models.

use crate::{AgentKind, ModelChoice, ModelInfo, ModelPick};

/// The effort a fast pick asks for, when the model takes it.
const FAST_EFFORT: &str = "low";
/// Claude's documented alias for its latest fast model (`claude --help`, `--model`).
const CLAUDE_FAST_ALIAS: &str = "haiku";

pub(crate) fn resolve(kind: AgentKind, choice: &ModelChoice, models: &[ModelInfo]) -> ModelPick {
    match choice {
        ModelChoice::Named(id) => ModelPick { model: Some(id.clone()), effort: None },
        ModelChoice::CliDefault => ModelPick::default(),
        ModelChoice::Fastest => fastest(kind, models),
    }
}

fn fastest(kind: AgentKind, models: &[ModelInfo]) -> ModelPick {
    let described_fast =
        models.iter().find(|model| model.description.to_lowercase().contains("fast"));
    let chosen = match kind {
        AgentKind::Codex => described_fast.or_else(|| models.iter().find(|model| model.is_default)),
        AgentKind::Claude => described_fast,
    };
    match (chosen, kind) {
        (Some(model), _) => ModelPick {
            model: Some(model.id.clone()),
            effort: model
                .efforts
                .iter()
                .any(|effort| effort == FAST_EFFORT)
                .then(|| FAST_EFFORT.to_owned()),
        },
        (None, AgentKind::Claude) => {
            ModelPick { model: Some(CLAUDE_FAST_ALIAS.to_owned()), effort: None }
        }
        (None, AgentKind::Codex) => ModelPick::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use crate::{AgentKind, ModelChoice, ModelInfo, ModelPick};

    fn model(id: &str, description: &str, is_default: bool, efforts: &[&str]) -> ModelInfo {
        ModelInfo {
            id: id.to_owned(),
            display_name: id.to_owned(),
            description: description.to_owned(),
            is_default,
            efforts: efforts.iter().map(|effort| (*effort).to_owned()).collect(),
        }
    }

    fn pick(model: &str, effort: Option<&str>) -> ModelPick {
        ModelPick { model: Some(model.to_owned()), effort: effort.map(str::to_owned) }
    }

    #[test]
    fn fastest_codex_model_is_the_first_one_described_as_fast_at_low_effort() {
        let models = [
            model("workhorse", "Latest workhorse model for coding.", true, &["low", "high"]),
            model("small", "Fast and affordable model for easier tasks.", false, &["low"]),
            model("older-small", "Older fast and efficient model.", false, &["low"]),
        ];
        assert_eq!(
            resolve(AgentKind::Codex, &ModelChoice::Fastest, &models),
            pick("small", Some("low"))
        );
    }

    #[test]
    fn fastest_codex_model_falls_back_to_the_default_and_skips_an_effort_it_lacks() {
        let models = [
            model("frontier", "Frontier intelligence.", false, &["low"]),
            model("workhorse", "Latest workhorse model.", true, &["medium", "high"]),
        ];
        assert_eq!(
            resolve(AgentKind::Codex, &ModelChoice::Fastest, &models),
            pick("workhorse", None)
        );
        assert_eq!(resolve(AgentKind::Codex, &ModelChoice::Fastest, &[]), ModelPick::default());
    }

    #[test]
    fn fastest_claude_model_is_the_listed_fast_alias_or_haiku() {
        let models = [
            model("opus", "For complex work and everyday tasks", true, &["low", "high"]),
            model("haiku", "Fastest for quick answers", false, &["low", "high"]),
        ];
        assert_eq!(
            resolve(AgentKind::Claude, &ModelChoice::Fastest, &models),
            pick("haiku", Some("low"))
        );
        assert_eq!(resolve(AgentKind::Claude, &ModelChoice::Fastest, &[]), pick("haiku", None));
    }

    #[test]
    fn named_and_cli_default_choices_pass_through() {
        let named = ModelChoice::Named("gpt-6-astra".to_owned());
        assert_eq!(resolve(AgentKind::Codex, &named, &[]), pick("gpt-6-astra", None));
        assert_eq!(resolve(AgentKind::Claude, &ModelChoice::CliDefault, &[]), ModelPick::default());
    }
}
