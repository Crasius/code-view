//! Repository sections: with the option on, the overview's cards group their rows by repository,
//! each group under a header that names the repository, counts its rows, says when the newest one
//! was updated and opens the repository's own list. Groups follow their newest row, so the card's
//! order (newest first; starred repositories first while the Favorites lane is hidden) carries over
//! to the groups, and the rows keep that order within each. The Favorites card mixes connections,
//! so a group there is one repository of one connection, its header led by the platform mark.
//!
//! A header opens and closes its section (click, Enter, Space); the closed ones are saved
//! ([`review_store::RepositorySections`]). A closed section keeps its header and count; its rows
//! leave the card, and J and K skip them. Sections open and close at once: the card around them
//! does not move otherwise, and a slide of every row below would be the larger change.

use cv_base::theme::{self, Palette, icons, motion, style};
use cv_runtime::route::{Navigator, Route};
use cv_widgets::numbers::counted;
use cv_widgets::{elide, provider_icon, row, time_text};
use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Painter, Rect, Sense, Stroke, Ui, pos2, vec2,
};
use review_core::PullRequestSummary;
use review_store::{RepositorySections, SectionCard, SectionKey};

use crate::pull_request_list::list_rows::header::{MARK_TO_TITLE, paint_caret};
use crate::pull_request_list::list_rows::{
    CHIP_HEIGHT, FILES_BUTTON, FILES_INSET, ListAction, ROW_PADDING, Row, chip_width, paint_chip,
};
use crate::pull_request_list::queries::ConnectionResult;

/// A section header's height; shorter than a row, so it reads as the rows' heading.
pub(crate) const SECTION_HEIGHT: f32 = 40.0;

/// One repository's header in a card.
pub(crate) struct Section<'a> {
    card: SectionCard,
    /// The connection the repository is reached through: its platform mark in the Favorites card.
    result: &'a ConnectionResult,
    repository_key: &'a str,
    /// Its matching rows, without those on their way out.
    count: usize,
    /// Its most recently updated row; `None` while only leaving rows are left in it.
    newest: Option<&'a PullRequestSummary>,
    is_open: bool,
    /// 1, or how much is left of a section whose last rows are leaving: it closes up with them.
    presence: f32,
}

impl Section<'_> {
    pub(crate) fn height(&self) -> f32 {
        SECTION_HEIGHT * self.presence
    }

    fn key(&self) -> SectionKey {
        SectionKey {
            card: self.card,
            connection_id: self.result.connection_id.clone(),
            repository_key: self.repository_key.to_owned(),
        }
    }

    /// The repository, and in the Favorites card, which mixes connections, its connection too.
    fn place(&self) -> String {
        match self.card {
            SectionCard::Favorites => {
                format!("{} on {}", self.repository_key, self.result.connection_name)
            }
            SectionCard::Connection => self.repository_key.to_owned(),
        }
    }

    /// "team/app, 3 pull requests, expanded": what the header says to a screen reader.
    fn accessible_name(&self) -> String {
        let rows = counted(self.count, "pull request", "pull requests");
        let state = if self.is_open { "expanded" } else { "collapsed" };
        format!("{}, {rows}, {state}", self.place())
    }
}

/// The pull request of a row that shows one.
fn listed<'a>(row: &Row<'a>) -> Option<(&'a ConnectionResult, &'a PullRequestSummary)> {
    match row {
        Row::Item(result, pull_request) | Row::Leaving(result, pull_request, _) => {
            Some((result, pull_request))
        }
        _ => None,
    }
}

