//! Layout checks on a painted frame, for any harness running the app: what egui painted (every
//! layer's shapes with their clip rects) and the accessibility tree say where text is cut, drawn
//! over other text, or where controls collide or leave the window. No GPU needed.
//!
//! * **Cut text**: a text whose glyphs run past its clip rect sideways is cut without an
//!   ellipsis. Code (text in the code font), sideways scrolling, pan canvases and text inputs clip
//!   their content by design and are left out, as are cuts at the top and bottom, where scrolling
//!   views end.
//! * **Widening widgets**: a widget that asked for more width than its row or column had left and
//!   ends up cut (egui's `show_expand_width` debug marks, painted for the checked frame only).
//!   egui grows the column to it, so everything after it in the column overflows as well.
//! * **Overlapping text**: two texts of the same layer whose glyphs overlap, row by row (a text
//!   wrapped after other widgets starts its first row mid-line). An opaque panel floating over
//!   code (the diff's find widget) hides the code under it: that code is left out, and the
//!   panel's own content is checked like any, though it lies in a code region.
//! * **Colliding controls**: two interactive siblings whose rects overlap without one holding the
//!   other.
//! * **Off-window**: a control or a popup's frame that runs past the window's edge (rows a popup
//!   scrolls out of its own view are clipped by it, not by the window, and count as inside).
//! * **Elided without a tooltip** ([`untold_elisions`]): a text cut with "…" whose full wording
//!   does not show on hover.

use egui::accesskit::Role;
use egui::epaint::{ClippedShape, Shape, TextShape};
use egui::{Context, LayerId, Order, Pos2, Rect, Ui};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::ReviewApp;
use super::a11y_check::is_icon_glyph;
use super::walkthrough::wait_until_unsettled;
use cv_pr_diff::find::bar::painted_now as find_widget_now;
use cv_widgets::surfaces;

/// Slack for anti-aliasing and pixel rounding.
const TOLERANCE: f32 = 1.0;
/// Overlaps smaller than this on either axis are glyph extents touching.
const OVERLAP: f32 = 2.0;

/// One text as painted: where its glyphs are, what clips it, and on which layer.
#[derive(Clone)]
struct PaintedText {
    layer: LayerId,
    text: String,
    ink: Rect,
    /// The glyphs of each row: a text wrapped in a row of widgets starts its first row where the
    /// widget before it ends, so `ink` covers more than the glyphs do.
    rows: Vec<Rect>,
    clip: Rect,
    is_elided: bool,
    /// Set in the code font: code lines keep their width and are cut at their column by design.
    is_code: bool,
}

impl PaintedText {
    fn visible(&self) -> Rect {
        self.ink.intersect(self.clip)
    }

    /// Whether the visible glyphs of a row of `self` and a row of `other` overlap.
    fn overlaps(&self, other: &Self) -> bool {
        self.rows.iter().any(|mine| {
            other.rows.iter().any(|theirs| {
                let overlap = mine.intersect(self.clip).intersect(theirs.intersect(other.clip));
                overlap.width() > OVERLAP && overlap.height() > OVERLAP
            })
        })
    }
}

struct Captured {
    texts: Vec<PaintedText>,
    /// Filled rects on layers above the panels (popup and tooltip frames, their rows): rect and clip.
    popups: Vec<(LayerId, Rect, Rect)>,
    /// Widgets that asked for more width than their parent had left, as egui's
    /// `show_expand_width` debug paints them: the widget's rect and clip, and the line where the
    /// room ended.
    widening: Vec<(Rect, Rect)>,
    room_ends: Vec<(f32, egui::Rangef)>,
    /// Regions that clip on purpose: code, sideways scrolling, pan canvases.
    exempt: Vec<Rect>,
    /// Opaque panels over code this frame: the diff's find widget.
    covers: Vec<Rect>,
    screen: Rect,
}

impl Default for Captured {
    fn default() -> Self {
        Self {
            texts: Vec::new(),
            popups: Vec::new(),
            widening: Vec::new(),
            room_ends: Vec::new(),
            exempt: Vec::new(),
            covers: Vec::new(),
            screen: Rect::NOTHING,
        }
    }
}

/// egui's `show_expand_width` marks: a light blue outline around the widget and red lines.
const EXPAND_OUTLINE: egui::Color32 = egui::Color32::LIGHT_BLUE;
const EXPAND_LINE: egui::Color32 = egui::Color32::from_rgb(200, 0, 0);

