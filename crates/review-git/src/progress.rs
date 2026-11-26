//! Parses git's `--progress` lines ("Receiving objects:  45% (123/456), 1.20 MiB | 3.00 MiB/s").

// RS-DONT-04: the input is git's stderr; parsing it never indexes.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

/// One progress update from a running git command.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitProgress {
    /// "Counting objects", "Receiving objects", "Resolving deltas", …
    pub phase: String,
    pub percent: Option<u8>,
    pub done: Option<u64>,
    pub total: Option<u64>,
    /// Transfer size and rate when git reports them ("1.20 MiB | 3.00 MiB/s").
    pub transfer: Option<String>,
}

/// What a long-running git command reports while it works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitEvent {
    /// The command about to run, without `--git-dir`. Credentials never appear in it: they reach
    /// git through the environment.
    Command(String),
    /// A `--progress` update; a newer one of the same phase replaces it.
    Progress(GitProgress),
    /// Any other line git wrote to stderr ("From https://…", " * [new ref] …", "fatal: …").
    Message(String),
}

pub type ProgressSink = dyn Fn(GitEvent) + Send + Sync;

const REMOTE_PREFIX: &str = "remote: ";

pub(crate) fn parse_line(line: &str) -> Option<GitProgress> {
    let line = line.trim().strip_prefix(REMOTE_PREFIX).unwrap_or(line.trim());
    let (phase, rest) = line.split_once(':')?;
    let phase = phase.trim();
    let is_phase_name = !phase.is_empty()
        && phase.chars().all(|character| character.is_alphabetic() || character == ' ');
    if !is_phase_name {
        return None;
    }
    let rest = rest.trim();
    let percent = rest.split_once('%').and_then(|(number, _)| number.trim().parse::<u8>().ok());
    let (done, total) = parse_counts(rest);
    let transfer = rest
        .split_once("),")
        .map(|(_, transfer)| {
            transfer.trim().trim_end_matches("done.").trim().trim_end_matches(',').trim().to_owned()
        })
        .filter(|transfer| !transfer.is_empty());
    let has_numbers = percent.is_some() || done.is_some();
    if !has_numbers {
        return None;
    }
    Some(GitProgress { phase: phase.to_owned(), percent, done, total, transfer })
}

/// "(123/456)" → (123, 456); "123, done." or "123" → (123, None).
fn parse_counts(rest: &str) -> (Option<u64>, Option<u64>) {
    if let Some((_, after)) = rest.split_once('(')
        && let Some((inner, _)) = after.split_once(')')
        && let Some((done, total)) = inner.split_once('/')
    {
        return (done.trim().parse().ok(), total.trim().parse().ok());
    }
    let leading: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (leading.parse().ok(), None)
}

#[cfg(test)]
mod tests {
    use super::parse_line;

    #[test]
    fn parses_receiving_objects() {
        let progress =
            parse_line("Receiving objects:  45% (123/456), 1.20 MiB | 3.00 MiB/s").unwrap();
        assert_eq!(progress.phase, "Receiving objects");
        assert_eq!(progress.percent, Some(45));
        assert_eq!((progress.done, progress.total), (Some(123), Some(456)));
        assert_eq!(progress.transfer.as_deref(), Some("1.20 MiB | 3.00 MiB/s"));
    }

    #[test]
    fn parses_remote_counting() {
        let progress = parse_line("remote: Counting objects: 100% (30/30), done.").unwrap();
        assert_eq!(progress.phase, "Counting objects");
        assert_eq!(progress.percent, Some(100));
        assert_eq!(progress.transfer, None);
    }

    #[test]
    fn parses_enumerating_without_percent() {
        let progress = parse_line("remote: Enumerating objects: 2405, done.").unwrap();
        assert_eq!(progress.phase, "Enumerating objects");
        assert_eq!(progress.done, Some(2405));
        assert_eq!(progress.percent, None);
    }

    #[test]
    fn ignores_other_output() {
        assert_eq!(parse_line("From https://github.com/cli/cli"), None);
        assert_eq!(parse_line("fatal: couldn't find remote ref refs/heads/x"), None);
    }
}
