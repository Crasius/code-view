//! One pull request row: avatar, title with its labels, the meta line, the repository, size,
//! status, approvals and checks columns, the Files button, and the row's context menu (favorite,
//! the repository's list, "Copy source branch", "Add me as reviewer"). Under a repository's section
//! header ([`super::cards::sections`]) the repository's column and the meta line show only the branches.

use std::sync::Arc;

use cv_base::platform::shortcut;
use cv_base::theme::{self, Palette, icons, motion, style};
use cv_runtime::route::Landing;
use cv_services::diff_stats::Shown;
use cv_services::self_review::{self, RowAsk};
use cv_widgets::numbers::{compact_count, number_prefix};
use cv_widgets::{avatar, badges, buttons, elide, provider_icon, row, star, states, time_text};
use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align2, Color32, CornerRadius, FontId, Galley, Id, Painter, Rect, Sense, Stroke, StrokeKind,
    Ui, pos2, vec2,
};
use review_core::{
    Check, LineCounts, ProviderKind, PullRequestState, PullRequestSummary, Reviewer,
};

use super::super::queries::ConnectionResult;
use super::super::statuses::ChecksShown;
use super::indicators::{self, Indicators, MAX_AVATARS, Votes};
use super::pending::{self, Marker};
use super::size::{self, count_color};
use super::{
    AVATAR_SIZE, CHIP_HEIGHT, Columns, FILES_BUTTON, FILES_GAP, LINE_OFFSET, META_MARK_GAP,
    ROW_PADDING, chip_width, paint_chip,
};

const MAX_LABELS: usize = 3;
const MIN_TITLE_WIDTH: f32 = 160.0;
/// Height of the hairline between the checks column and the Files button.
const FILES_DIVIDER: f32 = 24.0;
const FILES_TOOLTIP: &str =
    concat!("Open Files (changes)\n", shortcut!(shift Enter), " on a focused row");
const COPY_SOURCE_BRANCH: &str = "Copy source branch";
const COPY_LINK: &str = "Copy link";
const APPROVED_TOOLTIP: &str = "The approvals it needs are in and no reviewer blocks it. The reviewers beside it show who approved.";
/// Between the status pill's check mark and its word.
const STATUS_ICON_GAP: f32 = 4.0;

/// What a row was asked for.
pub enum RowPick {
    Open(Landing),
    /// The context menu: the repository's own list.
    Repository,
    /// The context menu: star or unstar the repository.
    ToggleFavorite,
    /// The size column's "Count lines".
    CountLines,
    /// The context menu: add the signed-in user as a reviewer.
    AddMe,
}

/// Where a row's repository is named.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RepositoryName {
    /// In the row: its column, or its meta line on a narrow card.
    InRow,
    /// In the section header above the row ([`super::cards::sections`]).
    InSection,
}

/// Where and how one pull request row is painted.
pub struct ItemRow<'a> {
    pub repository: RepositoryName,
    pub rect: Rect,
    pub id: Id,
    pub is_last: bool,
    /// The card's fade.
    pub opacity: f32,
    /// The repository is starred.
    pub is_favorite: bool,
    /// The meta line marks the repository as starred (the overview without its Favorites lane).
    pub is_starred: bool,
    pub shown: &'a Shown,
    /// The row's "Add me as reviewer" state.
    pub ask: RowAsk<'a>,
    /// What the checks indicator shows.
    pub checks: ChecksShown<'a>,
    /// The reviewers with their votes: the listing's, or those its checks brought.
    pub reviewers: &'a [Reviewer],
    /// The approvals the platform requires, once known.
    pub required_approvals: Option<&'a Check>,
    /// A write of the user's on its way to this pull request, in words ("Merging…").
    pub pending: Option<&'a str>,
}

impl<'a> ItemRow<'a> {
    fn votes(&self) -> Votes<'a> {
        Votes { reviewers: self.reviewers, required: self.required_approvals }
    }
}

