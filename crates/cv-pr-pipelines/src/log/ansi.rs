//! Turns one log line with ANSI SGR colors and Azure DevOps / GitHub `##[...]` markers into styled text.

// RS-DONT-04: parses CI log text, so indexing, string slicing and narrowing casts are checked here.
#![warn(clippy::string_slice, clippy::indexing_slicing, clippy::cast_possible_truncation)]

use cv_base::theme::Palette;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

/// A run of text with one color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub color: Option<AnsiColor>,
}

/// The 16 standard terminal colors (bright ones map to the same role).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnsiColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMarker {
    Error,
    Warning,
    Section,
    Command,
    Plain,
}

/// Splits on SGR sequences; every other escape sequence is dropped.
pub fn parse(line: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut color = None;
    let mut text = String::new();
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\u{1b}' {
            text.push(character);
            continue;
        }
        if chars.peek() != Some(&'[') {
            continue;
        }
        chars.next();
        let mut parameters = String::new();
        let mut command = None;
        for next in chars.by_ref() {
            if next.is_ascii_alphabetic() {
                command = Some(next);
                break;
            }
            parameters.push(next);
        }
        if command != Some('m') {
            continue;
        }
        if !text.is_empty() {
            spans.push(Span { text: std::mem::take(&mut text), color });
        }
        color = apply_sgr(&parameters, color);
    }
    if !text.is_empty() || spans.is_empty() {
        spans.push(Span { text, color });
    }
    spans
}

fn apply_sgr(parameters: &str, current: Option<AnsiColor>) -> Option<AnsiColor> {
    let mut color = current;
    let codes: Vec<u16> = parameters.split(';').map(|code| code.parse().unwrap_or(0)).collect();
    let mut index = 0;
    while let Some(&code) = codes.get(index) {
        match code {
            0 | 39 => color = None,
            30..=37 => color = Some(standard(code - 30)),
            90..=97 => color = Some(standard(code - 90)),
            // 256-color and true-color foregrounds: skip their arguments, keep the current color.
            38 => index += if codes.get(index + 1) == Some(&5) { 2 } else { 4 },
            _ => {}
        }
        index += 1;
    }
    color
}

fn standard(index: u16) -> AnsiColor {
    match index {
        0 => AnsiColor::Black,
        1 => AnsiColor::Red,
        2 => AnsiColor::Green,
        3 => AnsiColor::Yellow,
        4 => AnsiColor::Blue,
        5 => AnsiColor::Magenta,
        6 => AnsiColor::Cyan,
        _ => AnsiColor::White,
    }
}

pub fn marker(text: &str) -> LineMarker {
    let body = strip_timestamp(text).trim_start();
    let lower_prefix: String = body.chars().take(12).collect::<String>().to_ascii_lowercase();
    if lower_prefix.starts_with("##[error]") {
        LineMarker::Error
    } else if lower_prefix.starts_with("##[warning]") {
        LineMarker::Warning
    } else if lower_prefix.starts_with("##[section]") || lower_prefix.starts_with("##[group]") {
        LineMarker::Section
    } else if lower_prefix.starts_with("##[command]") {
        LineMarker::Command
    } else {
        LineMarker::Plain
    }
}

/// Azure DevOps and GitHub prefix every line with an ISO timestamp; returns the rest.
pub fn strip_timestamp(text: &str) -> &str {
    let byte = |index: usize| text.as_bytes().get(index).copied();
    let looks_like_timestamp =
        text.len() > 20 && byte(4) == Some(b'-') && byte(7) == Some(b'-') && byte(10) == Some(b'T');
    if !looks_like_timestamp {
        return text;
    }
    match text.split_once(' ') {
        Some((timestamp, rest)) if timestamp.len() < 40 => rest,
        _ => text,
    }
}

pub fn layout(line: &str, palette: &Palette, font: &FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    let body = strip_timestamp(line);
    // `body` is a suffix of `line`, so the difference is the timestamp's length.
    let timestamp = line.get(..line.len() - body.len()).unwrap_or_default();
    if !timestamp.is_empty() {
        job.append(timestamp, 0.0, format(font, palette.text_secondary));
    }
    let base = match marker(line) {
        LineMarker::Error => palette.error,
        LineMarker::Warning => palette.warning,
        LineMarker::Section | LineMarker::Command => palette.accent,
        LineMarker::Plain => palette.text,
    };
    for span in parse(body) {
        let color = span.color.map_or(base, |color| resolve(color, palette));
        job.append(&span.text, 0.0, format(font, color));
    }
    job
}

fn format(font: &FontId, color: Color32) -> TextFormat {
    TextFormat { font_id: font.clone(), color, ..Default::default() }
}

fn resolve(color: AnsiColor, palette: &Palette) -> Color32 {
    match color {
        AnsiColor::Red => palette.error,
        AnsiColor::Green => palette.success,
        AnsiColor::Yellow => palette.warning,
        AnsiColor::Blue | AnsiColor::Cyan => palette.accent,
        AnsiColor::Magenta => Color32::from_rgb(0xB1, 0x46, 0xC2),
        AnsiColor::Black => palette.text_secondary,
        AnsiColor::White => palette.text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_sgr_colors_and_drops_other_escapes() {
        let spans = parse("plain \u{1b}[31mred\u{1b}[0m back\u{1b}[2K");
        assert_eq!(
            spans,
            [
                Span { text: "plain ".into(), color: None },
                Span { text: "red".into(), color: Some(AnsiColor::Red) },
                Span { text: " back".into(), color: None },
            ]
        );
    }

    #[test]
    fn skips_extended_color_arguments() {
        let spans = parse("\u{1b}[38;5;196;32mgreen");
        assert_eq!(spans, [Span { text: "green".into(), color: Some(AnsiColor::Green) }]);
    }

    #[test]
    fn detects_markers_after_timestamps() {
        assert_eq!(marker("2024-05-01T10:00:00.1234567Z ##[error]Build failed"), LineMarker::Error);
        assert_eq!(marker("##[group]Run cargo test"), LineMarker::Section);
        assert_eq!(marker("compiling"), LineMarker::Plain);
        assert_eq!(strip_timestamp("2024-05-01T10:00:00.1234567Z hello"), "hello");
        assert_eq!(strip_timestamp("short"), "short");
    }
}
