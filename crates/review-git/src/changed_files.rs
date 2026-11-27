//! Parses `git diff-tree -z --raw [--numstat]` output into [`ChangedFile`]s.

// RS-DONT-04: the input is git's output; parsing it never indexes.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use std::collections::{HashMap, HashSet};

use review_core::{FileCategory, ProviderError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
}

impl ChangeKind {
    fn from_status_letter(letter: u8) -> Option<Self> {
        let kind = match letter {
            b'A' => ChangeKind::Added,
            b'M' => ChangeKind::Modified,
            b'D' => ChangeKind::Deleted,
            b'R' => ChangeKind::Renamed,
            b'C' => ChangeKind::Copied,
            b'T' => ChangeKind::TypeChanged,
            _ => return None,
        };
        Some(kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    /// Path in the head commit (for deletions: the deleted path).
    pub path: String,
    /// Source path of a rename or copy.
    pub old_path: Option<String>,
    pub kind: ChangeKind,
    /// All-zero for added files.
    pub old_oid: String,
    /// All-zero for deleted files.
    pub new_oid: String,
    /// `None` until line counts are known (phase 2) or for binary files.
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub is_binary: bool,
    pub category: FileCategory,
    /// Every changed line differs only in whitespace or its line ending (known with the line counts).
    pub whitespace_only: bool,
}

impl ChangedFile {
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// The base side has content (not an added file).
    pub fn has_old_blob(&self) -> bool {
        !is_zero_oid(&self.old_oid)
    }

    /// The head side has content (not a deleted file).
    pub fn has_new_blob(&self) -> bool {
        !is_zero_oid(&self.new_oid)
    }

    /// Both sides hold the same bytes: a pure rename or copy, a mode or type change, or an empty
    /// file added or deleted. Any other change has changed lines (or is binary).
    pub fn has_same_content(&self) -> bool {
        let is_empty_blob = |oid: &str| EMPTY_BLOBS.contains(&oid);
        self.old_oid == self.new_oid
            || (self.kind == ChangeKind::Added && is_empty_blob(&self.new_oid))
            || (self.kind == ChangeKind::Deleted && is_empty_blob(&self.old_oid))
    }

    /// `path` changed as `kind`: old blob "1" x 40, new blob "2" x 40, the missing side of an added
    /// or deleted file "0" x 40, no counts, not binary, categorised by its path.
    #[cfg(any(test, feature = "test-support"))]
    pub fn sample(path: &str, kind: ChangeKind) -> Self {
        let blob = |digit: char, missing: ChangeKind| {
            let digit = if kind == missing { ZERO_OID_CHAR } else { digit };
            digit.to_string().repeat(40)
        };
        Self {
            path: path.to_owned(),
            old_path: None,
            kind,
            old_oid: blob('1', ChangeKind::Added),
            new_oid: blob('2', ChangeKind::Deleted),
            additions: None,
            deletions: None,
            is_binary: false,
            category: FileCategory::classify(path),
            whitespace_only: false,
        }
    }
}

/// The empty blob's id in SHA-1 and SHA-256 repositories.
const EMPTY_BLOBS: [&str; 2] = [
    "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391",
    "473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813",
];

const ZERO_OID_CHAR: char = '0';

fn is_zero_oid(oid: &str) -> bool {
    oid.chars().all(|character| character == ZERO_OID_CHAR)
}

pub(crate) fn parse_raw(output: &str) -> Result<Vec<ChangedFile>> {
    let mut tokens = Tokens::new(output);
    let mut files = Vec::new();
    while let Some(token) = tokens.next() {
        let file = parse_raw_record(token, &mut tokens)?;
        files.push(file);
    }
    Ok(files)
}

pub(crate) fn parse_raw_with_numstat(output: &str) -> Result<Vec<ChangedFile>> {
    let mut tokens = Tokens::new(output);
    let mut files: Vec<ChangedFile> = Vec::new();
    let mut index_by_path: HashMap<String, usize> = HashMap::new();
    while let Some(token) = tokens.next() {
        if token.starts_with(':') {
            let file = parse_raw_record(token, &mut tokens)?;
            index_by_path.insert(file.path.clone(), files.len());
            files.push(file);
            continue;
        }
        let (additions, deletions, path) = parse_numstat_record(token, &mut tokens)?;
        let Some(file) = index_by_path.get(&path).and_then(|&index| files.get_mut(index)) else {
            continue;
        };
        set_counts(file, additions, deletions);
    }
    Ok(files)
}

/// Counts the files at `indexes` from `git diff-tree -z --numstat` output over their paths
/// (`counts`) and from the same with `-w` (`ignoring_whitespace`), as [`parse_raw_with_numstat`]
/// and [`mark_whitespace_only`] would over the whole list.
pub(crate) fn apply_counts(
    files: &mut [ChangedFile],
    indexes: &[usize],
    counts: &str,
    ignoring_whitespace: &str,
) -> Result<()> {
    let mut tokens = Tokens::new(counts);
    let mut counted = HashMap::new();
    while let Some(token) = tokens.next() {
        let (additions, deletions, path) = parse_numstat_record(token, &mut tokens)?;
        counted.insert(path, (additions, deletions));
    }
    let changed_paths = numstat_paths(ignoring_whitespace)?;
    for &index in indexes {
        let Some(file) = files.get_mut(index) else {
            continue;
        };
        if let Some(&(additions, deletions)) = counted.get(&file.path) {
            set_counts(file, additions, deletions);
        }
        file.whitespace_only = is_whitespace_only(file, &changed_paths);
    }
    Ok(())
}

/// The list with its renames: each pair of `renamed` (`git diff-tree -M --diff-filter=R`, counted)
/// takes the place of its added file, and its deleted file leaves the list, as in one
/// `git diff-tree -M` over the comparison. `files` lists the comparison without renames.
pub(crate) fn pair_renames(files: Vec<ChangedFile>, renamed: Vec<ChangedFile>) -> Vec<ChangedFile> {
    let sources: HashSet<String> =
        renamed.iter().filter_map(|file| file.old_path.clone()).collect();
    let mut by_destination: HashMap<String, ChangedFile> =
        renamed.into_iter().map(|file| (file.path.clone(), file)).collect();
    files
        .into_iter()
        .filter_map(|file| match file.kind {
            ChangeKind::Deleted if sources.contains(&file.path) => None,
            ChangeKind::Added => Some(by_destination.remove(&file.path).unwrap_or(file)),
            _ => Some(file),
        })
        .collect()
}

/// A binary file reports no counts.
fn set_counts(file: &mut ChangedFile, additions: Option<u32>, deletions: Option<u32>) {
    file.is_binary = additions.is_none();
    file.additions = additions;
    file.deletions = deletions;
}

/// `:100644 100644 <old> <new> M` followed by one path, or two for renames and copies.
fn parse_raw_record(header: &str, tokens: &mut Tokens<'_>) -> Result<ChangedFile> {
    let fields: Vec<&str> = header.trim_start_matches(':').split(' ').collect();
    let [old_mode, new_mode, old_oid, new_oid, status] = fields.as_slice() else {
        return Err(malformed(header));
    };
    let status_letter = status.as_bytes().first().copied().ok_or_else(|| malformed(header))?;
    let kind = ChangeKind::from_status_letter(status_letter).ok_or_else(|| malformed(header))?;
    let first_path = tokens.next().ok_or_else(|| malformed(header))?.to_owned();
    let has_two_paths = matches!(kind, ChangeKind::Renamed | ChangeKind::Copied);
    let (path, old_path) = if has_two_paths {
        let new_path = tokens.next().ok_or_else(|| malformed(header))?.to_owned();
        (new_path, Some(first_path))
    } else {
        (first_path, None)
    };
    let is_gitlink = *old_mode == "160000" || *new_mode == "160000";
    let category = FileCategory::classify(&path);
    Ok(ChangedFile {
        path,
        old_path,
        kind,
        old_oid: (*old_oid).to_owned(),
        new_oid: (*new_oid).to_owned(),
        additions: None,
        deletions: None,
        is_binary: is_gitlink,
        category,
        whitespace_only: false,
    })
}

/// Marks the text files with changed lines that `git diff-tree -z --numstat -w` output (`output`)
/// leaves out: with whitespace ignored, they have no changes.
pub(crate) fn mark_whitespace_only(files: &mut [ChangedFile], output: &str) -> Result<()> {
    let changed_paths = numstat_paths(output)?;
    for file in files {
        file.whitespace_only = is_whitespace_only(file, &changed_paths);
    }
    Ok(())
}

/// The paths `git diff-tree -z --numstat` output (`output`) lists.
fn numstat_paths(output: &str) -> Result<HashSet<String>> {
    let mut tokens = Tokens::new(output);
    let mut paths = HashSet::new();
    while let Some(token) = tokens.next() {
        let (_, _, path) = parse_numstat_record(token, &mut tokens)?;
        paths.insert(path);
    }
    Ok(paths)
}

/// `file` has changed lines, and the whitespace-ignoring counts (`changed_paths`) leave it out.
fn is_whitespace_only(file: &ChangedFile, changed_paths: &HashSet<String>) -> bool {
    let changed_lines = file.additions.unwrap_or(0).saturating_add(file.deletions.unwrap_or(0));
    !file.is_binary && changed_lines > 0 && !changed_paths.contains(&file.path)
}

/// `<added>\t<deleted>\t<path>` or, for renames, `<added>\t<deleted>\t` followed by two path tokens.
/// Binary files report `-` for both counts.
fn parse_numstat_record(
    record: &str,
    tokens: &mut Tokens<'_>,
) -> Result<(Option<u32>, Option<u32>, String)> {
    let mut parts = record.splitn(3, '\t');
    let additions = parts.next().ok_or_else(|| malformed(record))?;
    let deletions = parts.next().ok_or_else(|| malformed(record))?;
    let inline_path = parts.next().ok_or_else(|| malformed(record))?;
    let path = if inline_path.is_empty() {
        let _source = tokens.next().ok_or_else(|| malformed(record))?;
        tokens.next().ok_or_else(|| malformed(record))?.to_owned()
    } else {
        inline_path.to_owned()
    };
    Ok((additions.parse().ok(), deletions.parse().ok(), path))
}

fn malformed(record: &str) -> ProviderError {
    ProviderError::Local(format!("unexpected git diff-tree output: {record:?}"))
}

/// NUL-separated tokens, skipping empty trailing ones.
struct Tokens<'a> {
    parts: std::str::Split<'a, char>,
}

impl<'a> Tokens<'a> {
    fn new(output: &'a str) -> Self {
        Self { parts: output.split('\0') }
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        self.parts.by_ref().find(|part| !part.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "1111111111111111111111111111111111111111";
    const NEW: &str = "2222222222222222222222222222222222222222";
    const ZERO: &str = "0000000000000000000000000000000000000000";

    #[test]
    fn parses_raw_records() {
        let output = format!(
            ":100644 100644 {OLD} {NEW} M\0src/a.rs\0:000000 100644 {ZERO} {NEW} A\0tests/b.rs\0:100644 000000 {OLD} {ZERO} D\0README.md\0"
        );
        let files = parse_raw(&output).unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].kind, ChangeKind::Modified);
        assert_eq!(files[1].category, FileCategory::Tests);
        assert_eq!(files[2].kind, ChangeKind::Deleted);
        assert!(!files[2].has_new_blob());
        assert!(!files[1].has_old_blob());
    }