/// Captures the shapes of every layer at the end of each pass while [`inspect`] asks for them.
pub fn install(ctx: &Context) {
    ctx.add_plugin(ClipCapture::default());
}

/// Holds the frame [`frame`] armed it for, captured at the end of that pass.
#[derive(Default)]
struct ClipCapture {
    is_capturing: bool,
    captured: Option<Captured>,
}

impl egui::Plugin for ClipCapture {
    fn debug_name(&self) -> &'static str {
        "clip_check"
    }

    fn on_end_pass(&mut self, ui: &mut Ui) {
        if self.is_capturing {
            self.captured = Some(capture(ui.ctx()));
        }
    }
}

fn capture(ctx: &Context) -> Captured {
    let mut layers: Vec<LayerId> = ctx.memory(|memory| memory.layer_ids().collect());
    if !layers.contains(&LayerId::background()) {
        layers.insert(0, LayerId::background());
    }
    let mut captured = Captured {
        exempt: surfaces::clipping_regions(ctx),
        covers: find_widget_now(ctx).into_iter().collect(),
        screen: ctx.content_rect(),
        ..Captured::default()
    };
    for layer in layers {
        let transform = ctx.layer_transform_to_global(layer);
        let shapes: Vec<ClippedShape> = ctx.graphics(|graphics| {
            graphics
                .get(layer)
                .map(|list| list.all_entries().cloned().collect())
                .unwrap_or_default()
        });
        for mut clipped in shapes {
            if let Some(transform) = transform {
                clipped.transform(transform);
            }
            collect(layer, clipped.clip_rect, &clipped.shape, &mut captured);
        }
    }
    captured
}

#[expect(
    clippy::float_cmp,
    reason = "egui's debug lines are vertical when both ends share the x it painted them at"
)]
fn collect(layer: LayerId, clip: Rect, shape: &Shape, captured: &mut Captured) {
    match shape {
        Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(layer, clip, shape, captured)),
        Shape::Text(text) => {
            if let Some(painted) = painted_text(layer, clip, text) {
                captured.texts.push(painted);
            }
        }
        Shape::Rect(rect)
            if rect.stroke.color == EXPAND_OUTLINE && rect.fill == egui::Color32::TRANSPARENT =>
        {
            captured.widening.push((rect.rect, clip));
        }
        Shape::Rect(rect) if is_popup(layer) && rect.blur_width == 0.0 && rect.fill.a() > 0 => {
            captured.popups.push((layer, rect.rect, clip));
        }
        Shape::LineSegment { points: [a, b], stroke }
            if stroke.color == EXPAND_LINE && a.x == b.x =>
        {
            captured.room_ends.push((a.x, egui::Rangef::new(a.y.min(b.y), a.y.max(b.y))));
        }
        _ => {}
    }
}

/// The window's edge, not a scroll area inside the popup, cuts `rect`: on a side where `rect`
/// leaves the window, its clip reaches that edge. Rows a popup scrolls out of its own view are fine.
fn is_cut_by_the_window(rect: Rect, clip: Rect, screen: Rect) -> bool {
    (rect.top() < screen.top() - TOLERANCE && clip.top() <= screen.top() + TOLERANCE)
        || (rect.bottom() > screen.bottom() + TOLERANCE
            && clip.bottom() >= screen.bottom() - TOLERANCE)
        || (rect.left() < screen.left() - TOLERANCE && clip.left() <= screen.left() + TOLERANCE)
        || (rect.right() > screen.right() + TOLERANCE && clip.right() >= screen.right() - TOLERANCE)
}

fn is_popup(layer: LayerId) -> bool {
    matches!(layer.order, Order::Foreground | Order::Tooltip)
}

fn painted_text(layer: LayerId, clip: Rect, shape: &TextShape) -> Option<PaintedText> {
    let text = shape.galley.text().trim();
    if text.is_empty() || shape.angle != 0.0 || shape.opacity_factor <= 0.0 {
        return None;
    }
    let ink = shape.galley.mesh_bounds.translate(shape.pos.to_vec2());
    let is_code = shape
        .galley
        .job
        .sections
        .iter()
        .all(|section| section.format.font_id.family == egui::FontFamily::Monospace);
    let rows = shape
        .galley
        .rows
        .iter()
        .map(|row| row.visuals.mesh_bounds.translate(shape.pos.to_vec2() + row.pos.to_vec2()))
        .filter(Rect::is_positive)
        .collect();
    ink.is_positive().then(|| PaintedText {
        layer,
        text: text.to_owned(),
        ink,
        rows,
        clip,
        is_elided: shape.galley.elided,
        is_code,
    })
}

