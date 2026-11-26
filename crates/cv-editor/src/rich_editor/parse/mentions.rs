//! Mentions in parsed text: `@login` on GitHub and GitLab, `@<id>` on Azure DevOps.

// RS-DONT-04: reads pull request comments, so indexing, slicing and narrowing are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing, clippy::cast_possible_truncation)]

use review_core::ProviderKind;

use crate::rich_editor::model::{ATOM_CHAR, Atom, Inline, Mark, Marks};

/// Turns `@login` (GitHub, GitLab) and `@<id>` (Azure DevOps) in plain text into mentions. An
/// Azure DevOps id that starts with a letter reads as an HTML tag, so `@` before such a raw tag
/// is a mention too.
pub fn find_mentions(inline: &mut Inline, platform: ProviderKind) {
    let has_at = inline.runs().iter().any(|run| run.atom.is_none() && run.text.contains('@'));
    if !has_at {
        return;
    }
    let mut out = Inline::default();
    let mut previous: Option<char> = None;
    let runs = inline.runs().to_vec();
    let mut index = 0;
    while let Some(run) = runs.get(index) {
        index += 1;
        let is_plain_text =
            run.atom.is_none() && !run.marks.has(Mark::Code) && run.marks.link.is_none();
        if !is_plain_text {
            match &run.atom {
                Some(atom) => out.push_atom(atom.clone(), &run.marks),
                None => out.push_text(&run.text, &run.marks),
            }
            previous = run.text.chars().last();
            continue;
        }
        let mut text = run.text.as_str();
        // `@` closing this run, followed by a raw tag that is an Azure DevOps identity.
        let next_id = runs.get(index).and_then(|next| match &next.atom {
            Some(Atom::Raw(raw)) if platform == ProviderKind::AzureDevOps => identity_tag(raw),
            _ => None,
        });
        let takes_tag = next_id.is_some() && text.ends_with('@') && {
            let before = text.strip_suffix('@').and_then(|rest| rest.chars().last());
            starts_mention(before.or(previous))
        };
        if takes_tag {
            text = text.strip_suffix('@').unwrap_or(text);
        }
        push_with_mentions(&mut out, text, &run.marks, previous, platform);
        if let Some(key) = next_id.filter(|_| takes_tag) {
            out.push_atom(Atom::Mention { key }, &run.marks);
            index += 1;
        }
        previous = run.text.chars().last();
    }
    *inline = out;
}

/// `<id>` of an Azure DevOps identity: hex digits and hyphens.
fn identity_tag(raw: &str) -> Option<String> {
    let id = raw.strip_prefix('<')?.strip_suffix('>')?;
    let is_id = id.len() >= 8 && id.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-');
    is_id.then(|| id.to_owned())
}

fn starts_mention(before: Option<char>) -> bool {
    before.is_none_or(|ch| !(ch.is_alphanumeric() || matches!(ch, '_' | '@' | '.' | '/' | '`')))
}

fn push_with_mentions(
    out: &mut Inline,
    text: &str,
    marks: &Marks,
    mut previous: Option<char>,
    platform: ProviderKind,
) {
    let mut plain_start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        if ch == '@' && starts_mention(previous) {
            let rest = text.get(at + 1..).unwrap_or_default();
            if let Some(key_len) = mention_key_len(rest, platform) {
                out.push_text(text.get(plain_start..at).unwrap_or_default(), marks);
                let key = rest.get(..key_len).unwrap_or_default();
                let key = key.strip_prefix('<').and_then(|id| id.strip_suffix('>')).unwrap_or(key);
                out.push_atom(Atom::Mention { key: key.to_owned() }, marks);
                plain_start = at + 1 + key_len;
                while chars.peek().is_some_and(|(next, _)| *next < plain_start) {
                    chars.next();
                }
                previous = Some(ATOM_CHAR);
                continue;
            }
        }
        previous = Some(ch);
    }
    out.push_text(text.get(plain_start..).unwrap_or_default(), marks);
}

/// The length in bytes of the mention key after an `@`, if one is there.
fn mention_key_len(rest: &str, platform: ProviderKind) -> Option<usize> {
    if platform == ProviderKind::AzureDevOps {
        let end = rest.find('>')?;
        let id = rest.get(1..end)?;
        let is_id = rest.starts_with('<')
            && id.len() >= 8
            && id.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-');
        return is_id.then_some(end + 1);
    }
    if !rest.chars().next().is_some_and(|ch| ch.is_ascii_alphanumeric()) {
        return None;
    }
    let allows_dot = platform == ProviderKind::GitLab;
    let len = rest
        .find(|ch: char| {
            !(ch.is_ascii_alphanumeric()
                || matches!(ch, '-' | '_' | '/')
                || (allows_dot && ch == '.'))
        })
        .unwrap_or(rest.len());
    let key = rest.get(..len)?.trim_end_matches(['.', '-', '/']);
    (!key.is_empty()).then_some(key.len())
}
