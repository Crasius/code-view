//! Settings → Storage: how much the cache folder holds, as a total and a bar stacked by category,
//! the categories with the limits retention keeps them under, and "Clear cache" with the folder.
//! Sizes are measured on the retention thread; until they arrive each size slot shows a skeleton
//! of the same size, so nothing moves when they do.

use std::path::Path;

use cv_base::theme::{self, FileHue, icons, style};
use cv_runtime::toasts::ToastAction;
use cv_services::Services;
use cv_widgets::{buttons, row, skeleton};
use egui::{Align, Color32, Layout, Rect, RichText, Sense, Ui, vec2};
use review_store::retention::{
    LOG_LIMITS, MIRROR_LIMITS, Outcome, StorageStatus, StorageUsage, VIEWED_LIMITS,
};

use super::rows;

/// Sizes on screen are measured again when the section has been open this long.
const REMEASURE_SECONDS: f64 = 30.0;
const SIZE_WIDTH: f32 = 132.0;
const METER_WIDTH: f32 = 120.0;
const METER_HEIGHT: f32 = 4.0;
const STACK_HEIGHT: f32 = 10.0;
const FOOTER_HEIGHT: f32 = 44.0;
const SWATCH: f32 = 10.0;

#[derive(Default)]
pub struct StorageSection {
    is_confirming: bool,
    measured_at: Option<f64>,
}

struct Category {
    hue: FileHue,
    name: &'static str,
    rule: String,
    bytes: Option<u64>,
    limit: Option<u64>,
}

impl StorageSection {
    /// The retention thread's numbers, measured again once they are older than
    /// [`REMEASURE_SECONDS`] while the section is shown.
    fn status(&mut self, services: &Services) -> StorageStatus {
        let now = services.now();
        if self.measured_at.is_none_or(|at| now - at > REMEASURE_SECONDS) {
            services.retention.measure();
            self.measured_at = Some(now);
        }
        services.retention.status()
    }

    /// The total, the stacked bar and a row per category.
    pub fn usage(&mut self, ui: &mut Ui, services: &Services) {
        let status = self.status(services);
        let categories = categories(status.usage);
        total(ui, status.usage);
        ui.add_space(style::SPACE_S);
        stack(ui, &categories, status.usage);
        ui.add_space(style::SPACE_L);
        rows::card(ui, |ui| {
            for (index, category) in categories.iter().enumerate() {
                if index > 0 {
                    rows::divider(ui);
                }
                category_row(ui, category);
            }
        });
    }

    /// The folder, "Open folder" and "Clear cache", which asks first.
    pub fn clear(&mut self, ui: &mut Ui, services: &mut Services) {
        let status = self.status(services);
        rows::card(ui, |ui| self.footer(ui, services, &status));
    }

    fn footer(&mut self, ui: &mut Ui, services: &mut Services, status: &StorageStatus) {
        let palette = theme::palette(ui.ctx());
        ui.allocate_ui_with_layout(vec2(ui.available_width(), FOOTER_HEIGHT), Layout::right_to_left(Align::Center), |ui| {
            ui.set_min_height(FOOTER_HEIGHT);
            ui.add_space(style::SPACE_S);
            if self.is_confirming {
                let is_escaped = ui.input(|input| input.key_pressed(egui::Key::Escape));
                if buttons::secondary(ui, "Cancel").on_hover_text("Esc").clicked() || is_escaped {
                    self.is_confirming = false;
                }
                if buttons::danger(ui, icons::TRASH, "Clear cache").clicked() {
                    services.retention.clear_cache();
                    self.is_confirming = false;
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add_space(style::SPACE_M);
                    let question = "Delete all mirrors, logs and saved pages? Viewed marks stay. Pull requests download what they need again.";
                    ui.add(egui::Label::new(RichText::new(question).color(palette.text)).truncate());
                });
                return;
            }
            let clear = buttons::secondary_busy(ui, icons::TRASH, "Clear cache", status.is_clearing);
            if clear.clicked() {
                self.is_confirming = true;
            }
            if buttons::subtle(ui, icons::FOLDER_OPEN, "Open folder").clicked() {
                let folder = services.paths.cache_dir().to_path_buf();
                match create_folder(&folder) {
                    Ok(()) => services.toasts.open_folder(ui.ctx(), &folder),
                    Err(message) => {
                        let path = folder.display().to_string();
                        services.toasts.error(message, ToastAction::copy("Copy path", path));
                    }
                }
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add_space(style::SPACE_M);
                summary(ui, services, status);
            });
        });
    }
}