/// Paints one frame and returns it as the checks see it.
fn frame(harness: &mut Harness<'_, ReviewApp>) -> Captured {
    harness
        .ctx
        .with_plugin(|capture: &mut ClipCapture| capture.is_capturing = true)
        .expect("clip_check::install was called on this harness");
    harness.ctx.all_styles_mut(|style| style.debug.show_expand_width = true);
    harness.step();
    harness.ctx.all_styles_mut(|style| style.debug.show_expand_width = false);
    harness
        .ctx
        .with_plugin(|capture: &mut ClipCapture| {
            capture.is_capturing = false;
            capture.captured.take()
        })
        .flatten()
        .expect("the armed pass captured its shapes")
}

/// An accessibility node as the checks see it: its name, its rect in points and its parent.
struct Control {
    name: String,
    rect: Rect,
    /// The parent's id in the tree, as accesskit's consumer packs it into a number.
    parent: Option<u128>,
}

/// The accessibility nodes whose role is wanted.
fn nodes(harness: &Harness<'_, ReviewApp>, is_wanted: impl Fn(Role) -> bool) -> Vec<Control> {
    let pixels_per_point = harness.ctx.pixels_per_point();
    harness
        .query_all_by(|node| is_wanted(node.role()))
        .filter_map(|node| {
            let accesskit = node.accesskit_node();
            let bounds = accesskit.bounding_box()?;
            let rect = Rect::from_min_max(
                Pos2::new(bounds.x0 as f32, bounds.y0 as f32) / pixels_per_point,
                Pos2::new(bounds.x1 as f32, bounds.y1 as f32) / pixels_per_point,
            );
            let name = accesskit
                .label()
                .or_else(|| accesskit.value())
                .unwrap_or_else(|| format!("{:?}", accesskit.role()));
            Some(Control {
                name,
                rect,
                parent: node.parent().map(|parent| u128::from(parent.accesskit_node().id())),
            })
        })
        .collect()
}

fn is_interactive(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::Link
            | Role::CheckBox
            | Role::RadioButton
            | Role::Switch
            | Role::Tab
            | Role::MenuItem
            | Role::MenuItemRadio
            | Role::MenuItemCheckBox
            | Role::ComboBox
            | Role::TextInput
            | Role::MultilineTextInput
            | Role::SearchInput
            | Role::Slider
            | Role::SpinButton
    )
}

fn is_text_input(role: Role) -> bool {
    matches!(role, Role::TextInput | Role::MultilineTextInput | Role::SearchInput)
}

/// Every layout problem of the frame painted next, one line each.
pub fn inspect(harness: &mut Harness<'_, ReviewApp>) -> Vec<String> {
    let captured = frame(harness);
    let inputs: Vec<Rect> =
        nodes(harness, is_text_input).into_iter().map(|control| control.rect).collect();
    // A cover's own content is checked although it lies in a code region.
    let is_covered = |rect: Rect| captured.covers.iter().any(|cover| cover.contains_rect(rect));
    let is_exempt = |rect: Rect| {
        inputs.iter().any(|input| input.intersects(rect))
            || (captured.exempt.iter().any(|region| region.intersects(rect)) && !is_covered(rect))
    };
    let controls = nodes(harness, is_interactive);
    let mut findings = cut_texts(&captured, &is_exempt);
    findings.extend(overlapping_texts(&captured, &is_exempt));
    findings.extend(widened_widgets(&captured, &is_exempt));
    findings.extend(popups_leaving_the_window(&captured));
    findings.extend(stray_controls(&controls, captured.screen, &is_exempt));
    findings.sort();
    findings.dedup();
    findings
}

/// Texts whose glyphs run past their clip; only sideways: scrolling views cut rows at their top
/// and bottom edges by design.
fn cut_texts(captured: &Captured, is_exempt: &dyn Fn(Rect) -> bool) -> Vec<String> {
    let mut findings = Vec::new();
    for text in &captured.texts {
        let (ink, clip) = (text.ink, text.clip);
        if !ink.intersects(clip) || text.is_code || is_exempt(ink) {
            continue;
        }
        if ink.left() < clip.left() - TOLERANCE || ink.right() > clip.right() + TOLERANCE {
            findings.push(format!(
                "cut text {:?}: glyphs {} past the clip {}",
                short(&text.text),
                show(ink),
                show(clip)
            ));
        }
    }
    findings
}

