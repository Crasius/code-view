//! How the editor's text looks: the rendered comment's text formats (see
//! `markdown::render::typeset`) and the labels of mention, image and HTML chips.

use cv_base::theme::{Palette, fonts, icons, style};
use cv_markdown::{CODE_SIZE_DELTA, SCRIPT_SCALE};
use egui::text::TextFormat;
use egui::{Align, Color32, FontFamily, FontId, Stroke};
use review_core::ProviderKind;

use super::{ChipKind, heading_size};
use crate::markdown_editor::PullRequestContext;
use crate::rich_editor::model::{Atom, Block, Container, Kind, Mark, Marks};

/// How a block's text looks.
pub(crate) struct Look {
    pub size: f32,
    is_heading: bool,
    color: Color32,
    palette: Palette,
}

impl Look {
    pub(crate) fn of(block: &Block, palette: &Palette) -> Self {
        let is_quoted =
            block.nesting.iter().any(|container| matches!(container, Container::Quote(None)));
        let (size, is_heading) = match block.kind {
            Kind::Heading(level) => (heading_size(level), true),
            _ => (style::BODY_SIZE, false),
        };
        let color = match block.kind {
            Kind::Heading(6) => palette.text_secondary,
            _ if is_quoted => palette.text_secondary,
            _ => palette.text,
        };
        Self { size, is_heading, color, palette: *palette }
    }

    pub(crate) fn code_format(&self) -> TextFormat {
        TextFormat {
            font_id: fonts::code_font(style::BODY_SIZE - CODE_SIZE_DELTA),
            color: self.palette.text,
            valign: Align::BOTTOM,
            ..TextFormat::default()
        }
    }

    /// The renderer's text format for `marks` (see `markdown::render::typeset`), plus the marks
    /// the editor writes as HTML.
    pub(crate) fn format(&self, marks: &Marks) -> TextFormat {
        let family = if self.is_heading || marks.has(Mark::Bold) {
            fonts::semibold()
        } else {
            FontFamily::Proportional
        };
        let mut size = self.size;
        let mut valign = Align::BOTTOM;
        if marks.has(Mark::Subscript) || marks.has(Mark::Superscript) {
            size *= SCRIPT_SCALE;
            valign = if marks.has(Mark::Superscript) { Align::TOP } else { Align::BOTTOM };
        }
        let color = if marks.link.is_some() { self.palette.accent } else { self.color };
        let mut format = TextFormat {
            font_id: FontId::new(size, family),
            color,
            italics: marks.has(Mark::Italic),
            valign,
            ..TextFormat::default()
        };
        if marks.has(Mark::Strike) {
            format.strikethrough = Stroke::new(1.0, color);
        }
        if marks.has(Mark::Underline) {
            format.underline = Stroke::new(1.0, color);
        }
        if marks.has(Mark::Code) || marks.has(Mark::Kbd) {
            format.font_id = fonts::code_font(size - CODE_SIZE_DELTA);
            format.background =
                if marks.has(Mark::Kbd) { self.palette.pressed } else { self.palette.hover };
            format.expand_bg = 2.0;
        }
        format
    }
}

/// What an atom's chip says.
pub(crate) fn chip_label(
    atom: &Atom,
    people: Option<&PullRequestContext>,
    platform: ProviderKind,
) -> (String, ChipKind) {
    match atom {
        Atom::Mention { key } => {
            let name = match platform {
                ProviderKind::AzureDevOps => people
                    .and_then(|context| {
                        context.people.iter().find(|person| person.id.eq_ignore_ascii_case(key))
                    })
                    .map_or("someone", |person| person.display_name.as_str()),
                _ => key.as_str(),
            };
            (format!("@{name}"), ChipKind::Mention)
        }
        Atom::Image { alt, src, .. } => {
            let name = if alt.is_empty() { src.rsplit('/').next().unwrap_or(src) } else { alt };
            (format!("{}  {name}", icons::IMAGE), ChipKind::Image)
        }
        Atom::Raw(source) => {
            let shown: String = source.chars().take(40).collect();
            let label = if shown.len() < source.len() { format!("{shown}…") } else { shown };
            (label, ChipKind::Raw)
        }
    }
}