/// "1.2 GB used" in large type; a skeleton of its size until the sizes arrive.
fn total(ui: &mut Ui, usage: Option<StorageUsage>) {
    let palette = theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        let Some(usage) = usage else {
            let (rect, response) =
                ui.allocate_exact_size(vec2(96.0, style::TITLE_SIZE + 4.0), Sense::hover());
            skeleton::paint_bar(ui, rect);
            skeleton::announce(&response, "Measuring the cache");
            return;
        };
        ui.label(
            RichText::new(format_bytes(usage.total()))
                .font(theme::fonts::semibold_font(style::TITLE_SIZE)),
        );
        ui.label(RichText::new("used").color(palette.text_secondary));
    });
}

/// One bar across the width, a segment per category in its color, sized by its share of the total.
fn stack(ui: &mut Ui, categories: &[Category], usage: Option<StorageUsage>) {
    let palette = theme::palette(ui.ctx());
    let (track, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), STACK_HEIGHT), Sense::hover());
    let Some(usage) = usage else {
        skeleton::paint_bar(ui, track);
        return;
    };
    let painter = ui.painter().with_clip_rect(track);
    painter.rect_filled(track, STACK_HEIGHT / 2.0, palette.pressed);
    let total = usage.total().max(1) as f32;
    let mut left = track.left();
    for category in categories {
        let width = track.width() * category.bytes.unwrap_or_default() as f32 / total;
        if width <= 0.0 {
            continue;
        }
        let segment = Rect::from_min_size(egui::pos2(left, track.top()), vec2(width, STACK_HEIGHT));
        painter.rect_filled(segment, 0, palette.file_hue(category.hue));
        left += width;
    }
    let description: Vec<String> = categories
        .iter()
        .map(|category| {
            format!("{} {}", category.name, format_bytes(category.bytes.unwrap_or_default()))
        })
        .collect();
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, description.join(", ")));
}

fn categories(usage: Option<StorageUsage>) -> [Category; 4] {
    [
        Category {
            hue: FileHue::Blue,
            name: "Git mirrors",
            rule: format!(
                "Repository copies for diffs. Deleted after {} days unused; over {}, the least recently used go first.",
                days(MIRROR_LIMITS.max_age),
                format_bytes(MIRROR_LIMITS.max_total.unwrap_or_default())
            ),
            bytes: usage.map(|usage| usage.mirrors),
            limit: MIRROR_LIMITS.max_total,
        },
        Category {
            hue: FileHue::Amber,
            name: "Pipeline logs",
            rule: format!(
                "Job logs you opened. Deleted after {} days; over {}, the oldest go first.",
                days(LOG_LIMITS.max_age),
                format_bytes(LOG_LIMITS.max_total.unwrap_or_default())
            ),
            bytes: usage.map(|usage| usage.logs),
            limit: LOG_LIMITS.max_total,
        },
        Category {
            hue: FileHue::Teal,
            name: "Saved pages",
            rule: format!(
                "Pull request lists, overviews and the colors of large diffs, so they show at once after a restart and refresh in place. Over {}, the least recently used go first.",
                format_bytes(review_store::COLD_CACHE_BUDGET_BYTES)
            ),
            bytes: usage.map(|usage| usage.cold),
            limit: Some(review_store::COLD_CACHE_BUDGET_BYTES),
        },
        Category {
            hue: FileHue::Purple,
            name: "Other",
            rule: format!(
                "Viewed marks, kept {} days after you last open the pull request. Clearing the cache keeps them.",
                days(VIEWED_LIMITS.max_age)
            ),
            bytes: usage.map(|usage| usage.other),
            limit: None,
        },
    ]
}

