//! Lays out a Markdown sample covering every construct a pull request description uses, and the
//! code blocks, tables and zoom of the renderer.

use cv_base::theme::{self, Palette};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use review_core::ProviderKind;
use review_store::ThemePreference;

use super::RenderOptions;

const SAMPLE: &str = "\
This **automated pull request**, created by **1ES**, is part of the **[Inventory-As-Code](https://aka.ms/iac)** onboarding.

#🚀 What's Changing?

- This PR adds inventory metadata to your repository sourced from `Product Catalog`.
- Once merged, this metadata will supersede [Product Catalog](https://example.com/catalog), which will eventually be \
retired. You'll manage repository inventory directly via source code and this line wraps onto a second row.
- Repositories under orgs that have [opted into auto-merge](https://example.com/auto) are merged within 24 hours.

##1. Review the YAML file(s)

1. Ensure the metadata is accurate.
2. Run `dotnet build` locally, see https://aka.ms/inventory-as-code.
   - nested bullet
   - another nested bullet

## Proper heading

### Third level with `code`

Paragraph with **bold**, *italic*, ~~strike~~ and a [link](https://example.com).
A second line of the same paragraph.

- [x] Done task
- [ ] Open task

| Column | Value |
| --- | ---: |
| `alpha` | 1 |
| beta | 22 |

> A block quote that explains something
> over two lines.

> [!WARNING]
> Merging retires the old catalog entry.

```rust
fn main() {
    println!(\"hello\");
}
```

---

![diagram](https://example.com/diagram.png)

Last paragraph.
";

#[test]
fn markdown_sample_lays_out_headings_and_list_items() {
    let mut harness = harness();
    harness.run();
    let problems = check_layout(&harness);
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The code zoom scales the whole document: type, spacing and markers. Wide enough that no line
/// wraps at either scale: where a line wraps depends on the font's widths, which differ between
/// machines.
#[test]
fn scale_grows_the_rendered_height() {
    let heights: Vec<f32> = [1.0, 1.5]
        .into_iter()
        .map(|scale| {
            let height = std::rc::Rc::new(std::cell::Cell::new(0.0));
            let measured = height.clone();
            let mut harness = Harness::builder()
                .with_size([4000.0, 3000.0])
                .with_context_setup(|ctx| {
                    theme::fonts::install(ctx);
                    theme::activate(ctx, ThemePreference::Light);
                })
                .build_ui(move |ui| {
                    let options = RenderOptions {
                        platform: Some(ProviderKind::AzureDevOps),
                        scale,
                        ..RenderOptions::default()
                    };
                    let response =
                        ui.scope(|ui| super::render(ui, "sample", SAMPLE, options)).response;
                    measured.set(response.rect.height());
                });
            harness.run();
            height.get()
        })
        .collect();
    let ratio = heights[1] / heights[0];
    assert!((1.35..=1.65).contains(&ratio), "heights {heights:?}, ratio {ratio}");
}

const CODE_DOCUMENT: &str =
    "Intro paragraph.\n\n```rust\nfn main() { let answer = 42; }\n```\n\nAfter the code.\n";
const CODE_LINE: &str = "fn main() { let answer = 42; }";

/// A document in a scroll area scrolled to its end, as a long preview often is: a layout change
/// there also moves the scroll offset.
fn code_harness() -> Harness<'static> {
    Harness::builder()
        .with_size([520.0, 180.0])
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::activate(ctx, ThemePreference::Light);
        })
        .build_ui(|ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                ui.add_space(120.0);
                super::render(ui, "code", CODE_DOCUMENT, RenderOptions::default());
            });
        })
}

