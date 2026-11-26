//! Native AppKit regression check; runs on the main thread (no GPU or network).
//!
//! ```text
//! cargo run -p cv-base --example check_macos_traffic_lights
//! ```
#[cfg(target_os = "macos")]
#[path = "../src/platform/macos/traffic_lights.rs"]
mod traffic_lights;

#[cfg(target_os = "macos")]
#[expect(unsafe_code, reason = "a native test window: AppKit's constructor and view reads")]
#[expect(clippy::print_stdout, reason = "the check reports every button it measured")]
fn main() {
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSBackingStoreType, NSWindow, NSWindowButton, NSWindowStyleMask,
        NSWindowTitleVisibility,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    let mtm = MainThreadMarker::new().expect("run on the main thread");
    let _app = NSApplication::sharedApplication(mtm);
    // repo-lint: expect(RL007) a test window built the way AppKit makes one, macOS only
    // SAFETY: AppKit is initialized on the main thread and the window is retained for the check.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(960.0, 600.0)),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable
                | NSWindowStyleMask::FullSizeContentView,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    window.setTitlebarAppearsTransparent(true);
    window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    for (width, height, zoom) in
        [(960.0, 600.0, 1.0), (1200.0, 800.0, 1.5), (800.0, 500.0, 0.75), (960.0, 600.0, 1.0)]
    {
        window.setContentSize(NSSize::new(width, height));
        let content = window.contentView().expect("a window has a content view");
        let expected_center = 20.0 * zoom;
        let inset = traffic_lights::align(&window, 40.0 * zoom, expected_center);
        for kind in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            let button =
                window.standardWindowButton(kind).expect("a titled window has its buttons");
            let frame = button.frame();
            // repo-lint: expect(RL007) reads the native view tree the check measures
            // SAFETY: the retained window owns the live title-bar view hierarchy.
            let parent = unsafe { button.superview() }.expect("a button sits in the title bar");
            let center = NSPoint::new(
                frame.origin.x + frame.size.width / 2.0,
                frame.origin.y + frame.size.height / 2.0,
            );
            let point = content.convertPoint_fromView(center, Some(&parent));
            let from_top =
                if content.isFlipped() { point.y } else { content.bounds().size.height - point.y };
            println!(
                "{width}x{height}, zoom={zoom}: button {kind:?} center={from_top:.2}, expected={expected_center:.2}"
            );
            assert!(
                (from_top - expected_center).abs() < 0.1,
                "traffic lights must be vertically centered"
            );
            assert!(
                frame.origin.y >= -0.1
                    && frame.origin.y + frame.size.height <= parent.bounds().size.height + 0.1,
                "the native parent must contain the entire button so it remains clickable"
            );
            assert!(
                point.x + frame.size.width / 2.0 < inset,
                "reserve room for all three native buttons"
            );
            // repo-lint: expect(RL007) reads the native view tree the check measures
            // SAFETY: the retained window owns the root view for this whole check.
            let root = unsafe { content.superview() }.expect("the content view has a parent");
            let root_point = root.convertPoint_fromView(point, Some(&content));
            let hit = root.hitTest(root_point).expect("the button center must hit a native view");
            assert!(hit.isDescendantOf(&button), "native hit testing must still reach the button");
        }
    }
    println!("Native traffic-light alignment passed at three zoom levels and after resizing.");
}

#[cfg(not(target_os = "macos"))]
#[expect(clippy::print_stderr, reason = "says why the check does nothing here")]
fn main() {
    eprintln!("This check requires macOS.");
}
