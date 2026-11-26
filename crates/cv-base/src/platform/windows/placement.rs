//! Brings the main window back where it was closed (see `cv_shell::window_placement`): reads the
//! window's placement and puts the hidden window back before eframe shows it.
//!
//! - All rects are screen coordinates in physical pixels; the process is per-monitor DPI aware
//!   (winit), so they mean the same on every monitor.
//! - A rect whose title bar is no longer on any monitor (a monitor unplugged, a resolution or DPI
//!   change) moves onto the nearest monitor's work area, shrunk to fit; without a usable rect the
//!   window opens centered.
//! - Moving the hidden window onto a monitor with another DPI makes winit rescale it
//!   (`WM_DPICHANGED`); the rect is set again afterwards, now without a DPI change.
//! - A maximized window is maximized while DWM-cloaked and hidden again, so it appears maximized on
//!   its monitor at the first show, with its restored rect kept for the Restore button. That
//!   maximize activates; when the launcher asked for an inactive window (a start in the
//!   background), the window refuses activation (`WS_EX_NOACTIVATE`) for it, so the start leaves
//!   the foreground where it was.
#![expect(
    unsafe_code,
    reason = "Win32 window and monitor queries; each unsafe block makes one call or one read"
)]

use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use review_store::WindowPlacement;
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST,
    MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, MonitorFromWindow,
};
use windows_sys::Win32::System::Threading::{GetStartupInfoW, STARTF_USESHOWWINDOW, STARTUPINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, IsIconic, IsZoomed,
    SHOW_WINDOW_CMD, SW_HIDE, SW_MINIMIZE, SW_SHOWMAXIMIZED, SW_SHOWMINNOACTIVE, SW_SHOWNA,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED, WS_EX_NOACTIVATE,
};
use windows_sys::core::BOOL;

/// Physical pixels of the title bar that must lie on a monitor to drag the window back.
const GRIP_WIDTH: i32 = 120;
const GRIP_HEIGHT: i32 = 32;
/// A saved rect smaller than this in either direction is not one a person left; it is ignored.
const MIN_EXTENT: i32 = 100;
/// How much larger than a work area a window may be and stay as it is: the invisible resize
/// borders of a snapped window reach past the work area.
const SIZE_SLACK: i32 = 32;

/// A rect in physical screen pixels; `right` and `bottom` are exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScreenRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl ScreenRect {
    fn from_size(left: i32, top: i32, width: i32, height: i32) -> Self {
        Self { left, top, right: left.saturating_add(width), bottom: top.saturating_add(height) }
    }

    fn of(placement: &WindowPlacement) -> Self {
        Self::from_size(placement.x, placement.y, placement.width, placement.height)
    }

    fn width(self) -> i32 {
        self.right - self.left
    }

    fn height(self) -> i32 {
        self.bottom - self.top
    }

    fn intersection(self, other: ScreenRect) -> Option<ScreenRect> {
        let overlap = ScreenRect {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        (overlap.width() > 0 && overlap.height() > 0).then_some(overlap)
    }

    fn area(self) -> i64 {
        i64::from(self.width()) * i64::from(self.height())
    }

    /// Twice the center, to stay in integers (wide enough to square any distance).
    fn doubled_center(self) -> (i128, i128) {
        (
            i128::from(self.left) + i128::from(self.right),
            i128::from(self.top) + i128::from(self.bottom),
        )
    }
}

