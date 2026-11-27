//! Where the cards and their rows lie in the list's scroll area, drawn or not: the list paints
//! from it, and J and K ([`super::keys`]) scroll to a row that is not drawn yet; and where a row's
//! columns lie across a card ([`Columns`]).

use cv_base::theme::style;
use egui::{Pos2, Rect, pos2, vec2};

use super::{
    AVATAR_SIZE, COLUMN_GAP, FILES_BUTTON, FILES_GAP, FILES_INSET, GROUP_GAP, Group, HEADER_HEIGHT,
    REPOSITORY_MIN_CARD, REPOSITORY_WIDTH, ROW_PADDING, Row, SIDE_MARGIN, SIZE_WIDTH,
    STATUS_MIN_CARD, STATUS_WIDTH, indicators, row_height,
};

/// The cards' column in the scroll area: where the first card starts and how wide the area is.
#[derive(Clone, Copy)]
pub(super) struct Column {
    pub origin: Pos2,
    pub width: f32,
}

impl Column {
    /// The rect of a card whose top is at `top`.
    pub(crate) fn card(self, top: f32, group: &Group<'_>) -> Rect {
        Rect::from_min_size(
            pos2(self.origin.x + SIDE_MARGIN, top),
            vec2(self.width - SIDE_MARGIN * 2.0 - style::SCROLL_GUTTER, group.height()),
        )
    }

    /// Where the focusable row at `place` lies, drawn or not.
    pub(crate) fn row(self, groups: &[Group<'_>], place: usize) -> Option<Rect> {
        let mut top = self.origin.y + GROUP_GAP;
        let mut before = 0;
        for group in groups {
            let card = self.card(top, group);
            top = card.bottom() + GROUP_GAP;
            let rows = group.focusable_rows();
            if place >= before + rows {
                before += rows;
                continue;
            }
            let mut row_top = card.top() + HEADER_HEIGHT;
            for row in &group.body {
                let height = row_height(row);
                if matches!(row, Row::Item(..)) {
                    if before == place {
                        return Some(Rect::from_min_size(
                            pos2(card.left(), row_top),
                            vec2(card.width(), height),
                        ));
                    }
                    before += 1;
                }
                row_top += height;
            }
        }
        None
    }
}

/// Horizontal spans of a row's columns for one card width.
pub(super) struct Columns {
    pub text: (f32, f32),
    pub repository: Option<(f32, f32)>,
    pub size: Option<(f32, f32)>,
    pub status: Option<(f32, f32)>,
    pub reviewers: (f32, f32),
    pub checks: (f32, f32),
    /// The checks column is wide enough for "passed/all" after its icon.
    pub checks_count: bool,
    pub files: (f32, f32),
}

impl Columns {
    pub(crate) fn for_row(rect: Rect) -> Self {
        let files_right = rect.right() - FILES_INSET;
        let files = (files_right - FILES_BUTTON, files_right);
        let mut right = files.0 - FILES_GAP;
        let mut take = |width: f32| {
            let span = (right - width, right);
            right -= width + COLUMN_GAP;
            span
        };
        let checks_count = rect.width() >= STATUS_MIN_CARD;
        let checks =
            take(if checks_count { indicators::CHECKS_WIDE } else { indicators::CHECKS_NARROW });
        let reviewers = take(indicators::APPROVALS_WIDTH);
        let status = (rect.width() >= STATUS_MIN_CARD).then(|| take(STATUS_WIDTH));
        let size = (rect.width() >= REPOSITORY_MIN_CARD).then(|| take(SIZE_WIDTH));
        let repository = (rect.width() >= REPOSITORY_MIN_CARD).then(|| take(REPOSITORY_WIDTH));
        let text_left = rect.left() + ROW_PADDING + AVATAR_SIZE + style::SPACE_M + 2.0;
        Self {
            text: (text_left, right + COLUMN_GAP - style::SPACE_L),
            repository,
            size,
            status,
            reviewers,
            checks,
            checks_count,
            files,
        }
    }
}
