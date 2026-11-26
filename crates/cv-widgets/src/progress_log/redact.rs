//! Credentials out of a log line before it reaches the screen or "Copy log".

// RS-DONT-04: parses git output, so indexing, string slicing and narrowing casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing, clippy::cast_possible_truncation)]

const REDACTED: &str = "<redacted>";

/// Removes credentials from a log line: userinfo in URLs, `Authorization` and `extraHeader`
/// values, `-c` config values, `token=` parameters, `Bearer`/`Basic` credentials and
/// token-shaped words (GitHub and GitLab token prefixes).
pub fn redact(text: &str) -> String {
    let text = redact_userinfo(text);
    let text = redact_after_markers(&text, &["authorization", "extraheader"]);
    let text = redact_config_flags(&text);
    let text = redact_token_parameters(&text);
    redact_credential_words(&text)
}

/// `https://user:token@host/path` → `https://host/path`.
fn redact_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find("://") {
        let (head, tail) = rest.split_at(index + 3);
        out.push_str(head);
        let end = tail
            .find(|character: char| {
                character == '/'
                    || character.is_whitespace()
                    || matches!(character, '\'' | '"' | '>' | ')' | ']')
            })
            .unwrap_or(tail.len());
        let (authority, after) = tail.split_at_checked(end).unwrap_or((tail, ""));
        out.push_str(authority.rsplit_once('@').map_or(authority, |(_, host)| host));
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Everything after a marker (up to a quote or the end of the line) is a secret:
/// `http.extraHeader=Authorization: Basic …`, `Authorization: Bearer …`.
fn redact_after_markers(text: &str, markers: &[&str]) -> String {
    let Some((start, length)) = markers
        .iter()
        .filter_map(|marker| {
            find_ignore_ascii_case(text, marker).map(|index| (index, marker.len()))
        })
        .min()
    else {
        return text.to_owned();
    };
    // Fail closed: a split that cannot happen (the offsets come from ASCII matches) hides the line.
    let Some((head, after_marker)) = text.split_at_checked(start + length) else {
        return REDACTED.to_owned();
    };
    let value = after_marker.trim_start_matches([':', '=', ' ']);
    let separator = after_marker.strip_suffix(value).unwrap_or_default();
    let value_len = value.find(['\'', '"']).unwrap_or(value.len());
    let Some((secret, tail)) = value.split_at_checked(value_len) else {
        return REDACTED.to_owned();
    };
    let mut out = format!("{head}{separator}");
    if !secret.is_empty() {
        out.push_str(REDACTED);
    }
    out.push_str(&redact_after_markers(tail, markers));
    out
}

/// The byte index of the first match of an ASCII `needle`, ignoring ASCII case. An ASCII byte
/// never sits inside a multi-byte char, so the index is a char boundary.
fn find_ignore_ascii_case(haystack: &str, needle: &str) -> Option<usize> {
    let needle = needle.as_bytes();
    if needle.is_empty() {
        return Some(0);
    }
    haystack.as_bytes().windows(needle.len()).position(|window| window.eq_ignore_ascii_case(needle))
}

/// `git -c http.extraHeader=… fetch` → `git -c http.extraHeader=<redacted> fetch`: every `-c`
/// value, whatever its key.
fn redact_config_flags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut words = text.split(' ').peekable();
    let mut is_first = true;
    while let Some(word) = words.next() {
        if !is_first {
            out.push(' ');
        }
        is_first = false;
        out.push_str(word);
        if word == "-c"
            && let Some(key) =
                words.peek().and_then(|setting| setting.split_once('=')).map(|(key, _)| key)
        {
            out.push(' ');
            out.push_str(key);
            out.push('=');
            out.push_str(REDACTED);
            words.next();
        }
    }
    out
}