/// What the parts of one row paint with.
struct RowPaint<'a> {
    rect: Rect,
    painter: &'a Painter,
    palette: &'a Palette,
    /// The row's own response, for its hover and press.
    response: &'a egui::Response,
    id: Id,
}

impl RowPaint<'_> {
    /// The height of the row's middle.
    fn center(&self) -> f32 {
        self.rect.center().y
    }
}

/// Paints one pull request; returns what it was asked for: the row opens the pull request as
/// usual, the Files button (or Shift+Enter on the focused row) on its Files tab; its context menu
/// stars or unstars the repository (`is_favorite` says which) and opens the repository's list;
/// the size column may offer to count the lines.
pub fn paint(
    ui: &mut Ui,
    item: &ItemRow<'_>,
    result: &ConnectionResult,
    pull_request: &PullRequestSummary,
) -> Option<RowPick> {
    let ItemRow { rect, id, is_last, opacity, is_favorite, shown, ask, .. } = *item;
    let palette = theme::palette(ui.ctx());
    let response = ui.interact(rect, id, Sense::click());
    let accessible_name = format!("{} {}", pull_request.number, pull_request.title);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &accessible_name));
    let corner = if is_last {
        CornerRadius { sw: style::RADIUS_CARD, se: style::RADIUS_CARD, ..CornerRadius::ZERO }
    } else {
        CornerRadius::ZERO
    };
    let fill = row::state_fill(ui, &response, Color32::TRANSPARENT);
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect.shrink(1.0), corner, fill);
    }
    cv_widgets::focus::ring(ui, &response, rect);

    let mut painter = ui.painter_at(rect);
    painter.multiply_opacity(opacity * item.pending.map_or(1.0, |_| style::PENDING_OPACITY));
    let columns = Columns::for_row(rect);
    let center = rect.center().y;
    let row = RowPaint { rect, painter: &painter, palette: &palette, response: &response, id };
    avatar::paint(
        &painter,
        pos2(rect.left() + ROW_PADDING + AVATAR_SIZE / 2.0, center),
        AVATAR_SIZE,
        &pull_request.author,
    );
    let is_title_cut =
        paint_title(&painter, &palette, columns.text, center - LINE_OFFSET, pull_request);
    let number = format!("{}{}", number_prefix(result.kind), pull_request.number);
    let marker = item.pending.map(|doing| Marker { doing, number: &number, card_opacity: opacity });
    let marks = (item.is_starred, marker.as_ref());
    let place = Place::of(&columns, item.repository);
    let (is_meta_cut, meta) =
        paint_meta(ui, &row, &columns, place, (result, pull_request), shown, marks);
    if let Some(span) = columns.repository {
        paint_place(ui, &row, span, pull_request, item.repository);
    }
    let mut counts_lines = false;
    if let Some(span) = columns.size {
        let column = Rect::from_x_y_ranges(span.0..=span.1, rect.y_range());
        counts_lines = size::paint(ui, &painter, id.with("size"), column, shown, &number);
    }
    if let Some(span) = columns.status {
        paint_status(ui, &row, span, &number, pull_request, item);
    }
    let opens_approvals =
        paint_indicators(ui, &row, &columns, item, result.kind, &number, pull_request);
    // Interacted after the row, so it sits on top: its click never reaches the row.
    let opens_files = paint_files_button(ui, &row, columns.files, &number);
    let opens_files_by_key = response.has_focus()
        && ui.input(|input| {
            input.modifiers.matches_exact(egui::Modifiers::SHIFT)
                && input.key_pressed(egui::Key::Enter)
        });

    // A cut title or meta line reads in full on hover (the title also through the row's accessible name).
    let response = if is_title_cut || is_meta_cut {
        response.on_hover_text(format!("{}\n{meta}", pull_request.title))
    } else {
        response
    };
    if let Some(pick) = row_menu(&response, pull_request, is_favorite, &ask) {
        return Some(pick);
    }
    if counts_lines {
        return Some(RowPick::CountLines);
    }
    if opens_files || opens_files_by_key {
        Some(RowPick::Open(Landing::Files))
    } else if response.clicked() || opens_approvals {
        Some(RowPick::Open(Landing::Default))
    } else {
        None
    }
}

