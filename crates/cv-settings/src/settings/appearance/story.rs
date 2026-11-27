//! The stories the miniatures of Settings › Appearance › Pull request header play
//! ([`super::miniature`]): what the page does at each moment of a mode's loop, and the captions
//! that say it.
//!
//! The stories and their timings are the approved design's CSS keyframes: each property moves
//! between its stops eased in and out, as CSS `ease-in-out` does, and the captions fade linearly.
//! Hide on scroll and Always shown tell the same 6 s story, scrolling down and back up, where only
//! the first moves the header; Start minimized tells a 7 s one: arriving from another tab, the
//! header tucking away, the pointer above the tabs letting it peek and the next scroll tucking it
//! away again. Sizes are in points of the design.

use cv_base::theme::{icons, motion};
use review_store::HeaderMode;

use super::miniature::{HEADER, tab_left};

/// The width the design placed the pointer's free positions on; they scale with the miniature.
const DESIGN_WIDTH: f32 = 204.0;

/// A property's value at stops through the loop: (share of the loop, value).
type Stops = [(f32, f32)];

const SLIDE_SECONDS: f64 = 6.0;
const MINIMIZED_SECONDS: f64 = 7.0;

// Hide on scroll and Always shown: rest, scroll down, hold, scroll up (the content first, then
// the header), rest. Only Hide on scroll moves the header.
const SLIDE_HEADER: &Stops =
    &[(0.0, 0.0), (0.10, 0.0), (0.24, HEADER), (0.72, HEADER), (0.84, 0.0), (1.0, 0.0)];
const SLIDE_LINES: &Stops =
    &[(0.0, 0.0), (0.10, 0.0), (0.40, 96.0), (0.52, 96.0), (0.70, 0.0), (1.0, 0.0)];
const SLIDE_THUMB: &Stops =
    &[(0.0, 0.0), (0.10, 0.0), (0.40, 62.0), (0.52, 62.0), (0.70, 0.0), (1.0, 0.0)];
const SLIDE_CAPTIONS: [&Stops; 3] = [
    &[(0.0, 1.0), (0.08, 1.0), (0.10, 0.0), (0.84, 0.0), (0.86, 1.0), (1.0, 1.0)],
    &[(0.0, 0.0), (0.09, 0.0), (0.11, 1.0), (0.50, 1.0), (0.52, 0.0), (1.0, 0.0)],
    &[(0.0, 0.0), (0.52, 0.0), (0.54, 1.0), (0.84, 1.0), (0.86, 0.0), (1.0, 0.0)],
];

// Start minimized: arrive from another tab, tuck away, peek from above, tuck away on scroll.
const MINIMIZED_HEADER: &Stops = &[
    (0.0, 0.0),
    (0.14, 0.0),
    (0.22, HEADER),
    (0.44, HEADER),
    (0.52, 0.0),
    (0.66, 0.0),
    (0.78, HEADER),
    (0.94, HEADER),
    (1.0, 0.0),
];
/// The underline from the tab before (0) to the card's tab (1).
const MINIMIZED_UNDERLINE: &Stops =
    &[(0.0, 0.0), (0.10, 0.0), (0.16, 1.0), (0.94, 1.0), (1.0, 0.0)];
const MINIMIZED_LINES: &Stops = &[(0.0, 0.0), (0.74, 0.0), (0.90, 40.0), (1.0, 40.0)];
const MINIMIZED_THUMB: &Stops = &[(0.0, 0.0), (0.74, 0.0), (0.90, 26.0), (1.0, 26.0)];
/// Where the pointer is through the loop: coming in, on the card's tab, above the tab bar, on the
/// content.
const MINIMIZED_POINTER: [(f32, Spot); 8] = [
    (0.0, Spot::Entry),
    (0.04, Spot::Entry),
    (0.12, Spot::Tab),
    (0.22, Spot::Tab),
    (0.34, Spot::AboveTabs),
    (0.66, Spot::AboveTabs),
    (0.74, Spot::Content),
    (1.0, Spot::Content),
];
const MINIMIZED_CAPTIONS: [&Stops; 4] = [
    &[(0.0, 1.0), (0.10, 1.0), (0.12, 0.0), (0.94, 0.0), (0.96, 1.0), (1.0, 1.0)],
    &[(0.0, 0.0), (0.11, 0.0), (0.13, 1.0), (0.42, 1.0), (0.44, 0.0), (1.0, 0.0)],
    &[(0.0, 0.0), (0.43, 0.0), (0.45, 1.0), (0.68, 1.0), (0.70, 0.0), (1.0, 0.0)],
    &[(0.0, 0.0), (0.69, 0.0), (0.71, 1.0), (0.94, 1.0), (0.96, 0.0), (1.0, 0.0)],
];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Spot {
    Entry,
    Tab,
    AboveTabs,
    Content,
}

