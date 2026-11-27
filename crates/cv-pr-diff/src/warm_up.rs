//! Compiles the grammars of a pull request's changed files ahead, as soon as the file list is
//! known (on any tab, from the cached list when the pull request was open before), so the first
//! file of each language colors without compiling first. Every language of the changed files is
//! warmed: the file the Files tab opens first, then by changed lines and files, until the process
//! budget for compiled grammars ([`grammar::BUDGET_BYTES`]) is spent. The process keeps the
//! compiled sets (see [`grammar::compiled`]). Nothing here asks the platform: the paths come from
//! the local git mirror.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use review_git::ChangedFile;

use cv_syntax::grammar::{self, Grammar};
use cv_syntax::highlight;
use cv_syntax::pool::{Pool, Priority};

/// Two threads, next to the coloring pool's: the first two languages compile at once, and the UI
/// and git keep the other cores.
static POOL: Pool = Pool::new("grammar-warm", 2);

/// A changed file as the warm-up sees it.
pub(crate) struct Changed {
    pub path: String,
    /// Added and deleted lines, 0 while they are not counted yet.
    pub lines: u64,
}

/// The text files of `files`.
pub(crate) fn changed(files: &[ChangedFile]) -> Vec<Changed> {
    files
        .iter()
        .filter(|file| !file.is_binary)
        .map(|file| Changed {
            path: file.path.clone(),
            lines: u64::from(file.additions.unwrap_or(0)) + u64::from(file.deletions.unwrap_or(0)),
        })
        .collect()
}

/// The languages of `files`, each once: `first`'s (the file the Files tab opens) first, then the
/// ones with the most changed lines, then with the most files. Loads the grammar set on first use,
/// so only call this from background work.
pub(crate) fn order(files: &[Changed], first: Option<&str>) -> Vec<Grammar> {
    struct Language {
        grammar: Grammar,
        lines: u64,
        files: usize,
    }
    let mut languages: Vec<Language> = Vec::new();
    for file in files {
        let Some(grammar) = grammar::for_path(&file.path, "") else {
            continue;
        };
        match languages.iter_mut().find(|language| language.grammar.is(grammar)) {
            Some(language) => {
                language.lines += file.lines;
                language.files += 1;
            }
            None => languages.push(Language { grammar, lines: file.lines, files: 1 }),
        }
    }
    languages.sort_by_key(|language| std::cmp::Reverse((language.lines, language.files)));
    let mut grammars: Vec<Grammar> =
        languages.into_iter().map(|language| language.grammar).collect();
    let first = first.and_then(|path| grammar::for_path(path, ""));
    if let Some(position) =
        first.and_then(|first| grammars.iter().position(|grammar| grammar.is(first)))
    {
        let first = grammars.remove(position);
        grammars.insert(0, first);
    }
    grammars
}

/// One pull request's warm-up; the languages not started yet are skipped once it is cancelled.
pub(crate) struct WarmUp {
    cancel: Arc<AtomicBool>,
}

impl Drop for WarmUp {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Warms the languages of `files` in [`order`], two at a time. Drop the returned handle to stop
/// (another file list, the pull request closed).
pub(crate) fn start(files: Vec<Changed>, first: Option<String>) -> WarmUp {
    let cancel = Arc::new(AtomicBool::new(false));
    let job_cancel = cancel.clone();
    POOL.submit(Priority::Urgent, move || {
        if job_cancel.load(Ordering::Relaxed) {
            return;
        }
        let mut grammars = order(&files, first.as_deref()).into_iter();
        let spent = Arc::new(AtomicUsize::new(0));
        // This thread goes on with the first language; the other one takes the next.
        let first = grammars.next();
        for grammar in grammars {
            let (cancel, spent) = (job_cancel.clone(), spent.clone());
            POOL.submit(Priority::Queued, move || warm(grammar, &cancel, &spent));
        }
        if let Some(grammar) = first {
            warm(grammar, &job_cancel, &spent);
        }
    });
    WarmUp { cancel }
}

fn warm(grammar: Grammar, cancel: &AtomicBool, spent: &AtomicUsize) {
    if cancel.load(Ordering::Relaxed) || spent.load(Ordering::Relaxed) >= grammar::BUDGET_BYTES {
        return;
    }
    let working = highlight::warm_up(grammar);
    spent.fetch_add(working.estimated_bytes(), Ordering::Relaxed);
}

/// Warms the language of `path` (a file being opened) before the queued languages.
pub fn path_now(path: String) {
    POOL.submit(Priority::Urgent, move || {
        if let Some(grammar) = grammar::for_path(&path, "") {
            highlight::warm_up(grammar);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files_of(files: &[(&str, u64)]) -> Vec<Changed> {
        files
            .iter()
            .map(|(path, lines)| Changed { path: (*path).to_owned(), lines: *lines })
            .collect()
    }

    fn names(grammars: &[Grammar]) -> Vec<&str> {
        grammars.iter().map(|grammar| grammar.syntax.name.as_str()).collect()
    }

    const FILES: &[(&str, u64)] = &[
        ("README.md", 4),
        ("src/lib.rs", 100),
        ("src/main.rs", 50),
        ("web/app.ts", 120),
        ("web/util.ts", 0),
        ("App/Program.cs", 10),
        ("ci.yml", 10),
        ("config.zzqq", 900),
        ("db/schema.sql", 1),
    ];

    #[test]
    fn warms_every_language_most_changed_lines_first() {
        let grammars = order(&files_of(FILES), None);
        // Rust 150 lines, TypeScript 120, C# and YAML 10 each (in list order), Markdown 4, SQL 1.
        assert_eq!(
            names(&grammars),
            ["Rust", "TypeScript", "C#", "YAML", "Markdown", "SQL"],
            "every language, not only the most common ones"
        );
    }

    #[test]
    fn the_file_opened_first_goes_first() {
        let grammars = order(&files_of(FILES), Some("db/schema.sql"));
        assert_eq!(names(&grammars), ["SQL", "Rust", "TypeScript", "C#", "YAML", "Markdown"]);
        let grammars = order(&files_of(FILES), Some("notes.zzqq"));
        assert_eq!(names(&grammars)[0], "Rust", "a first file without a grammar changes nothing");
    }

    #[test]
    fn more_files_win_a_tie_in_lines() {
        let grammars = order(&files_of(&[("a.py", 0), ("b.go", 0), ("c.go", 0)]), None);
        assert_eq!(names(&grammars), ["Go", "Python"]);
    }

    #[test]
    fn warms_every_language() {
        let paths = ["a.py", "b.go", "c.json", "d.toml", "e.sql"];
        let grammars: Vec<Grammar> =
            paths.iter().map(|path| grammar::for_path(path, "").unwrap()).collect();
        for grammar in &grammars {
            grammar::forget(*grammar);
        }
        // Held here, so the process budget never drops them while other tests compile theirs.
        let _held: Vec<_> = grammars.iter().map(|grammar| grammar::compiled(*grammar)).collect();
        let _warm_up = start(files_of(&paths.map(|path| (path, 1))), Some("d.toml".into()));
        cv_testkit::poll_until("every language warmed", || {
            grammars.iter().all(|grammar| grammar::is_warmed(*grammar))
        });
    }
}