/// The approvals and checks columns, with the spinner of a running "Add me as reviewer"; true when
/// the approvals were clicked.
fn paint_indicators(
    ui: &Ui,
    row: &RowPaint<'_>,
    columns: &Columns,
    item: &ItemRow<'_>,
    kind: ProviderKind,
    number: &str,
    pull_request: &PullRequestSummary,
) -> bool {
    let (painter, palette) = (row.painter, row.palette);
    let indicators =
        Indicators { id: row.id, y: row.center(), number, kind, web_url: &pull_request.web_url };
    let ring = if row.response.is_pointer_button_down_on() {
        palette.pressed
    } else {
        motion::mix(palette.surface, palette.hover, row::hover_amount(ui, row.response))
    };
    let span = columns.reviewers;
    let opens =
        indicators::paint_approvals(ui, painter, palette, span, &indicators, item.votes(), ring);
    if item.ask.is_running {
        paint_joining(ui, row, span, item.reviewers.len());
    }
    let (checks, with_count) = (item.checks, columns.checks_count);
    indicators::paint_checks(ui, painter, palette, columns.checks, &indicators, checks, with_count);
    opens
}

/// The meta line under the title, led by the platform mark, so a row says where it lives without
/// its card header, with `is_starred` a filled star after it, and then the marker of a write on
/// its way ([`pending`]). Returns whether it was cut, and its full text.
fn paint_meta(
    ui: &Ui,
    row: &RowPaint<'_>,
    columns: &Columns,
    place: Place,
    (result, pull_request): (&ConnectionResult, &PullRequestSummary),
    shown: &Shown,
    (is_starred, marker): (bool, Option<&Marker<'_>>),
) -> (bool, String) {
    let meta_y = row.center() + LINE_OFFSET;
    let counted = match shown {
        Shown::Counted(stat) if columns.size.is_none() => stat.lines,
        _ => None,
    };
    let (job, meta) = meta_line(result.kind, pull_request, counted, place, row.palette);
    let mark = Rect::from_min_size(
        pos2(columns.text.0, meta_y - icons::SIZE_SMALL / 2.0),
        vec2(icons::SIZE_SMALL, icons::SIZE_SMALL),
    );
    provider_icon::paint(row.painter, row.palette, mark.center(), icons::SIZE_SMALL, result.kind);
    provider_icon::interact(ui, mark, row.id.with("platform"), result.kind, &result.base_url);
    let mut meta_left = mark.right() + META_MARK_GAP;
    if is_starred {
        let star = mark.translate(vec2(meta_left - mark.left(), 0.0));
        star::paint(row.painter, row.palette, star.center(), icons::SIZE_SMALL, true, 0.0);
        let name = format!("{} is a favorite", pull_request.repository_key);
        ui.interact(star, row.id.with("starred"), Sense::hover())
            .on_hover_text("A favorite repository: its pull requests come first")
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, &name));
        meta_left = star.right() + META_MARK_GAP;
    }
    if let Some(marker) = marker {
        let at = pos2(meta_left, meta_y);
        meta_left = pending::paint(ui, row.rect, row.id, at, marker) + META_MARK_GAP;
    }
    let galley = elide::job(row.painter, job, columns.text.1 - meta_left);
    let is_cut = galley.elided;
    let at = pos2(meta_left, meta_y - galley.size().y / 2.0);
    row.painter.galley(at, galley, row.palette.text_secondary);
    (is_cut, meta)
}

