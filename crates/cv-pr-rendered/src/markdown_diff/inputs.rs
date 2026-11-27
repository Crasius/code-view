//! What the rendered Markdown diff takes each frame ([`RenderedInputs`], [`RenderedImages`]) and
//! what it hands back ([`RenderedEvent`]).

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use review_core::{DiffSide, ProviderKind, ReviewThread};
use review_store::DiffLayout;

use super::MarkdownModel;
use cv_markdown::{ImageBase, RenderOptions};
use cv_pr_threads::ThreadEvent;

pub struct RenderedInputs<'a> {
    /// Both versions parsed and lined up, by the load of the file.
    pub model: &'a Arc<MarkdownModel>,
    pub platform: ProviderKind,
    pub layout: DiffLayout,
    /// Unchanged blocks kept around each change; [`FULL_FILE_CONTEXT`] folds nothing.
    ///
    /// [`FULL_FILE_CONTEXT`]: review_git::FULL_FILE_CONTEXT
    pub context: u32,
    /// The code zoom: type, spacing and markers grow with it.
    pub scale: f32,
    /// The user may comment: a block's gutter button opens a comment on its source lines.
    pub can_comment: bool,
    /// The comment threads on the file.
    pub threads: &'a [&'a ReviewThread],
    /// "Show comments in the diff": the threads' cards show under their blocks. Off, a count in
    /// the gutter stands for them and shows them there on a click.
    pub show_threads: bool,
    /// "Scroll past the end": the rows hold room below the last one (see
    /// [`past_end`](cv_pr_diff::diff_overview::past_end)).
    pub past_end: bool,
}

impl RenderedInputs<'_> {
    /// How the blocks render: the platform's Markdown at the code zoom.
    pub(crate) fn render_options(&self) -> RenderOptions {
        RenderOptions {
            platform: Some(self.platform),
            scale: self.scale,
            ..RenderOptions::default()
        }
    }
}

/// How the images of the two versions load and compare.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RenderedImages {
    /// Where relative image paths of the old and of the new version point.
    pub bases: [Option<ImageBase>; 2],
    /// Content ids (blob ids) of the repository images each version shows, by source as written:
    /// a block whose image changed while its source stayed the same counts as edited, so the
    /// two images show side by side.
    pub content: [BTreeMap<String, String>; 2],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderedEvent {
    /// The user finished moving the boundary between Before and After.
    SplitResized,
    /// The user finished resizing the reading column (or reset it, or it switched to full width).
    WidthResized,
    /// Comment on these source lines of a block: the text diff takes over, the lines selected.
    Comment { side: DiffSide, lines: Range<u32> },
    /// Show this source line in the text diff.
    ShowSource { side: DiffSide, line: u32 },
    /// New only: the user clicked a removed blocks' wedge to see them all, side by side.
    ShowSideBySide,
    /// Something done on a thread's card: a reply, a status change.
    Thread(ThreadEvent),
    /// Show the thread in the text diff, where its line shows (the rendered text leaves it out).
    ShowThreadSource { thread_id: String },
    /// The user switched "Scroll past the end" in the overview strip's menu, to this.
    SwitchPastEnd(bool),
}