/// Visible texts on one layer that paint over each other.
fn overlapping_texts(captured: &Captured, is_exempt: &dyn Fn(Rect) -> bool) -> Vec<String> {
    let is_under_a_cover = |text: &PaintedText| {
        text.is_code && captured.covers.iter().any(|cover| cover.intersects(text.visible()))
    };
    let visible: Vec<&PaintedText> = captured
        .texts
        .iter()
        .filter(|text| text.visible().is_positive() && !is_exempt(text.visible()))
        .filter(|text| !is_under_a_cover(text))
        .collect();
    let mut findings = Vec::new();
    for (index, a) in visible.iter().enumerate() {
        for b in &visible[index + 1..] {
            if a.layer != b.layer || (a.text == b.text && a.ink == b.ink) {
                continue;
            }
            if a.overlaps(b) {
                findings.push(format!(
                    "overlapping text {:?} {} and {:?} {}",
                    short(&a.text),
                    show(a.ink),
                    short(&b.text),
                    show(b.ink)
                ));
            }
        }
    }
    findings
}

/// egui grows a column to its widest child, so one widget wider than its room widens everything
/// after it in that column too: report the widget, not the knock-on cuts alone. Only those that end
/// up cut: a wrapping row moves a widget that does not fit to the next line.
#[expect(
    clippy::float_cmp,
    reason = "a room's end line and the widget share the edges egui painted them with, exactly"
)]
fn widened_widgets(captured: &Captured, is_exempt: &dyn Fn(Rect) -> bool) -> Vec<String> {
    let is_cut = |(rect, clip): &&(Rect, Rect)| {
        rect.intersects(*clip)
            && (rect.left() < clip.left() - TOLERANCE || rect.right() > clip.right() + TOLERANCE)
    };
    let mut findings = Vec::new();
    for (rect, _) in captured.widening.iter().filter(is_cut).filter(|(rect, _)| !is_exempt(*rect)) {
        let room = captured
            .room_ends
            .iter()
            .find(|(x, span)| {
                *x > rect.left()
                    && *x < rect.right()
                    && span.min == rect.top()
                    && span.max == rect.bottom()
            })
            .map_or(String::from("?"), |(x, _)| format!("{:.0}", x - rect.left()));
        let inside = captured
            .texts
            .iter()
            .filter(|text| {
                rect.expand(TOLERANCE).contains_rect(text.ink)
                    || rect.intersects(text.ink) && text.ink.width() <= rect.width()
            })
            .map(|text| short(&text.text))
            .next()
            .unwrap_or_default();
        findings.push(format!(
            "widget {:?} {} is wider than its room of {room}",
            inside,
            show(*rect)
        ));
    }
    findings
}

/// Popup frames the window cuts off.
fn popups_leaving_the_window(captured: &Captured) -> Vec<String> {
    let screen = captured.screen;
    let mut findings = Vec::new();
    for (layer, rect, clip) in &captured.popups {
        if !screen.expand(TOLERANCE).contains_rect(*rect)
            && rect.intersects(screen)
            && is_cut_by_the_window(*rect, *clip, screen)
        {
            findings.push(format!(
                "popup frame {} of {:?} leaves the window {}",
                show(*rect),
                layer.id,
                show(screen)
            ));
        }
    }
    findings
}

/// Controls that run past the window's sides, and sibling controls that overlap.
fn stray_controls(
    controls: &[Control],
    screen: Rect,
    is_exempt: &dyn Fn(Rect) -> bool,
) -> Vec<String> {
    let mut findings = Vec::new();
    for Control { name, rect, .. } in controls {
        let sideways =
            rect.left() < screen.left() - TOLERANCE || rect.right() > screen.right() + TOLERANCE;
        if sideways && rect.intersects(screen) && !is_exempt(*rect) {
            findings.push(format!(
                "control {:?} {} runs past the window {}",
                short(name),
                show(*rect),
                show(screen)
            ));
        }
    }
    for (index, a) in controls.iter().enumerate() {
        for b in &controls[index + 1..] {
            if a.parent != b.parent || a.rect.contains_rect(b.rect) || b.rect.contains_rect(a.rect)
            {
                continue;
            }
            let overlap = a.rect.intersect(b.rect);
            if overlap.width() > OVERLAP && overlap.height() > OVERLAP && !is_exempt(overlap) {
                findings.push(format!(
                    "controls {:?} {} and {:?} {} overlap",
                    short(&a.name),
                    show(a.rect),
                    short(&b.name),
                    show(b.rect)
                ));
            }
        }
    }
    findings
}