/// The page's state in one frame of a story.
pub(crate) struct Scene {
    /// Points of the header slid up out of view.
    pub header_hidden: f32,
    /// Points the content scrolled down.
    pub lines: f32,
    /// The scroll bar thumb's offset from the top of its track.
    pub thumb: f32,
    /// The underline between the tab before (0) and the card's tab (1).
    pub underline: f32,
    /// The pointer's tip, from the miniature's top left corner.
    pub pointer: Option<egui::Vec2>,
}

/// A line under the miniature: an arrow saying which way things move, and what happens.
pub(crate) struct Caption {
    pub icon: Option<&'static str>,
    pub text: String,
}

/// The story's share of its loop `seconds` after it started.
pub(crate) fn share(mode: HeaderMode, seconds: f64) -> f32 {
    let period = match mode {
        HeaderMode::StartMinimized => MINIMIZED_SECONDS,
        HeaderMode::AlwaysShown | HeaderMode::HideOnScroll => SLIDE_SECONDS,
    };
    (seconds.rem_euclid(period) / period) as f32
}

/// The frame shown while nobody watches: the page at rest, a Start minimized header tucked away.
pub(crate) fn still(mode: HeaderMode) -> Scene {
    let header_hidden = if mode == HeaderMode::StartMinimized { HEADER } else { 0.0 };
    Scene { header_hidden, lines: 0.0, thumb: 0.0, underline: 1.0, pointer: None }
}

/// The frame at `share` of the story of `mode`, on a miniature `width` wide for tab `tab`.
pub(crate) fn scene_at(mode: HeaderMode, tab: usize, width: f32, share: f32) -> Scene {
    let eased = |stops: &Stops| sample(stops, share, motion::ease_in_out_cubic);
    match mode {
        HeaderMode::AlwaysShown => Scene {
            header_hidden: 0.0,
            lines: eased(SLIDE_LINES),
            thumb: eased(SLIDE_THUMB),
            underline: 1.0,
            pointer: None,
        },
        HeaderMode::HideOnScroll => Scene {
            header_hidden: eased(SLIDE_HEADER),
            lines: eased(SLIDE_LINES),
            thumb: eased(SLIDE_THUMB),
            underline: 1.0,
            pointer: None,
        },
        HeaderMode::StartMinimized => {
            let spot = |at: Spot| spot_offset(at, tab, width);
            let xs = MINIMIZED_POINTER.map(|(time, at)| (time, spot(at).x));
            let ys = MINIMIZED_POINTER.map(|(time, at)| (time, spot(at).y));
            Scene {
                header_hidden: eased(MINIMIZED_HEADER),
                lines: eased(MINIMIZED_LINES),
                thumb: eased(MINIMIZED_THUMB),
                underline: eased(MINIMIZED_UNDERLINE),
                pointer: Some(egui::vec2(eased(&xs), eased(&ys))),
            }
        }
    }
}

/// The pointer's tip at `spot`, from the miniature's top left corner.
fn spot_offset(spot: Spot, tab: usize, width: f32) -> egui::Vec2 {
    let scale = width / DESIGN_WIDTH;
    match spot {
        Spot::Entry => egui::vec2((210.0 * scale).min(width - 6.0), 130.0),
        Spot::Tab => egui::vec2(tab_left(tab) + 6.0, 40.0),
        Spot::AboveTabs => egui::vec2(120.0 * scale, 2.0),
        Spot::Content => egui::vec2(150.0 * scale, 110.0),
    }
}

/// The value of `stops` at `share` of the loop, moving between two stops by `ease`.
fn sample(stops: &Stops, share: f32, ease: fn(f32) -> f32) -> f32 {
    let &(_, first) = stops.first().expect("stops are never empty");
    let mut before = (0.0, first);
    for &(time, value) in stops {
        if share <= time {
            let span = time - before.0;
            let progress = if span > 0.0 { (share - before.0) / span } else { 1.0 };
            return before.1 + (value - before.1) * ease(progress);
        }
        before = (time, value);
    }
    before.1
}