/// Groups the card's pull request rows (one unbroken run in `body`, leaving rows among them) by
/// repository, each group under its header; a closed section keeps only its header.
pub(crate) fn split<'a>(body: &mut Vec<Row<'a>>, card: SectionCard, sections: &RepositorySections) {
    let Some(start) = body.iter().position(|row| listed(row).is_some()) else {
        return;
    };
    let run = body.iter().skip(start).take_while(|row| listed(row).is_some()).count();
    let mut groups: Vec<(&'a ConnectionResult, &'a PullRequestSummary, Vec<Row<'a>>)> = Vec::new();
    for row in body.drain(start..start + run) {
        let Some((result, pull_request)) = listed(&row) else {
            continue;
        };
        let known = groups.iter_mut().find(|(known, first, _)| {
            known.connection_id == result.connection_id
                && first.repository_key == pull_request.repository_key
        });
        match known {
            Some((.., rows)) => rows.push(row),
            None => groups.push((result, pull_request, vec![row])),
        }
    }
    let mut grouped = Vec::with_capacity(run + groups.len());
    for (result, first, rows) in groups {
        let section = section(card, result, &first.repository_key, &rows, sections);
        let is_open = section.is_open;
        grouped.push(Row::Section(section));
        if is_open {
            grouped.extend(rows);
        }
    }
    body.splice(start..start, grouped);
}

/// The header of `rows`, all of `repository_key` reached through `result`.
fn section<'a>(
    card: SectionCard,
    result: &'a ConnectionResult,
    repository_key: &'a str,
    rows: &[Row<'a>],
    sections: &RepositorySections,
) -> Section<'a> {
    let updated = |pull_request: &PullRequestSummary| {
        pull_request.updated_at.unwrap_or(pull_request.created_at)
    };
    let mut count = 0;
    let mut newest: Option<&'a PullRequestSummary> = None;
    let mut leaving_left: f32 = 0.0;
    for row in rows {
        match row {
            Row::Item(_, pull_request) => {
                count += 1;
                let pull_request: &'a PullRequestSummary = pull_request;
                if newest.is_none_or(|known| updated(known) < updated(pull_request)) {
                    newest = Some(pull_request);
                }
            }
            Row::Leaving(.., gone) => leaving_left = leaving_left.max(1.0 - gone),
            _ => {}
        }
    }
    Section {
        card,
        result,
        repository_key,
        count,
        newest,
        is_open: !sections.is_collapsed(card, &result.connection_id, repository_key),
        presence: if count > 0 { 1.0 } else { leaving_left },
    }
}

/// Paints `section` in `rect`, the whole header the button that opens and closes it, the arrow at
/// its right end (in line with the rows' Files buttons) the way to the repository's list. Returns
/// the toggle; the arrow goes to the list itself. A section closing up with its last leaving rows
/// takes no clicks.
pub(crate) fn paint(
    ui: &Ui,
    rect: Rect,
    section: &Section<'_>,
    (is_last, opacity): (bool, f32),
    navigator: &mut Navigator,
) -> Option<ListAction> {
    let palette = theme::palette(ui.ctx());
    let full = Rect::from_min_size(rect.min, vec2(rect.width(), SECTION_HEIGHT));
    let id = ui.scope_id().with((
        "repository_section",
        section.card,
        &section.result.connection_id,
        section.repository_key,
    ));
    let is_settled = section.presence >= 1.0;
    let toggle = is_settled.then(|| ui.interact(full, id, Sense::click()));
    let rest = palette.surface_subtle;
    let fill = toggle.as_ref().map_or(rest, |response| row::state_fill(ui, response, rest));
    let bottom = if is_last { style::RADIUS_CARD } else { 0 };
    let corner = CornerRadius { sw: bottom, se: bottom, ..CornerRadius::ZERO };
    let mut painter = ui.painter_at(rect);
    painter.multiply_opacity(opacity * section.presence);
    painter.rect_filled(full.shrink2(vec2(1.0, 0.0)), corner, fill);

    let center_y = full.center().y;
    let left =
        paint_caret(&painter, &palette, full.left() + ROW_PADDING, center_y, section.is_open);
    let mark = pos2(left + icons::SIZE / 2.0, center_y);
    match section.card {
        SectionCard::Favorites => {
            provider_icon::paint(&painter, &palette, mark, icons::SIZE, section.result.kind);
        }
        SectionCard::Connection => {
            let font = icons::rest_font(&palette, icons::SIZE);
            painter.text(mark, Align2::CENTER_CENTER, icons::BOOKS, font, palette.text_secondary);
        }
    }
    let list = Rect::from_center_size(
        pos2(full.right() - FILES_INSET - FILES_BUTTON / 2.0, center_y),
        vec2(FILES_BUTTON, FILES_BUTTON),
    );
    let title_left = left + MARK_TO_TITLE;
    let is_cut = paint_text(
        &painter,
        &palette,
        section,
        (title_left, list.left() - style::SPACE_M),
        center_y,
    );

    let toggle = toggle?;
    toggle.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, section.accessible_name())
    });
    cv_widgets::focus::ring(ui, &toggle, full);
    let tooltip = if section.is_open { "Collapse" } else { "Expand" };
    // The platform mark alone does not say which connection: the Favorites card names it here.
    let names_place = is_cut || section.card == SectionCard::Favorites;
    let toggle = if names_place {
        toggle.on_hover_text(format!("{}\n{tooltip}", section.place()))
    } else {
        toggle.on_hover_text(tooltip)
    };
    // Interacted after the header, so it sits on top: its click never toggles the section.
    if paint_list_button(ui, &painter, &palette, list, id, section) {
        navigator.go(Route::RepositoryPullRequests {
            connection_id: section.result.connection_id.clone(),
            repository_key: section.repository_key.to_owned(),
        });
    }
    toggle.clicked().then(|| ListAction::ToggleSection(section.key()))
}

