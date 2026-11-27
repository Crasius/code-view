//! What the user does in the card or panel: ask (with attached lines), stop, retry, switch agent
//! or model, start over (and undo that), continue in a terminal, sign in, and move the agent to
//! new commits. [`act`] carries out the [`Action`] a view returned.

use std::path::{Path, PathBuf};

use cv_pr_session::session::PullRequestSession;
use cv_runtime::toasts::{ToastAction, WaysOut};
use cv_services::Services;
use review_core::short_sha;
use review_store::{Chat, ChatQuote, TurnEnd, TurnKind};

use super::{Action, Assistant, brief, checkout, links, transcript};

/// Asks `question`, with the attached lines when `with_quote` (the mini chat's follow-up leaves
/// them for the card); the prompt goes once checkout and session are there. Returns whether the
/// question went out: only then may the caller empty its draft.
pub(super) fn ask(assistant: &mut Assistant, question: &str, with_quote: bool) -> bool {
    let question = question.trim();
    if question.is_empty() || assistant.is_busy() {
        return false;
    }
    let quote = if with_quote { assistant.quote.take() } else { None };
    let prompt = match &quote {
        Some(quote) => quoted_prompt(quote, question),
        None => question.to_owned(),
    };
    transcript::begin(&mut assistant.chat, TurnKind::Question, question, quote, prompt.clone());
    assistant.pending = Some(prompt);
    assistant.block = None;
    assistant.previous = None;
    true
}

pub(crate) fn act(
    assistant: &mut Assistant,
    action: Action,
    session: &PullRequestSession,
    services: &mut Services,
) {
    match action {
        Action::Ask(question) => {
            if ask(assistant, &question, true) {
                assistant.composer.clear();
            }
        }
        Action::Stop => stop(assistant),
        Action::Retry => retry(assistant),
        Action::SwitchAgent(agent) => {
            if agent != assistant.chat.agent {
                assistant.save(services);
                assistant.show_conversation(agent, services);
            }
        }
        Action::SwitchModel(choice) => {
            assistant.chat.model = choice;
            // The chip offers no model while an answer runs; the next question starts the CLI on
            // this one, resumed.
            assistant.runner.end();
            assistant.model_in_use = None;
        }
        Action::NewConversation => new_conversation(assistant),
        Action::UndoNewConversation => undo_new_conversation(assistant),
        Action::ContinueInTerminal => continue_in_terminal(assistant, services),
        Action::SignIn => {
            let dir = terminal_dir(assistant, services);
            let (program, args) = review_agent::sign_in_command(assistant.chat.agent);
            open_terminal(services, &dir, &program, &args);
        }
        Action::CheckAgain => {
            services.agents.check_again(assistant.chat.agent);
            assistant.block = None;
        }
        Action::Resume => assistant.block = None,
        Action::UpdateAgent => update_agent(assistant, session),
        Action::Expand => {
            assistant.is_expanded = true;
            assistant.focus_composer = true;
        }
        Action::Collapse => assistant.is_expanded = false,
        Action::DropQuote => assistant.quote = None,
    }
    assistant.save(services);
}

/// Stops the answer; a question still waiting for its checkout or session is dropped.
pub(super) fn stop(assistant: &mut Assistant) {
    if assistant.runner.has_session() && assistant.pending.is_none() {
        assistant.runner.interrupt();
        assistant.is_stopping = true;
    } else {
        assistant.pending = None;
        assistant.move_to = None;
        transcript::stop(&mut assistant.chat);
    }
}

/// Asks the last, failed question again; an agent whose process ended starts again for it, as
/// Restart would start it.
pub(super) fn retry(assistant: &mut Assistant) {
    if let Some(turn) = assistant.chat.turns.last_mut() {
        turn.answer.clear();
        turn.steps.clear();
        turn.usage = None;
        turn.end = TurnEnd::Running;
        assistant.pending = Some(turn.prompt.clone());
        assistant.block = None;
    }
}

