//! The loaded bodies of the cards near the view: the diff, the diff view showing its compacted
//! rows (with their comments) and syntax colors. A bounded least-recently-shown cache: scrolling on drops the bodies left behind
//! (their exact heights stay known), so memory follows the screen, not the size of the pull
//! request. Laid-out text is never kept here; egui's text cache drops what a frame did not draw.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use review_git::{FileDiff, LineKind};

use super::layout::{self, Shape, Shapes};
use cv_code::diff_rows::{self, Row};
use cv_pr_diff::diff_view::DiffView;
use cv_syntax::highlight::LineSpans;

/// Bodies kept at most: more than the cards one screen and the screens around it hold.
pub(crate) const MAX_BODIES: usize = 160;
const MAX_BYTES: usize = 96 * 1024 * 1024;
/// What the diff view keeps per row: the row, its offset, its columns and changed words.
const ROW_BYTES: usize = std::mem::size_of::<Row>() + 16;
/// The first line of a Git LFS pointer file.
const LFS_POINTER: &str = "version https://git-lfs.github.com/spec/v1";

/// Why a loaded card shows a placeholder instead of code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placeholder {
    Binary,
    /// A Git LFS pointer: the content lives on the LFS server.
    Lfs,
    /// Renamed, copied or a mode change with the same content.
    Unchanged,
    /// More changed lines than a card shows (see [`layout::INLINE_MAX_CHANGED_LINES`]), found
    /// from the diff itself when it loaded before the file's lines were counted.
    TooLarge {
        changed: u32,
    },
}

pub(crate) struct CardBody {
    pub diff: Arc<FileDiff>,
    /// The file view's diff view in its compact layout (inline or side by side): it paints the
    /// rows and takes comments on them as the file view does.
    pub view: DiffView,
    shapes: Shapes,
    pub placeholder: Option<Placeholder>,
    /// Syntax colors by diff line, shared with the file view when it worked them out.
    pub spans: Option<Arc<LineSpans>>,
    /// The queued coloring, cancelled when the body is dropped.
    pub coloring: Option<Cancel>,
    last_shown: u64,
    /// The bytes the cache counted for this body.
    counted: usize,
}

impl CardBody {
    /// The body of `diff`; `is_forced` when the user asked to see a change above the inline limit.
    pub(crate) fn new(diff: Arc<FileDiff>, context: u32, is_forced: bool) -> Self {
        let placeholder = placeholder_for(&diff, is_forced);
        let shape = |split| {
            if placeholder.is_some() {
                return Shape::Placeholder;
            }
            let (lines, separators) = diff_rows::compact_shape(&diff, context, split);
            Shape::Rows { lines, separators }
        };
        let shapes = Shapes { inline: shape(false), split: shape(true) };
        Self {
            diff,
            view: DiffView::default(),
            shapes,
            placeholder,
            spans: None,
            coloring: None,
            last_shown: 0,
            counted: 0,
        }
    }

    /// The code rows and separators inline and side by side, without comment cards.
    pub(crate) fn shapes(&self) -> Shapes {
        self.shapes
    }

    fn heap_size(&self) -> usize {
        let spans = self.spans.as_ref().map_or(0, |spans| spans.heap_size());
        // The view holds the rows of one layout at a time; inline has the more.
        let rows = match self.shapes.inline {
            Shape::Rows { lines, separators } => (lines + separators) as usize,
            Shape::Placeholder => 0,
        };
        self.diff.heap_size() + rows * ROW_BYTES + spans
    }
}

impl Drop for CardBody {
    fn drop(&mut self) {
        if let Some(coloring) = &self.coloring {
            coloring.cancel();
        }
    }
}

/// Set once the card is gone, so its queued coloring returns at once.
#[derive(Clone, Default)]
pub(crate) struct Cancel(pub Arc<AtomicBool>);

impl Cancel {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn placeholder_for(diff: &FileDiff, is_forced: bool) -> Option<Placeholder> {
    if diff.is_binary {
        return Some(Placeholder::Binary);
    }
    if diff.is_empty() {
        return Some(Placeholder::Unchanged);
    }
    let changed = diff.count(LineKind::Added) + diff.count(LineKind::Deleted);
    let changed = u32::try_from(changed).unwrap_or(u32::MAX);
    if changed > layout::INLINE_MAX_CHANGED_LINES && !is_forced {
        return Some(Placeholder::TooLarge { changed });
    }
    // A pointer file on both sides (or the one side of an added or deleted file).
    let mut changed = diff.lines().iter().filter(|line| {
        matches!(line.kind, LineKind::Added | LineKind::Deleted | LineKind::Context)
    });
    let is_pointer =
        changed.clone().count() <= 8 && changed.any(|line| diff.text(line) == LFS_POINTER);
    is_pointer.then_some(Placeholder::Lfs)
}

/// Card bodies by path, the least recently shown dropped first.
#[derive(Default)]
pub(crate) struct BodyCache {
    entries: HashMap<String, CardBody>,
    bytes: usize,
}

impl BodyCache {
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    /// The body of `path`, marked as shown in `frame`.
    pub(crate) fn show(&mut self, path: &str, frame: u64) -> Option<&mut CardBody> {
        let body = self.entries.get_mut(path)?;
        body.last_shown = frame;
        Some(body)
    }

    pub(crate) fn get_mut(&mut self, path: &str) -> Option<&mut CardBody> {
        self.entries.get_mut(path)
    }

    #[cfg(test)]
    pub fn get(&self, path: &str) -> Option<&CardBody> {
        self.entries.get(path)
    }