/// How many tooltip delays the pointer rests on a cut text before the audit takes it as having no
/// tooltip: egui shows one after one delay.
const TOOLTIP_DELAYS: f64 = 2.0;

/// Texts cut with "…" whose full wording does not appear in a tooltip on hover: hovers each one
/// for [`TOOLTIP_DELAYS`] of simulated time, and lets its tooltip close before the next. Returns
/// one line per text.
pub fn untold_elisions(harness: &mut Harness<'_, ReviewApp>) -> Vec<String> {
    let captured = frame(harness);
    // An open popup takes the pointer; its own texts are checked once it is the page.
    if captured.texts.iter().any(|text| is_popup(text.layer)) {
        return Vec::new();
    }
    let mut elided: Vec<PaintedText> = captured
        .texts
        .iter()
        .filter(|text| {
            text.is_elided
                && text.visible().is_positive()
                && !captured.exempt.iter().any(|region| region.intersects(text.visible()))
        })
        .cloned()
        .collect();
    elided.sort_by(|a, b| a.text.cmp(&b.text));
    elided.dedup_by(|a, b| a.text == b.text);
    let time = |harness: &Harness<'_, ReviewApp>| harness.ctx.input(|input| input.time);
    let patience = TOOLTIP_DELAYS * f64::from(harness.ctx.global_style().interaction.tooltip_delay);
    let mut findings = Vec::new();
    for text in elided {
        harness.hover_at(text.visible().center());
        let deadline = time(harness) + patience;
        let mut is_told = false;
        while !is_told && time(harness) <= deadline {
            is_told = frame(harness).texts.iter().any(|tip| {
                tip.layer.order == Order::Tooltip && !tip.is_elided && tells(&tip.text, &text.text)
            });
        }
        if !is_told {
            findings.push(format!(
                "elided text {:?} {} has no tooltip with its full text",
                short(&text.text),
                show(text.ink)
            ));
        }
        harness.event(egui::Event::PointerGone);
        wait_until_unsettled(harness, "the tooltip to close", |harness| {
            harness.ctx.memory(|memory| {
                !memory
                    .areas()
                    .visible_layer_ids()
                    .iter()
                    .any(|layer| layer.order == Order::Tooltip)
            })
        });
    }
    findings
}

/// Whether the tooltip `tip` holds `text` in full. A tooltip is also the accessible description,
/// so it tells an icon of `text` in words ([`super::a11y_check`]): "feature → main" reads
/// "feature into main". The runs of `text` between its icons are looked for in `tip`, in order.
fn tells(tip: &str, text: &str) -> bool {
    let mut rest = tip;
    text.split(is_icon_glyph).map(str::trim).filter(|run| !run.is_empty()).all(|run| {
        rest.find(run).is_some_and(|at| {
            rest = &rest[at + run.len()..];
            true
        })
    })
}

fn short(text: &str) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() > 48 {
        format!("{}…", flat.chars().take(47).collect::<String>())
    } else {
        flat
    }
}

fn show(rect: Rect) -> String {
    format!("[{:.0},{:.0} {:.0}×{:.0}]", rect.left(), rect.top(), rect.width(), rect.height())
}

#[cfg(test)]
mod tests {
    use cv_base::theme::icons::ARROW_RIGHT;

    use super::tells;

    #[test]
    fn a_tooltip_tells_a_text_only_with_all_of_it() {
        let branches = format!("!4  ·  feature {ARROW_RIGHT} main");
        let cases = [
            ("Fix the parser\nShow this commit's changes", "Fix the parser", true),
            ("Show this commit's changes", "Fix the parser", false),
            ("Fix the", "Fix the parser", false),
            ("!4  ·  feature into main", branches.as_str(), true),
            ("!4  ·  main into feature", branches.as_str(), false),
            ("!4  ·  feature", branches.as_str(), false),
        ];
        for (tip, text, is_told) in cases {
            assert_eq!(tells(tip, text), is_told, "{tip:?} tells {text:?}");
        }
    }
}