/// Puts this agent's conversation away for Undo and starts an empty one; a running answer stops,
/// the checkout stays.
fn new_conversation(assistant: &mut Assistant) {
    let fresh = Chat {
        agent: assistant.chat.agent,
        model: assistant.chat.model.clone(),
        head: assistant.checkout.as_ref().map(|checkout| checkout.head.clone()),
        ..Chat::default()
    };
    let mut old = std::mem::replace(&mut assistant.chat, fresh);
    if !old.turns.is_empty() {
        transcript::stop(&mut old);
        assistant.previous = Some(old);
    }
    assistant.chats.remove(assistant.chat.agent);
    restart(assistant);
}

/// Brings back the conversation [`new_conversation`] put away; its session resumes with the next
/// question.
fn undo_new_conversation(assistant: &mut Assistant) {
    let Some(old) = assistant.previous.take() else {
        return;
    };
    assistant.chat = old;
    restart(assistant);
    assistant.links.to_check = (0..assistant.chat.turns.len()).collect();
}

/// Ends the CLI and what waited for it, before another conversation shows.
fn restart(assistant: &mut Assistant) {
    assistant.runner.end();
    assistant.pending = None;
    assistant.move_to = None;
    assistant.block = None;
    assistant.is_stopping = false;
    assistant.model_in_use = None;
    assistant.links = links::LinkedAnswers::default();
    assistant.mini.close();
}

/// "Update the agent": moves the checkout to the new head, then tells the agent.
fn update_agent(assistant: &mut Assistant, session: &PullRequestSession) {
    let (Some(detail), Some(old)) = (session.detail.ready(), assistant.chat.head.clone()) else {
        return;
    };
    if assistant.is_busy() {
        return;
    }
    let prompt = brief::update_note(&old, &detail.head_sha);
    let question = format!("Moved to the new commits ({})", short_sha(&detail.head_sha));
    transcript::begin(&mut assistant.chat, TurnKind::Update, &question, None, prompt.clone());
    assistant.move_to = Some(detail.head_sha.clone());
    assistant.pending = Some(prompt);
}

fn terminal_dir(assistant: &Assistant, services: &Services) -> PathBuf {
    let dir = checkout::dir(&services.paths, &assistant.key.repository_key, assistant.key.number);
    if dir.is_dir() { dir } else { services.paths.cache_dir().to_path_buf() }
}

/// The agent's CLI in a terminal: this conversation's session resumed, or a fresh one in the
/// checkout before the first answer.
fn continue_in_terminal(assistant: &Assistant, services: &mut Services) {
    if assistant.terminal_block().is_some() {
        return;
    }
    let session_id = assistant.chat.session_id.as_deref();
    let (program, args) = review_agent::terminal_command(assistant.chat.agent, session_id);
    let dir = terminal_dir(assistant, services);
    open_terminal(services, &dir, &program, &args);
}

/// Runs `program` with `args` in a terminal in `dir`; when no terminal opens, the toast hands
/// over the command and the folder to run it from.
fn open_terminal(services: &mut Services, dir: &Path, program: &str, args: &[String]) {
    if let Err(error) = services.agents.open_terminal(dir, program, args) {
        let command = std::iter::once(program)
            .chain(args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        let ways_out = WaysOut::new(ToastAction::copy("Copy command", command))
            .and(ToastAction::open_folder(dir));
        services.toasts.error(format!("Could not open a terminal: {error}"), ways_out);
    }
}

/// The prompt for a question about quoted lines.
fn quoted_prompt(quote: &ChatQuote, question: &str) -> String {
    let side = if quote.is_old_side { "base" } else { "head" };
    let lines = if quote.start == quote.end {
        format!("line {}", quote.start)
    } else {
        format!("lines {}-{}", quote.start, quote.end)
    };
    format!(
        "About {}, {lines} of the {side} version:\n```\n{}\n```\n\n{question}",
        quote.path,
        quote.text.trim_end_matches('\n')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quoted_question_names_the_lines_and_their_side() {
        let quote = ChatQuote {
            path: "Billing/InvoiceIssuer.cs".into(),
            start: 77,
            end: 78,
            is_old_side: false,
            text: "if (vat != VatStatus.Invalid)\n    return Treatment.ReverseCharge;\n".into(),
        };
        assert_eq!(
            quoted_prompt(&quote, "Why this change?"),
            "About Billing/InvoiceIssuer.cs, lines 77-78 of the head version:\n```\nif (vat != \
             VatStatus.Invalid)\n    return Treatment.ReverseCharge;\n```\n\nWhy this change?"
        );
    }
}