/// Where a saved window rect goes on the monitors there are now: unchanged while its title bar is
/// reachable on one of the `work_areas` and it is not larger than that one, else moved (and
/// shrunk) onto the nearest one. `None`
/// when the rect is unusable or there is no monitor; the window then opens centered.
fn fit(saved: ScreenRect, work_areas: &[ScreenRect]) -> Option<ScreenRect> {
    if saved.width() < MIN_EXTENT || saved.height() < MIN_EXTENT {
        return None;
    }
    let grip = ScreenRect { bottom: saved.top + GRIP_HEIGHT, ..saved };
    let is_in_place = work_areas.iter().any(|area| {
        let is_reachable = grip.intersection(*area).is_some_and(|overlap| {
            overlap.width() >= GRIP_WIDTH.min(saved.width()) && overlap.height() >= GRIP_HEIGHT / 2
        });
        is_reachable
            && saved.width() <= area.width() + SIZE_SLACK
            && saved.height() <= area.height() + SIZE_SLACK
    });
    if is_in_place {
        return Some(saved);
    }
    let nearest = nearest_area(saved, work_areas)?;
    let width = saved.width().min(nearest.width());
    let height = saved.height().min(nearest.height());
    let left = saved.left.clamp(nearest.left, nearest.right - width);
    let top = saved.top.clamp(nearest.top, nearest.bottom - height);
    Some(ScreenRect::from_size(left, top, width, height))
}

/// The work area covering most of `rect`, else the one whose center is closest to its center.
fn nearest_area(rect: ScreenRect, work_areas: &[ScreenRect]) -> Option<ScreenRect> {
    let covering = work_areas
        .iter()
        .filter_map(|area| rect.intersection(*area).map(|overlap| (overlap.area(), *area)))
        .max_by_key(|(covered, _)| *covered)
        .map(|(_, area)| area);
    covering.or_else(|| {
        let (x, y) = rect.doubled_center();
        work_areas.iter().copied().min_by_key(|area| {
            let (area_x, area_y) = area.doubled_center();
            (area_x - x).pow(2) + (area_y - y).pow(2)
        })
    })
}

/// A window of `width` x `height` centered on `work_area`, shrunk to fit it.
fn centered(width: i32, height: i32, work_area: ScreenRect) -> ScreenRect {
    let width = width.min(work_area.width());
    let height = height.min(work_area.height());
    ScreenRect::from_size(
        work_area.left + (work_area.width() - width) / 2,
        work_area.top + (work_area.height() - height) / 2,
        width,
        height,
    )
}

/// The app's top-level window.
pub struct PlacedWindow(HWND);

impl PlacedWindow {
    /// The Win32 window of `creation`; `None` in tests, which have no real window.
    pub fn of(creation: &eframe::CreationContext<'_>) -> Option<Self> {
        let RawWindowHandle::Win32(handle) = creation.window_handle().ok()?.as_raw() else {
            return None;
        };
        Some(Self(handle.hwnd.get() as HWND))
    }

    /// Moves the hidden window to `saved`, fitted onto today's monitors, or centers it; then
    /// maximizes it if it was.
    pub fn restore(&self, saved: Option<WindowPlacement>) {
        let Some(current) = self.rect() else {
            return;
        };
        let target =
            saved.and_then(|placement| fit(ScreenRect::of(&placement), &work_areas())).or_else(
                || self.work_area().map(|area| centered(current.width(), current.height(), area)),
            );
        if let Some(target) = target {
            self.set_rect(target);
        }
        if saved.is_some_and(|placement| placement.is_maximized) {
            self.maximize_hidden();
        }
    }

    /// Where the window is now: its restored rect (the snapped one while snapped) and whether
    /// it is maximized. A minimized window reports what it would restore to.
    pub fn read(&self) -> Option<WindowPlacement> {
        let mut placement = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // SAFETY: a query on this app's live window; the struct carries its size and lives for
        // the call.
        if unsafe { GetWindowPlacement(self.0, &raw mut placement) } == 0 {
            return None;
        }
        // SAFETY: state queries on this app's live window; no pointers.
        let is_zoomed = unsafe { IsZoomed(self.0) } != 0;
        // SAFETY: as above.
        let is_iconic = unsafe { IsIconic(self.0) } != 0;
        // Maximized and minimized windows are elsewhere; the placement keeps the rect they restore to.
        let rect = if is_zoomed || is_iconic {
            from_workspace(placement.rcNormalPosition)
        } else {
            self.rect()?
        };
        let is_maximized =
            is_zoomed || (is_iconic && placement.flags & WPF_RESTORETOMAXIMIZED != 0);
        Some(WindowPlacement {
            x: rect.left,
            y: rect.top,
            width: rect.width(),
            height: rect.height(),
            is_maximized,
        })
    }