/// Hovering a code block shows its copy button over the corner and moves nothing: the text after
/// the block keeps its place, frame after frame. The button used to be laid out with `ui.put`,
/// which pulled the cursor back up into the block, so the text below jumped up over it while the
/// pointer was inside (and the scroll offset with it), then back: the flicker.
#[test]
fn hovering_a_code_block_moves_nothing() {
    let mut harness = code_harness();
    harness.run_steps(4);
    let after = |harness: &Harness<'_>| harness.get_by_label("After the code.").rect();
    let code = |harness: &Harness<'_>| harness.get_by_label(CODE_LINE).rect();
    let (resting_after, resting_code) = (after(&harness), code(&harness));
    harness.hover_at(resting_code.center());
    let mut seen = Vec::new();
    for _ in 0..8 {
        harness.step();
        seen.push((after(&harness), code(&harness)));
    }
    for (frame, (after, code)) in seen.iter().enumerate() {
        assert_eq!((*after, *code), (resting_after, resting_code), "frame {frame} after hovering");
    }
    let button = harness.get_by_role_and_label(Role::Button, "Copy code").rect();
    harness.hover_at(button.center());
    harness.run_steps(3);
    assert_eq!(after(&harness), resting_after, "hovering the button itself moves nothing either");
}

/// Every text shape painted last frame: its text and the colors of its sections.
fn painted_texts(harness: &Harness<'_>) -> Vec<(String, Vec<egui::Color32>)> {
    fn walk(shape: &egui::Shape, out: &mut Vec<(String, Vec<egui::Color32>)>) {
        match shape {
            egui::Shape::Text(text) => {
                let colors =
                    text.galley.job.sections.iter().map(|section| section.format.color).collect();
                out.push((text.galley.text().to_owned(), colors));
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &harness.output().shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// A fenced block with a language shows in that language's syntax colors once they are worked
/// out in the background; they are worked out once, however often the block is painted.
#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the colors are worked out on a background thread that offers the test no wake-up"
)]
fn fenced_code_takes_its_language_colors_once() {
    let mut harness = code_harness();
    let colors_of_code = |harness: &Harness<'_>| {
        painted_texts(harness)
            .into_iter()
            .find(|(text, _)| text == CODE_LINE)
            .map(|(_, colors)| colors.into_iter().collect::<std::collections::HashSet<_>>().len())
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut distinct = 0;
    while std::time::Instant::now() < deadline {
        harness.step();
        distinct = colors_of_code(&harness).unwrap_or(0);
        if distinct > 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(distinct > 2, "keyword, number and plain text take their own colors, got {distinct}");
    let code = CODE_LINE;
    harness.hover_at(harness.get_by_label(code).rect().center());
    harness.run_steps(10);
    assert_eq!(
        cv_syntax::code_colors::computed_count("rust", code),
        1,
        "painting again reuses the colors"
    );
}

fn check_layout(harness: &Harness<'_>) -> Vec<String> {
    let mut problems = Vec::new();
    for heading in ["🚀 What's Changing?", "1. Review the YAML file(s)", "Proper heading"] {
        if harness.query_by_role_and_label(Role::Heading, heading).is_none() {
            problems.push(format!("{heading:?} is not a heading"));
        }
    }
    // A list item is one galley: the link is part of its line, not a separate widget.
    let item =
        "Repositories under orgs that have opted into auto-merge are merged within 24 hours.";
    if harness.query_by_label(item).is_none() {
        problems.push(format!("list item {item:?} is not laid out as one text"));
    }
    problems
}

fn harness() -> Harness<'static> {
    let palette = Palette::LIGHT;
    let builder = Harness::builder().with_size([760.0, 1240.0]).with_context_setup(|ctx| {
        theme::fonts::install(ctx);
        theme::activate(ctx, ThemePreference::Light);
    });
    builder.build_ui(move |ui| {
        egui::Frame::new().fill(palette.ground).inner_margin(theme::style::SPACE_L).show(
            ui,
            |ui| {
                ui.set_width(ui.available_width());
                theme::style::card_frame(&palette).inner_margin(theme::style::SPACE_L).show(
                    ui,
                    |ui| {
                        ui.set_width(ui.available_width());
                        super::render(
                            ui,
                            "sample",
                            SAMPLE,
                            RenderOptions::for_platform(ProviderKind::AzureDevOps),
                        );
                    },
                );
            },
        );
    })
}

const CARD_TABLE: &str = "\
| Field | Rule |
| --- | --- |
| `Invoice.SupplierLegalName` | holds the legal-form suffix of Sec. 5a (1) GmbHG, in full, never shortened |
| `Invoice.Number` | unique per supplier and year |
";

/// A comment card's table shares the card's width: the cells wrap (code spans too) and every
/// cell's text stays inside the card, as in the rendered diff.
#[test]
fn a_table_in_a_narrow_card_wraps_inside_it() {
    let card = std::rc::Rc::new(std::cell::Cell::new(egui::Rect::NOTHING));
    let seen = card.clone();
    let mut harness = Harness::builder()
        .with_size([360.0, 600.0])
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::activate(ctx, ThemePreference::Light);
        })
        .build_ui(move |ui| {
            ui.set_width(300.0);
            let response = ui
                .scope(|ui| super::render(ui, "card", CARD_TABLE, RenderOptions::default()))
                .response;
            seen.set(egui::Rect::from_min_size(
                response.rect.min,
                egui::vec2(300.0, response.rect.height()),
            ));
        });
    harness.run();
    let card = card.get();
    let long = "holds the legal-form suffix of Sec. 5a (1) GmbHG, in full, never shortened";
    for text in [
        "Field",
        "Rule",
        "Invoice.SupplierLegalName",
        "unique per supplier and year",
        "Invoice.Number",
        long,
    ] {
        let rect = harness.get_by_label(text).rect();
        assert!(
            card.expand(0.5).contains_rect(rect),
            "{text:?} at {rect:?} is outside the card {card:?}"
        );
    }
    let row = harness.get_by_label("Invoice.Number").rect().height();
    assert!(harness.get_by_label(long).rect().height() > 1.5 * row, "the long cell wraps");
}

