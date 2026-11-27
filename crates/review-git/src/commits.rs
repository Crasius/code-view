//! Commit list of a pull request from `git log`.

// RS-DONT-04: the input is git's output; parsing it never indexes.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

/// Unit separator between fields, record separator between commits.
pub(crate) const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s%x1e";
const FIELD_SEPARATOR: char = '\u{1f}';
const RECORD_SEPARATOR: char = '\u{1e}';

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub sha: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    /// Unix seconds.
    pub authored_at: i64,
    pub subject: String,
}

pub(crate) fn parse(output: &[u8]) -> Vec<CommitInfo> {
    let text = String::from_utf8_lossy(output);
    text.split(RECORD_SEPARATOR)
        .filter_map(|record| parse_record(record.trim_start_matches('\n')))
        .collect()
}

fn parse_record(record: &str) -> Option<CommitInfo> {
    let mut fields = record.split(FIELD_SEPARATOR);
    let sha = fields.next().filter(|sha| !sha.is_empty())?.to_owned();
    let parents =
        fields.next()?.split(' ').filter(|parent| !parent.is_empty()).map(str::to_owned).collect();
    let author_name = fields.next()?.to_owned();
    let author_email = fields.next()?.to_owned();
    let authored_at = fields.next()?.parse().ok()?;
    let subject = fields.next()?.to_owned();
    Some(CommitInfo { sha, parents, author_name, author_email, authored_at, subject })
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_log_records() {
        let output = "aaa\u{1f}p1 p2\u{1f}Ada\u{1f}ada@x.org\u{1f}1700000000\u{1f}Merge things\u{1e}\nbbb\u{1f}p3\u{1f}Bob\u{1f}bob@x.org\u{1f}1700000001\u{1f}Fix\u{1e}\n";
        let commits = parse(output.as_bytes());
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, ["p1", "p2"]);
        assert_eq!(commits[1].subject, "Fix");
        assert_eq!(commits[1].authored_at, 1_700_000_001);
    }
}