    fn rect(&self) -> Option<ScreenRect> {
        let mut rect = RECT::default();
        // SAFETY: a query on this app's live window into a local RECT.
        (unsafe { GetWindowRect(self.0, &raw mut rect) } != 0).then(|| screen_rect(rect))
    }

    /// The work area of the monitor the window is on.
    fn work_area(&self) -> Option<ScreenRect> {
        // SAFETY: a monitor query for this app's live window.
        monitor_info(unsafe { MonitorFromWindow(self.0, MONITOR_DEFAULTTONEAREST) })
            .map(|info| screen_rect(info.rcWork))
    }

    fn set_rect(&self, target: ScreenRect) {
        // The first move may land on a monitor with another DPI, where winit rescales the window;
        // the second sets the rect again on that monitor.
        for _ in 0..2 {
            // SAFETY: repositions this app's own window; no z-order or activation change.
            unsafe {
                SetWindowPos(
                    self.0,
                    std::ptr::null_mut(),
                    target.left,
                    target.top,
                    target.width(),
                    target.height(),
                    SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE,
                );
            }
            if self.rect() == Some(target) {
                return;
            }
        }
    }

    /// Maximizes the hidden window on its monitor without anything showing: cloaked, shown
    /// maximized, hidden again. eframe's first show then shows it maximized.
    fn maximize_hidden(&self) {
        let set_cloak = |is_cloaked: BOOL| {
            // SAFETY: sets a documented DWM attribute of this app's window from a local BOOL.
            unsafe {
                DwmSetWindowAttribute(
                    self.0,
                    DWMWA_CLOAK as u32,
                    (&raw const is_cloaked).cast(),
                    std::mem::size_of::<BOOL>() as u32,
                )
            }
        };
        let is_cloaked = set_cloak(1) >= 0;
        if !is_cloaked {
            // Without the cloak the maximize would flash an unpainted window; start restored instead.
            return;
        }
        // `SW_SHOWMAXIMIZED` activates, and an activation here takes the foreground for good (the
        // first show then finds the process in front). Unless the launcher asked for an active
        // window, the window refuses activation for this one show.
        // SAFETY: a style query of this app's own window; no pointers.
        let ex_style = unsafe { GetWindowLongPtrW(self.0, GWL_EXSTYLE) };
        let refuses_activation = !launch_activates(launch_show_command());
        if refuses_activation {
            // SAFETY: a style change of this app's own window; no pointers.
            unsafe { SetWindowLongPtrW(self.0, GWL_EXSTYLE, ex_style | WS_EX_NOACTIVATE as isize) };
        }
        // SAFETY: a show-state change of this app's own window; no pointers.
        unsafe { ShowWindow(self.0, SW_SHOWMAXIMIZED) };
        // SAFETY: as above.
        unsafe { ShowWindow(self.0, SW_HIDE) };
        if refuses_activation {
            // SAFETY: puts back the style read above.
            unsafe { SetWindowLongPtrW(self.0, GWL_EXSTYLE, ex_style) };
        }
        set_cloak(0);
    }
}

/// How the launcher asked the first window to show (`STARTUPINFO.wShowWindow`), if it did.
fn launch_show_command() -> Option<SHOW_WINDOW_CMD> {
    let mut startup =
        STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    // SAFETY: fills a local struct that carries its size.
    unsafe { GetStartupInfoW(&raw mut startup) };
    (startup.dwFlags & STARTF_USESHOWWINDOW != 0)
        .then(|| SHOW_WINDOW_CMD::from(startup.wShowWindow))
}