/// The format of every painted section whose text is `wanted`.
fn painted_formats(harness: &Harness<'_>, wanted: &str) -> Vec<egui::TextFormat> {
    fn walk(shape: &egui::Shape, wanted: &str, out: &mut Vec<egui::TextFormat>) {
        match shape {
            egui::Shape::Text(text) => {
                let job = &text.galley.job;
                for section in &job.sections {
                    let range = section.byte_range.start.0..section.byte_range.end.0;
                    if job.text.get(range) == Some(wanted) {
                        out.push(section.format.clone());
                    }
                }
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, wanted, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &harness.output().shapes {
        walk(&clipped.shape, wanted, &mut out);
    }
    out
}

/// The HTML marks the rich editor writes render as their tags do in a browser: underlined,
/// smaller and raised or lowered, and as a key cap.
#[test]
fn html_marks_render_underlined_scripted_and_as_keys() {
    let mut harness = Harness::builder()
        .with_size([520.0, 160.0])
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::activate(ctx, ThemePreference::Light);
        })
        .build_ui(|ui| {
            let text = "plain <ins>under</ins> H<sub>low</sub> x<sup>high</sup> <kbd>Key</kbd>";
            super::render(ui, "marks", text, RenderOptions::default());
        });
    harness.run_steps(2);
    let format = |text: &str| {
        painted_formats(&harness, text)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("{text} is painted"))
    };
    let plain = format("plain ");
    assert_eq!(plain.underline, egui::Stroke::NONE);
    assert!(format("under").underline.width > 0.0, "<ins> underlines");
    let (low, high) = (format("low"), format("high"));
    assert!(low.font_id.size < plain.font_id.size && high.font_id.size < plain.font_id.size);
    assert_eq!(high.valign, egui::Align::TOP, "superscript sits at the top of its row");
    let key = format("Key");
    assert_ne!(key.background, egui::Color32::TRANSPARENT, "a key has a cap");
    assert_eq!(key.font_id.family, theme::fonts::code_font(1.0).family);
}
