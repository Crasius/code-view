//! The log outside the viewer: as plain text for the clipboard (all of it or the selection), and
//! as the downloaded file for saving. Each reads the file once, on request, off the UI thread.

use std::borrow::Cow;
use std::io;
use std::path::Path;

use cv_widgets::text_selection::Selection;
use review_store::LogFile;

use super::super::ansi;

/// The whole log as plain text: each line as the platform delivered it, without ANSI escape
/// sequences and without the ISO timestamp Azure DevOps and GitHub put before every line (the
/// viewer shows it dimmed, apart from the text). Lines are joined with `\n`.
pub(crate) fn plain_text(path: &Path) -> io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(plain(&String::from_utf8_lossy(&bytes)))
}

fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (line, has_newline) = match line.strip_suffix('\n') {
            Some(line) => (line.strip_suffix('\r').unwrap_or(line), true),
            None => (line, false),
        };
        out.push_str(&body(line));
        if has_newline {
            out.push('\n');
        }
    }
    out
}

/// A line as copied: without the platform timestamp before it and without ANSI escapes. Text
/// selection positions count characters of this.
pub(crate) fn body(line: &str) -> Cow<'_, str> {
    let body = ansi::strip_timestamp(line);
    if body.contains('\u{1b}') {
        Cow::Owned(ansi::parse(body).into_iter().map(|span| span.text).collect())
    } else {
        Cow::Borrowed(body)
    }
}

/// The selected text of `file`, each line stripped as [`plain_text`] strips it. Reads only the
/// selected lines.
pub(crate) fn selected_text(file: &LogFile, selection: &Selection) -> io::Result<String> {
    let (start, end) = selection.bounds();
    let lines = file.read_lines(start.row..end.row + 1)?;
    Ok(selection.text(|row| lines.get(row - start.row).map(|line| body(line))))
}

/// A file name for the saved log: the job title with the characters Windows forbids replaced.
pub(crate) fn file_name(title: &str) -> String {
    let name: String = title
        .chars()
        .map(|character| {
            if character.is_control() || r#"<>:"/\|?*"#.contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let name = name.trim().trim_end_matches('.');
    format!("{}.log", if name.is_empty() { "pipeline" } else { name })
}

/// Copies the downloaded log, byte for byte, to `destination`.
pub(crate) fn save(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::copy(source, destination).map(|_| ())
}

#[cfg(test)]
mod tests {
    use cv_testkit::ScratchDir;
    use cv_widgets::text_selection::{Caret, Selection};
    use review_store::LogFile;

    use super::{file_name, plain, selected_text};

    #[test]
    fn strips_escapes_and_platform_timestamps_only() {
        let log = "2024-05-01T10:00:00.1234567Z ##[section]Starting: Build\r\n\
                   2024-05-01T10:00:01.0000000Z \u{1b}[32mok\u{1b}[0m 3 tests\u{1b}[2K\n\
                   plain line with  two spaces\n\
                   \n\
                   last";
        assert_eq!(
            plain(log),
            "##[section]Starting: Build\nok 3 tests\nplain line with  two spaces\n\nlast"
        );
    }

    #[test]
    fn copies_a_selection_as_plain_text() {
        let scratch = ScratchDir::new("log-selection");
        let path = scratch.path().join("selection.log");
        std::fs::write(&path, "2024-05-01T10:00:00.1234567Z \u{1b}[32mgreen\u{1b}[0m words here\r\nsecond line\r\nthird\n").expect("write");
        let file = LogFile::open(&path).expect("index");
        let mut selection = Selection::caret((), Caret::new(0, 6));
        selection.extend_to(Caret::new(2, 2));
        assert_eq!(selected_text(&file, &selection).expect("read"), "words here\nsecond line\nth");
        assert_eq!(
            selected_text(&file, &Selection::lines((), 0, 1)).expect("read"),
            "green words here\nsecond line\n"
        );
    }

    #[test]
    fn names_the_file_after_the_job() {
        assert_eq!(file_name("Build: test / lint"), "Build_ test _ lint.log");
        assert_eq!(file_name("  "), "pipeline.log");
    }
}
