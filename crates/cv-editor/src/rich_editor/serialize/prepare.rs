//! Inline content made ready to write: platform fallbacks, line breaks the context cannot hold,
//! trimmed lines and emphasis kept off edge spaces; and the check that written text reads back.

use review_core::ProviderKind;

use super::inline::{Context, run_source};
use crate::rich_editor::model::{ATOM_CHAR, Atom, HARD_BREAK, Inline, Mark, Marks, Run};
use crate::rich_editor::parse;

/// Whether `written` parses back to `runs`.
pub fn reads_back(written: &str, runs: &[Run], platform: ProviderKind) -> bool {
    let parsed = parse::parse(written, platform);
    let Some(block) = parsed.blocks.first().filter(|_| parsed.blocks.len() == 1) else {
        return runs.is_empty();
    };
    flatten(block.content.runs(), platform) == flatten(runs, platform)
}

/// Chars with their marks, atoms as their source, for comparing a write with its read.
fn flatten(runs: &[Run], platform: ProviderKind) -> Vec<(char, Marks)> {
    let mut chars = Vec::new();
    for run in runs {
        chars.extend(run_source(run, platform).chars().map(|ch| (ch, run.marks.clone())));
    }
    chars
}

/// Runs ready to write: line breaks a heading or cell cannot hold become spaces, whitespace at
/// line edges goes (Markdown drops it), emphasis leaves the whitespace at its edges (a delimiter
/// next to a space does not open or close), and a key becomes code where `<kbd>` is stripped.
pub fn prepared(inline: &Inline, platform: ProviderKind, context: Context) -> Vec<Run> {
    let mut units: Vec<(char, Marks, Option<Atom>)> = Vec::with_capacity(inline.len());
    for run in inline.runs() {
        let mut marks = run.marks.clone();
        if marks.has(Mark::Kbd) && platform == ProviderKind::AzureDevOps {
            marks = marks.without(Mark::Kbd).with(Mark::Code);
        }
        if let Some(atom) = &run.atom {
            units.push((ATOM_CHAR, marks, Some(atom.clone())));
            continue;
        }
        for ch in run.text.chars() {
            let ch = match ch {
                '\n' | HARD_BREAK if context != Context::Text || marks.has(Mark::Code) => ' ',
                other => other,
            };
            units.push((ch, marks.clone(), None));
        }
    }
    trim_lines(&mut units);
    for mark in [Mark::Bold, Mark::Italic, Mark::Strike] {
        let mut start = 0;
        while start < units.len() {
            if !units[start].1.has(mark) {
                start += 1;
                continue;
            }
            let end =
                (start..units.len()).find(|&at| !units[at].1.has(mark)).unwrap_or(units.len());
            let leading = units[start..end].iter().take_while(|unit| is_space(unit.0)).count();
            let trailing =
                units[start..end].iter().rev().take_while(|unit| is_space(unit.0)).count();
            for at in (start..start + leading).chain(end - trailing..end) {
                units[at].1.set(mark, false);
            }
            start = end;
        }
    }
    let mut out = Inline::default();
    for (ch, marks, atom) in units {
        match atom {
            Some(atom) => out.push_atom(atom, &marks),
            None => out.push_text(ch.encode_utf8(&mut [0; 4]), &marks),
        }
    }
    out.runs().to_vec()
}

fn is_space(ch: char) -> bool {
    ch == ' ' || ch == '\t' || ch == '\n' || ch == HARD_BREAK
}

/// Drops spaces at the start and end of the text and of every line, and line breaks at the ends.
fn trim_lines(units: &mut Vec<(char, Marks, Option<Atom>)>) {
    let is_blank = |unit: &(char, Marks, Option<Atom>)| {
        unit.2.is_none() && !unit.1.has(Mark::Code) && matches!(unit.0, ' ' | '\t')
    };
    let mut kept = Vec::with_capacity(units.len());
    let mut at_line_start = true;
    for unit in units.drain(..) {
        let is_break = unit.2.is_none() && (unit.0 == '\n' || unit.0 == HARD_BREAK);
        if is_break {
            while kept.last().is_some_and(is_blank) {
                kept.pop();
            }
            if !at_line_start {
                kept.push(unit);
            }
            at_line_start = true;
            continue;
        }
        if at_line_start && is_blank(&unit) {
            continue;
        }
        at_line_start = false;
        kept.push(unit);
    }
    while kept.last().is_some_and(|last| {
        is_blank(last) || (last.2.is_none() && matches!(last.0, '\n' | HARD_BREAK))
    }) {
        kept.pop();
    }
    *units = kept;
}