/// The repository's key (its namespace quiet, its name strong), the row count in a chip and, at
/// the right end, when the newest row was updated, between `room`'s ends: a narrow card drops the
/// time first, then cuts the key. True when the key was cut.
fn paint_text(
    painter: &Painter,
    palette: &Palette,
    section: &Section<'_>,
    (left, right): (f32, f32),
    center_y: f32,
) -> bool {
    let chip_font = FontId::proportional(style::SMALL_SIZE - 1.0);
    let count =
        painter.layout_no_wrap(section.count.to_string(), chip_font, palette.text_secondary);
    let chip_width = chip_width(&count);
    let updated = section.newest.map(|newest| {
        let small = FontId::proportional(style::SMALL_SIZE);
        let moment = newest.updated_at.unwrap_or(newest.created_at);
        let text = format!("Updated {}", time_text::relative(moment));
        painter.layout_no_wrap(text, small, palette.text_secondary)
    });
    let job = key_job(section.repository_key, palette);
    let key_width = painter.layout_job(job.clone()).size().x;
    let needed = key_width + style::SPACE_S + chip_width;
    let updated = updated.filter(|time| left + needed + style::SPACE_L + time.size().x <= right);
    let key_right = updated.as_ref().map_or(right, |time| right - time.size().x - style::SPACE_L);
    let key = elide::job(painter, job, key_right - left - style::SPACE_S - chip_width);
    let is_cut = key.elided;
    let key_end = left + key.size().x;
    painter.galley(pos2(left, center_y - key.size().y / 2.0), key, palette.text);
    let chip = Rect::from_min_size(
        pos2(key_end + style::SPACE_S, center_y - CHIP_HEIGHT / 2.0),
        vec2(chip_width, CHIP_HEIGHT),
    );
    let border = Stroke::new(1.0, palette.border);
    paint_chip(painter, chip, count, (palette.surface, palette.text_secondary), Some(border));
    if let Some(time) = updated {
        let at = pos2(right - time.size().x, center_y - time.size().y / 2.0);
        painter.galley(at, time, palette.text_secondary);
    }
    is_cut
}

/// "team/" quiet, "app" strong: the name is what tells sections apart, the namespace where it lives.
fn key_job(repository_key: &str, palette: &Palette) -> LayoutJob {
    let (namespace, name) =
        repository_key.split_at(repository_key.rfind('/').map_or(0, |at| at + 1));
    let mut job = LayoutJob::default();
    let body = FontId::proportional(style::BODY_SIZE);
    job.append(namespace, 0.0, TextFormat::simple(body, palette.text_secondary));
    let strong = theme::fonts::semibold_font(style::BODY_SIZE);
    job.append(name, 0.0, TextFormat::simple(strong, palette.text));
    job
}

/// The ghost button that opens the repository's own list; true when clicked.
fn paint_list_button(
    ui: &Ui,
    painter: &Painter,
    palette: &Palette,
    rect: Rect,
    id: Id,
    section: &Section<'_>,
) -> bool {
    let response = ui.interact(rect, id.with("list"), Sense::click());
    let name = format!("Pull requests of {}", section.repository_key);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &name));
    let hover = row::hover_amount(ui, &response);
    let fill = row::state_fill(ui, &response, Color32::TRANSPARENT);
    if fill != Color32::TRANSPARENT {
        painter.rect_filled(rect, style::RADIUS_CONTROL, fill);
    }
    let glyph = motion::mix(palette.text_secondary, palette.accent_text, hover);
    let font = icons::rest_font(palette, icons::SIZE);
    painter.text(rect.center(), Align2::CENTER_CENTER, icons::ARROW_RIGHT, font, glyph);
    cv_widgets::focus::ring(ui, &response, rect);
    response.on_hover_text(name).clicked()
}
