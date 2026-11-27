//! Keep AppKit's real buttons centered in the app-drawn title bar.
//! All coordinates here are native macOS points, not pixels or zoomed egui points.
#![expect(unsafe_code, reason = "objc2 marks reading a view's superview unsafe")]

use objc2_app_kit::{NSView, NSWindow, NSWindowButton, NSWindowStyleMask};
use objc2_foundation::NSPoint;

/// The traffic lights and the room after them, in points, when they cannot be measured.
pub(super) const FALLBACK_INSET: f64 = 76.0;
/// How many views may lie between a traffic light and the window's root view.
const MAX_TITLE_BAR_DEPTH: usize = 8;
/// Room between the last traffic light and the app's content, in points.
const GAP_AFTER_BUTTONS: f64 = 12.0;

pub(crate) fn align(window: &NSWindow, height: f64, center_from_top: f64) -> f64 {
    // AppKit owns the sliding title bar and button layout in native full screen.
    if window.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return 0.0;
    }
    let Some(content) = window.contentView() else { return FALLBACK_INSET };
    // SAFETY: callers retain the window on its main thread for this entire operation.
    let Some(root) = (unsafe { content.superview() }) else { return FALLBACK_INSET };
    let buttons: Vec<_> = [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ]
    .into_iter()
    .filter_map(|kind| window.standardWindowButton(kind))
    .collect();
    let Some(close) = buttons.first() else { return FALLBACK_INSET };

    // Extend only the title-bar ancestors, never the content view or the window's root view.
    // Moving a button below the standard short title bar without extending its parents clips
    // its native hit area. Walking to the known root avoids depending on private class names.
    let mut ancestors = Vec::new();
    // SAFETY: the retained window owns every ancestor inspected below.
    let mut parent = unsafe { close.superview() };
    let mut reached_root = false;
    while let Some(view) = parent {
        if std::ptr::eq::<NSView>(&raw const *view, &raw const *root) {
            reached_root = true;
            break;
        }
        // SAFETY: as above, `view` is one of those ancestors.
        parent = unsafe { view.superview() };
        ancestors.push(view);
        if ancestors.len() > MAX_TITLE_BAR_DEPTH {
            return FALLBACK_INSET;
        }
    }
    if !reached_root || ancestors.is_empty() {
        return FALLBACK_INSET;
    }
    for view in ancestors.iter().rev() {
        // SAFETY: the retained window owns `view` and its parent.
        let Some(parent) = (unsafe { view.superview() }) else { return FALLBACK_INSET };
        let bounds = parent.bounds();
        let mut frame = view.frame();
        frame.size.height = height;
        frame.origin.y = if parent.isFlipped() {
            bounds.origin.y
        } else {
            bounds.origin.y + bounds.size.height - height
        };
        if view.frame() != frame {
            view.setFrame(frame);
        }
    }

    let bounds = content.bounds();
    let y = if content.isFlipped() {
        bounds.origin.y + center_from_top
    } else {
        bounds.origin.y + bounds.size.height - center_from_top
    };
    let mut right_edge: f64 = 0.0;
    for button in buttons {
        // SAFETY: the retained window owns its standard buttons and their parents.
        let Some(parent) = (unsafe { button.superview() }) else { continue };
        let frame = button.frame();
        let center = parent.convertPoint_fromView(NSPoint::new(bounds.origin.x, y), Some(&content));
        let origin = NSPoint::new(frame.origin.x, center.y - frame.size.height / 2.0);
        if frame.origin != origin {
            button.setFrameOrigin(origin);
        }
        let right = content.convertPoint_fromView(
            NSPoint::new(origin.x + frame.size.width, origin.y),
            Some(&parent),
        );
        right_edge = right_edge.max(right.x - bounds.origin.x);
    }
    right_edge + GAP_AFTER_BUTTONS
}
