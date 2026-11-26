//! Links in rendered text: which glyph the pointer is on, and the right-click menu of a link.

use cv_base::theme::icons;
use cv_widgets::glyph;
use egui::Ui;

use crate::{InApp, ResolvedLink, request_in_app};

/// The right-click menu of a link: both ways to open it, and its address.
pub(super) fn link_menu(ui: &mut Ui, target: &str, link: &ResolvedLink) {
    if link.in_app != InApp::No
        && glyph::button(ui, icons::GIT_PULL_REQUEST, "Open in Code View").clicked()
    {
        request_in_app(ui.ctx(), target);
        ui.close();
    }
    if glyph::button(ui, icons::ARROW_SQUARE_OUT, "Open in browser").clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(&link.web_url));
        ui.close();
    }
    if glyph::button(ui, icons::LINK, "Copy link").clicked() {
        ui.ctx().copy_text(link.web_url.clone());
        ui.close();
    }
}

pub(super) fn is_url(target: &str) -> bool {
    target.starts_with("https://") || target.starts_with("http://")
}

/// The byte offset of the glyph under `point`, relative to the galley's top left.
pub(super) fn byte_at(galley: &egui::Galley, point: egui::Vec2) -> Option<usize> {
    let mut chars_before = 0;
    for placed in &galley.rows {
        let row_rect = placed.rect();
        if point.y >= row_rect.top() && point.y < row_rect.bottom() {
            let x = point.x - placed.pos.x;
            let column =
                placed.row.glyphs.iter().position(|glyph| x >= glyph.pos.x && x < glyph.max_x())?;
            return galley.job.text.char_indices().nth(chars_before + column).map(|(byte, _)| byte);
        }
        chars_before +=
            placed.row.char_count_excluding_newline().0 + usize::from(placed.ends_with_newline);
    }
    None
}