/// The repository over the branches, in its column; under a section header, which names the
/// repository, the source branch over the target. Both in full on hover.
fn paint_place(
    ui: &Ui,
    row: &RowPaint<'_>,
    (left, right): (f32, f32),
    pull_request: &PullRequestSummary,
    repository: RepositoryName,
) {
    let (painter, palette) = (row.painter, row.palette);
    let (title_y, meta_y) = (row.center() - LINE_OFFSET, row.center() + LINE_OFFSET);
    let width = right - left;
    let small = FontId::proportional(style::SMALL_SIZE);
    let (top, top_font, bottom, tooltip) = if repository == RepositoryName::InSection {
        let target = format!("{} {}", icons::ARROW_RIGHT, pull_request.target_branch);
        (pull_request.source_branch.clone(), small.clone(), target, branches_in_words(pull_request))
    } else {
        let tooltip =
            format!("{}\n{}", pull_request.repository_key, branches_in_words(pull_request));
        let name_font = theme::fonts::semibold_font(style::SMALL_SIZE);
        (pull_request.repository_name.clone(), name_font, branches(pull_request), tooltip)
    };
    let top = elide::line(painter, &top, top_font, palette.text, width);
    painter.galley(pos2(left, title_y - top.size().y / 2.0), top, palette.text);
    let bottom = elide::line(painter, &bottom, small, palette.text_secondary, width);
    painter.galley(pos2(left, meta_y - bottom.size().y / 2.0), bottom, palette.text_secondary);
    let hit = Rect::from_x_y_ranges(left..=right, row.rect.y_range());
    ui.interact(hit, row.id.with("repository"), Sense::hover()).on_hover_text(tooltip);
}

/// What a row's meta line says of where the pull request lives, after its platform mark.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Place {
    /// Nothing: the card is wide enough for the repository's column.
    Column,
    /// The branches: a narrow card whose section header names the repository.
    Branches,
    /// The repository and the branches: a narrow card without sections.
    RepositoryAndBranches,
}

impl Place {
    fn of(columns: &Columns, repository: RepositoryName) -> Self {
        match (columns.repository.is_some(), repository) {
            (true, _) => Self::Column,
            (false, RepositoryName::InSection) => Self::Branches,
            (false, RepositoryName::InRow) => Self::RepositoryAndBranches,
        }
    }
}

/// The meta line and its plain text (the branches in words): number, author and age, then what a
/// narrow card has no columns for: the `counted` lines in color, and the `place`.
fn meta_line(
    kind: ProviderKind,
    pull_request: &PullRequestSummary,
    counted: Option<LineCounts>,
    place: Place,
    palette: &Palette,
) -> (LayoutJob, String) {
    let small = FontId::proportional(style::SMALL_SIZE);
    let updated = pull_request.updated_at.unwrap_or(pull_request.created_at);
    let verb = if pull_request.updated_at.is_some() { "updated" } else { "created" };
    let mut meta = format!(
        "{}{}  ·  {}  ·  {verb} {}",
        number_prefix(kind),
        pull_request.number,
        pull_request.author.display_name,
        time_text::relative(updated),
    );
    let secondary = TextFormat::simple(small.clone(), palette.text_secondary);
    let mut job = LayoutJob::default();
    job.append(&meta, 0.0, secondary.clone());
    if let Some(lines) = counted {
        let (added, deleted) = (
            format!("+{}", compact_count(lines.added)),
            format!("−{}", compact_count(lines.deleted)),
        );
        job.append("  ·  ", 0.0, secondary.clone());
        job.append(
            &added,
            0.0,
            TextFormat::simple(small.clone(), count_color(lines.added, palette.success, palette)),
        );
        job.append(" ", 0.0, secondary.clone());
        job.append(
            &deleted,
            0.0,
            TextFormat::simple(small, count_color(lines.deleted, palette.error, palette)),
        );
        for part in ["  ·  ", &added, " ", &deleted] {
            meta.push_str(part);
        }
    }
    let repository = match place {
        Place::Column => return (job, meta),
        Place::Branches => String::new(),
        Place::RepositoryAndBranches => format!("  ·  {}", pull_request.repository_name),
    };
    let with_branches = |branches: String| format!("{repository}  ·  {branches}");
    job.append(&with_branches(branches(pull_request)), 0.0, secondary);
    meta.push_str(&with_branches(branches_in_words(pull_request)));
    (job, meta)
}