    #[test]
    fn merges_numstat_including_renames_and_binaries() {
        let output = format!(
            ":100644 100644 {OLD} {NEW} R087\0old/name.rs\0new/name.rs\0:100644 100644 {OLD} {NEW} M\0logo.png\0:100644 100644 {OLD} {NEW} M\0src/main.rs\0\
             3\t1\t\0old/name.rs\0new/name.rs\0-\t-\tlogo.png\010\t2\tsrc/main.rs\0"
        );
        let files = parse_raw_with_numstat(&output).unwrap();
        assert_eq!(files[0].path, "new/name.rs");
        assert_eq!(files[0].old_path.as_deref(), Some("old/name.rs"));
        assert_eq!((files[0].additions, files[0].deletions), (Some(3), Some(1)));
        assert!(files[1].is_binary);
        assert_eq!(files[2].additions, Some(10));
        assert!(!files[2].is_binary);
    }

    #[test]
    fn marks_files_that_whitespace_ignoring_counts_leave_out() {
        let output = format!(
            ":100644 100644 {OLD} {NEW} R087\0old/name.rs\0new/name.rs\0:100644 100644 {OLD} {NEW} M\0logo.png\0:100644 100644 {OLD} {NEW} M\0src/main.rs\0:100644 100755 {OLD} {OLD} M\0run.sh\0\
             3\t1\t\0old/name.rs\0new/name.rs\0-\t-\tlogo.png\010\t2\tsrc/main.rs\00\t0\trun.sh\0"
        );
        let mut files = parse_raw_with_numstat(&output).unwrap();
        // Only the rename has other than whitespace changes.
        let ignoring_whitespace = "2\t1\t\0old/name.rs\0new/name.rs\0-\t-\tlogo.png\0";
        mark_whitespace_only(&mut files, ignoring_whitespace).unwrap();
        let marked: Vec<(&str, bool)> =
            files.iter().map(|file| (file.path.as_str(), file.whitespace_only)).collect();
        assert_eq!(
            marked,
            [("new/name.rs", false), ("logo.png", false), ("src/main.rs", true), ("run.sh", false)]
        );
    }