fn category_row(ui: &mut Ui, category: &Category) {
    let palette = theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        ui.set_min_height(style::ROW_TWO_LINE);
        ui.add_space(style::SPACE_S);
        swatch(ui, palette.file_hue(category.hue));
        ui.add_space(style::SPACE_XS);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(style::SPACE_S);
            meter(ui, category);
            ui.allocate_ui_with_layout(
                vec2(SIZE_WIDTH, style::ROW_TWO_LINE),
                Layout::right_to_left(Align::Center),
                |ui| {
                    ui.set_width(SIZE_WIDTH);
                    size_text(ui, category);
                },
            );
            row::centered_lines(ui, |ui| {
                ui.spacing_mut().item_spacing.y = style::SPACE_LINE;
                ui.label(
                    RichText::new(category.name)
                        .font(theme::fonts::semibold_font(style::BODY_SIZE)),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(&category.rule)
                            .size(style::SMALL_SIZE)
                            .color(palette.text_secondary),
                    )
                    .truncate(),
                );
            });
        });
    });
}

fn size_text(ui: &mut Ui, category: &Category) {
    let palette = theme::palette(ui.ctx());
    let Some(bytes) = category.bytes else {
        let (rect, response) = ui.allocate_exact_size(vec2(72.0, 14.0), Sense::hover());
        skeleton::paint_bar(ui, rect);
        skeleton::announce(&response, &format!("Measuring {}", category.name.to_lowercase()));
        return;
    };
    let text = match category.limit {
        Some(limit) => format!("{} of {}", format_bytes(bytes), format_bytes(limit)),
        None => format_bytes(bytes),
    };
    ui.label(RichText::new(text).color(palette.text));
}

/// A thin bar of how much of its limit a category uses; categories without a limit keep the slot empty.
fn meter(ui: &mut Ui, category: &Category) {
    let palette = theme::palette(ui.ctx());
    let (slot, response) =
        ui.allocate_exact_size(vec2(METER_WIDTH, style::ROW_TWO_LINE), Sense::hover());
    let Some(limit) = category.limit else { return };
    let track = Rect::from_center_size(slot.center(), vec2(METER_WIDTH, METER_HEIGHT));
    let Some(bytes) = category.bytes else {
        skeleton::paint_bar(ui, track);
        return;
    };
    let share = (bytes as f64 / limit.max(1) as f64).clamp(0.0, 1.0) as f32;
    let painter = ui.painter();
    painter.rect_filled(track, 2, palette.pressed);
    let used = Rect::from_min_size(
        track.min,
        vec2((track.width() * share).max(if bytes > 0 { 2.0 } else { 0.0 }), METER_HEIGHT),
    );
    let fill = if share >= 0.9 { palette.warning } else { palette.accent };
    painter.rect_filled(used, 2, fill);
    let percent = (share * 100.0).round();
    let name = category.name;
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::ProgressIndicator,
            true,
            format!("{name}: {percent}% of the limit"),
        )
    });
}

/// The footer's left side: the cache folder, or what the last "Clear cache" did.
fn summary(ui: &mut Ui, services: &Services, status: &StorageStatus) {
    let palette = theme::palette(ui.ctx());
    if status.is_clearing {
        ui.label(RichText::new("Clearing…").color(palette.text_secondary));
        return;
    }
    let total = status.usage.map(|usage| format!("Total {}", format_bytes(usage.total())));
    match status.cleared {
        Some(cleared) if cleared.kept_in_use > 0 => {
            ui.label(RichText::new(kept_message(cleared)).color(palette.text_secondary));
            if buttons::subtle(ui, icons::ARROW_CLOCKWISE, "Clear again").clicked() {
                services.retention.clear_cache();
            }
        }
        Some(cleared) => {
            decorative_icon(ui, icons::CHECK_CIRCLE, icons::SIZE, palette.success);
            let freed = format!("Freed {}.", format_bytes(cleared.freed));
            let text = match total {
                Some(total) => format!("{freed} {total}."),
                None => freed,
            };
            ui.label(RichText::new(text).color(palette.text_secondary));
        }
        None => {
            let folder = services.paths.cache_dir().display().to_string();
            let text = RichText::new(folder)
                .monospace()
                .size(style::SMALL_SIZE)
                .color(palette.text_secondary);
            ui.add(egui::Label::new(text).truncate());
        }
    }
}