/// The row's context menu; its first entry, the star, opens under the pointer.
fn row_menu(
    response: &egui::Response,
    pull_request: &PullRequestSummary,
    is_favorite: bool,
    ask: &RowAsk<'_>,
) -> Option<RowPick> {
    let repository_key = &pull_request.repository_key;
    let mut picked = None;
    response.context_menu(|ui| {
        let palette = theme::palette(ui.ctx());
        let star_label = if is_favorite {
            format!("Remove {repository_key} from favorites")
        } else {
            format!("Add {repository_key} to favorites")
        };
        if buttons::menu_item(ui, icons::STAR, &star_label, None, Some(palette.favorite)).clicked()
        {
            picked = Some(RowPick::ToggleFavorite);
            ui.close();
        }
        if buttons::menu_item(
            ui,
            icons::BOOKS,
            &format!("Pull requests of {repository_key}"),
            None,
            None,
        )
        .clicked()
        {
            picked = Some(RowPick::Repository);
            ui.close();
        }
        // A row click opens the pull request, so its link and branch are copied from here.
        if buttons::menu_item(ui, icons::LINK, COPY_LINK, None, None).clicked() {
            ui.ctx().copy_text(pull_request.web_url.clone());
            ui.close();
        }
        if buttons::menu_item(ui, icons::GIT_BRANCH, COPY_SOURCE_BRANCH, None, None).clicked() {
            ui.ctx().copy_text(pull_request.source_branch.clone());
            ui.close();
        }
        if pull_request.state == PullRequestState::Open {
            ui.separator();
            if self_review::menu_entry(ui, pull_request, ask) {
                picked = Some(RowPick::AddMe);
                ui.close();
            }
        }
    });
    picked
}

/// A spinner left of the approvals stack while "Add me as reviewer" runs, where the user's avatar
/// appears once it worked; a full stack has no free slot and shows none.
fn paint_joining(ui: &Ui, row: &RowPaint<'_>, (left, right): (f32, f32), reviewers: usize) {
    if reviewers >= MAX_AVATARS {
        return;
    }
    let x = indicators::slot_x(right, reviewers);
    let slot =
        Rect::from_center_size(pos2(x.max(left), row.center()), vec2(icons::SIZE, icons::SIZE));
    states::spinner(ui, slot, row.palette.text_secondary);
}