    #[test]
    fn counts_only_the_files_of_a_chunk() {
        let output = format!(
            ":100644 100644 {OLD} {NEW} M\0a.rs\0:100644 100644 {OLD} {NEW} M\0logo.png\0:100644 100644 {OLD} {NEW} M\0b.rs\0:100644 100644 {OLD} {NEW} M\0c.rs\0"
        );
        let mut files = parse_raw(&output).unwrap();
        let counts = "4\t2\ta.rs\0-\t-\tlogo.png\x003\t3\tb.rs\0";
        // b.rs changed only whitespace; c.rs is in another chunk.
        apply_counts(&mut files, &[0, 1, 2], counts, "4\t1\ta.rs\0-\t-\tlogo.png\0").unwrap();
        let counted: Vec<(Option<u32>, bool, bool)> = files
            .iter()
            .map(|file| (file.additions, file.is_binary, file.whitespace_only))
            .collect();
        assert_eq!(
            counted,
            [
                (Some(4), false, false),
                (None, true, false),
                (Some(3), false, true),
                (None, false, false)
            ]
        );
    }

    #[test]
    fn a_renamed_pair_takes_the_place_of_its_added_file() {
        let listed = format!(
            ":100644 000000 {OLD} {ZERO} D\0b/old.rs\0:000000 100644 {ZERO} {NEW} A\0a/new.rs\0:100644 100644 {OLD} {NEW} M\0c.rs\0:100644 000000 {OLD} {ZERO} D\0gone.rs\0"
        );
        let files = parse_raw(&listed).unwrap();
        let renamed = parse_raw_with_numstat(&format!(
            ":100644 100644 {OLD} {NEW} R090\0b/old.rs\0a/new.rs\01\t1\t\0b/old.rs\0a/new.rs\0"
        ))
        .unwrap();
        let paired = pair_renames(files, renamed);
        let listed: Vec<(&str, ChangeKind, Option<u32>)> =
            paired.iter().map(|file| (file.path.as_str(), file.kind, file.additions)).collect();
        assert_eq!(
            listed,
            [
                ("a/new.rs", ChangeKind::Renamed, Some(1)),
                ("c.rs", ChangeKind::Modified, None),
                ("gone.rs", ChangeKind::Deleted, None)
            ]
        );
    }

    #[test]
    fn tells_same_content_from_changed_content() {
        let mut file =
            parse_raw(&format!(":100644 100755 {OLD} {OLD} M\0run.sh\0")).unwrap().remove(0);
        assert!(file.has_same_content(), "a mode change");
        file.new_oid = NEW.to_owned();
        assert!(!file.has_same_content());
        let empty = parse_raw(&format!(
            ":000000 100644 {ZERO} e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 A\0empty.txt\0"
        ))
        .unwrap();
        assert!(empty[0].has_same_content(), "an empty file added");
    }
}