fn linear(progress: f32) -> f32 {
    progress
}

pub(crate) fn idle_caption() -> Caption {
    Caption { icon: None, text: "Hover a choice to watch it".to_owned() }
}

/// What the story of `mode` is about, shown when it cannot play: its first move.
pub(crate) fn main_caption(mode: HeaderMode, name: &str) -> Caption {
    story(mode, name).into_iter().nth(1).unwrap_or_else(idle_caption)
}

/// The story's captions with their opacity at `share` of the loop; the hidden ones are left out.
pub(crate) fn captions(mode: HeaderMode, name: &str, share: f32) -> Vec<(Caption, f32)> {
    let stops: &[&Stops] = match mode {
        HeaderMode::StartMinimized => &MINIMIZED_CAPTIONS,
        HeaderMode::AlwaysShown | HeaderMode::HideOnScroll => &SLIDE_CAPTIONS,
    };
    story(mode, name)
        .into_iter()
        .zip(stops)
        .map(|(caption, stops)| (caption, sample(stops, share, linear)))
        .filter(|(_, opacity)| *opacity > 0.0)
        .collect()
}

/// The caption most visible of `captions`: the one screen readers and tests read.
pub(crate) fn shown(captions: &[(Caption, f32)]) -> Option<&Caption> {
    captions.iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|(caption, _)| caption)
}

/// The captions of the story of `mode` in order.
fn story(mode: HeaderMode, name: &str) -> Vec<Caption> {
    let caption = |icon: Option<&'static str>, text: &str| Caption { icon, text: text.to_owned() };
    match mode {
        HeaderMode::StartMinimized => vec![
            caption(None, "On another tab"),
            Caption {
                icon: Some(icons::ARROW_RIGHT),
                text: format!("Opening {name}: the header tucks away"),
            },
            caption(Some(icons::ARROW_UP), "Pointing above the tabs: it peeks back"),
            caption(Some(icons::ARROW_DOWN), "Scrolling on: it tucks away again"),
        ],
        HeaderMode::AlwaysShown => vec![
            caption(None, "At the top"),
            caption(Some(icons::ARROW_DOWN), "Scrolling down: the header stays"),
            caption(Some(icons::ARROW_UP), "Scrolling up: nothing moves above the tabs"),
        ],
        HeaderMode::HideOnScroll => vec![
            caption(None, "At the top"),
            caption(Some(icons::ARROW_DOWN), "Scrolling down: the header slides away"),
            caption(Some(icons::ARROW_UP), "Scrolling up: content first, then the header"),
        ],
    }
}

#[cfg(test)]
mod tests {
    use review_store::HeaderMode;

    use super::{
        MINIMIZED_SECONDS, SLIDE_HEADER, captions, linear, main_caption, sample, share, shown,
        story,
    };

    #[test]
    fn a_property_holds_at_its_stops_and_moves_between_them() {
        assert_eq!(sample(SLIDE_HEADER, 0.05, linear), 0.0, "resting at the top");
        let halfway = sample(SLIDE_HEADER, 0.17, linear);
        assert!((halfway - 17.0).abs() < 1e-3, "halfway down: {halfway}");
        assert_eq!(sample(SLIDE_HEADER, 0.5, linear), 34.0, "held hidden");
        assert_eq!(sample(SLIDE_HEADER, 1.0, linear), 0.0, "back at the end of the loop");
    }

    /// The caption read out is the one most visible: the story's captions follow each other.
    #[test]
    fn the_minimized_story_tells_its_steps_in_order() {
        let mode = HeaderMode::StartMinimized;
        let read = |seconds: f64| {
            let captions = captions(mode, "Files", share(mode, seconds));
            shown(&captions).map(|caption| caption.text.clone()).unwrap_or_default()
        };
        let steps: Vec<String> =
            story(mode, "Files").into_iter().map(|caption| caption.text).collect();
        let read_steps: Vec<String> = [0.0, 1.5, 4.0, 6.0].into_iter().map(read).collect();
        assert_eq!(read_steps, steps, "one moment per caption, in story order");
        assert_eq!(read(MINIMIZED_SECONDS + 1.5), read(1.5), "the story loops");
        let main = main_caption(HeaderMode::HideOnScroll, "Files").text;
        let slide = story(HeaderMode::HideOnScroll, "Files");
        assert_eq!(main, slide[1].text, "the still caption is the first move");
    }
}