    /// Adds a body; then drops the least recently shown ones beyond the limits, never one shown in
    /// `frame`, wanted (`keep`) or with a new comment being written in it.
    pub(crate) fn insert(
        &mut self,
        path: String,
        mut body: CardBody,
        frame: u64,
        keep: &dyn Fn(&str) -> bool,
    ) {
        body.last_shown = frame;
        body.counted = body.heap_size();
        self.bytes += body.counted;
        if let Some(replaced) = self.entries.insert(path, body) {
            self.bytes -= replaced.counted;
        }
        while self.entries.len() > MAX_BODIES || self.bytes > MAX_BYTES {
            let oldest = self
                .entries
                .iter()
                .filter(|(path, body)| {
                    body.last_shown < frame && !keep(path) && body.view.composer().is_none()
                })
                .min_by_key(|(_, body)| body.last_shown)
                .map(|(path, _)| path.clone());
            let Some(oldest) = oldest else {
                break;
            };
            if let Some(evicted) = self.entries.remove(&oldest) {
                self.bytes -= evicted.counted;
            }
        }
    }

    /// Takes out `path`'s body when its diff, loaded before the lines were counted, showed it too
    /// large, so it is built again in full; returns its changed lines.
    pub(crate) fn take_too_large(&mut self, path: &str) -> Option<u32> {
        let Some(Placeholder::TooLarge { changed }) = self.entries.get(path)?.placeholder else {
            return None;
        };
        if let Some(body) = self.entries.remove(path) {
            self.bytes -= body.counted;
        }
        Some(changed)
    }

    /// The spans arrived for `path`: they count towards the memory too.
    pub(crate) fn set_spans(&mut self, path: &str, spans: LineSpans) {
        if let Some(body) = self.entries.get_mut(path) {
            self.bytes -= body.counted;
            body.spans = Some(Arc::new(spans));
            body.coloring = None;
            body.counted = body.heap_size();
            self.bytes += body.counted;
        }
    }

    /// Stops the coloring queued for every body, e.g. while the file view takes the coloring thread.
    pub(crate) fn cancel_coloring(&mut self) {
        for body in self.entries.values_mut() {
            if let Some(coloring) = body.coloring.take() {
                coloring.cancel();
            }
        }
    }

    /// A body waits for its colors.
    #[cfg(test)]
    pub fn is_coloring(&self) -> bool {
        self.entries.values().any(|body| body.coloring.is_some())
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use review_git::FileDiff;

    use super::{BodyCache, CardBody, MAX_BODIES, Placeholder};
    use crate::layout::{INLINE_MAX_CHANGED_LINES, Shape, Shapes};

    fn body(text: &str) -> CardBody {
        CardBody::new(Arc::new(FileDiff::parse_unified(text.as_bytes())), 3, false)
    }

    #[test]
    fn counts_the_compacted_rows_inline_and_side_by_side() {
        let card = body(concat!(
            "@@ -5,3 +5,3 @@\n",
            " a\n",
            "-b\n",
            "+B\n",
            "@@ -60,2 +60,2 @@\n",
            " c\n",
            "-d\n",
            "+D\n"
        ));
        // Inline: a, -b, +B, separator, c, -d, +D. Side by side: a, -b|+B, separator, c, -d|+D.
        let expected = Shapes {
            inline: Shape::Rows { lines: 6, separators: 1 },
            split: Shape::Rows { lines: 4, separators: 1 },
        };
        assert_eq!(card.shapes(), expected);
    }

    #[test]
    fn tells_binary_lfs_and_unchanged_files_apart() {
        let binary = FileDiff::parse_unified(b"Binary files a/x.png and b/x.png differ\n");
        assert_eq!(
            CardBody::new(Arc::new(binary), 3, false).placeholder,
            Some(Placeholder::Binary)
        );
        assert_eq!(
            CardBody::new(Arc::new(FileDiff::default()), 3, false).placeholder,
            Some(Placeholder::Unchanged)
        );
        let pointer = body(
            "@@ -1,3 +1,3 @@\n version https://git-lfs.github.com/spec/v1\n-oid sha256:aaa\n+oid sha256:bbb\n size 12\n",
        );
        assert_eq!(pointer.placeholder, Some(Placeholder::Lfs));
        assert_eq!(
            pointer.shapes(),
            Shapes { inline: Shape::Placeholder, split: Shape::Placeholder }
        );
    }

    #[test]
    fn a_change_above_the_inline_limit_is_too_large_unless_the_user_asked() {
        let changed = INLINE_MAX_CHANGED_LINES + 1;
        let added = "+line\n".repeat(changed as usize);
        let diff = Arc::new(FileDiff::parse_unified(
            format!("@@ -0,0 +1,{changed} @@\n{added}").as_bytes(),
        ));
        let card = CardBody::new(diff.clone(), 3, false);
        assert_eq!(card.placeholder, Some(Placeholder::TooLarge { changed }));
        assert_eq!(card.shapes(), Shapes { inline: Shape::Placeholder, split: Shape::Placeholder });
        assert_eq!(CardBody::new(diff, 3, true).placeholder, None, "Show here shows it");
    }

    #[test]
    fn drops_the_least_recently_shown_bodies_but_never_wanted_ones() {
        let mut cache = BodyCache::default();
        for index in 0..MAX_BODIES + 40 {
            cache.insert(
                format!("f{index}"),
                body("@@ -1 +1 @@\n-a\n+b\n"),
                index as u64,
                &|path| path == "f0",
            );
        }
        assert_eq!(cache.len(), MAX_BODIES);
        assert!(cache.contains("f0"), "a wanted body stays");
        assert!(!cache.contains("f1") && cache.contains(&format!("f{}", MAX_BODIES + 39)));
    }
}
