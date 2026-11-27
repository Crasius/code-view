//! A block's inline content: runs of text with equal marks, atoms as one-char runs.

use std::ops::Range;
use std::sync::Arc;

use super::{ATOM_CHAR, Atom, Link, Marks};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Run {
    /// For an atom, one `ATOM_CHAR`.
    pub text: String,
    pub marks: Marks,
    pub atom: Option<Atom>,
}

impl Run {
    fn chars(&self) -> usize {
        self.text.chars().count()
    }
}

/// The inline content of a block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub(crate) struct Inline {
    runs: Vec<Run>,
}

impl Inline {
    pub(crate) fn plain(text: &str) -> Self {
        let mut inline = Self::default();
        inline.push_text(text, &Marks::default());
        inline
    }

    pub(crate) fn runs(&self) -> &[Run] {
        &self.runs
    }

    pub(crate) fn push_text(&mut self, text: &str, marks: &Marks) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.runs.last_mut()
            && last.atom.is_none()
            && last.marks == *marks
        {
            last.text.push_str(text);
            return;
        }
        self.runs.push(Run { text: text.to_owned(), marks: marks.clone(), atom: None });
    }

    pub(crate) fn push_atom(&mut self, atom: Atom, marks: &Marks) {
        self.runs.push(Run { text: ATOM_CHAR.to_string(), marks: marks.clone(), atom: Some(atom) });
    }

    fn push_run(&mut self, run: Run) {
        match run.atom {
            Some(atom) => self.push_atom(atom, &run.marks),
            None => self.push_text(&run.text, &run.marks),
        }
    }

    /// Length in chars.
    pub(crate) fn len(&self) -> usize {
        self.runs.iter().map(Run::chars).sum()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// The text, atoms as `ATOM_CHAR`.
    pub(crate) fn text(&self) -> String {
        self.runs.iter().map(|run| run.text.as_str()).collect()
    }

    /// Appends `other`, merging the runs where they meet.
    pub(crate) fn append(&mut self, other: Inline) {
        for run in other.runs {
            self.push_run(run);
        }
    }

    /// The content without its marks, for a block whose kind changes between text and source:
    /// atoms stay atoms (a mention moved into a code block is still written as its source, never
    /// as the placeholder char) and each text run passes through `text`.
    pub(crate) fn unmarked(&self, text: impl Fn(&str) -> String) -> Inline {
        let mut out = Inline::default();
        for run in &self.runs {
            match &run.atom {
                Some(atom) => out.push_atom(atom.clone(), &Marks::default()),
                None => out.push_text(&text(&run.text), &Marks::default()),
            }
        }
        out
    }

    /// The part `range` (chars) as its own inline.
    pub(crate) fn slice(&self, range: Range<usize>) -> Inline {
        let mut out = Inline::default();
        let mut start = 0;
        for run in &self.runs {
            let len = run.chars();
            let end = start + len;
            let from = range.start.max(start);
            let to = range.end.min(end);
            if from < to {
                if run.atom.is_some() {
                    out.push_run(run.clone());
                } else {
                    let text: String =
                        run.text.chars().skip(from - start).take(to - from).collect();
                    out.push_text(&text, &run.marks);
                }
            }
            start = end;
        }
        out
    }

    /// Cuts the inline at `at` (chars); returns the part after it.
    pub(crate) fn split_off(&mut self, at: usize) -> Inline {
        let len = self.len();
        let tail = self.slice(at.min(len)..len);
        *self = self.slice(0..at.min(len));
        tail
    }

    /// Removes the chars in `range`.
    pub(crate) fn remove(&mut self, range: Range<usize>) {
        let len = self.len();
        let mut kept = self.slice(0..range.start.min(len));
        kept.append(self.slice(range.end.min(len)..len));
        *self = kept;
    }

    /// Inserts `other` at char `at`.
    pub(crate) fn insert(&mut self, at: usize, other: Inline) {
        let tail = self.split_off(at);
        self.append(other);
        self.append(tail);
    }

    /// The marks text typed at `offset` takes: those of the char before it (the first char at
    /// the start). A link does not grow at its end, as in the web editors.
    pub(crate) fn marks_at(&self, offset: usize) -> Marks {
        let before = offset.checked_sub(1).and_then(|index| self.run_index(index));
        let Some((index, _)) = before.or_else(|| self.run_index(offset)) else {
            return Marks::default();
        };
        let run = &self.runs[index];
        let mut marks = run.marks.clone();
        let after = self.run_index(offset).map(|(after, _)| after);
        if marks.link.is_some()
            && after.is_none_or(|after| self.runs[after].marks.link != marks.link)
        {
            marks.link = None;
        }
        marks
    }

    /// The run holding char `index` and the char offset that run starts at.
    pub(crate) fn run_index(&self, index: usize) -> Option<(usize, usize)> {
        let mut start = 0;
        for (position, run) in self.runs.iter().enumerate() {
            let end = start + run.chars();
            if index < end {
                return Some((position, start));
            }
            start = end;
        }
        None
    }

    /// Changes the marks of every char in `range`.
    pub(crate) fn map_marks(&mut self, range: Range<usize>, change: impl Fn(&mut Marks)) {
        let len = self.len();
        let (start, end) = (range.start.min(len), range.end.min(len));
        let mut middle = self.slice(start..end);
        for run in &mut middle.runs {
            change(&mut run.marks);
        }
        let mut rebuilt = self.slice(0..start);
        rebuilt.append(middle);
        rebuilt.append(self.slice(end..len));
        *self = rebuilt;
    }

    /// Whether `range` holds chars and every one of them passes `test`.
    pub(crate) fn all_marked(&self, range: Range<usize>, test: impl Fn(&Marks) -> bool) -> bool {
        let slice = self.slice(range);
        !slice.runs.is_empty() && slice.runs.iter().all(|run| test(&run.marks))
    }

    /// The char range of the link around char `index`, as far as the same link reaches.
    pub(crate) fn link_range(&self, index: usize) -> Option<(Range<usize>, Arc<Link>)> {
        let (position, run_start) = self.run_index(index)?;
        let link = self.runs[position].marks.link.clone()?;
        let is_same = |run: &Run| run.marks.link.as_deref() == Some(&*link);
        let mut start = run_start;
        for run in self.runs[..position].iter().rev().take_while(|run| is_same(run)) {
            start -= run.chars();
        }
        let end = run_start
            + self.runs[position..]
                .iter()
                .take_while(|run| is_same(run))
                .map(Run::chars)
                .sum::<usize>();
        Some((start..end, link))
    }
}