/// Leads into the "Clear again" button next to it.
fn kept_message(cleared: Outcome) -> String {
    let items = if cleared.kept_in_use == 1 {
        "1 item is".to_owned()
    } else {
        format!("{} items are", cleared.kept_in_use)
    };
    format!(
        "Freed {}. {items} in use by an open pull request; close it, then",
        format_bytes(cleared.freed)
    )
}

/// The category's color in the stacked bar; its name says what it is.
fn swatch(ui: &mut Ui, color: Color32) {
    let (slot, _) = ui.allocate_exact_size(vec2(icons::SIZE, icons::SIZE), Sense::hover());
    ui.painter().rect_filled(Rect::from_center_size(slot.center(), vec2(SWATCH, SWATCH)), 2, color);
}

/// A glyph painted without a widget, so screen readers skip it; the text next to it says it all.
fn decorative_icon(ui: &mut Ui, glyph: &str, size: f32, color: egui::Color32) {
    let palette = theme::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        icons::rest_font(&palette, size),
        color,
    );
}

/// Creates the cache folder before "Open folder" opens it. A failure is not dropped
/// (RS-DONT-05): the message names the folder, so the toast says which path failed.
fn create_folder(folder: &Path) -> Result<(), String> {
    std::fs::create_dir_all(folder)
        .map_err(|error| format!("Could not create the cache folder {}: {error}", folder.display()))
}

fn days(age: std::time::Duration) -> u64 {
    age.as_secs() / (24 * 60 * 60)
}

/// "0 B", "820 KB", "3.4 MB", "2.31 GB" (binary units, as Explorer shows them). A size whose
/// rounded number would reach 1024 takes the next unit: "1.0 MB", never "1024 KB".
pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    let kilobytes = value / KIB;
    let megabytes = kilobytes / KIB;
    if value < KIB {
        format!("{bytes} B")
    } else if kilobytes.round() < KIB {
        format!("{kilobytes:.0} KB")
    } else if (megabytes * 10.0).round() / 10.0 < KIB {
        format!("{megabytes:.1} MB")
    } else {
        let gigabytes = value / KIB / KIB / KIB;
        if gigabytes.fract() == 0.0 {
            format!("{gigabytes:.0} GB")
        } else {
            format!("{gigabytes:.2} GB")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{create_folder, format_bytes};

    #[test]
    fn creating_the_cache_folder_reports_the_failing_path() {
        let root = std::env::temp_dir().join(format!("review-storage-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("temp root");
        let fresh = root.join("cache");
        assert_eq!(create_folder(&fresh), Ok(()));
        assert!(fresh.is_dir());

        // A file where a parent folder belongs makes create_dir_all fail.
        let blocker = root.join("blocker");
        std::fs::write(&blocker, b"").expect("blocker file");
        let blocked = blocker.join("cache");
        let message = create_folder(&blocked).expect_err("a file blocks the folder");
        assert!(message.starts_with("Could not create the cache folder "), "{message}");
        assert!(message.contains(&blocked.display().to_string()), "{message}");

        std::fs::remove_dir_all(&root).expect("clean up");
    }

    #[test]
    fn formats_sizes_like_explorer() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(820 * 1024), "820 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024 + 400 * 1024), "3.4 MB");
        assert_eq!(format_bytes(10 * 1024 * 1024 * 1024), "10 GB");
        assert_eq!(format_bytes(2_480_000_000), "2.31 GB");
    }

    #[test]
    fn a_size_that_rounds_up_to_1024_takes_the_next_unit() {
        assert_eq!(format_bytes(1024 * 1024 - 1), "1.0 MB");
        assert_eq!(format_bytes(1023 * 1024), "1023 KB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 - 1), "1.00 GB");
        assert_eq!(format_bytes(1023 * 1024 * 1024), "1023.0 MB");
    }
}