/// The ghost button at the row's right end that opens the Files tab. Always there but quiet: an
/// outline and a stronger glyph come in with the row's hover, a raised fill with its own. True when clicked.
fn paint_files_button(
    ui: &mut Ui,
    row: &RowPaint<'_>,
    (left, right): (f32, f32),
    number: &str,
) -> bool {
    let (painter, palette, id, y) = (row.painter, row.palette, row.id, row.center());
    let divider_x = left - FILES_GAP / 2.0;
    painter.vline(
        divider_x,
        (y - FILES_DIVIDER / 2.0)..=(y + FILES_DIVIDER / 2.0),
        Stroke::new(1.0, palette.border),
    );
    let rect =
        Rect::from_x_y_ranges(left..=right, (y - FILES_BUTTON / 2.0)..=(y + FILES_BUTTON / 2.0));
    let response = ui.interact(rect, id.with("open_files"), Sense::click());
    let name = format!("Open Files (changes) of {number}");
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
    let row_hover = motion::toggle(
        ui.ctx(),
        id.with("files_row_hover"),
        row.response.contains_pointer(),
        motion::FAST,
    );
    let own_hover = row::hover_amount(ui, &response);
    let (fill, stroke) = if response.is_pointer_button_down_on() {
        (palette.pressed, palette.border_strong)
    } else {
        let outline = motion::mix(Color32::TRANSPARENT, palette.border, row_hover);
        (
            palette.surface.gamma_multiply(own_hover),
            motion::mix(outline, palette.border_strong, own_hover),
        )
    };
    painter.rect(rect, style::RADIUS_CONTROL, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    let glyph = motion::mix(
        motion::mix(palette.text_secondary, palette.text, row_hover),
        palette.accent_text,
        own_hover,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        icons::FILE_CODE,
        icons::rest_font(palette, icons::SIZE),
        glyph,
    );
    cv_widgets::focus::ring(ui, &response, rect);
    response.on_hover_text(FILES_TOOLTIP).clicked()
}

/// Title, elided to leave room for up to [`MAX_LABELS`] label chips after it. True when it was cut.
fn paint_title(
    painter: &Painter,
    palette: &Palette,
    (left, right): (f32, f32),
    y: f32,
    pull_request: &PullRequestSummary,
) -> bool {
    let width = right - left;
    let chip_font = FontId::proportional(style::SMALL_SIZE - 1.0);
    let chips: Vec<(Arc<Galley>, Color32)> = pull_request
        .labels
        .iter()
        .take(MAX_LABELS)
        .map(|label| {
            (
                painter.layout_no_wrap(label.name.clone(), chip_font.clone(), palette.text),
                badges::label_fill(label, palette),
            )
        })
        .collect();
    let chips_width: f32 =
        chips.iter().map(|(galley, _)| chip_width(galley) + style::SPACE_XS).sum();
    let title_room =
        if width - chips_width >= MIN_TITLE_WIDTH { width - chips_width } else { width };
    let title = elide::line(
        painter,
        &pull_request.title,
        theme::fonts::semibold_font(style::BODY_SIZE),
        palette.text,
        title_room,
    );
    let is_cut = title.elided;
    let mut x = left + title.size().x + style::SPACE_S;
    painter.galley(pos2(left, y - title.size().y / 2.0), title, palette.text);
    for (galley, fill) in chips {
        let size = vec2(chip_width(&galley), CHIP_HEIGHT);
        let rect = Rect::from_min_size(pos2(x, y - CHIP_HEIGHT / 2.0), size);
        if rect.right() > right {
            break;
        }
        paint_chip(painter, rect, galley, (fill, palette.text), None);
        x = rect.right() + style::SPACE_XS;
    }
    is_cut
}

/// A tinted pill with the state in words, centered in its column. An open pull request that counts
/// as approved reads "Approved" with a check mark, so it stands apart from "Merged" by shape, not
/// only by its word. Its tooltip and accessible name say the state and, for an open one, the
/// approvals the platform requires.
fn paint_status(
    ui: &Ui,
    row: &RowPaint<'_>,
    (left, right): (f32, f32),
    number: &str,
    pull_request: &PullRequestSummary,
    item: &ItemRow<'_>,
) {
    let (painter, palette, y) = (row.painter, row.palette, row.center());
    let required = item.required_approvals.map(|required| required.state);
    let is_approved = review_core::is_approved(item.reviewers, required);
    let success_fill = palette.success.gamma_multiply(if palette.is_dark { 0.22 } else { 0.12 });
    // (word, color, fill, the approval's check mark and tooltip)
    let (text, color, fill, approval) = match (pull_request.state, pull_request.is_draft) {
        (PullRequestState::Open, true) => {
            ("Draft", palette.text_secondary, palette.surface_subtle, None)
        }
        (PullRequestState::Open, false) if is_approved => {
            ("Approved", palette.success, success_fill, Some((icons::CHECK, APPROVED_TOOLTIP)))
        }
        (PullRequestState::Open, false) => ("Active", palette.accent_text, palette.selection, None),
        (PullRequestState::Merged, _) => ("Merged", palette.success, success_fill, None),
        (PullRequestState::Closed, _) => {
            ("Abandoned", palette.text_secondary, palette.surface_subtle, None)
        }
    };
    let mut job = LayoutJob::default();
    if let Some((mark, _)) = approval {
        let icon = TextFormat::simple(FontId::proportional(style::SMALL_SIZE), color);
        job.append(mark, 0.0, icon);
    }
    let gap = if approval.is_some() { STATUS_ICON_GAP } else { 0.0 };
    let words = TextFormat::simple(theme::fonts::semibold_font(style::SMALL_SIZE), color);
    job.append(text, gap, words);
    let galley = painter.layout_job(job);
    // The check mark takes some of the inset, so "Approved" fits the status column.
    let padding = if approval.is_some() { 16.0 } else { 20.0 };
    let pill = Rect::from_center_size(
        pos2(f32::midpoint(left, right), y),
        vec2(galley.size().x + padding, 22.0),
    );
    painter.rect_filled(pill, 11, fill);
    if fill == palette.surface_subtle {
        painter.rect_stroke(pill, 11, Stroke::new(1.0, palette.border), StrokeKind::Inside);
    }
    painter.galley(pill.center() - galley.size() / 2.0, galley, color);
    let mut lines = vec![format!("Status of {number}: {text}")];
    if let Some((_, tooltip)) = approval {
        lines.push(tooltip.to_owned());
    }
    if pull_request.state == PullRequestState::Open
        && let Some(required) = item.required_approvals
    {
        lines.push(indicators::required_line(required));
    }
    let name = lines.join("\n");
    let response = ui.interact(pill, row.id.with("status"), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &name));
    if lines.len() > 1 {
        response.on_hover_text(name);
    }
}