/// Whether the launcher wants the window active: as for a plain start (Explorer, a shortcut,
/// `Start-Process`) unless it named a show command that leaves the window inactive (a start in
/// the background, `scripts/start-background.ps1`).
fn launch_activates(show: Option<SHOW_WINDOW_CMD>) -> bool {
    !matches!(
        show,
        Some(SW_HIDE | SW_SHOWNOACTIVATE | SW_MINIMIZE | SW_SHOWMINNOACTIVE | SW_SHOWNA)
    )
}

/// The work areas of all monitors attached now.
fn work_areas() -> Vec<ScreenRect> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        areas: LPARAM,
    ) -> BOOL {
        // SAFETY: `areas` is the `Vec` passed to `EnumDisplayMonitors` below, alive for the call.
        let areas = unsafe { &mut *(areas as *mut Vec<ScreenRect>) };
        if let Some(info) = monitor_info(monitor) {
            areas.push(screen_rect(info.rcWork));
        }
        1
    }
    let mut areas: Vec<ScreenRect> = Vec::new();
    // SAFETY: enumerates the desktop's monitors; the callback only touches `areas`.
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect),
            (&raw mut areas) as LPARAM,
        );
    }
    areas
}

fn monitor_info(monitor: HMONITOR) -> Option<MONITORINFO> {
    let mut info =
        MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: the struct carries its size and lives for the call; a stale handle only fails.
    (!monitor.is_null() && unsafe { GetMonitorInfoW(monitor, &raw mut info) } != 0).then_some(info)
}

/// `rcNormalPosition` is in workspace coordinates: relative to the primary monitor's work area,
/// which differs from the screen when the taskbar sits at the top or left.
fn from_workspace(rect: RECT) -> ScreenRect {
    // SAFETY: a monitor query for a point.
    let primary =
        monitor_info(unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) });
    let (dx, dy) = primary.map_or((0, 0), |info| {
        (info.rcWork.left - info.rcMonitor.left, info.rcWork.top - info.rcMonitor.top)
    });
    let rect = screen_rect(rect);
    ScreenRect {
        left: rect.left + dx,
        top: rect.top + dy,
        right: rect.right + dx,
        bottom: rect.bottom + dy,
    }
}