/// `?access_token=…`, `private_token=…`, `token=…` up to the next `&`, quote or space.
fn redact_token_parameters(text: &str) -> String {
    const MARKER: &str = "token=";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(found) = find_ignore_ascii_case(rest, MARKER) {
        // Fail closed, as in redact_after_markers.
        let Some((head, after)) = rest.split_at_checked(found + MARKER.len()) else {
            return REDACTED.to_owned();
        };
        let value_len = after
            .find(|character: char| {
                character == '&'
                    || character.is_whitespace()
                    || matches!(character, '\'' | '"' | '#')
            })
            .unwrap_or(after.len());
        let Some((secret, tail)) = after.split_at_checked(value_len) else {
            return REDACTED.to_owned();
        };
        out.push_str(head);
        if !secret.is_empty() {
            out.push_str(REDACTED);
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// Token-shaped words (`ghp_…`, `github_pat_…`, `glpat-…`) and the credential after `Bearer` or
/// `Basic`.
fn redact_credential_words(text: &str) -> String {
    const PREFIXES: [&str; 8] =
        ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_", "glpat-", "gloas-"];
    let mut out = String::with_capacity(text.len());
    let mut previous = "";
    for (index, word) in text.split(' ').enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let bare = word.trim_matches(|character: char| {
            matches!(character, '\'' | '"' | ',' | ';' | '(' | ')')
        });
        let is_token =
            PREFIXES.iter().any(|prefix| bare.starts_with(prefix) && bare.len() > prefix.len() + 8);
        let follows_scheme =
            previous.eq_ignore_ascii_case("bearer") || previous.eq_ignore_ascii_case("basic");
        if is_token || (follows_scheme && bare.len() >= 12) {
            out.push_str(&word.replacen(bare, REDACTED, 1));
        } else {
            out.push_str(word);
        }
        previous = bare;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn credentials_are_redacted() {
        let cases = [
            (
                "Remote https://user:ghp_secret0123456789@github.com/cli/cli.git",
                "Remote https://github.com/cli/cli.git",
            ),
            (
                "fatal: unable to access 'https://oauth2:glpat-abcdef123456@gitlab.com/g/p.git/': 403",
                "fatal: unable to access 'https://gitlab.com/g/p.git/': 403",
            ),
            (
                "https://pat@dev.azure.com/org/project/_git/repo",
                "https://dev.azure.com/org/project/_git/repo",
            ),
            (
                "$ git -c http.extraHeader=Authorization: Basic OnBhdA== fetch origin",
                "$ git -c http.extraHeader=<redacted>",
            ),
            (
                "$ git -c core.askPass=secret fetch --progress origin",
                "$ git -c core.askPass=<redacted> fetch --progress origin",
            ),
            ("Authorization: Bearer abc.def.ghi", "Authorization: <redacted>"),
            (
                "GIT_CONFIG_VALUE_8='AUTHORIZATION: basic dXNlcjpwYXNz' set",
                "GIT_CONFIG_VALUE_8='AUTHORIZATION: <redacted>' set",
            ),
            (
                "https://gitlab.com/api/v4/projects?private_token=abc123&page=2",
                "https://gitlab.com/api/v4/projects?private_token=<redacted>&page=2",
            ),
            ("token github_pat_11ABCDEFG0123456789 leaked", "token <redacted> leaked"),
            ("sent Bearer eyJhbGciOiJIUzI1NiJ9 to the host", "sent Bearer <redacted> to the host"),
        ];
        for (input, expected) in cases {
            assert_eq!(redact(input), expected, "{input}");
        }
        for harmless in [
            "$ git fetch --progress --no-tags origin +refs/pull/12/head:refs/review/pull/12/head",
            "Receiving objects: 64% (41,200/64,380)",
            "Mirror C:\\Cache\\github.com_cli_cli.git",
        ] {
            assert_eq!(redact(harmless), harmless);
        }
    }

    /// git prints localized messages and user paths; multi-byte text around a marker keeps the
    /// offsets of the case-insensitive matches valid.
    #[test]
    fn multi_byte_text_around_markers_is_kept() {
        assert_eq!(
            redact("Zugriff verweigert für „repo“: Authorization: Bearer äöü-secret 'done'"),
            "Zugriff verweigert für „repo“: Authorization: <redacted>'done'"
        );
        assert_eq!(
            redact("Größe 3 MiB ?TOKEN=geheim€&page=2 Übertragung"),
            "Größe 3 MiB ?TOKEN=<redacted>&page=2 Übertragung"
        );
    }
}