/// "feature → main" as the row paints it.
fn branches(pull_request: &PullRequestSummary) -> String {
    format!("{} {} {}", pull_request.source_branch, icons::ARROW_RIGHT, pull_request.target_branch)
}

/// "feature into main": the branches in tooltips and for screen readers, where the arrow's glyph
/// reads as nothing.
fn branches_in_words(pull_request: &PullRequestSummary) -> String {
    format!("{} into {}", pull_request.source_branch, pull_request.target_branch)
}

#[cfg(test)]
mod tests {
    use review_core::{LineCounts, ProviderKind};

    use super::super::tests::row;
    use super::{Place, meta_line};
    use cv_base::theme::Palette;

    #[test]
    fn narrow_cards_carry_the_counted_lines_after_the_age() {
        let palette = Palette::LIGHT;
        let lines = LineCounts { added: 1240, deleted: 0 };
        let place = Place::RepositoryAndBranches;
        let (job, text) = meta_line(ProviderKind::GitLab, &row(4), Some(lines), place, &palette);
        assert!(text.starts_with("!4  ·  Ada  ·  created "), "{text}");
        assert!(
            text.contains("  ·  +1,240 −0  ·  app  ·  feature"),
            "the counts follow the age, the place comes last: {text}"
        );
        let color_of = |part: &str| {
            let start = job.text.find(part).expect("in the line");
            job.sections
                .iter()
                .find(|section| section.byte_range.start == egui::text::ByteIndex(start))
                .map(|section| section.format.color)
        };
        assert_eq!(color_of("+1,240"), Some(palette.success));
        assert_eq!(color_of("−0"), Some(palette.text_disabled), "a zero steps back");
        let (_, wide) = meta_line(ProviderKind::GitLab, &row(4), None, Place::Column, &palette);
        assert!(
            !wide.contains('+') && !wide.contains("app"),
            "wide cards have columns for both: {wide}"
        );
        let (_, sectioned) =
            meta_line(ProviderKind::GitLab, &row(4), None, Place::Branches, &palette);
        assert!(
            sectioned.ends_with("  ·  feature into main") && !sectioned.contains("app"),
            "under a section header the branches follow the age alone: {sectioned}"
        );
    }
}