fn screen_rect(rect: RECT) -> ScreenRect {
    ScreenRect { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom }
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SW_HIDE, SW_MINIMIZE, SW_SHOW, SW_SHOWDEFAULT, SW_SHOWMAXIMIZED, SW_SHOWMINNOACTIVE,
        SW_SHOWNA, SW_SHOWNOACTIVATE, SW_SHOWNORMAL,
    };

    use super::{ScreenRect, centered, fit, launch_activates};

    #[test]
    fn a_background_launch_keeps_the_restored_maximize_inactive() {
        // A plain start, or one that asks for a shown window, comes to the front.
        for show in [None, Some(SW_SHOWNORMAL), Some(SW_SHOW), Some(SW_SHOWDEFAULT)] {
            assert!(launch_activates(show), "{show:?}");
        }
        assert!(launch_activates(Some(SW_SHOWMAXIMIZED)));
        // `scripts/start-background.ps1` starts with SW_SHOWNOACTIVATE, or SW_SHOWMINNOACTIVE.
        for show in [SW_SHOWNOACTIVATE, SW_SHOWMINNOACTIVE, SW_SHOWNA, SW_MINIMIZE, SW_HIDE] {
            assert!(!launch_activates(Some(show)), "{show}");
        }
    }

    /// 1920x1080 primary with a 48 px taskbar, and a 2560x1440 monitor (150 %) to its left.
    const PRIMARY: ScreenRect = ScreenRect { left: 0, top: 0, right: 1920, bottom: 1032 };
    const LEFT: ScreenRect = ScreenRect { left: -2560, top: -200, right: 0, bottom: 1192 };

    fn rect(left: i32, top: i32, width: i32, height: i32) -> ScreenRect {
        ScreenRect::from_size(left, top, width, height)
    }

    #[test]
    fn a_rect_on_a_monitor_stays_where_it_was() {
        let on_primary = rect(200, 100, 1440, 900);
        assert_eq!(fit(on_primary, &[PRIMARY, LEFT]), Some(on_primary));
        // On the second monitor at negative coordinates, at its own (physical) size.
        let on_left = rect(-2400, -150, 2160, 1350);
        assert_eq!(fit(on_left, &[PRIMARY, LEFT]), Some(on_left));
        // Partly off-screen is fine while enough of the title bar can be grabbed.
        let hanging_right = rect(1700, 300, 1440, 900);
        assert_eq!(fit(hanging_right, &[PRIMARY]), Some(hanging_right));
        // The invisible resize border above a snapped or edge-aligned window is fine too.
        let at_the_top = rect(-7, -1, 974, 1040);
        assert_eq!(fit(at_the_top, &[PRIMARY]), Some(at_the_top));
    }

    #[test]
    fn a_rect_on_an_unplugged_monitor_moves_onto_the_nearest_one() {
        // Saved on the left monitor, which is gone: onto the primary, shrunk to its work area.
        assert_eq!(fit(rect(-2400, -150, 2160, 1350), &[PRIMARY]), Some(rect(0, 0, 1920, 1032)));
        // Smaller than the primary: keeps its size, moved just inside the edge it was beyond.
        assert_eq!(fit(rect(-1500, 300, 1200, 800), &[PRIMARY]), Some(rect(0, 232, 1200, 800)));
        // Far right, beyond any monitor.
        assert_eq!(
            fit(rect(5000, 200, 1000, 700), &[LEFT, PRIMARY]),
            Some(rect(920, 200, 1000, 700))
        );
    }

    #[test]
    fn a_title_bar_out_of_reach_brings_the_window_back() {
        // Only the bottom of the window shows: the title bar is above the work area.
        assert_eq!(fit(rect(300, -800, 1200, 900), &[PRIMARY]), Some(rect(300, 0, 1200, 900)));
        // Only a sliver of the title bar shows at the right edge: too little to grab.
        assert_eq!(fit(rect(1880, 100, 1200, 900), &[PRIMARY]), Some(rect(720, 100, 1200, 900)));
        // Below the taskbar.
        assert_eq!(fit(rect(300, 1040, 1200, 900), &[PRIMARY]), Some(rect(300, 132, 1200, 900)));
    }

    #[test]
    fn a_resolution_change_shrinks_the_window_onto_the_smaller_monitor() {
        let smaller = ScreenRect { left: 0, top: 0, right: 1280, bottom: 680 };
        assert_eq!(fit(rect(1000, 600, 1440, 900), &[smaller]), Some(rect(0, 0, 1280, 680)));
    }

    #[test]
    fn unusable_rects_and_no_monitor_fall_back_to_the_centered_default() {
        assert_eq!(fit(rect(100, 100, 0, 900), &[PRIMARY]), None);
        assert_eq!(fit(rect(100, 100, 1440, -5), &[PRIMARY]), None);
        assert_eq!(fit(rect(100, 100, 40, 40), &[PRIMARY]), None);
        assert_eq!(fit(rect(100, 100, 1440, 900), &[]), None);
        // Extreme values from a hand-edited file neither overflow nor panic.
        assert_eq!(fit(rect(i32::MAX - 10, i32::MIN, i32::MAX, i32::MAX), &[PRIMARY]), None);
        assert_eq!(
            fit(rect(i32::MAX - 5000, i32::MIN, 3000, 3000), &[PRIMARY, LEFT]),
            Some(PRIMARY)
        );
    }

    #[test]
    fn centering_fits_the_default_size_onto_the_work_area() {
        assert_eq!(centered(1440, 900, PRIMARY), rect(240, 66, 1440, 900));
        assert_eq!(centered(2160, 1350, PRIMARY), PRIMARY);
        assert_eq!(centered(2160, 1350, LEFT), rect(-2360, -179, 2160, 1350));
    }
}
